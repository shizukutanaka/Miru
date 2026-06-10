//! Token revocation registry — the "panic" rotation primitive.
//!
//! Per the security playbook §1.5 ("Token rotation on suspected compromise"):
//!
//! > Provide a one-click "panic" UI that rotates *all* outstanding capability
//! > tokens, dumps the audit-log tip to Rekor, and disconnects all peers.
//! > This is the analogue of AnyDesk's mandatory password reset.
//!
//! ## Design
//!
//! Two complementary mechanisms compose into a "rotation" event:
//!
//! 1. **Revocation list** (this module). Persistent, append-only set of JTI
//!    UUIDs that should be rejected even if their signature + expiry checks
//!    pass. New revocations apply immediately to all in-flight token
//!    verifications. The host UI's "panic" button calls
//!    [`RevocationList::revoke_all_known`].
//!
//! 2. **Issuer key rotation**. The host's Ed25519 issuer key can be
//!    re-generated, after which previously-minted tokens fail signature
//!    verification at the agent worker. This is heavier (the user must
//!    re-issue tokens) but is the strongest control.
//!
//! In v0.1 we ship only (1). Issuer-key rotation is v1.0.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevocationEntry {
    pub jti: Uuid,
    pub revoked_at_ms: u64,
    pub reason: String,
}

/// Upper bound on entries loaded into memory. Tokens are TTL-capped at 15
/// minutes, so a list anywhere near this size means the file is corrupted or
/// an attacker is using it as a memory-exhaustion vector — refuse to start
/// rather than OOM.
pub const MAX_REVOKED_ENTRIES: usize = 1_000_000;

pub struct RevocationList {
    path: PathBuf,
    revoked: RwLock<HashSet<Uuid>>,
}

impl RevocationList {
    /// Open or create the revocation list at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let mut revoked = HashSet::new();
        if path.exists() {
            let f = std::fs::File::open(&path).context("open revocation list")?;
            for (idx, line) in BufReader::new(f).lines().enumerate() {
                let line = line.context("read revocation list")?;
                if line.trim().is_empty() { continue; }
                let entry: RevocationEntry = serde_json::from_str(&line)
                    .with_context(|| format!("parse revocation entry line {}", idx + 1))?;
                revoked.insert(entry.jti);
                if revoked.len() > MAX_REVOKED_ENTRIES {
                    anyhow::bail!(
                        "revocation list exceeds {MAX_REVOKED_ENTRIES} entries — refusing to load (corrupted or hostile file?)"
                    );
                }
            }
        }
        Ok(Self {
            path,
            revoked: RwLock::new(revoked),
        })
    }

    /// Revoke a specific token by JTI. Idempotent.
    pub fn revoke(&self, jti: Uuid, reason: impl Into<String>) -> Result<()> {
        let reason = reason.into();
        {
            let mut g = self.revoked.write().map_err(|e| anyhow::anyhow!("poisoned: {e}"))?;
            if !g.insert(jti) {
                // Already revoked; don't double-write.
                return Ok(());
            }
        }
        let entry = RevocationEntry {
            jti,
            revoked_at_ms: now_ms(),
            reason,
        };
        let line = serde_json::to_string(&entry)?;
        let mut f = OpenOptions::new()
            .create(true).append(true).open(&self.path)
            .context("append revocation list")?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        Ok(())
    }

    /// Panic rotation: revoke every JTI in `known_jtis`. Use the host UI's
    /// in-memory list of issued tokens. Returns count revoked.
    ///
    /// This is the user-visible "log out everywhere" button.
    pub fn revoke_all_known(&self, known_jtis: &[Uuid], reason: impl Into<String>) -> Result<usize> {
        let reason = reason.into();
        let mut count = 0;
        for jti in known_jtis {
            self.revoke(*jti, reason.clone())?;
            count += 1;
        }
        Ok(count)
    }

    /// Check whether a JTI is revoked.
    pub fn is_revoked(&self, jti: &Uuid) -> bool {
        self.revoked.read().map(|s| s.contains(jti)).unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.revoked.read().map(|s| s.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn revoke_then_check() {
        let dir = tempdir().unwrap();
        let rl = RevocationList::open(dir.path().join("rev.log")).unwrap();
        let jti = Uuid::new_v4();
        assert!(!rl.is_revoked(&jti));
        rl.revoke(jti, "test").unwrap();
        assert!(rl.is_revoked(&jti));
    }

    #[test]
    fn revocation_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rev.log");
        let jti1 = Uuid::new_v4();
        let jti2 = Uuid::new_v4();
        {
            let rl = RevocationList::open(&path).unwrap();
            rl.revoke(jti1, "first").unwrap();
            rl.revoke(jti2, "second").unwrap();
        }
        let rl2 = RevocationList::open(&path).unwrap();
        assert!(rl2.is_revoked(&jti1));
        assert!(rl2.is_revoked(&jti2));
        assert_eq!(rl2.len(), 2);
    }

    #[test]
    fn panic_rotation_revokes_all() {
        let dir = tempdir().unwrap();
        let rl = RevocationList::open(dir.path().join("rev.log")).unwrap();
        let jtis: Vec<_> = (0..5).map(|_| Uuid::new_v4()).collect();
        let n = rl.revoke_all_known(&jtis, "panic").unwrap();
        assert_eq!(n, 5);
        for jti in &jtis {
            assert!(rl.is_revoked(jti));
        }
    }

    #[test]
    fn revoke_is_idempotent() {
        let dir = tempdir().unwrap();
        let rl = RevocationList::open(dir.path().join("rev.log")).unwrap();
        let jti = Uuid::new_v4();
        rl.revoke(jti, "a").unwrap();
        rl.revoke(jti, "b").unwrap(); // ignored
        assert_eq!(rl.len(), 1);
    }
}
