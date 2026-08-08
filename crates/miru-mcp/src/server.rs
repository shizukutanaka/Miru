//! MCP server — translates Claude/LLM tool calls into Miru protocol messages.
//!
//! Each tool call:
//!   1. Parses arguments against its JSON Schema
//!   2. Maps to a Capability
//!   3. Calls AgentSession::authorize() — may prompt human via callback
//!   4. If authorized, sends the corresponding Msg to the host via the existing
//!      Miru transport
//!   5. Returns the result (or error) as JSON-RPC

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use miru_agent::{AgentSession, Capability};
use miru_common::message::{ClipboardFormat, ClipboardSync, InputEvent, InputKind, MouseButton};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::protocol::{Content, ToolCallParams, ToolCallResult};
use crate::rate_limit::{RateLimitError, RateLimiter};
use crate::replay::{ReplayDecision, ReplayGuard};

/// Trait implemented by whatever pipes input back to the host.
/// We use a trait so tests can stub it out.
#[async_trait::async_trait]
pub trait HostBridge: Send + Sync {
    async fn send_input(&self, evt: InputEvent) -> Result<()>;
    async fn send_clipboard(&self, sync: ClipboardSync) -> Result<()>;
    async fn capture_screen(&self, display: u8) -> Result<Vec<u8>>;
    async fn read_clipboard(&self) -> Result<String>;
    async fn open_url(&self, url: &str) -> Result<()>;
    async fn status(&self) -> Result<Value>;
}

pub struct McpServer {
    session: Arc<AgentSession>,
    bridge: Arc<dyn HostBridge>,
    rate_limiter: RateLimiter,
    replay_guard: ReplayGuard,
    /// Monotonic per-session frame counter for screen-capture provenance.
    frame_seq: std::sync::atomic::AtomicU64,
}

impl McpServer {
    pub fn new(session: Arc<AgentSession>, bridge: Arc<dyn HostBridge>) -> Self {
        Self {
            session,
            bridge,
            rate_limiter: RateLimiter::new(),
            replay_guard: ReplayGuard::new(),
            frame_seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Replay check for requests that carry `(timestamp, nonce)` metadata
    /// (AttestMCP §VI-C). Stdio calls without metadata skip this layer and rely
    /// on parent-allowlist + token TTL; networked/forwarded calls should always
    /// supply it. Returns Err on stale timestamp or duplicate nonce.
    pub fn check_replay(&self, timestamp_secs: u64, nonce: [u8; 16]) -> Result<()> {
        match self.replay_guard.check(timestamp_secs, nonce) {
            ReplayDecision::Fresh => Ok(()),
            ReplayDecision::DuplicateNonce => {
                bail!("replay rejected: duplicate nonce")
            }
            ReplayDecision::StaleTimestamp => {
                bail!("replay rejected: timestamp outside 30s validity window")
            }
        }
    }

    /// Centralised gate for every consequential tool call. Sequence:
    ///   1. Rate-limit check (per-capability token bucket + 24h cap).
    ///   2. AgentSession::authorize (token capability check + confirmation).
    ///   3. (caller proceeds with action)
    ///
    /// Rejection at any stage raises an error returned to the LLM client AND
    /// produces an audit-log entry via the session/audit machinery.
    fn gate(&self, cap: Capability, action: Value, summary: &str) -> Result<()> {
        if let Err(e) = self.rate_limiter.try_consume(cap) {
            // Rate-limit denials currently propagate as MCP errors only.
            // The audit log is keyed off AgentSession::authorize; we pre-empt
            // here, so emit a separate audit entry through a dedicated path
            // when v0.3 wires denial events into the chained log.
            match e {
                RateLimitError::BucketEmpty { retry_after } => {
                    bail!(
                        "rate limit: capability {:?} bucket empty, retry in {}ms",
                        cap,
                        retry_after.as_millis()
                    );
                }
                RateLimitError::DailyCapReached { cap_max, .. } => {
                    bail!("rate limit: daily cap of {cap_max} for {cap:?} reached");
                }
            }
        }
        self.session.authorize(cap, action, summary)
    }

    pub async fn handle_tool_call(&self, params: ToolCallParams) -> ToolCallResult {
        match self.dispatch(&params.name, &params.arguments).await {
            Ok(content) => ToolCallResult {
                content,
                is_error: false,
            },
            Err(e) => ToolCallResult {
                content: vec![Content::Text {
                    text: format!("Error: {e}"),
                }],
                is_error: true,
            },
        }
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Vec<Content>> {
        match name {
            "miru_screen_capture" => self.tool_screen_capture(args).await,
            "miru_mouse_move" => self.tool_mouse_move(args).await,
            "miru_mouse_click" => self.tool_mouse_click(args).await,
            "miru_scroll" => self.tool_scroll(args).await,
            "miru_key_type" => self.tool_key_type(args).await,
            "miru_key_combo" => self.tool_key_combo(args).await,
            "miru_clipboard_read" => self.tool_clipboard_read(args).await,
            "miru_clipboard_write" => self.tool_clipboard_write(args).await,
            "miru_open_url" => self.tool_open_url(args).await,
            "miru_status" => self.tool_status().await,
            _ => bail!("unknown tool: {name}"),
        }
    }

    // ─── Tool implementations ────────────────────────────────────────────────

    async fn tool_screen_capture(&self, args: &Value) -> Result<Vec<Content>> {
        let display_raw = args.get("display").and_then(|v| v.as_u64()).unwrap_or(0);
        if display_raw > u8::MAX as u64 {
            bail!("display index {display_raw} out of range (max {})", u8::MAX);
        }
        let display = display_raw as u8;
        self.gate(
            Capability::ScreenRead,
            json!({"display": display}),
            "screen_capture",
        )?;
        let png = self.bridge.capture_screen(display).await?;

        // Optional sub-region crop (normalized 0-1 coordinates).
        let png = if let Some(region) = args.get("region") {
            let rx = region.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let ry = region.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let rw = region.get("width").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            let rh = region.get("height").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            if !(0.0..=1.0).contains(&rx) || !(0.0..=1.0).contains(&ry)
                || rw <= 0.0 || rh <= 0.0
                || rx + rw > 1.0 + f32::EPSILON || ry + rh > 1.0 + f32::EPSILON
            {
                bail!("capture_screen: region coordinates out of [0,1] bounds");
            }
            crop_png_region(&png, rx, ry, rw, rh)
                .unwrap_or(png) // on failure, fall back to full capture
        } else {
            png
        };

        // Provenance tagging (defense against Visual Prompt Injection,
        // arXiv:2506.02456). We cannot stop a malicious instruction embedded in
        // the pixels from reaching the model — the literature shows model-level
        // defenses are unreliable (Carlini et al., "The Attacker Moves Second",
        // bypass >90%). What we CAN do is make every frame the agent sees
        // forensically traceable: record (display, timestamp, SHA-256, seq) so
        // that if the agent later takes a harmful action, the exact frame that
        // induced it is identifiable and hash-verifiable after the fact.
        let seq = self
            .frame_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Record provenance in the audit log (digest computed in miru-agent).
        let digest = self.session.audit_screen_capture(display, &png, seq);

        Ok(vec![
            Content::Text {
                text: format!(
                    "Captured display {} ({} bytes) [frame#{} sha256:{}…]",
                    display,
                    png.len(),
                    seq,
                    &digest[..digest.len().min(16)]
                ),
            },
            Content::Image {
                data: B64.encode(png),
                mime_type: "image/png",
            },
        ])
    }

    async fn tool_mouse_move(&self, args: &Value) -> Result<Vec<Content>> {
        let x = (args
            .get("x")
            .and_then(|v| v.as_f64())
            .context("x missing")? as f32)
            .clamp(0.0, 1.0);
        let y = (args
            .get("y")
            .and_then(|v| v.as_f64())
            .context("y missing")? as f32)
            .clamp(0.0, 1.0);
        self.gate(
            Capability::PointerMove,
            json!({"x": x, "y": y}),
            "mouse_move",
        )?;
        let evt = InputEvent {
            kind: InputKind::MouseMove { x, y, display: 0 },
            timestamp_ms: now_ms(),
        };
        self.bridge.send_input(evt).await?;
        Ok(vec![Content::Text {
            text: format!("Mouse moved to ({x:.3}, {y:.3})"),
        }])
    }

    async fn tool_mouse_click(&self, args: &Value) -> Result<Vec<Content>> {
        let x = (args
            .get("x")
            .and_then(|v| v.as_f64())
            .context("x missing")? as f32)
            .clamp(0.0, 1.0);
        let y = (args
            .get("y")
            .and_then(|v| v.as_f64())
            .context("y missing")? as f32)
            .clamp(0.0, 1.0);
        let button_str = args
            .get("button")
            .and_then(|v| v.as_str())
            .unwrap_or("left");
        let double = args
            .get("double")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let button = match button_str {
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            _ => MouseButton::Left,
        };

        self.gate(
            Capability::PointerClick,
            json!({"x": x, "y": y, "button": button_str, "double": double}),
            "mouse_click",
        )?;

        let times = if double { 2 } else { 1 };
        for _ in 0..times {
            let down = InputEvent {
                kind: InputKind::MouseDown { button, x, y },
                timestamp_ms: now_ms(),
            };
            let up = InputEvent {
                kind: InputKind::MouseUp { button, x, y },
                timestamp_ms: now_ms(),
            };
            self.bridge.send_input(down).await?;
            self.bridge.send_input(up).await?;
        }

        Ok(vec![Content::Text {
            text: format!(
                "Clicked {} at ({:.3}, {:.3}){}",
                button_str,
                x,
                y,
                if double { " (double)" } else { "" }
            ),
        }])
    }

    async fn tool_scroll(&self, args: &Value) -> Result<Vec<Content>> {
        let dx = (args.get("dx").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32)
            .clamp(-100.0, 100.0);
        let dy = (args
            .get("dy")
            .and_then(|v| v.as_f64())
            .context("dy missing")? as f32)
            .clamp(-100.0, 100.0);
        let x = (args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32).clamp(0.0, 1.0);
        let y = (args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32).clamp(0.0, 1.0);

        self.gate(
            Capability::PointerMove, // scrolling reuses pointer cap
            json!({"dx": dx, "dy": dy}),
            "scroll",
        )?;

        let evt = InputEvent {
            kind: InputKind::Scroll { dx, dy, x, y },
            timestamp_ms: now_ms(),
        };
        self.bridge.send_input(evt).await?;
        Ok(vec![Content::Text {
            text: format!("Scrolled dx={dx}, dy={dy}"),
        }])
    }

    async fn tool_key_type(&self, args: &Value) -> Result<Vec<Content>> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .context("text missing")?;
        const KEY_TYPE_MAX_CHARS: usize = 4096;
        if text.chars().count() > KEY_TYPE_MAX_CHARS {
            bail!("key_type: text exceeds maximum of {KEY_TYPE_MAX_CHARS} characters");
        }
        self.gate(
            Capability::KeyType,
            json!({"len": text.len()}), // don't log raw text in audit
            "key_type",
        )?;
        let evt = InputEvent {
            kind: InputKind::Text {
                text: text.to_string(),
            },
            timestamp_ms: now_ms(),
        };
        self.bridge.send_input(evt).await?;
        Ok(vec![Content::Text {
            text: format!("Typed {} characters", text.chars().count()),
        }])
    }

    async fn tool_key_combo(&self, args: &Value) -> Result<Vec<Content>> {
        let combo = args
            .get("combo")
            .and_then(|v| v.as_str())
            .context("combo missing")?;
        if combo.len() > 64 {
            bail!("key_combo: combo string too long ({} bytes)", combo.len());
        }
        self.gate(Capability::KeyCombo, json!({"combo": combo}), "key_combo")?;
        // Parse "Ctrl+Shift+T" → modifier mask + key
        let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
        if parts.is_empty() {
            bail!("empty combo");
        }
        if parts.len() > 10 {
            bail!("key_combo: too many tokens ({} > 10 max)", parts.len());
        }
        let mut modifiers = 0u8;
        let mut key_str = "";
        // Physical modifier keys, in press order. A chord CANNOT be expressed
        // by the `modifiers` bitmask alone: only the macOS backend reads it
        // (as CGEventFlags); the Linux and Windows backends ignore the field
        // entirely, so "Ctrl+C" used to arrive as a bare "c" there. Real
        // modifier key events are the portable way to build a chord.
        let mut mod_codes: Vec<&'static str> = Vec::new();
        for p in &parts {
            match *p {
                "Shift" => {
                    modifiers |= 0x01;
                    mod_codes.push("ShiftLeft");
                }
                "Ctrl" | "Control" => {
                    modifiers |= 0x02;
                    mod_codes.push("ControlLeft");
                }
                "Alt" | "Option" => {
                    modifiers |= 0x04;
                    mod_codes.push("AltLeft");
                }
                "Cmd" | "Meta" | "Win" => {
                    modifiers |= 0x08;
                    mod_codes.push("MetaLeft");
                }
                _ => key_str = p,
            }
        }
        let key = parse_key(key_str)?;
        let key_code = parse_key_code(key_str);

        // Press modifiers → press/release the key → release modifiers in
        // reverse order, so nothing is left latched on the host.
        let mut events: Vec<InputEvent> = Vec::with_capacity(mod_codes.len() * 2 + 2);
        for code in &mod_codes {
            events.push(modifier_event(code, true, modifiers));
        }
        events.push(InputEvent {
            kind: InputKind::KeyDown { key, modifiers, code: key_code.clone() },
            timestamp_ms: now_ms(),
        });
        events.push(InputEvent {
            kind: InputKind::KeyUp { key, modifiers, code: key_code },
            timestamp_ms: now_ms(),
        });
        for code in mod_codes.iter().rev() {
            events.push(modifier_event(code, false, modifiers));
        }
        for evt in events {
            self.bridge.send_input(evt).await?;
        }

        Ok(vec![Content::Text {
            text: format!("Sent key combo: {combo}"),
        }])
    }

    async fn tool_clipboard_read(&self, _args: &Value) -> Result<Vec<Content>> {
        self.gate(Capability::ClipboardRead, json!({}), "clipboard_read")?;
        let text = self.bridge.read_clipboard().await?;
        const MAX_CLIP_BYTES: usize = 1024 * 1024; // 1 MiB — prevent LLM context overflow
        if text.len() > MAX_CLIP_BYTES {
            return Ok(vec![Content::Text {
                text: format!(
                    "[clipboard content truncated: {} bytes exceeds 1 MiB limit]",
                    text.len()
                ),
            }]);
        }
        Ok(vec![Content::Text { text }])
    }

    async fn tool_clipboard_write(&self, args: &Value) -> Result<Vec<Content>> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .context("text missing")?;
        const CLIPBOARD_WRITE_MAX_BYTES: usize = 1024 * 1024; // 1 MiB, matching JSON schema maxLength
        if text.len() > CLIPBOARD_WRITE_MAX_BYTES {
            bail!("clipboard_write: text exceeds maximum of {CLIPBOARD_WRITE_MAX_BYTES} bytes");
        }
        self.gate(
            Capability::ClipboardWrite,
            json!({"len": text.len()}),
            "clipboard_write",
        )?;
        self.bridge
            .send_clipboard(ClipboardSync {
                format: ClipboardFormat::Text,
                data: text.as_bytes().to_vec(),
            })
            .await?;
        Ok(vec![Content::Text {
            text: format!("Clipboard set ({} bytes)", text.len()),
        }])
    }

    async fn tool_open_url(&self, args: &Value) -> Result<Vec<Content>> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .context("url missing")?;
        if url.len() > 2048 {
            bail!("open_url: URL too long ({} bytes)", url.len());
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            bail!("only http(s) URLs allowed");
        }
        self.gate(Capability::OpenUrl, json!({"url": url}), "open_url")?;
        self.bridge.open_url(url).await?;
        Ok(vec![Content::Text {
            text: format!("Opened {url}"),
        }])
    }

    async fn tool_status(&self) -> Result<Vec<Content>> {
        let status = self.bridge.status().await?;
        let token_remaining = self.session.token.seconds_remaining();
        let summary = json!({
            "agent_label": self.session.token.payload.sub,
            "token_seconds_remaining": token_remaining,
            "capabilities": self.session.token.payload.caps,
            "host_status": status,
        });
        Ok(vec![Content::Text {
            text: serde_json::to_string_pretty(&summary)?,
        }])
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Build a KeyDown/KeyUp for a physical modifier key (e.g. "ControlLeft").
fn modifier_event(code: &str, down: bool, modifiers: u8) -> InputEvent {
    // Legacy `key` is the VK for hosts that predate the `code` field.
    let key = miru_input::keymap::code_to_vk(code).map(u32::from).unwrap_or(0);
    let code = Some(code.to_string());
    InputEvent {
        kind: if down {
            InputKind::KeyDown { key, modifiers, code }
        } else {
            InputKind::KeyUp { key, modifiers, code }
        },
        timestamp_ms: now_ms(),
    }
}

/// Map a key name to the W3C UI Events physical-key identifier the host uses
/// to look up the real per-OS key code (see `miru_input::keymap`).
///
/// Without this the agent path sends only the legacy VK number, which the host
/// casts straight into evdev/CGKeyCode — different numbering spaces, so "A"
/// injected as evdev 65 is KEY_F7. Returning None falls back to that legacy
/// behaviour rather than failing the call.
fn parse_key_code(s: &str) -> Option<String> {
    let mut chars = s.chars();
    if let Some(c) = chars.next().filter(|_| chars.next().is_none()) {
        let c = c.to_ascii_uppercase();
        if c.is_ascii_alphabetic() {
            return Some(format!("Key{c}"));
        }
        if c.is_ascii_digit() {
            return Some(format!("Digit{c}"));
        }
    }
    Some(
        match s {
            "Enter" | "Return" => "Enter",
            "Tab" => "Tab",
            "Escape" | "Esc" => "Escape",
            "Backspace" => "Backspace",
            "Delete" | "Del" => "Delete",
            "Space" => "Space",
            "Up" => "ArrowUp",
            "Down" => "ArrowDown",
            "Left" => "ArrowLeft",
            "Right" => "ArrowRight",
            "Home" => "Home",
            "End" => "End",
            "PageUp" => "PageUp",
            "PageDown" => "PageDown",
            "F1" => "F1",
            "F2" => "F2",
            "F3" => "F3",
            "F4" => "F4",
            "F5" => "F5",
            "F6" => "F6",
            "F7" => "F7",
            "F8" => "F8",
            "F9" => "F9",
            "F10" => "F10",
            "F11" => "F11",
            "F12" => "F12",
            _ => return None,
        }
        .to_string(),
    )
}

/// Parse a key name to a Windows VK-style scancode.
/// Kept as the legacy `key` field for hosts that predate `code`; new hosts
/// prefer `parse_key_code`.
fn parse_key(s: &str) -> Result<u32> {
    let mut chars = s.chars();
    if let Some(c) = chars.next().filter(|_| chars.next().is_none()) {
        let c = c.to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Ok(c as u32);
        }
    }
    Ok(match s {
        "Enter" | "Return" => 0x0D,
        "Tab" => 0x09,
        "Escape" | "Esc" => 0x1B,
        "Backspace" => 0x08,
        "Delete" | "Del" => 0x2E,
        "Space" => 0x20,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
        _ => bail!("unknown key: {s}"),
    })
}

/// Crop a PNG image to a normalized sub-region.
/// Returns None on any image processing error (caller falls back to full image).
fn crop_png_region(png: &[u8], rx: f32, ry: f32, rw: f32, rh: f32) -> Option<Vec<u8>> {
    use image::ImageReader;
    use std::io::Cursor;

    let img = ImageReader::new(Cursor::new(png))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;

    let full_w = img.width() as f32;
    let full_h = img.height() as f32;

    let x = (rx.clamp(0.0, 1.0) * full_w) as u32;
    let y = (ry.clamp(0.0, 1.0) * full_h) as u32;
    let w = (rw.clamp(0.0, 1.0) * full_w) as u32;
    let h = (rh.clamp(0.0, 1.0) * full_h) as u32;
    let w = w.min(img.width().saturating_sub(x)).max(1);
    let h = h.min(img.height().saturating_sub(y)).max(1);

    let cropped = img.crop_imm(x, y, w, h);
    let mut out = Vec::new();
    cropped
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_letters() {
        assert_eq!(parse_key("a").unwrap(), 'A' as u32);
        assert_eq!(parse_key("Z").unwrap(), 'Z' as u32);
        assert_eq!(parse_key("5").unwrap(), '5' as u32);
    }

    #[test]
    fn parse_key_code_maps_to_w3c_physical_keys() {
        assert_eq!(parse_key_code("a").as_deref(), Some("KeyA"));
        assert_eq!(parse_key_code("Z").as_deref(), Some("KeyZ"));
        assert_eq!(parse_key_code("5").as_deref(), Some("Digit5"));
        assert_eq!(parse_key_code("Enter").as_deref(), Some("Enter"));
        assert_eq!(parse_key_code("Esc").as_deref(), Some("Escape"));
        assert_eq!(parse_key_code("Up").as_deref(), Some("ArrowUp"));
        assert_eq!(parse_key_code("F11").as_deref(), Some("F11"));
    }

    /// Unknown names must degrade to the legacy path, not fail the tool call.
    #[test]
    fn parse_key_code_unknown_is_none() {
        assert_eq!(parse_key_code("NoSuchKey"), None);
    }

    /// Every code we emit must resolve in the host's mapping table — otherwise
    /// the host silently falls back to the broken raw cast.
    #[test]
    fn emitted_codes_resolve_in_host_keymap() {
        for name in [
            "a", "z", "0", "9", "Enter", "Return", "Tab", "Escape", "Esc", "Backspace", "Delete",
            "Del", "Space", "Up", "Down", "Left", "Right", "Home", "End", "PageUp", "PageDown",
            "F1", "F12",
        ] {
            let code = parse_key_code(name).unwrap_or_else(|| panic!("{name} unmapped"));
            assert!(
                miru_input::keymap::lookup(&code).is_some(),
                "{name} → {code} missing from miru_input::keymap"
            );
        }
    }

    /// The modifier keys a chord presses must also resolve.
    #[test]
    fn chord_modifier_codes_resolve() {
        for code in ["ShiftLeft", "ControlLeft", "AltLeft", "MetaLeft"] {
            assert!(miru_input::keymap::lookup(code).is_some(), "{code}");
            assert!(miru_input::keymap::code_to_vk(code).is_some(), "{code} vk");
        }
    }

    #[test]
    fn parse_key_named() {
        assert_eq!(parse_key("Enter").unwrap(), 0x0D);
        assert_eq!(parse_key("F12").unwrap(), 0x7B);
        assert!(parse_key("Bogus").is_err());
    }
}
