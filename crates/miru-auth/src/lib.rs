//! Authentication & authorization.
//!
//! Identity:    Ed25519 keypair per device (persistent on disk)
//! Pairing:     6-digit PIN (PBKDF2-SHA256 → ephemeral key for first connection)
//! Long-term:   Argon2id (avoid RustDesk's CVE-2026-30785)
//! Trust:       TOFU (Trust On First Use) — pubkey pinned on accept
//! Permissions: per-device ACL (view-only / control / file-transfer)

pub mod argon;
pub mod fingerprint_log;

use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use ring::pbkdf2;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, num::NonZeroU32, path::Path};
use tracing::info;

pub use argon::{hash_password, verify_password, SecretBytes};

const PBKDF2_ITERATIONS: u32 = 100_000;

// ─── Device identity ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct DeviceIdentity {
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
        }
    }

    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let bytes = std::fs::read(path)?;
            if bytes.len() != 32 {
                bail!("invalid identity file");
            }
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("bad key length"))?;
            let signing_key = SigningKey::from_bytes(&arr);
            let verifying_key = signing_key.verifying_key();
            Ok(Self {
                signing_key,
                verifying_key,
            })
        } else {
            let id = Self::generate();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Write the private key with restricted permissions from the start.
            // On Unix, std::fs::write would create the file 0o644 (world-readable),
            // and a later set_permissions call has a TOCTOU window. Instead, open
            // with O_CREAT|O_EXCL and mode 0o600 atomically via OpenOptionsExt.
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)?;
                f.write_all(&id.signing_key.to_bytes())?;
            }
            #[cfg(not(unix))]
            std::fs::write(path, id.signing_key.to_bytes())?;
            info!("Generated new device identity");
            Ok(id)
        }
    }

    pub fn pubkey_b64(&self) -> String {
        B64.encode(self.verifying_key.as_bytes())
    }

    pub fn pubkey_fingerprint(&self) -> String {
        let digest = ring::digest::digest(&ring::digest::SHA256, self.verifying_key.as_bytes());
        digest.as_ref()[..8]
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.signing_key.sign(msg)
    }

    pub fn verify(pubkey: &[u8; 32], msg: &[u8], sig: &Signature) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(pubkey) else {
            return false;
        };
        // verify_strict (ZIP-215) rejects small-order pubkeys + non-canonical R encodings.
        vk.verify_strict(msg, sig).is_ok()
    }
}

// ─── PIN pairing ──────────────────────────────────────────────────────────────

pub fn generate_pin() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!("{:06}", rng.gen_range(0..1_000_000))
}

/// Derive an AES key from a PIN + device ID using PBKDF2-SHA256.
/// First-connection only (one-time use); subsequent uses TOFU pubkey.
pub fn pin_to_key(pin: &str, device_id: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    let salt = device_id.as_bytes();
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PBKDF2_ITERATIONS).unwrap(),
        salt,
        pin.as_bytes(),
        &mut key,
    );
    key
}

// ─── Permission model ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ViewOnly,
    Control,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub device_id: String,
    pub pubkey_b64: String,
    pub fingerprint: String,
    pub permission: Permission,
    pub first_seen: u64,
    pub last_seen: u64,
    pub friendly_name: Option<String>,
}

// ─── ACL store (TOFU) ─────────────────────────────────────────────────────────

#[derive(Default, Serialize, Deserialize)]
pub struct AclStore {
    peers: HashMap<String, TrustedPeer>,
}

impl AclStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes).unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn check(&self, device_id: &str, pubkey_b64: &str) -> TrustDecision {
        match self.peers.get(device_id) {
            None => TrustDecision::Unknown,
            Some(peer) if peer.pubkey_b64 == pubkey_b64 => TrustDecision::Trusted(peer.permission),
            Some(_) => TrustDecision::PubkeyMismatch,
        }
    }

    pub fn trust(&mut self, peer: TrustedPeer) {
        info!(
            "Trusted peer: {} (fpr {})",
            peer.device_id, peer.fingerprint
        );
        self.peers.insert(peer.device_id.clone(), peer);
    }

    /// Update the `last_seen` timestamp for a known peer (call on every successful reconnect).
    pub fn touch(&mut self, device_id: &str, now: u64) {
        if let Some(peer) = self.peers.get_mut(device_id) {
            peer.last_seen = now;
        }
    }

    pub fn revoke(&mut self, device_id: &str) -> bool {
        self.peers.remove(device_id).is_some()
    }

    pub fn list(&self) -> Vec<&TrustedPeer> {
        self.peers.values().collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    Trusted(Permission),
    Unknown,
    PubkeyMismatch,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_format() {
        for _ in 0..20 {
            let pin = generate_pin();
            assert_eq!(pin.len(), 6);
            assert!(pin.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn pin_to_key_deterministic() {
        let k1 = pin_to_key("123456", "DEVID");
        let k2 = pin_to_key("123456", "DEVID");
        assert_eq!(k1, k2);
    }

    #[test]
    fn pin_to_key_different_pin() {
        let k1 = pin_to_key("123456", "DEVID");
        let k2 = pin_to_key("654321", "DEVID");
        assert_ne!(k1, k2);
    }

    #[test]
    fn ed25519_sign_verify() {
        let id = DeviceIdentity::generate();
        let msg = b"hello miru";
        let sig = id.sign(msg);
        assert!(DeviceIdentity::verify(
            id.verifying_key.as_bytes(),
            msg,
            &sig
        ));
        assert!(!DeviceIdentity::verify(
            id.verifying_key.as_bytes(),
            b"different",
            &sig
        ));
    }

    #[test]
    fn fingerprint_format() {
        let id = DeviceIdentity::generate();
        let fpr = id.pubkey_fingerprint();
        assert_eq!(fpr.len(), 23);
        assert_eq!(fpr.matches(':').count(), 7);
    }

    #[test]
    fn acl_tofu_flow() {
        let mut acl = AclStore::default();
        assert_eq!(acl.check("AAAA-BBBB", "key1"), TrustDecision::Unknown);

        acl.trust(TrustedPeer {
            device_id: "AAAA-BBBB".into(),
            pubkey_b64: "key1".into(),
            fingerprint: "AB:CD".into(),
            permission: Permission::Control,
            first_seen: 0,
            last_seen: 0,
            friendly_name: None,
        });

        assert_eq!(
            acl.check("AAAA-BBBB", "key1"),
            TrustDecision::Trusted(Permission::Control)
        );
        assert_eq!(
            acl.check("AAAA-BBBB", "EVIL_KEY"),
            TrustDecision::PubkeyMismatch
        );
    }

    /// Regression: the pubkey_b64 stored in the ACL MUST be the actual
    /// cryptographic pubkey (base64), NOT the device_id string.
    /// If device_id were stored as pubkey_b64, any peer claiming a known
    /// device_id would pass check() because both sides of the comparison
    /// evaluate to the same device_id string — making TOFU pinning a no-op
    /// and enabling impersonation by any passive eavesdropper on the signal.
    #[test]
    fn acl_touch_updates_last_seen() {
        let mut acl = AclStore::default();
        acl.trust(TrustedPeer {
            device_id: "dev1".into(),
            pubkey_b64: "key1".into(),
            fingerprint: "AB:CD".into(),
            permission: Permission::Control,
            first_seen: 1000,
            last_seen: 1000,
            friendly_name: None,
        });
        acl.touch("dev1", 9999);
        let peer = acl.list().into_iter().find(|p| p.device_id == "dev1").unwrap();
        assert_eq!(peer.last_seen, 9999, "touch must update last_seen");
        assert_eq!(peer.first_seen, 1000, "touch must not change first_seen");
        // touching unknown device is a no-op
        acl.touch("no-such-device", 9999); // must not panic
    }

    #[test]
    fn acl_rejects_device_id_impersonation() {
        let alice_id = DeviceIdentity::generate();
        let mallory_id = DeviceIdentity::generate();
        let device_id_str = "alice-device-uuid";

        // Correctly store Alice's actual pubkey (as base64) in the ACL.
        let alice_pubkey_b64 = alice_id.pubkey_b64();
        let mallory_pubkey_b64 = mallory_id.pubkey_b64();
        // sanity: they must be different keys
        assert_ne!(alice_pubkey_b64, mallory_pubkey_b64);

        let mut acl = AclStore::default();
        acl.trust(TrustedPeer {
            device_id: device_id_str.into(),
            pubkey_b64: alice_pubkey_b64.clone(),
            fingerprint: alice_id.pubkey_fingerprint(),
            permission: Permission::Control,
            first_seen: 0,
            last_seen: 0,
            friendly_name: None,
        });

        // Alice re-connects with her real key → Trusted.
        assert_eq!(
            acl.check(device_id_str, &alice_pubkey_b64),
            TrustDecision::Trusted(Permission::Control),
        );

        // Mallory connects claiming Alice's device_id but with her own key → Mismatch.
        assert_eq!(
            acl.check(device_id_str, &mallory_pubkey_b64),
            TrustDecision::PubkeyMismatch,
            "impersonation must be detected — storing device_id as pubkey_b64 would hide this"
        );

        // Extra: the BUG would have stored device_id_str as pubkey_b64.
        // Verify that pattern is rejected even if we check with the literal device_id.
        assert_eq!(
            acl.check(device_id_str, device_id_str),
            TrustDecision::PubkeyMismatch,
            "device_id must NOT be accepted as a valid pubkey"
        );
    }
}
