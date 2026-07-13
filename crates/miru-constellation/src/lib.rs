//! Constellation — multi-device presence federation.
//!
//! All competitors model "device A connects to device B" as a 1:1 session.
//! Real life is N:N: I have a laptop, phone, desktop, and a server. I want
//! to see all of them, hand off between them, and treat them as one fabric.
//!
//! # Concepts
//!
//! - **Constellation** — a set of devices owned by one user, identified by
//!   a "constellation key" (Ed25519). Each device is signed into the constellation.
//! - **Capability advertisement** — each device broadcasts what it can do
//!   (display resolutions, has microphone, current battery, idle/active).
//! - **Hub** — the device currently coordinating discovery (typically the most
//!   stable device — laptop or desktop). Hubs swap if one goes offline.
//! - **Handoff** — moving an active session from device A to device B without
//!   re-pairing or losing state. Like Apple Continuity but cross-OS.
//!
//! # Why Ed25519 not OAuth or accounts
//!
//! Centralized accounts are a single point of compromise (see: every breach
//! in the past decade). Each device holds its own key; the constellation is
//! a graph of cryptographically signed memberships. No central authority.

use anyhow::{bail, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

// ─── Capabilities a device advertises ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceCapabilities {
    /// Display resolutions (px), if any. Empty for headless.
    pub displays: Vec<(u32, u32)>,
    /// Has microphone.
    pub has_microphone: bool,
    /// Has speakers.
    pub has_speakers: bool,
    /// Has camera.
    pub has_camera: bool,
    /// Has GPU encode capability (NVENC/AMF/QSV/VT/VAAPI).
    pub has_hw_encode: bool,
    /// Battery level (0-100), or None on AC.
    pub battery_pct: Option<u8>,
    /// Form factor for UI display.
    pub form_factor: FormFactor,
    /// Free name like "Desktop ホームオフィス".
    pub friendly_name: String,
    /// OS family ("linux", "macos", "windows", "ios", "android").
    pub os_family: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FormFactor {
    Desktop,
    Laptop,
    Phone,
    Tablet,
    Server,
    Embedded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    /// User is actively using this device.
    Active,
    /// Device is on but not actively used (screen locked, idle > 5min).
    Idle,
    /// Device is on but in low-power mode.
    Sleeping,
    /// Last-known state before going offline.
    Offline,
}

// ─── Device record signed into a constellation ────────────────────────────────

/// A device joining a constellation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceJoin {
    pub device_id: Uuid,
    /// Device's own pubkey (separate from constellation key).
    pub device_pubkey_b64: String,
    /// When the device joined (unix seconds).
    pub joined_at: u64,
    /// Signature by the constellation key over `device_pubkey || joined_at || device_id`.
    pub constellation_signature_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePresence {
    pub device_id: Uuid,
    pub capabilities: DeviceCapabilities,
    pub status: DeviceStatus,
    /// Last update (unix seconds).
    pub last_seen: u64,
    /// Reachable via this signal URL.
    pub signal_url: Option<String>,
}

// ─── Constellation registry ───────────────────────────────────────────────────

/// In-process state of the constellation: known devices + their presence.
pub struct Constellation {
    constellation_pubkey: VerifyingKey,
    members: RwLock<HashMap<Uuid, DeviceJoin>>,
    presence: RwLock<HashMap<Uuid, DevicePresence>>,
}

impl Constellation {
    pub fn new(constellation_pubkey: VerifyingKey) -> Self {
        Self {
            constellation_pubkey,
            members: RwLock::new(HashMap::new()),
            presence: RwLock::new(HashMap::new()),
        }
    }

    /// Issue a new join token (called on the constellation's "primary" device,
    /// the one that holds the constellation private key — typically a desktop).
    pub fn issue_join(constellation_key: &SigningKey, device_pubkey: &VerifyingKey) -> DeviceJoin {
        use base64::{engine::general_purpose::STANDARD as B64, Engine};

        let device_id = Uuid::new_v4();
        let joined_at = unix_now();
        let mut blob = Vec::with_capacity(64 + 16);
        blob.extend_from_slice(device_pubkey.as_bytes());
        blob.extend_from_slice(&joined_at.to_le_bytes());
        blob.extend_from_slice(device_id.as_bytes());

        let sig: Signature = constellation_key.sign(&blob);

        DeviceJoin {
            device_id,
            device_pubkey_b64: B64.encode(device_pubkey.as_bytes()),
            joined_at,
            constellation_signature_b64: B64.encode(sig.to_bytes()),
        }
    }

    /// Verify and accept a device join.
    pub fn accept_join(&self, join: DeviceJoin) -> Result<()> {
        use base64::{engine::general_purpose::STANDARD as B64, Engine};

        let pk_bytes = B64.decode(&join.device_pubkey_b64)?;
        let pk_arr: [u8; 32] = pk_bytes.try_into().map_err(|_| anyhow::anyhow!("pk len"))?;
        // Verify the signature
        let mut blob = Vec::with_capacity(64);
        blob.extend_from_slice(&pk_arr);
        blob.extend_from_slice(&join.joined_at.to_le_bytes());
        blob.extend_from_slice(join.device_id.as_bytes());

        let sig_bytes = B64.decode(&join.constellation_signature_b64)?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("sig len"))?;
        let sig = Signature::from_bytes(&sig_arr);

        self.constellation_pubkey
            .verify_strict(&blob, &sig)
            .map_err(|_| anyhow::anyhow!("constellation signature invalid"))?;

        self.members.write().insert(join.device_id, join);
        Ok(())
    }

    /// Update the presence info for a member device.
    pub fn update_presence(&self, presence: DevicePresence) -> Result<()> {
        if !self.members.read().contains_key(&presence.device_id) {
            bail!("device not a constellation member");
        }
        self.presence.write().insert(presence.device_id, presence);
        Ok(())
    }

    /// Snapshot of all online members + their presence.
    pub fn snapshot(&self) -> Vec<DevicePresence> {
        let presence = self.presence.read();
        let members = self.members.read();
        members
            .keys()
            .filter_map(|id| presence.get(id).cloned())
            .collect()
    }

    pub fn member_count(&self) -> usize {
        self.members.read().len()
    }
}

// ─── Handoff token ────────────────────────────────────────────────────────────

/// A handoff is "device A is currently in a session with peer X; device B
/// takes over." The token authorizes peer X to accept device B's connection
/// in place of device A's, without re-running the trust-on-first-use flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffToken {
    pub session_id: Uuid,
    /// Outgoing device.
    pub from_device: Uuid,
    /// Incoming device.
    pub to_device: Uuid,
    /// Unix seconds when the token expires (typical 60s).
    pub expires_at: u64,
    /// Constellation signature.
    pub signature_b64: String,
}

/// Hard cap on handoff TTL — mirrors miru_agent::token::MAX_TTL_SECS. Without
/// this, an unbounded caller-supplied ttl_secs could overflow the
/// unix_now() + ttl_secs addition (panicking in debug/overflow-checked
/// builds, wrapping to an unexpected timestamp in release).
pub const MAX_HANDOFF_TTL_SECS: u64 = 300;

impl HandoffToken {
    pub fn issue(
        constellation_key: &SigningKey,
        session_id: Uuid,
        from_device: Uuid,
        to_device: Uuid,
        ttl_secs: u64,
    ) -> Self {
        use base64::{engine::general_purpose::STANDARD as B64, Engine};
        let expires_at = unix_now().saturating_add(ttl_secs.min(MAX_HANDOFF_TTL_SECS));
        let mut blob = Vec::with_capacity(56); // 3×16 (UUIDs) + 8 (expires_at)
        blob.extend_from_slice(session_id.as_bytes());
        blob.extend_from_slice(from_device.as_bytes());
        blob.extend_from_slice(to_device.as_bytes());
        blob.extend_from_slice(&expires_at.to_le_bytes());
        let sig: Signature = constellation_key.sign(&blob);

        Self {
            session_id,
            from_device,
            to_device,
            expires_at,
            signature_b64: B64.encode(sig.to_bytes()),
        }
    }

    pub fn verify(&self, constellation_pubkey: &VerifyingKey) -> Result<()> {
        use base64::{engine::general_purpose::STANDARD as B64, Engine};
        if unix_now() > self.expires_at {
            bail!("handoff token expired");
        }
        let mut blob = Vec::with_capacity(56); // 3×16 (UUIDs) + 8 (expires_at)
        blob.extend_from_slice(self.session_id.as_bytes());
        blob.extend_from_slice(self.from_device.as_bytes());
        blob.extend_from_slice(self.to_device.as_bytes());
        blob.extend_from_slice(&self.expires_at.to_le_bytes());
        let sig_bytes = B64.decode(&self.signature_b64)?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("sig len"))?;
        let sig = Signature::from_bytes(&sig_arr);
        constellation_pubkey
            .verify_strict(&blob, &sig)
            .map_err(|_| anyhow::anyhow!("handoff signature invalid"))?;
        Ok(())
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn make_caps() -> DeviceCapabilities {
        DeviceCapabilities {
            displays: vec![(2560, 1440)],
            has_microphone: true,
            has_speakers: true,
            has_camera: true,
            has_hw_encode: true,
            battery_pct: None,
            form_factor: FormFactor::Desktop,
            friendly_name: "Desktop".into(),
            os_family: "linux".into(),
        }
    }

    #[test]
    fn join_and_verify() {
        let constellation_key = SigningKey::generate(&mut OsRng);
        let device_key = SigningKey::generate(&mut OsRng);

        let join = Constellation::issue_join(&constellation_key, &device_key.verifying_key());

        let constellation = Constellation::new(constellation_key.verifying_key());
        constellation.accept_join(join).unwrap();
        assert_eq!(constellation.member_count(), 1);
    }

    #[test]
    fn join_with_wrong_constellation_key_rejected() {
        let real_key = SigningKey::generate(&mut OsRng);
        let attacker_key = SigningKey::generate(&mut OsRng);
        let device_key = SigningKey::generate(&mut OsRng);

        // Attacker forges a join with their own key
        let forged = Constellation::issue_join(&attacker_key, &device_key.verifying_key());

        let constellation = Constellation::new(real_key.verifying_key());
        assert!(constellation.accept_join(forged).is_err());
    }

    #[test]
    fn presence_update_requires_membership() {
        let constellation_key = SigningKey::generate(&mut OsRng);
        let constellation = Constellation::new(constellation_key.verifying_key());

        let unknown_id = Uuid::new_v4();
        let presence = DevicePresence {
            device_id: unknown_id,
            capabilities: make_caps(),
            status: DeviceStatus::Active,
            last_seen: unix_now(),
            signal_url: None,
        };
        assert!(constellation.update_presence(presence).is_err());
    }

    #[test]
    fn handoff_token_verifies() {
        let key = SigningKey::generate(&mut OsRng);
        let token = HandoffToken::issue(&key, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), 60);
        assert!(token.verify(&key.verifying_key()).is_ok());
    }

    #[test]
    fn handoff_token_rejects_other_signer() {
        let key1 = SigningKey::generate(&mut OsRng);
        let key2 = SigningKey::generate(&mut OsRng);
        let token = HandoffToken::issue(&key1, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), 60);
        assert!(token.verify(&key2.verifying_key()).is_err());
    }
}
