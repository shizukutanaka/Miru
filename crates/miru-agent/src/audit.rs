//! Audit log for AI agent actions.
//!
//! Every action by an AI agent is appended (chained via SHA-256) so the log is
//! tamper-evident. If an attacker modifies a past entry, the chain breaks and
//! verification fails.
//!
//! Use cases:
//!   - Compliance ("which actions did the AI take last quarter?")
//!   - Debugging ("why did the agent click on that button?")
//!   - Security forensics ("did the agent escape its sandbox?")

use anyhow::{bail, Context, Result};
use ring::digest;
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::token::Capability;

/// One audit log entry. Append-only; later entries reference earlier hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Sequence number (0 for genesis).
    pub seq: u64,
    /// SHA-256 of the previous entry's bytes (hex). 64 zeros for genesis.
    pub prev_hash: String,
    /// Unix milliseconds.
    pub timestamp_ms: u64,
    /// Token jti this action used.
    pub token_jti: String,
    /// Capability invoked.
    pub capability: Capability,
    /// Action-specific details (JSON-encoded for flexibility).
    pub action: serde_json::Value,
    /// Whether human confirmation was given (Some(true)/Some(false)/None=not required).
    pub confirmed: Option<bool>,
    /// Result: ok / denied / error.
    pub outcome: AuditOutcome,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Ok,
    Denied,
    Error,
}

impl AuditEntry {
    /// SHA-256 of the canonical JSON encoding of this entry.
    pub fn hash(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("serialize entry");
        let d = digest::digest(&digest::SHA256, &bytes);
        hex::encode(d.as_ref())
    }
}

/// Tamper-evident append-only log.
pub struct AuditLog {
    path: PathBuf,
    file: Mutex<BufWriter<std::fs::File>>,
    last_hash: Mutex<String>,
    next_seq: Mutex<u64>,
}

impl AuditLog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Read existing entries to recover the last hash + seq
        let (last_hash, next_seq) = if path.exists() {
            replay_chain(path)?
        } else {
            ("0".repeat(64), 0)
        };

        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        // Restrictive permissions applied atomically at creation on Unix —
        // no window where another local user can open the file first.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts
            .open(path)
            .with_context(|| format!("open audit log {}", path.display()))?;

        // For pre-existing files, mode() above has no effect — tighten them too.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }

        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(BufWriter::new(file)),
            last_hash: Mutex::new(last_hash),
            next_seq: Mutex::new(next_seq),
        })
    }

    /// Append a new entry. Atomic: either fully written + chained or rejected.
    ///
    /// **Privacy**: the action payload is automatically redacted via
    /// [`crate::redact::redact_action`] before being written. Callers may pass
    /// raw plaintext (e.g. a `text` field for KeyType); redaction will replace
    /// it with `text_len` + `text_sha256` per the audit redaction matrix.
    /// This is enforced HERE (not at call sites) so we cannot accidentally
    /// log a secret by forgetting to redact at one call site.
    pub fn append(
        &self,
        token_jti: &str,
        capability: Capability,
        action: serde_json::Value,
        confirmed: Option<bool>,
        outcome: AuditOutcome,
    ) -> Result<u64> {
        // Single source of truth for redaction. Always applied.
        let action = crate::redact::redact_action(capability, action);

        // Mutex poisoning is cooperative — panics elsewhere don't half-write
        // our append-only structures, so recovering with into_inner() is safe.
        let mut last_hash = self.last_hash.lock().unwrap_or_else(|p| p.into_inner());
        let mut next_seq = self.next_seq.lock().unwrap_or_else(|p| p.into_inner());

        let entry = AuditEntry {
            seq: *next_seq,
            prev_hash: last_hash.clone(),
            timestamp_ms: now_ms(),
            token_jti: token_jti.to_string(),
            capability,
            action,
            confirmed,
            outcome,
        };

        let line = serde_json::to_string(&entry)?;
        let entry_hash = entry.hash();

        let mut f = self.file.lock().unwrap_or_else(|p| p.into_inner());
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        // flush() pushes BufWriter's internal buffer to the kernel; sync_data()
        // then tells the OS to commit to storage so a power failure cannot lose
        // the entry. Both are required for durable audit logging.
        f.flush()?;
        f.get_ref().sync_data()?;

        *last_hash = entry_hash;
        let seq = *next_seq;
        *next_seq += 1;
        Ok(seq)
    }

    /// Re-read the entire log and verify the chain. Returns Ok(count) on success.
    pub fn verify(&self) -> Result<u64> {
        let (_, count) = replay_chain(&self.path)?;
        Ok(count)
    }

    /// Read and verify all entries, returning them in order. Fails if the chain
    /// is broken (same integrity check as `verify`). For audit inspection tools.
    pub fn read_all(&self) -> Result<Vec<AuditEntry>> {
        read_all_verified(&self.path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Read all entries; verify each entry's prev_hash matches the previous entry's hash.
/// Returns (last_hash, next_seq) on success.
fn replay_chain(path: &Path) -> Result<(String, u64)> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path)?;
    let r = BufReader::new(f);

    let mut prev_hash = "0".repeat(64);
    let mut seq = 0u64;
    for (lineno, line) in r.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: AuditEntry =
            serde_json::from_str(&line).with_context(|| format!("parse line {}", lineno + 1))?;

        if entry.seq != seq {
            bail!(
                "seq mismatch at line {}: expected {}, got {}",
                lineno + 1,
                seq,
                entry.seq
            );
        }
        if entry.prev_hash != prev_hash {
            bail!(
                "chain broken at seq {}: expected prev_hash={}, got {}",
                seq,
                prev_hash,
                entry.prev_hash
            );
        }

        prev_hash = entry.hash();
        seq += 1;
    }

    Ok((prev_hash, seq))
}

/// Verify a log file's hash-chain integrity without opening it for writing.
/// Returns the entry count on success, or a descriptive error on tampering.
/// This is the entry point for offline audit verification tools.
pub fn verify_path(path: &Path) -> Result<u64> {
    let (_, count) = replay_chain(path)?;
    Ok(count)
}

/// Read all entries while verifying the hash chain. Returns the entries in
/// order on success; errors if the chain is broken (tamper-evident).
/// Public entry point for offline audit inspection tools.
pub fn read_all_verified(path: &Path) -> Result<Vec<AuditEntry>> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path)?;
    let r = BufReader::new(f);

    let mut entries = Vec::new();
    let mut prev_hash = "0".repeat(64);
    let mut seq = 0u64;
    for (lineno, line) in r.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: AuditEntry =
            serde_json::from_str(&line).with_context(|| format!("parse line {}", lineno + 1))?;
        if entry.seq != seq {
            bail!(
                "seq mismatch at line {}: expected {}, got {}",
                lineno + 1,
                seq,
                entry.seq
            );
        }
        if entry.prev_hash != prev_hash {
            bail!("chain broken at seq {seq}: prev_hash mismatch");
        }
        prev_hash = entry.hash();
        seq += 1;
        entries.push(entry);
    }
    Ok(entries)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ─── hex helpers (avoid pulling in `hex` crate) ──────────────────────────────

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_log_path() -> PathBuf {
        let id = format!(
            "miru-audit-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(id)
    }

    #[test]
    fn append_and_verify() {
        let path = temp_log_path();
        let log = AuditLog::open(&path).unwrap();

        log.append(
            "jti1",
            Capability::ScreenRead,
            json!({"x": 100}),
            None,
            AuditOutcome::Ok,
        )
        .unwrap();
        log.append(
            "jti1",
            Capability::PointerClick,
            json!({"x": 100, "y": 50}),
            None,
            AuditOutcome::Ok,
        )
        .unwrap();
        log.append(
            "jti2",
            Capability::ShellExec,
            json!({"cmd": "ls"}),
            Some(false),
            AuditOutcome::Denied,
        )
        .unwrap();

        assert_eq!(log.verify().unwrap(), 3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn chain_breaks_on_tamper() {
        let path = temp_log_path();
        {
            let log = AuditLog::open(&path).unwrap();
            log.append(
                "jti",
                Capability::ScreenRead,
                json!({}),
                None,
                AuditOutcome::Ok,
            )
            .unwrap();
            log.append(
                "jti",
                Capability::PointerMove,
                json!({"x": 1}),
                None,
                AuditOutcome::Ok,
            )
            .unwrap();
        }

        // Tamper: modify the first line's content
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        lines[0] = lines[0].replace("\"x\":1", "\"x\":99");
        // We don't have x:1 in line 0; replace timestamp instead
        lines[0] = lines[0].replacen(
            "\"capability\":\"screen_read\"",
            "\"capability\":\"shell_exec\"",
            1,
        );
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let log2 = AuditLog::open(&path);
        // open() runs replay_chain which will detect the broken chain
        assert!(log2.is_err() || log2.unwrap().verify().is_err());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reopens_correctly() {
        let path = temp_log_path();
        {
            let log = AuditLog::open(&path).unwrap();
            log.append(
                "jti",
                Capability::ScreenRead,
                json!({}),
                None,
                AuditOutcome::Ok,
            )
            .unwrap();
        }
        // Reopen and append more
        let log = AuditLog::open(&path).unwrap();
        log.append(
            "jti",
            Capability::PointerMove,
            json!({"x": 5}),
            None,
            AuditOutcome::Ok,
        )
        .unwrap();
        assert_eq!(log.verify().unwrap(), 2);

        std::fs::remove_file(&path).ok();
    }
}
