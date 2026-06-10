//! AI agent control layer for Miru.
//!
//! Provides:
//!   - Capability-scoped Ed25519 tokens (`token`)
//!   - Tamper-evident audit log (`audit`)
//!   - Runtime authorization gate (`session`)
//!
//! Usage from a host:
//! ```ignore
//! use miru_agent::{AgentToken, AgentSession, Capability};
//! use std::sync::Arc;
//!
//! // 1. Issue a token (typically via UI button)
//! let token = AgentToken::issue(
//!     &host_signing_key,
//!     "claude-code-session",
//!     Capability::assistant_default(),
//!     std::time::Duration::from_secs(3600),
//!     None,
//! );
//! // ↓ Pass `token.to_string()` to the AI agent (e.g., as env var or arg).
//!
//! // 2. When the agent connects, parse + verify the token, then open a session
//! let token = AgentToken::parse_and_verify(token_str, &host_pubkey)?;
//! let audit = Arc::new(AuditLog::open(&audit_path)?);
//! let session = AgentSession::open(token, audit, confirm_fn);
//!
//! // 3. Each action goes through authorize()
//! session.authorize(Capability::PointerClick, json!({"x":100,"y":200}), "click")?;
//! ```

pub mod audit;
pub mod redact;
pub mod revocation;
pub mod session;
pub mod token;

pub use audit::{AuditEntry, AuditLog, AuditOutcome};
pub use session::{AgentSession, ConfirmFn, ConfirmRequest};
pub use token::{AgentScope, AgentToken, AgentTokenPayload, Capability, SecurityLevel};
