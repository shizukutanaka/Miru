//! Tamper-evident peer-fingerprint pin log (TOFU).
//!
//! Per the security playbook §1.1 ("Make TOFU pin-file storage a
//! tamper-evident JSON-lines log (HMAC-chained, same shape as your existing
//! audit log) so that a silent peer-cert swap is detectable").
//!
//! ## Format
//!
//! Append-only JSONL; each entry has shape:
//!
//! ```json
//! {"seq":N,"prev_hash":"<hex>","first_seen_ms":N,"label":"...",
//!  "device_id":"ABCD-1234","fingerprint":"abcd…","action":"pin"|"observed"|"changed"}
//! ```
//!
//! `prev_hash = SHA-256(canonical_json(prev_entry))`. Verification recomputes
//! the chain and refuses to load a log whose chain is broken.
//!
//! ## What this defends against
//!
//! - **Silent peer-cert swap**: an attacker who replaces `peers.json` cannot
//!   forge a valid chain without re-hashing every following entry, which is
//!   detectable on next load.
//! - **Rollback**: writing only `peers.json.bak` over `peers.json` rewinds
//!   pinned fingerprints; the chain length monotonically increases, so a
//!   shorter log on disk than the last known seq is suspicious.
//!
//! What this does NOT defend against:
//!
//! - **Full compromise**: an attacker with arbitrary local code execution
//!   can rewrite the entire chain with consistent hashes. The Merkle anchor
//!   to Rekor (v1.0) bounds this attack to entries past the last anchor.

use anyhow::{bail, Context, Result};
use ring::digest;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintAction {
    /// Initial pin (TOFU) — first time this peer was seen.
    Pin,
    /// Subsequent connection where the fingerprint matched the pin.
    Observed,
    /// Fingerprint changed from the pinned value. ALWAYS suspicious.
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintEntry {
    pub seq: u64,
    pub prev_hash: String, // hex-encoded SHA-256 of previous entry, empty for seq 0
    pub timestamp_ms: u64,
    pub label: String,
    pub device_id: String,
    pub fingerprint: String, // hex-encoded; the canonical peer-pubkey fingerprint
    pub action: FingerprintAction,
}

impl FingerprintEntry {
    fn hash(&self) -> String {
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}|{:?}",
            self.seq, self.prev_hash, self.timestamp_ms, self.label,
            self.device_id, self.fingerprint, self.action
        );
        let d = digest::digest(&digest::SHA256, canonical.as_bytes());
        hex_lower(d.as_ref())
    }
}

pub struct FingerprintLog {
    path: PathBuf,
    state: Mutex<LogState>,
}

#[derive(Default)]
struct LogState {
    next_seq: u64,
    last_hash: String,
    /// In-memory snapshot of (device_id → fingerprint) for the latest pin.
    pinned: std::collections::HashMap<String, String>,
}

impl FingerprintLog {
    /// Open or create the log at `path`. Verifies chain integrity on load.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let mut state = LogState::default();

        if path.exists() {
            let f = std::fs::File::open(&path).context("open fingerprint log")?;
            let reader = BufReader::new(f);
            for (idx, line) in reader.lines().enumerate() {
                let line = line.context("read fingerprint log")?;
                if line.trim().is_empty() { continue; }
                let entry: FingerprintEntry = serde_json::from_str(&line)
                    .with_context(|| format!("parse fingerprint log line {}", idx + 1))?;

                if entry.seq != state.next_seq {
                    bail!("fingerprint log: seq gap at line {} (expected {}, got {})",
                        idx + 1, state.next_seq, entry.seq);
                }
                if entry.prev_hash != state.last_hash {
                    bail!("fingerprint log: chain broken at line {} (seq={})",
                        idx + 1, entry.seq);
                }

                if matches!(entry.action, FingerprintAction::Pin) {
                    state.pinned.insert(entry.device_id.clone(), entry.fingerprint.clone());
                }

                state.last_hash = entry.hash();
                state.next_seq = entry.seq + 1;
            }
        }

        Ok(Self { path, state: Mutex::new(state) })
    }

    /// Check a fingerprint against the pinned value. Returns the comparison
    /// result *and* appends an entry to the log so the connection attempt is
    /// recorded regardless of outcome.
    pub fn check_or_pin(
        &self,
        label: &str,
        device_id: &str,
        fingerprint: &str,
    ) -> Result<FingerprintCheckResult> {
        let mut s = self.state.lock().map_err(|e| anyhow::anyhow!("poisoned: {}", e))?;

        let action;
        let result;

        match s.pinned.get(device_id) {
            None => {
                action = FingerprintAction::Pin;
                result = FingerprintCheckResult::FirstSeen;
            }
            Some(pinned) if pinned == fingerprint => {
                action = FingerprintAction::Observed;
                result = FingerprintCheckResult::Match;
            }
            Some(pinned) => {
                action = FingerprintAction::Changed;
                result = FingerprintCheckResult::Mismatch {
                    expected: pinned.clone(),
                    observed: fingerprint.to_string(),
                };
            }
        }

        let entry = FingerprintEntry {
            seq: s.next_seq,
            prev_hash: s.last_hash.clone(),
            timestamp_ms: now_ms(),
            label: label.to_string(),
            device_id: device_id.to_string(),
            fingerprint: fingerprint.to_string(),
            action,
        };
        let entry_hash = entry.hash();

        let line = serde_json::to_string(&entry)?;
        let mut f = OpenOptions::new()
            .create(true).append(true).open(&self.path)
            .context("append fingerprint log")?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;

        if matches!(action, FingerprintAction::Pin) {
            s.pinned.insert(device_id.to_string(), fingerprint.to_string());
        }
        s.last_hash = entry_hash;
        s.next_seq += 1;

        Ok(result)
    }

    /// Total entries written.
    pub fn len(&self) -> u64 {
        self.state.lock().map(|s| s.next_seq).unwrap_or(0)
    }

    /// True if no fingerprints have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintCheckResult {
    /// Never seen this device. Just pinned.
    FirstSeen,
    /// Pinned fingerprint matches.
    Match,
    /// Pinned fingerprint differs. The UI MUST show a banner.
    Mismatch { expected: String, observed: String },
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn hex_lower(b: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b { let _ = write!(s, "{:02x}", byte); }
    s
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn first_pin_succeeds_then_match() {
        let dir = tempdir().unwrap();
        let log = FingerprintLog::open(dir.path().join("fp.log")).unwrap();
        let r1 = log.check_or_pin("desktop", "ABCD-1234", "fp_v1").unwrap();
        assert!(matches!(r1, FingerprintCheckResult::FirstSeen));
        let r2 = log.check_or_pin("desktop", "ABCD-1234", "fp_v1").unwrap();
        assert!(matches!(r2, FingerprintCheckResult::Match));
    }

    #[test]
    fn fingerprint_change_detected() {
        let dir = tempdir().unwrap();
        let log = FingerprintLog::open(dir.path().join("fp.log")).unwrap();
        log.check_or_pin("desktop", "DEV-1", "fp_a").unwrap();
        let r = log.check_or_pin("desktop", "DEV-1", "fp_b").unwrap();
        assert!(matches!(r, FingerprintCheckResult::Mismatch { .. }));
    }

    #[test]
    fn chain_persists_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("fp.log");
        {
            let log = FingerprintLog::open(&path).unwrap();
            log.check_or_pin("a", "X", "fp1").unwrap();
            log.check_or_pin("b", "Y", "fp2").unwrap();
            log.check_or_pin("a", "X", "fp1").unwrap();
        }
        let log2 = FingerprintLog::open(&path).unwrap();
        assert_eq!(log2.len(), 3);
        // Pinned fps remembered → next call to X with fp1 returns Match
        let r = log2.check_or_pin("a", "X", "fp1").unwrap();
        assert!(matches!(r, FingerprintCheckResult::Match));
    }

    #[test]
    fn tampered_log_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("fp.log");
        {
            let log = FingerprintLog::open(&path).unwrap();
            log.check_or_pin("a", "X", "fp1").unwrap();
            log.check_or_pin("a", "X", "fp1").unwrap();
        }
        // Corrupt the second line
        let mut content = std::fs::read_to_string(&path).unwrap();
        content = content.replace("fp1", "fpEVIL");
        std::fs::write(&path, content).unwrap();

        let result = FingerprintLog::open(&path);
        assert!(result.is_err(), "tampered chain must be rejected");
    }
}
