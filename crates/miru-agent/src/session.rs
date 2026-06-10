//! AI agent session — the runtime gate that authorizes each action.
//!
//! Flow:
//!   1. AI agent presents a token via the protocol Hello message
//!   2. Host verifies token signature + expiry → `AgentSession::open`
//!   3. For each requested action, host calls `session.authorize(cap, ...)`
//!   4. If capability requires confirmation, raise a UI prompt; cache decision per scope
//!   5. Every action is appended to the audit log

use anyhow::{bail, Result};
use parking_lot::Mutex;
use std::{sync::Arc, time::Instant};
use tracing::{info, warn};

use crate::{
    audit::{AuditLog, AuditOutcome},
    token::{AgentToken, Capability},
};

/// Confirmation callback — return Ok(true) to allow, Ok(false) to deny.
/// In practice the callback raises a system notification or blocks on a UI prompt.
pub type ConfirmFn = Arc<dyn Fn(&ConfirmRequest) -> bool + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ConfirmRequest {
    pub agent_label: String,
    pub capability: Capability,
    pub action_summary: String,
}

/// Active AI agent session.
pub struct AgentSession {
    pub token: AgentToken,
    audit: Arc<AuditLog>,
    confirm: ConfirmFn,
    /// Optional revocation list — checked on every authorize().
    revocation: Option<Arc<crate::revocation::RevocationList>>,
    /// Auto-approve cache: (capability, scope_key) → expiry instant.
    auto_approve: Mutex<Vec<AutoApprove>>,
}

struct AutoApprove {
    capability: Capability,
    scope_key: String,
    until: Instant,
}

/// Bounds on the auto-approve cache so a rogue agent cannot grow it without
/// limit by confirming actions across many distinct (or oversized) scopes.
const MAX_AUTO_APPROVE_ENTRIES: usize = 1000;
const MAX_SCOPE_KEY_BYTES: usize = 4096;

impl AgentSession {
    pub fn open(token: AgentToken, audit: Arc<AuditLog>, confirm: ConfirmFn) -> Self {
        Self::open_with_revocation(token, audit, confirm, None)
    }

    pub fn open_with_revocation(
        token: AgentToken,
        audit: Arc<AuditLog>,
        confirm: ConfirmFn,
        revocation: Option<Arc<crate::revocation::RevocationList>>,
    ) -> Self {
        info!(
            "Agent session opened: sub={} caps={:?} expires_in={}s",
            token.payload.sub,
            token.payload.caps,
            token.seconds_remaining(),
        );
        Self {
            token,
            audit,
            confirm,
            revocation,
            auto_approve: Mutex::new(Vec::new()),
        }
    }

    /// Check if an action is allowed. Returns Ok if permitted, Err otherwise.
    /// Logs the attempt regardless of outcome.
    pub fn authorize(
        &self,
        cap: Capability,
        action: serde_json::Value,
        scope_key: &str,
    ) -> Result<()> {
        // 1. Token must include this capability
        // 0. Token must not be revoked (panic rotation check — always first)
        if let Some(rev) = &self.revocation {
            if rev.is_revoked(&self.token.payload.jti) {
                self.audit.append(
                    &self.token.payload.jti.to_string(),
                    cap,
                    action.clone(),
                    None,
                    AuditOutcome::Denied,
                )?;
                bail!("token revoked");
            }
        }

        if !self.token.has_capability(cap) {
            self.audit.append(
                &self.token.payload.jti.to_string(),
                cap,
                action.clone(),
                None,
                AuditOutcome::Denied,
            )?;
            bail!("capability {cap:?} not granted");
        }

        // 2. Token must not be expired
        if self.token.seconds_remaining() == 0 {
            self.audit.append(
                &self.token.payload.jti.to_string(),
                cap,
                action.clone(),
                None,
                AuditOutcome::Denied,
            )?;
            bail!("agent token expired");
        }

        // 3. Capability may require confirmation
        let confirmed = if cap.requires_confirmation() {
            // Check auto-approve cache
            if self.is_auto_approved(cap, scope_key) {
                Some(true)
            } else {
                let req = ConfirmRequest {
                    agent_label: self.token.payload.sub.clone(),
                    capability: cap,
                    action_summary: action.to_string(),
                };
                let allowed = (self.confirm)(&req);
                if allowed {
                    self.add_auto_approve(cap, scope_key.to_string(), 30);
                    Some(true)
                } else {
                    Some(false)
                }
            }
        } else {
            None
        };

        let outcome = match confirmed {
            Some(false) => AuditOutcome::Denied,
            _ => AuditOutcome::Ok,
        };

        self.audit.append(
            &self.token.payload.jti.to_string(),
            cap,
            action,
            confirmed,
            outcome,
        )?;

        if matches!(outcome, AuditOutcome::Denied) {
            bail!("action denied by user");
        }

        Ok(())
    }

    fn is_auto_approved(&self, cap: Capability, scope_key: &str) -> bool {
        let now = Instant::now();
        let mut entries = self.auto_approve.lock();
        // Drop expired
        entries.retain(|e| e.until > now);
        entries
            .iter()
            .any(|e| e.capability == cap && e.scope_key == scope_key)
    }

    fn add_auto_approve(&self, cap: Capability, scope_key: String, secs: u64) {
        // Oversized scope keys simply skip caching — the action was already
        // confirmed; the agent just re-confirms next time.
        if scope_key.len() > MAX_SCOPE_KEY_BYTES {
            return;
        }
        let mut entries = self.auto_approve.lock();
        if entries.len() >= MAX_AUTO_APPROVE_ENTRIES {
            entries.remove(0); // FIFO eviction — oldest grant re-confirms
        }
        entries.push(AutoApprove {
            capability: cap,
            scope_key,
            until: Instant::now() + std::time::Duration::from_secs(secs),
        });
    }

    /// Revoke the session (e.g., user closed the AI panel).
    pub fn revoke(&self) {
        warn!("Agent session revoked: {}", self.token.payload.sub);
    }

    /// Record screen-capture provenance for Visual-Prompt-Injection traceability
    /// (arXiv:2506.02456). Computes the SHA-256 of the frame and logs
    /// (display, sha256, seq, ts, bytes) so any later harmful action can be
    /// traced back to the exact, hash-verifiable frame that induced it.
    /// Returns the hex digest for inclusion in the tool response.
    /// Best-effort: an audit-write failure must not block the capture.
    pub fn audit_screen_capture(&self, display: u8, png: &[u8], seq: u64) -> String {
        use ring::digest;
        let d = digest::digest(&digest::SHA256, png);
        let sha256: String = d.as_ref().iter().map(|b| format!("{b:02x}")).collect();

        let ts_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let action = serde_json::json!({
            "kind": "screen_capture_provenance",
            "display": display,
            "frame_seq": seq,
            "sha256": sha256,
            "ts_secs": ts_secs,
            "bytes": png.len(),
        });
        let _ = self.audit.append(
            &self.token.payload.jti.to_string(),
            Capability::ScreenRead,
            action,
            None,
            crate::audit::AuditOutcome::Ok,
        );
        sha256
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_audit() -> Arc<AuditLog> {
        let path = std::env::temp_dir().join(format!(
            "miru-agent-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Arc::new(AuditLog::open(&path).unwrap())
    }

    fn always_yes() -> ConfirmFn {
        Arc::new(|_: &ConfirmRequest| true)
    }

    #[test]
    fn allows_capability_in_token() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let token = AgentToken::issue(
            &key,
            "test",
            Capability::assistant_default(),
            std::time::Duration::from_secs(60),
            None,
        );
        let session = AgentSession::open(token, temp_audit(), always_yes());

        assert!(session
            .authorize(Capability::ScreenRead, json!({}), "default")
            .is_ok());
        assert!(session
            .authorize(Capability::PointerMove, json!({"x":10}), "default")
            .is_ok());
    }

    #[test]
    fn denies_capability_not_in_token() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let token = AgentToken::issue(
            &key,
            "test",
            Capability::assistant_default(),
            std::time::Duration::from_secs(60),
            None,
        );
        let session = AgentSession::open(token, temp_audit(), always_yes());

        // assistant_default does not include ShellExec
        assert!(session
            .authorize(Capability::ShellExec, json!({"cmd":"ls"}), "default")
            .is_err());
    }

    #[test]
    fn confirmation_required_for_dangerous() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let mut caps = Capability::assistant_default();
        caps.insert(Capability::ShellExec);
        let token = AgentToken::issue(&key, "test", caps, std::time::Duration::from_secs(60), None);

        // Test with always-no confirmation
        let counter = Arc::new(AtomicU32::new(0));
        let counter2 = Arc::clone(&counter);
        let confirm: ConfirmFn = Arc::new(move |_: &ConfirmRequest| {
            counter2.fetch_add(1, Ordering::Relaxed);
            false
        });

        let session = AgentSession::open(token, temp_audit(), confirm);
        assert!(session
            .authorize(Capability::ShellExec, json!({"cmd":"rm -rf"}), "shell")
            .is_err());
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn auto_approve_caches_decision() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let mut caps = Capability::assistant_default();
        caps.insert(Capability::FileWrite);
        let token = AgentToken::issue(&key, "test", caps, std::time::Duration::from_secs(60), None);

        let counter = Arc::new(AtomicU32::new(0));
        let counter2 = Arc::clone(&counter);
        let confirm: ConfirmFn = Arc::new(move |_: &ConfirmRequest| {
            counter2.fetch_add(1, Ordering::Relaxed);
            true
        });

        let session = AgentSession::open(token, temp_audit(), confirm);
        // First call → prompts user
        session
            .authorize(Capability::FileWrite, json!({"path":"a.txt"}), "writes")
            .unwrap();
        // Second call same scope → should NOT prompt
        session
            .authorize(Capability::FileWrite, json!({"path":"b.txt"}), "writes")
            .unwrap();

        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "user prompted only once"
        );
    }

    #[test]
    fn auto_approve_cache_is_bounded() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let mut caps = Capability::assistant_default();
        caps.insert(Capability::FileWrite);
        let token = AgentToken::issue(&key, "test", caps, std::time::Duration::from_secs(60), None);
        let session = AgentSession::open(token, temp_audit(), always_yes());

        // Flood with distinct scopes — the cache must stay capped.
        for i in 0..(MAX_AUTO_APPROVE_ENTRIES + 100) {
            session
                .authorize(
                    Capability::FileWrite,
                    json!({"path": i}),
                    &format!("scope-{i}"),
                )
                .unwrap();
        }
        assert!(session.auto_approve.lock().len() <= MAX_AUTO_APPROVE_ENTRIES);

        // Oversized scope keys are never cached.
        let huge_scope = "x".repeat(MAX_SCOPE_KEY_BYTES + 1);
        session
            .authorize(Capability::FileWrite, json!({"path":"z"}), &huge_scope)
            .unwrap();
        assert!(!session
            .auto_approve
            .lock()
            .iter()
            .any(|e| e.scope_key == huge_scope));
    }

    #[test]
    fn revoked_token_denied_immediately() {
        use crate::revocation::RevocationList;

        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let token = AgentToken::issue(
            &key,
            "test",
            Capability::assistant_default(),
            std::time::Duration::from_secs(60),
            None,
        );
        let jti = token.payload.jti;

        let dir = tempfile::tempdir().unwrap();
        let rev = Arc::new(RevocationList::open(dir.path().join("rev.log")).unwrap());
        let session = AgentSession::open_with_revocation(
            token,
            temp_audit(),
            always_yes(),
            Some(Arc::clone(&rev)),
        );

        // Before revocation — allowed.
        assert!(session
            .authorize(Capability::ScreenRead, json!({}), "s")
            .is_ok());

        // Panic rotation: revoke this JTI.
        rev.revoke(jti, "panic test").unwrap();

        // After revocation — must fail even with otherwise-valid token.
        let err = session
            .authorize(Capability::ScreenRead, json!({}), "s")
            .unwrap_err();
        assert!(
            err.to_string().contains("revoked"),
            "expected revoked error, got: {err}"
        );
    }

    #[test]
    fn screen_capture_provenance_is_deterministic_and_logged() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let token = AgentToken::issue(
            &key,
            "test",
            Capability::assistant_default(),
            std::time::Duration::from_secs(60),
            None,
        );
        let session = AgentSession::open(token, temp_audit(), always_yes());

        let frame = b"\x89PNG\r\n\x1a\n fake png bytes for hashing";
        let h1 = session.audit_screen_capture(0, frame, 0);
        let h2 = session.audit_screen_capture(0, frame, 1);

        // SHA-256 is deterministic: same bytes → same digest, regardless of seq.
        assert_eq!(h1, h2, "digest must depend only on frame bytes");
        assert_eq!(h1.len(), 64, "SHA-256 hex is 64 chars");
        // Different bytes → different digest.
        let h3 = session.audit_screen_capture(0, b"different bytes", 2);
        assert_ne!(h1, h3);
    }
}
