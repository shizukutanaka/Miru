//! AI Agent capability tokens.
//!
//! Why: AI agents (Claude Computer Use, OpenAI Operator, etc.) need to control
//! PCs but giving them the same trust as a human is dangerous. We issue
//! capability-scoped tokens that:
//!   - Expire (default 1 hour)
//!   - Limit scope (e.g., "browser tab only", "specific app", "view-only")
//!   - Require human confirmation for elevated actions
//!   - Are auditable (every use logged with the action that was taken)
//!
//! Token format (compact, no JWT):
//!   `miru-agent.<base64url(payload)>.<base64url(ed25519_sig)>`
//!   payload = JSON-serialized (serde_json) AgentTokenPayload
//!
//! This is structurally similar to JWT but uses Ed25519 (fast, modern) and
//! a compact binary payload (no JSON overhead, no algorithm-confusion bugs).

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

/// Security classification for a capability, after CSAgent (arXiv:2509.22256).
/// Determines how a context-aware gate should treat an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityLevel {
    /// Harmless; never needs confirmation (e.g. reading the screen).
    Normal,
    /// Safe under normal interactive context; a gate may check context before
    /// allowing (e.g. clicking, typing).
    Conditional,
    /// Irreversible or high-impact; always requires explicit confirmation
    /// (e.g. shell exec, file write).
    Dangerous,
}

/// Granular capabilities. Each AI action requires a capability bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Read screen pixels (the AI can "see")
    ScreenRead,
    /// Move the mouse / scroll
    PointerMove,
    /// Click the mouse
    PointerClick,
    /// Type text
    KeyType,
    /// Send modifier-key combinations (Ctrl+C, etc.)
    KeyCombo,
    /// Read clipboard (potentially sensitive)
    ClipboardRead,
    /// Write to clipboard
    ClipboardWrite,
    /// Read files in the shared folder
    FileRead,
    /// Write files in the shared folder
    FileWrite,
    /// Run shell commands (DANGEROUS — never default)
    ShellExec,
    /// Open URLs in browser
    OpenUrl,
}

impl Capability {
    /// Security classification, following the Contextual Integrity taxonomy of
    /// CSAgent (Gong et al., arXiv:2509.22256 §4.2):
    ///   - `Normal`: harmless, no confirmation (e.g. reading the screen).
    ///   - `Conditional`: safe under specific contexts, may warrant a check
    ///     (e.g. typing, clicking — fine during active use, suspicious if the
    ///     user is away).
    ///   - `Dangerous`: irreversible / high-impact, always needs confirmation
    ///     (e.g. shell exec, file write).
    ///
    /// This is richer than a binary allow/deny: a context-aware gate can let
    /// `Conditional` actions through during normal interactive use and only
    /// prompt when context is anomalous, reducing the alert fatigue that makes
    /// users habitually click "Allow" (CSAgent §3.1).
    pub fn security_level(self) -> SecurityLevel {
        match self {
            // Read-only observation — harmless.
            Capability::ScreenRead
            | Capability::PointerMove => SecurityLevel::Normal,

            // Interactive input + low-impact writes — safe in-context.
            Capability::PointerClick
            | Capability::KeyType
            | Capability::KeyCombo
            | Capability::ClipboardWrite
            | Capability::OpenUrl => SecurityLevel::Conditional,

            // Sensitive reads and irreversible / high-impact actions.
            Capability::ClipboardRead
            | Capability::FileRead
            | Capability::FileWrite
            | Capability::ShellExec => SecurityLevel::Dangerous,
        }
    }

    /// Capabilities that require explicit human confirmation each use.
    /// Equivalent to `security_level() == Dangerous`, kept for back-compat.
    pub fn requires_confirmation(self) -> bool {
        self.security_level() == SecurityLevel::Dangerous
    }

    pub fn all() -> &'static [Capability] {
        &[
            Capability::ScreenRead,
            Capability::PointerMove,
            Capability::PointerClick,
            Capability::KeyType,
            Capability::KeyCombo,
            Capability::ClipboardRead,
            Capability::ClipboardWrite,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::ShellExec,
            Capability::OpenUrl,
        ]
    }

    /// Reasonable default scope for "AI assists with my work" — readonly observation
    /// plus pointer/keyboard, but no shell/file write.
    pub fn assistant_default() -> HashSet<Capability> {
        [
            Capability::ScreenRead,
            Capability::PointerMove,
            Capability::PointerClick,
            Capability::KeyType,
            Capability::KeyCombo,
            Capability::ClipboardWrite,
            Capability::OpenUrl,
        ].into_iter().collect()
    }
}

/// Token payload — what the signature commits to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTokenPayload {
    /// Token id (for revocation lookup)
    pub jti: Uuid,
    /// Issuer (host's Ed25519 pubkey, base64).
    pub iss: String,
    /// Subject — short label like "claude-code-session".
    pub sub: String,
    /// Issued-at (unix seconds).
    pub iat: u64,
    /// Expires-at (unix seconds).
    pub exp: u64,
    /// Allowed capabilities.
    pub caps: HashSet<Capability>,
    /// Optional scope restrictions (e.g., process name, window title pattern).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<AgentScope>,
}

/// Optional scope restrictions on the token.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentScope {
    /// If set, only inputs within this window title regex are allowed.
    pub window_title_regex: Option<String>,
    /// If set, only this process name receives input.
    pub process_name: Option<String>,
    /// If set, file operations limited to this directory.
    pub file_root: Option<String>,
}

/// Issued token: payload + Ed25519 signature.
pub struct AgentToken {
    pub payload: AgentTokenPayload,
    pub signature: Signature,
}

/// Hard upper bound on token TTL — 15 minutes.
///
/// Per the security playbook (§0 v0.1 must-have list, item 6):
/// "capability-scoped Ed25519 tokens with TTL ≤ 15 min". This bound exists
/// because longer-lived tokens accumulate exfiltration risk: a compromised
/// LLM session, leaked Claude Desktop config, or stolen device cache can
/// each replay a still-valid token. With a 15-minute ceiling the worst-case
/// attacker window is bounded; the user can re-issue from the host UI as
/// often as needed (issuance is cheap).
///
/// `AgentToken::issue` clamps any TTL above this to 15 minutes. The host UI
/// should never offer a longer value in the first place; clamping is
/// defense-in-depth for any path that calls `issue` directly.
pub const MAX_TTL_SECS: u64 = 15 * 60;

/// Tolerance for `iat` lying in the future, to absorb NTP jitter between the
/// issuing host and a verifying process on another clock.
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;

impl AgentToken {
    pub fn issue(
        signing_key: &SigningKey,
        sub: impl Into<String>,
        caps: HashSet<Capability>,
        ttl: std::time::Duration,
        scope: Option<AgentScope>,
    ) -> Self {
        let now = unix_now();
        // HARD CAP — see MAX_TTL_SECS doc comment.
        let ttl_secs = ttl.as_secs().min(MAX_TTL_SECS);
        let payload = AgentTokenPayload {
            jti: Uuid::new_v4(),
            iss: B64URL.encode(signing_key.verifying_key().as_bytes()),
            sub: sub.into(),
            iat: now,
            exp: now + ttl_secs,
            caps,
            scope,
        };
        let bytes = canonical_payload_bytes(&payload);
        let signature = signing_key.sign(&bytes);
        Self { payload, signature }
    }

    /// Parse + verify against the issuer's public key.
    pub fn parse_and_verify(s: &str, issuer_pubkey: &VerifyingKey) -> Result<Self> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 || parts[0] != "miru-agent" {
            bail!("malformed token");
        }
        let payload_bytes = B64URL.decode(parts[1]).context("decode payload")?;
        let sig_bytes = B64URL.decode(parts[2]).context("decode sig")?;
        let sig_arr: [u8; 64] = sig_bytes.try_into()
            .map_err(|_| anyhow::anyhow!("sig wrong length"))?;
        let signature = Signature::from_bytes(&sig_arr);

        // Verify signature first — never deserialize untrusted bytes.
        // verify_strict (ZIP-215) rejects small-order pubkeys + non-canonical R.
        issuer_pubkey.verify_strict(&payload_bytes, &signature)
            .map_err(|_| anyhow::anyhow!("invalid token signature"))?;

        let payload: AgentTokenPayload = serde_json::from_slice(&payload_bytes)
            .context("deserialize payload")?;

        // Check issuer matches
        let pk_b64 = B64URL.encode(issuer_pubkey.as_bytes());
        if payload.iss != pk_b64 {
            bail!("token issuer mismatch");
        }

        // Logical consistency — our issuer always produces iat < exp, but
        // parse_and_verify accepts external input and must not trust it.
        if payload.iat > payload.exp {
            bail!("malformed token: iat ({}) is after exp ({})", payload.iat, payload.exp);
        }

        // Check expiration
        let now = unix_now();
        if now > payload.exp {
            bail!("token expired (exp={}, now={})", payload.exp, now);
        }
        if now + MAX_CLOCK_SKEW_SECS < payload.iat {
            bail!(
                "token issued-at is in the future by more than {MAX_CLOCK_SKEW_SECS}s (malformed or clock skew)"
            );
        }

        Ok(Self { payload, signature })
    }

    pub fn has_capability(&self, cap: Capability) -> bool {
        self.payload.caps.contains(&cap)
    }

    pub fn seconds_remaining(&self) -> u64 {
        self.payload.exp.saturating_sub(unix_now())
    }
}

impl std::fmt::Display for AgentToken {
    /// Compact string form: `miru-agent.<payload>.<sig>`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let payload_bytes = canonical_payload_bytes(&self.payload);
        write!(
            f,
            "miru-agent.{}.{}",
            B64URL.encode(&payload_bytes),
            B64URL.encode(self.signature.to_bytes()),
        )
    }
}

fn canonical_payload_bytes(p: &AgentTokenPayload) -> Vec<u8> {
    // JSON for human readability when debugging tokens; sorted keys for determinism.
    serde_json::to_vec(p).expect("serialize payload")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key() -> SigningKey {
        SigningKey::generate(&mut rand::rngs::OsRng)
    }

    #[test]
    fn token_roundtrip() {
        let key = make_key();
        let pk = key.verifying_key();

        let token = AgentToken::issue(
            &key,
            "test-agent",
            Capability::assistant_default(),
            std::time::Duration::from_secs(3600),
            None,
        );

        let s = token.to_string();
        assert!(s.starts_with("miru-agent."));
        assert_eq!(s.split('.').count(), 3);

        let parsed = AgentToken::parse_and_verify(&s, &pk).unwrap();
        assert_eq!(parsed.payload.sub, "test-agent");
        assert!(parsed.has_capability(Capability::ScreenRead));
        assert!(!parsed.has_capability(Capability::ShellExec));
    }

    #[test]
    fn tampered_token_rejected() {
        let key = make_key();
        let pk = key.verifying_key();

        let token = AgentToken::issue(
            &key, "test", Capability::assistant_default(),
            std::time::Duration::from_secs(60), None,
        );
        let s = token.to_string();
        // Flip a character in the payload section
        let mut chars: Vec<char> = s.chars().collect();
        let idx = "miru-agent.".len() + 5;
        chars[idx] = if chars[idx] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();

        assert!(AgentToken::parse_and_verify(&tampered, &pk).is_err());
    }

    #[test]
    fn wrong_issuer_rejected() {
        let key1 = make_key();
        let key2 = make_key();

        let token = AgentToken::issue(
            &key1, "test", Capability::assistant_default(),
            std::time::Duration::from_secs(60), None,
        );
        let s = token.to_string();

        // Verify against wrong key
        assert!(AgentToken::parse_and_verify(&s, &key2.verifying_key()).is_err());
    }

    #[test]
    fn expired_token_rejected() {
        let key = make_key();
        let pk = key.verifying_key();

        let mut token = AgentToken::issue(
            &key, "test", Capability::assistant_default(),
            std::time::Duration::from_secs(60), None,
        );
        // Force-expire
        token.payload.exp = unix_now() - 10;

        // Re-sign with new payload
        let sig = key.sign(&canonical_payload_bytes(&token.payload));
        let new_s = format!("miru-agent.{}.{}",
            B64URL.encode(canonical_payload_bytes(&token.payload)),
            B64URL.encode(sig.to_bytes()),
        );

        assert!(AgentToken::parse_and_verify(&new_s, &pk).is_err());
    }

    #[test]
    fn iat_after_exp_rejected() {
        let key = make_key();
        let pk = key.verifying_key();

        let mut token = AgentToken::issue(
            &key, "test", Capability::assistant_default(),
            std::time::Duration::from_secs(60), None,
        );
        // Forge a logically impossible payload: issued after it expires,
        // with `now` inside the [exp, iat] gap so only the iat<=exp check
        // can catch it.
        token.payload.iat = unix_now() + 200;
        token.payload.exp = unix_now() + 100;
        let sig = key.sign(&canonical_payload_bytes(&token.payload));
        let s = format!("miru-agent.{}.{}",
            B64URL.encode(canonical_payload_bytes(&token.payload)),
            B64URL.encode(sig.to_bytes()),
        );

        let err = match AgentToken::parse_and_verify(&s, &pk) {
            Err(e) => e,
            Ok(_) => panic!("token with iat > exp must be rejected"),
        };
        assert!(err.to_string().contains("iat"), "unexpected error: {err}");
    }

    #[test]
    fn far_future_iat_rejected() {
        let key = make_key();
        let pk = key.verifying_key();

        let mut token = AgentToken::issue(
            &key, "test", Capability::assistant_default(),
            std::time::Duration::from_secs(60), None,
        );
        token.payload.iat = unix_now() + MAX_CLOCK_SKEW_SECS + 100;
        token.payload.exp = token.payload.iat + 60;
        let sig = key.sign(&canonical_payload_bytes(&token.payload));
        let s = format!("miru-agent.{}.{}",
            B64URL.encode(canonical_payload_bytes(&token.payload)),
            B64URL.encode(sig.to_bytes()),
        );

        assert!(AgentToken::parse_and_verify(&s, &pk).is_err());
    }

    #[test]
    fn confirmation_required_for_dangerous_caps() {
        assert!(Capability::ShellExec.requires_confirmation());
        assert!(Capability::FileWrite.requires_confirmation());
        assert!(!Capability::ScreenRead.requires_confirmation());
        assert!(!Capability::PointerMove.requires_confirmation());
    }

    #[test]
    fn security_level_classification() {
        use SecurityLevel::*;
        // Normal: read-only observation.
        assert_eq!(Capability::ScreenRead.security_level(), Normal);
        assert_eq!(Capability::PointerMove.security_level(), Normal);
        // Conditional: interactive input, low-impact writes.
        assert_eq!(Capability::PointerClick.security_level(), Conditional);
        assert_eq!(Capability::KeyType.security_level(), Conditional);
        assert_eq!(Capability::OpenUrl.security_level(), Conditional);
        // Dangerous: irreversible / sensitive.
        assert_eq!(Capability::ShellExec.security_level(), Dangerous);
        assert_eq!(Capability::FileWrite.security_level(), Dangerous);
        assert_eq!(Capability::ClipboardRead.security_level(), Dangerous);
    }

    #[test]
    fn dangerous_level_implies_confirmation() {
        // Invariant: every Dangerous capability requires confirmation, and
        // vice versa (the two definitions must stay in sync).
        for &cap in Capability::all() {
            assert_eq!(
                cap.security_level() == SecurityLevel::Dangerous,
                cap.requires_confirmation(),
                "mismatch for {cap:?}"
            );
        }
    }

    #[test]
    fn assistant_default_excludes_dangerous() {
        let caps = Capability::assistant_default();
        assert!(!caps.contains(&Capability::ShellExec));
        assert!(!caps.contains(&Capability::FileWrite));
        assert!(!caps.contains(&Capability::ClipboardRead));
        assert!(caps.contains(&Capability::ScreenRead));
    }
}
