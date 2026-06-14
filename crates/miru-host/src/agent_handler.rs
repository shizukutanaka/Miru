// Scaffold — not yet fully wired into hot path. See roadmap.
#![allow(dead_code)]
//! Handler for AI-agent role connections.
//!
//! When a viewer connects with `Role::AiAgent`, the handshake's `pubkey` field
//! contains an additional segment: an Ed25519-signed agent token. The host:
//!   1. Verifies the token (signed by THIS host's key)
//!   2. Opens an AgentSession with the audit log
//!   3. Each incoming InputEvent goes through `gate_input` → authorize()
//!   4. ScreenRead is implicit: the host sends VideoFrames if + only if
//!      the token has Capability::ScreenRead
//!
//! This is parallel to `session.rs` (human viewer flow) but with the gate.

use anyhow::{bail, Result};
use miru_agent::{AgentSession, Capability};
use miru_common::message::{InputEvent, InputKind, MouseButton};
use serde_json::json;
use std::sync::Arc;

/// Wraps an AgentSession; gate_input() returns Err for unauthorized actions.
pub struct AgentHandler {
    pub session: Arc<AgentSession>,
}

impl AgentHandler {
    pub fn new(session: Arc<AgentSession>) -> Self {
        Self { session }
    }

    /// Authorize an InputEvent for the agent. If denied, returns Err and the
    /// caller drops the event. The audit log captures the attempt regardless.
    pub fn gate_input(&self, evt: &InputEvent) -> Result<()> {
        let (cap, scope_key, action) = match &evt.kind {
            InputKind::MouseMove { x, y, display } => (
                Capability::PointerMove,
                "mouse_move",
                json!({"x": x, "y": y, "display": display}),
            ),
            InputKind::MouseDown { button, x, y } | InputKind::MouseUp { button, x, y } => (
                Capability::PointerClick,
                "mouse_click",
                json!({"button": button_name(*button), "x": x, "y": y}),
            ),
            InputKind::Scroll { dx, dy, x, y } => (
                Capability::PointerMove,
                "scroll",
                json!({"dx": dx, "dy": dy, "x": x, "y": y}),
            ),
            InputKind::KeyDown { key, modifiers } | InputKind::KeyUp { key, modifiers } => {
                if *modifiers == 0 {
                    (Capability::KeyType, "key", json!({"key": key}))
                } else {
                    (
                        Capability::KeyCombo,
                        "key_combo",
                        json!({"key": key, "mods": modifiers}),
                    )
                }
            }
            InputKind::Text { text } => (
                Capability::KeyType,
                "key_type",
                json!({"len": text.len()}), // don't log raw text
            ),
        };

        self.session.authorize(cap, action, scope_key)
    }

    /// Filter video frame transmission: only allow if ScreenRead capability granted.
    pub fn allow_screen_send(&self) -> bool {
        self.session.token.has_capability(Capability::ScreenRead)
    }
}

fn button_name(b: MouseButton) -> &'static str {
    match b {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::X1 => "x1",
        MouseButton::X2 => "x2",
    }
}

/// Parse an agent token from the Hello message's `pubkey` field.
/// Format extension for `Role::AiAgent`: the standard handshake puts
/// `<eph_pub>:<identity_pub>:<sig>`. For agent role, an extra segment is appended:
/// `<eph_pub>:<identity_pub>:<sig>:<agent_token>`.
pub fn extract_agent_token(pubkey_field: &str) -> Result<&str> {
    // splitn(4) captures everything after the 3rd ':' as one token, preserving
    // any ':' characters inside the agent token itself.
    let parts: Vec<&str> = pubkey_field.splitn(4, ':').collect();
    if parts.len() < 4 {
        bail!("agent role requires extra agent_token segment in pubkey field");
    }
    Ok(parts[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_handles_extra_segment() {
        let s = "AAA:BBB:CCC:miru-agent.token.sig";
        assert_eq!(extract_agent_token(s).unwrap(), "miru-agent.token.sig");
    }

    #[test]
    fn extract_rejects_missing_segment() {
        assert!(extract_agent_token("AAA:BBB:CCC").is_err());
    }

    #[test]
    fn extract_preserves_colons_in_token() {
        // Token contains ':' — splitn(4) must not truncate it.
        let s = "AAA:BBB:CCC:part1:part2:part3";
        assert_eq!(extract_agent_token(s).unwrap(), "part1:part2:part3");
    }

    #[test]
    fn gate_allows_granted_capability_denies_others() {
        use ed25519_dalek::SigningKey;
        use miru_agent::audit::AuditLog;
        use miru_agent::{AgentSession, AgentToken};
        use miru_common::message::InputEvent;
        use std::collections::HashSet;
        use std::time::Duration;

        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        // Token grants only PointerMove — not KeyType.
        let caps: HashSet<Capability> = [Capability::PointerMove].into_iter().collect();
        let token = AgentToken::issue(&key, "test-agent", caps, Duration::from_secs(60), None);

        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(AuditLog::open(&dir.path().join("a.log")).unwrap());
        let confirm = Arc::new(|_: &miru_agent::ConfirmRequest| true);
        let session = Arc::new(AgentSession::open(token, audit, confirm));
        let handler = AgentHandler::new(session);

        // MouseMove → PointerMove granted → allowed.
        let move_evt = InputEvent {
            kind: InputKind::MouseMove {
                x: 10.0,
                y: 20.0,
                display: 0,
            },
            timestamp_ms: 0,
        };
        assert!(
            handler.gate_input(&move_evt).is_ok(),
            "PointerMove should be allowed"
        );

        // KeyDown → KeyType NOT granted → denied.
        let key_evt = InputEvent {
            kind: InputKind::KeyDown {
                key: 65,
                modifiers: 0,
            },
            timestamp_ms: 0,
        };
        assert!(
            handler.gate_input(&key_evt).is_err(),
            "KeyType should be denied"
        );
    }
}
