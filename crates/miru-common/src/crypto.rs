//! End-to-end encryption.
//!
//! Key exchange: X25519 ECDH (ephemeral, perfect forward secrecy)
//! Symmetric:    ChaCha20-Poly1305 AEAD (96-bit nonce, monotonic counter)
//! Identity:     Ed25519 (handled in miru-auth)
//!
//! ## Security model
//!
//! - **Send side**: monotonic counter, NEVER reused. Two directions of a
//!   session use different keys (derived via HKDF "miru-tx" / "miru-rx") to
//!   avoid any nonce-reuse risk between peers.
//! - **Receive side**: sliding-window replay protection. Packets with a
//!   sequence number we've already seen — or one too far behind the highest
//!   accepted — are rejected.
//! - **Clone**: forbidden. Cloning would duplicate the send counter and lead
//!   to nonce reuse under the same key — a complete break of
//!   ChaCha20-Poly1305. Use `derive_subkey` for independent ciphers.

use anyhow::{bail, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use x25519_dalek::{EphemeralSecret, PublicKey};

/// Bit-window size (we use a 128-bit bitmap; this is the cap).
const REPLAY_WINDOW: u64 = 128;
/// Minimum decrypt input: 8-byte seq + 16-byte tag.
const MIN_CIPHERTEXT_LEN: usize = 8 + 16;

/// Holds the negotiated symmetric key for an active session.
///
/// One `SessionCipher` per direction (logically send-only or receive-only).
/// To split a session into multiple streams (video / audio / control), use
/// `derive_subkey` rather than cloning. Do **NOT** clone.
pub struct SessionCipher {
    cipher: ChaCha20Poly1305,
    key_bytes: [u8; 32],
    /// Highest sequence number we've ever sent.
    send_nonce: AtomicU64,
    /// Replay-window state for decrypt.
    replay: Mutex<ReplayWindow>,
}

impl std::fmt::Debug for SessionCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material; just show send-counter for debugging.
        f.debug_struct("SessionCipher")
            .field("send_nonce", &self.send_nonce.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct ReplayWindow {
    /// Highest seq we've accepted (initialised lazily).
    highest: u64,
    /// True once we've seen any packet — distinguishes "highest=0 because never
    /// received" from "highest=0 because we accepted the first packet (seq 0)".
    initialised: bool,
    /// Bit i set means we've accepted seq = highest - i (for i in 0..128).
    bitmap: u128,
}

impl SessionCipher {
    pub fn new(key_bytes: [u8; 32]) -> Self {
        let key = Key::from(key_bytes);
        Self {
            cipher: ChaCha20Poly1305::new(&key),
            key_bytes,
            send_nonce: AtomicU64::new(0),
            replay: Mutex::new(ReplayWindow::default()),
        }
    }

    /// Derive an independent cipher with a domain-separation tag (HKDF-SHA256).
    /// Use this to split a session into multiple streams without nonce-reuse risk.
    pub fn derive_subkey(&self, info: &[u8]) -> Result<Self> {
        use ring::hkdf;
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"miru-subkey-v1");
        let prk = salt.extract(&self.key_bytes);
        let info_slices: [&[u8]; 1] = [info];
        let okm = prk
            .expand(&info_slices, hkdf::HKDF_SHA256)
            .map_err(|_| anyhow::anyhow!("hkdf expand failed"))?;
        let mut sub = [0u8; 32];
        okm.fill(&mut sub).map_err(|_| anyhow::anyhow!("hkdf fill failed"))?;
        Ok(Self::new(sub))
    }

    /// Encrypt plaintext. Output: [8-byte LE seq][ciphertext+tag].
    /// The send counter is monotonic; the same nonce is never reused for the
    /// lifetime of this `SessionCipher`.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let seq = self.send_nonce.fetch_add(1, Ordering::Relaxed);
        // Guard at u64::MAX-1 so the counter never wraps to 0 and causes
        // nonce reuse with the early-session seq values. Checking seq==u64::MAX
        // was wrong: by that point fetch_add had already wrapped the internal
        // counter to 0, so the NEXT call would return seq=0 (already used).
        if seq >= u64::MAX - 1 {
            bail!("send counter exhausted (rotate session)");
        }
        let nonce = nonce_from_u64(seq);
        let mut out = Vec::with_capacity(8 + plaintext.len() + 16);
        out.extend_from_slice(&seq.to_le_bytes());
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("encrypt: {e}"))?;
        out.extend(ct);
        Ok(out)
    }

    /// Decrypt. Reads nonce from first 8 bytes; rejects:
    ///   - inputs shorter than 24 bytes (no nonce + no tag)
    ///   - sequence numbers we've already accepted (replay)
    ///   - sequence numbers too far below highest accepted (window underflow)
    ///   - failed AEAD authentication
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < MIN_CIPHERTEXT_LEN {
            bail!("decrypt: input too short ({} bytes)", data.len());
        }
        let seq = u64::from_le_bytes(
            data[..8]
                .try_into()
                .map_err(|_| anyhow::anyhow!("decrypt: seq slice wrong length"))?,
        );

        // Replay check first — refuses to spend AEAD CPU on known-replayed bytes.
        self.check_and_record_seq(seq)?;

        let nonce = nonce_from_u64(seq);
        self.cipher
            .decrypt(&nonce, &data[8..])
            .map_err(|e| anyhow::anyhow!("decrypt: {e}"))
    }

    fn check_and_record_seq(&self, seq: u64) -> Result<()> {
        let mut w = self.replay.lock().unwrap_or_else(|p| p.into_inner());

        if !w.initialised {
            w.highest = seq;
            w.bitmap = 1;
            w.initialised = true;
            return Ok(());
        }

        if seq > w.highest {
            // Newer than anything seen — slide window forward.
            let delta = seq - w.highest;
            if delta >= 128 {
                w.bitmap = 1;
            } else {
                w.bitmap = (w.bitmap << delta) | 1;
            }
            w.highest = seq;
            Ok(())
        } else if seq == w.highest {
            bail!("replay: seq {seq} matches highest");
        } else {
            let offset = w.highest - seq;
            if offset >= REPLAY_WINDOW {
                bail!("replay: seq {} too old (highest={})", seq, w.highest);
            }
            let bit = 1u128 << offset;
            if w.bitmap & bit != 0 {
                bail!("replay: seq {seq} already seen");
            }
            w.bitmap |= bit;
            Ok(())
        }
    }

    /// Raw key bytes — for moving cipher across crate boundaries.
    /// Treat as sensitive; do not log.
    pub fn key_bytes(&self) -> &[u8; 32] {
        &self.key_bytes
    }
}

// SessionCipher is intentionally NOT `Clone`. Cloning would duplicate the
// send counter while sharing the key — textbook nonce reuse, breaking
// ChaCha20-Poly1305 confidentiality and integrity. Use `derive_subkey`.

fn nonce_from_u64(v: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&v.to_le_bytes());
    Nonce::from(n)
}

// ─── Key exchange ─────────────────────────────────────────────────────────────

pub struct KeyPair {
    secret: EphemeralSecret,
    pub public: PublicKey,
}

impl KeyPair {
    pub fn generate() -> Self {
        let secret = EphemeralSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn diffie_hellman(self, peer_public: &[u8; 32]) -> [u8; 32] {
        let peer = PublicKey::from(*peer_public);
        let shared = self.secret.diffie_hellman(&peer);
        *shared.as_bytes()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> SessionCipher {
        SessionCipher::new([42u8; 32])
    }

    fn pair() -> (SessionCipher, SessionCipher) {
        // For roundtrip tests we need same key but separate send-counters.
        // In production this is achieved by sender using `tx` subkey and
        // receiver using `rx` subkey on the OPPOSITE direction.
        // For these unit tests we just construct two with the same raw key.
        (cipher(), cipher())
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let (sender, receiver) = pair();
        let pt = b"hello world";
        let ct = sender.encrypt(pt).unwrap();
        let recovered = receiver.decrypt(&ct).unwrap();
        assert_eq!(recovered, pt);
    }

    #[test]
    fn replay_rejected() {
        let (sender, receiver) = pair();
        let ct = sender.encrypt(b"once").unwrap();
        assert!(receiver.decrypt(&ct).is_ok());
        // Identical ciphertext → same seq → replay rejection
        assert!(receiver.decrypt(&ct).is_err());
    }

    #[test]
    fn out_of_order_within_window_ok() {
        let (sender, receiver) = pair();
        let cts: Vec<_> = (0..5)
            .map(|i| sender.encrypt(format!("msg-{i}").as_bytes()).unwrap())
            .collect();
        // Receive newest first, then older
        assert!(receiver.decrypt(&cts[4]).is_ok());
        assert!(receiver.decrypt(&cts[2]).is_ok());
        assert!(receiver.decrypt(&cts[0]).is_ok());
        assert!(receiver.decrypt(&cts[1]).is_ok());
        assert!(receiver.decrypt(&cts[3]).is_ok());
        // Any replay rejected
        assert!(receiver.decrypt(&cts[2]).is_err());
    }

    #[test]
    fn very_old_seq_rejected() {
        let (sender, receiver) = pair();
        // Encrypt 200 packets, deliver only the latest
        for _ in 0..199 {
            sender.encrypt(b"skip").unwrap();
        }
        let recent = sender.encrypt(b"recent").unwrap();
        assert!(receiver.decrypt(&recent).is_ok());

        // Build a fake packet claiming seq=0 — replay window rejects before AEAD
        let mut old = Vec::with_capacity(MIN_CIPHERTEXT_LEN);
        old.extend_from_slice(&0u64.to_le_bytes());
        old.extend_from_slice(&[0u8; 16]);
        let err = receiver.decrypt(&old).unwrap_err().to_string();
        assert!(
            err.contains("too old") || err.contains("replay"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_short_input() {
        let r = cipher();
        assert!(r.decrypt(&[]).is_err());
        assert!(r.decrypt(&[0u8; 7]).is_err());
        assert!(r.decrypt(&[0u8; 23]).is_err());
    }

    #[test]
    fn derive_subkey_produces_independent_ciphers() {
        let base = SessionCipher::new([1u8; 32]);
        let tx = base.derive_subkey(b"miru-tx").unwrap();
        let rx = base.derive_subkey(b"miru-rx").unwrap();
        assert_ne!(tx.key_bytes(), rx.key_bytes());
        assert_ne!(tx.key_bytes(), base.key_bytes());
        let ct = tx.encrypt(b"data").unwrap();
        // Different subkey → AEAD auth fails
        assert!(rx.decrypt(&ct).is_err());
    }

    #[test]
    fn first_packet_with_high_seq_handled() {
        // Receiver should accept any first packet, regardless of seq value.
        let r = cipher();
        let s = cipher();
        for _ in 0..1000 {
            s.encrypt(b"discard").unwrap();
        }
        let ct = s.encrypt(b"hello").unwrap();
        let pt = r.decrypt(&ct).unwrap();
        assert_eq!(pt, b"hello");
    }
}
