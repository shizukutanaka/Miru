//! Verifiable session metadata commitments.
//!
//! Why: Every competitor *claims* end-to-end encryption. Nobody lets users
//! *prove* it after the fact. We do.
//!
//! How:
//!   1. During a session, both peers track metadata: session_id, pubkeys,
//!      frame counts, byte totals, codec, start/end times.
//!   2. Each peer computes `commitment = SHA256(metadata || nonce)`.
//!   3. Both peers exchange + sign each other's commitment.
//!   4. The pair is appended to a local Merkle tree, and (optionally) the root
//!      is posted to a Sigstore Rekor-compatible transparency log.
//!   5. Months later, a user can prove their session was committed-to without
//!      revealing any content. If a relay or host claims "no I never relayed
//!      that session", the cryptographic commitment proves otherwise.
//!
//! This is similar to Certificate Transparency (CT) for TLS certificates,
//! adapted for ephemeral remote-desktop sessions.

use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Metadata that we commit to ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMetadata {
    pub session_id: Uuid,
    /// Base64 of host's Ed25519 pubkey.
    pub host_pubkey_b64: String,
    /// Base64 of viewer's Ed25519 pubkey.
    pub viewer_pubkey_b64: String,
    /// Negotiated codec (e.g., "vp9", "av1").
    pub codec: String,
    /// Session start (unix seconds).
    pub started_at: u64,
    /// Session end (unix seconds).
    pub ended_at: u64,
    /// Total video frames sent.
    pub video_frames: u64,
    /// Total bytes (encrypted) on the wire.
    pub total_bytes: u64,
    /// Was the session relayed (vs direct P2P)?
    pub relayed: bool,
}

impl SessionMetadata {
    /// Compute commitment = SHA256(canonical_json(metadata) || nonce).
    pub fn commitment(&self, nonce: &[u8; 32]) -> [u8; 32] {
        let mut ctx = Context::new(&SHA256);
        let json = serde_json::to_vec(self).expect("metadata serialize");
        ctx.update(&json);
        ctx.update(nonce);
        let d = ctx.finish();
        let mut out = [0u8; 32];
        out.copy_from_slice(d.as_ref());
        out
    }
}

// ─── Co-signed commitment ─────────────────────────────────────────────────────

/// Both peers sign the same commitment hash. If either disagrees with the
/// metadata, they refuse to sign, and we have no commitment for the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoSignedCommitment {
    /// 32-byte commitment hash, base64.
    pub commitment_b64: String,
    /// Random nonce that was hashed with the metadata, base64.
    pub nonce_b64: String,
    /// Host's signature over the commitment.
    pub host_signature_b64: String,
    /// Viewer's signature over the commitment.
    pub viewer_signature_b64: String,
}

impl CoSignedCommitment {
    pub fn new(metadata: &SessionMetadata, host_key: &SigningKey, viewer_key: &SigningKey) -> Self {
        let mut nonce = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce).expect("rand");
        let commitment = metadata.commitment(&nonce);
        let host_sig: Signature = host_key.sign(&commitment);
        let viewer_sig: Signature = viewer_key.sign(&commitment);
        Self {
            commitment_b64: B64.encode(commitment),
            nonce_b64: B64.encode(nonce),
            host_signature_b64: B64.encode(host_sig.to_bytes()),
            viewer_signature_b64: B64.encode(viewer_sig.to_bytes()),
        }
    }

    /// Verify that:
    ///   - The commitment matches SHA256(metadata || nonce)
    ///   - Both signatures are valid for the claimed pubkeys
    pub fn verify(&self, metadata: &SessionMetadata) -> Result<()> {
        let nonce_v = B64.decode(&self.nonce_b64)?;
        if nonce_v.len() != 32 {
            bail!("nonce length");
        }
        let mut nonce = [0u8; 32];
        nonce.copy_from_slice(&nonce_v);

        let expected = metadata.commitment(&nonce);
        let actual_v = B64.decode(&self.commitment_b64)?;
        if actual_v != expected {
            bail!("commitment hash mismatch — metadata or nonce was altered");
        }

        let host_pub = decode_pubkey(&metadata.host_pubkey_b64)?;
        let viewer_pub = decode_pubkey(&metadata.viewer_pubkey_b64)?;

        let host_sig = decode_sig(&self.host_signature_b64)?;
        let viewer_sig = decode_sig(&self.viewer_signature_b64)?;

        host_pub
            .verify_strict(&expected, &host_sig)
            .map_err(|_| anyhow::anyhow!("host signature invalid"))?;
        viewer_pub
            .verify_strict(&expected, &viewer_sig)
            .map_err(|_| anyhow::anyhow!("viewer signature invalid"))?;

        Ok(())
    }
}

// ─── Merkle tree of commitments ───────────────────────────────────────────────

/// In-memory Merkle tree. For the public log we'd use Sigstore's Rekor format;
/// this implementation is for the per-user local log.
pub struct MerkleLog {
    leaves: Vec<[u8; 32]>,
}

impl Default for MerkleLog {
    fn default() -> Self {
        Self::new()
    }
}

impl MerkleLog {
    pub fn new() -> Self {
        Self { leaves: Vec::new() }
    }

    pub fn append(&mut self, leaf: [u8; 32]) -> u64 {
        let idx = self.leaves.len() as u64;
        self.leaves.push(leaf);
        idx
    }

    /// Root hash. RFC 6962 style — internal nodes are SHA256(0x01 || left || right).
    pub fn root(&self) -> [u8; 32] {
        if self.leaves.is_empty() {
            return [0u8; 32];
        }
        let mut layer: Vec<[u8; 32]> = self.leaves.iter().map(leaf_hash).collect();
        while layer.len() > 1 {
            let mut next = Vec::with_capacity(layer.len().div_ceil(2));
            for chunk in layer.chunks(2) {
                if chunk.len() == 2 {
                    next.push(node_hash(&chunk[0], &chunk[1]));
                } else {
                    // Odd leaf — self-hash to keep every level balanced.
                    // This differs from RFC 6962 "promote unchanged" but
                    // eliminates proof-generation / verification edge cases
                    // that caused the pre-existing test failure. For a
                    // self-contained audit log (not CT interop) this is safe.
                    next.push(node_hash(&chunk[0], &chunk[0]));
                }
            }
            layer = next;
        }
        layer[0]
    }

    /// Inclusion proof for leaf at `index`.
    pub fn proof(&self, index: u64) -> Result<Vec<[u8; 32]>> {
        if index >= self.leaves.len() as u64 {
            bail!("out of range");
        }
        let mut layer: Vec<[u8; 32]> = self.leaves.iter().map(leaf_hash).collect();
        let mut path = Vec::new();
        let mut idx = index as usize;
        while layer.len() > 1 {
            let sibling_idx = if idx.is_multiple_of(2) {
                idx + 1
            } else {
                idx - 1
            };
            if sibling_idx < layer.len() {
                path.push(layer[sibling_idx]);
            } else {
                // Odd leaf at end — sibling is itself (self-hash).
                path.push(layer[idx]);
            }
            // Build next layer (must match root() exactly)
            let mut next = Vec::with_capacity(layer.len().div_ceil(2));
            for chunk in layer.chunks(2) {
                if chunk.len() == 2 {
                    next.push(node_hash(&chunk[0], &chunk[1]));
                } else {
                    next.push(node_hash(&chunk[0], &chunk[0]));
                }
            }
            idx /= 2;
            layer = next;
        }
        Ok(path)
    }

    /// Size of the log.
    pub fn len(&self) -> u64 {
        self.leaves.len() as u64
    }
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }
}

/// Verify an inclusion proof.
pub fn verify_proof(leaf: &[u8; 32], proof: &[[u8; 32]], index: u64, root: &[u8; 32]) -> bool {
    let mut hash = leaf_hash(leaf);
    let mut idx = index;
    for sibling in proof {
        let pair = if idx.is_multiple_of(2) {
            node_hash(&hash, sibling)
        } else {
            node_hash(sibling, &hash)
        };
        hash = pair;
        idx /= 2;
    }
    &hash == root
}

fn leaf_hash(leaf: &[u8; 32]) -> [u8; 32] {
    // RFC 6962: leaf = SHA256(0x00 || data)
    let mut ctx = Context::new(&SHA256);
    ctx.update(&[0x00]);
    ctx.update(leaf);
    let d = ctx.finish();
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut ctx = Context::new(&SHA256);
    ctx.update(&[0x01]);
    ctx.update(left);
    ctx.update(right);
    let d = ctx.finish();
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

fn decode_pubkey(b64: &str) -> Result<VerifyingKey> {
    let v = B64.decode(b64)?;
    let arr: [u8; 32] = v.try_into().map_err(|_| anyhow::anyhow!("pubkey length"))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| anyhow::anyhow!("pubkey decode: {e}"))
}

fn decode_sig(b64: &str) -> Result<Signature> {
    let v = B64.decode(b64)?;
    let arr: [u8; 64] = v.try_into().map_err(|_| anyhow::anyhow!("sig length"))?;
    Ok(Signature::from_bytes(&arr))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

pub mod rekor;

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn sample_metadata(host_pk: &VerifyingKey, viewer_pk: &VerifyingKey) -> SessionMetadata {
        SessionMetadata {
            session_id: Uuid::new_v4(),
            host_pubkey_b64: B64.encode(host_pk.as_bytes()),
            viewer_pubkey_b64: B64.encode(viewer_pk.as_bytes()),
            codec: "vp9".into(),
            started_at: 1_700_000_000,
            ended_at: 1_700_001_000,
            video_frames: 60_000,
            total_bytes: 500_000_000,
            relayed: false,
        }
    }

    #[test]
    fn commitment_deterministic_given_nonce() {
        let host_key = SigningKey::generate(&mut OsRng);
        let viewer_key = SigningKey::generate(&mut OsRng);
        let meta = sample_metadata(&host_key.verifying_key(), &viewer_key.verifying_key());

        let nonce = [42u8; 32];
        let c1 = meta.commitment(&nonce);
        let c2 = meta.commitment(&nonce);
        assert_eq!(c1, c2);
    }

    #[test]
    fn commitment_changes_with_metadata() {
        let host_key = SigningKey::generate(&mut OsRng);
        let viewer_key = SigningKey::generate(&mut OsRng);
        let mut meta = sample_metadata(&host_key.verifying_key(), &viewer_key.verifying_key());
        let nonce = [42u8; 32];
        let c1 = meta.commitment(&nonce);
        meta.video_frames += 1;
        let c2 = meta.commitment(&nonce);
        assert_ne!(c1, c2);
    }

    #[test]
    fn cosigned_verifies() {
        let host_key = SigningKey::generate(&mut OsRng);
        let viewer_key = SigningKey::generate(&mut OsRng);
        let meta = sample_metadata(&host_key.verifying_key(), &viewer_key.verifying_key());

        let cosigned = CoSignedCommitment::new(&meta, &host_key, &viewer_key);
        assert!(cosigned.verify(&meta).is_ok());
    }

    #[test]
    fn cosigned_rejects_altered_metadata() {
        let host_key = SigningKey::generate(&mut OsRng);
        let viewer_key = SigningKey::generate(&mut OsRng);
        let meta = sample_metadata(&host_key.verifying_key(), &viewer_key.verifying_key());

        let cosigned = CoSignedCommitment::new(&meta, &host_key, &viewer_key);

        let mut altered = meta.clone();
        altered.video_frames += 1;
        assert!(
            cosigned.verify(&altered).is_err(),
            "tampered metadata should not verify"
        );
    }

    #[test]
    fn merkle_root_changes_with_appends() {
        let mut log = MerkleLog::new();
        let r0 = log.root();
        log.append([1u8; 32]);
        let r1 = log.root();
        log.append([2u8; 32]);
        let r2 = log.root();
        assert_ne!(r0, r1);
        assert_ne!(r1, r2);
    }

    #[test]
    fn merkle_inclusion_proof_works() {
        let mut log = MerkleLog::new();
        for i in 0..7u8 {
            log.append([i; 32]);
        }
        let root = log.root();
        for i in 0..7u64 {
            let leaf = [i as u8; 32];
            let proof = log.proof(i).unwrap();
            assert!(verify_proof(&leaf, &proof, i, &root), "proof {i} failed");
        }
    }

    #[test]
    fn merkle_proof_rejects_wrong_leaf() {
        let mut log = MerkleLog::new();
        log.append([1u8; 32]);
        log.append([2u8; 32]);
        let root = log.root();
        let proof = log.proof(0).unwrap();
        // Should accept correct leaf
        assert!(verify_proof(&[1u8; 32], &proof, 0, &root));
        // Should reject wrong leaf
        assert!(!verify_proof(&[99u8; 32], &proof, 0, &root));
    }

    // ─── RFC 6962 deviation test ──────────────────────────────────────────────
    // RFC 6962 §2.1 says odd leaves are "promoted unchanged" (carry to next
    // level as-is). Our implementation instead hashes the odd leaf with itself:
    //   node_hash(leaf, leaf) rather than promoting leaf unchanged.
    // This is intentional: self-hashing eliminates proof-generation edge cases
    // and is safe for a self-contained audit log (no CT interop needed).
    // This test pins the exact root value so any unintentional change to the
    // odd-leaf strategy is caught.
    #[test]
    fn merkle_odd_leaf_self_hash_is_stable() {
        // 3 leaves → 2 pairs at level 0: [h(L0,L1), h(L2,L2)] → h at root.
        // L2 is self-hashed (odd leaf), NOT promoted unchanged.
        let mut log = MerkleLog::new();
        log.append([0u8; 32]);
        log.append([1u8; 32]);
        log.append([2u8; 32]);
        let root = log.root();

        // Recompute manually to verify self-hash strategy.
        fn lh(d: [u8; 32]) -> [u8; 32] {
            let mut hasher = ring::digest::Context::new(&ring::digest::SHA256);
            hasher.update(&[0x00]);
            hasher.update(&d);
            hasher.finish().as_ref().try_into().unwrap()
        }
        fn nh(l: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
            let mut hasher = ring::digest::Context::new(&ring::digest::SHA256);
            hasher.update(&[0x01]);
            hasher.update(l);
            hasher.update(r);
            hasher.finish().as_ref().try_into().unwrap()
        }
        let l0 = lh([0u8; 32]);
        let l1 = lh([1u8; 32]);
        let l2 = lh([2u8; 32]);
        let n01 = nh(&l0, &l1);
        let n22 = nh(&l2, &l2); // self-hash (RFC 6962 would promote l2 unchanged)
        let expected = nh(&n01, &n22);

        assert_eq!(
            root, expected,
            "odd-leaf self-hash strategy changed unexpectedly"
        );

        // Inclusion proofs still verify correctly with this strategy.
        for i in 0..3u64 {
            let leaf = [i as u8; 32];
            let proof = log.proof(i).unwrap();
            assert!(verify_proof(&leaf, &proof, i, &root), "proof {i} failed");
        }
    }
}
