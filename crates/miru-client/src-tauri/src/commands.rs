//! Tauri commands — invoked from frontend via window.__TAURI__.invoke().

use crate::state::{AppState, SessionStats};
use miru_auth::TrustedPeer;
use miru_common::message::{InputEvent, InputKind, MouseButton};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tracing::info;

#[derive(Serialize, Deserialize, Clone)]
pub struct ConnectArgs {
    pub device_id: String,
    pub signal_url: String,
    pub pin: Option<String>,
}

#[tauri::command]
pub async fn connect(
    args: ConnectArgs,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    info!("UI → connect to {}", args.device_id);
    state.connect(args, app).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    state.disconnect().await;
    Ok(())
}

/// Resolve a pending first-connection TOFU pairing prompt (see the
/// "pairing_required" session-event). Called from the fingerprint
/// confirmation dialog once the user has compared it out-of-band.
#[tauri::command]
pub fn confirm_pairing(accept: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.confirm_pairing(accept);
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SendInputArgs {
    pub kind: String, // "mouse_move", "mouse_down", etc.
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub button: Option<String>,
    pub key: Option<u32>,
    pub modifiers: Option<u8>,
    pub dx: Option<f32>,
    pub dy: Option<f32>,
    pub text: Option<String>,
}

#[tauri::command]
pub async fn send_input(args: SendInputArgs, state: State<'_, AppState>) -> Result<(), String> {
    let kind = match args.kind.as_str() {
        "mouse_move" => InputKind::MouseMove {
            x: args.x.unwrap_or(0.0),
            y: args.y.unwrap_or(0.0),
            display: 0,
        },
        "mouse_down" => InputKind::MouseDown {
            button: parse_button(&args.button),
            x: args.x.unwrap_or(0.0),
            y: args.y.unwrap_or(0.0),
        },
        "mouse_up" => InputKind::MouseUp {
            button: parse_button(&args.button),
            x: args.x.unwrap_or(0.0),
            y: args.y.unwrap_or(0.0),
        },
        "scroll" => InputKind::Scroll {
            dx: args.dx.unwrap_or(0.0),
            dy: args.dy.unwrap_or(0.0),
            x: args.x.unwrap_or(0.0),
            y: args.y.unwrap_or(0.0),
        },
        "key_down" => InputKind::KeyDown {
            key: args.key.unwrap_or(0),
            modifiers: args.modifiers.unwrap_or(0),
        },
        "key_up" => InputKind::KeyUp {
            key: args.key.unwrap_or(0),
            modifiers: args.modifiers.unwrap_or(0),
        },
        "text" => {
            let text = args.text.unwrap_or_default();
            // Cap text injection to prevent unbounded allocations.
            const MAX_TEXT_BYTES: usize = 8192;
            if text.len() > MAX_TEXT_BYTES {
                return Err(format!("text input too long (max {MAX_TEXT_BYTES} bytes)"));
            }
            InputKind::Text { text }
        }
        _ => return Err(format!("unknown input kind: {}", args.kind)),
    };

    let evt = InputEvent {
        kind,
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };
    state.send_input(evt).await.map_err(|e| e.to_string())
}

fn parse_button(s: &Option<String>) -> MouseButton {
    match s.as_deref() {
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        Some("x1") => MouseButton::X1,
        Some("x2") => MouseButton::X2,
        _ => MouseButton::Left,
    }
}

#[tauri::command]
pub async fn send_clipboard(text: String, state: State<'_, AppState>) -> Result<(), String> {
    const MAX_CLIPBOARD_BYTES: usize = 10 * 1024 * 1024; // 10 MiB
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err("clipboard content too large (max 10 MiB)".to_string());
    }
    state.send_clipboard(text).await.map_err(|e| e.to_string())
}

/// Send a file to the connected host. `data_b64` is the base64-encoded file bytes.
/// Max 100 MB — larger files should use dedicated transfer mechanisms.
#[tauri::command]
pub async fn send_file(
    name: String,
    data_b64: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use base64::Engine as _;
    if name.trim().is_empty() {
        return Err("filename required".into());
    }
    // Pre-check base64 length to avoid allocating a large buffer before
    // knowing the decoded size. Base64 overhead is ~4/3; add a small margin.
    const MAX_DATA_BYTES: usize = 100 * 1024 * 1024;
    const MAX_B64_BYTES: usize = MAX_DATA_BYTES * 4 / 3 + 1024;
    if data_b64.len() > MAX_B64_BYTES {
        return Err("file exceeds 100 MB limit".into());
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(&data_b64)
        .map_err(|e| format!("base64 decode: {e}"))?;
    if data.len() > MAX_DATA_BYTES {
        return Err("file exceeds 100 MB limit".into());
    }
    // Sanitize filename: keep printable ASCII excluding path separators.
    let safe_name = name
        .chars()
        .filter(|&c| c.is_ascii() && !matches!(c, '/' | '\\' | '\0'))
        .take(255)
        .collect::<String>();
    // If the original name contained only non-ASCII characters (e.g. Japanese
    // filenames), safe_name is now empty — reject rather than forwarding an
    // empty filename to the host.
    if safe_name.trim().is_empty() {
        return Err("filename must contain at least one ASCII character".into());
    }
    state
        .send_file_transfer(safe_name, data)
        .await
        .map_err(|e| e.to_string())
}

/// Ask the host to send its current clipboard content immediately.
#[tauri::command]
pub async fn request_clipboard(state: State<'_, AppState>) -> Result<(), String> {
    state.send_msg(miru_common::message::Msg::RequestClipboard).await.map_err(|e| e.to_string())
}

/// Send a viewer QoS preference hint to the host.
/// `mode`: "quality" | "balanced" | "smooth"
/// `max_fps`: hard FPS cap (0 = no cap)
/// `min_quality`: quality floor 0–100 (0 = no floor)
#[tauri::command]
pub async fn send_qos_hint(
    mode: String,
    max_fps: u8,
    min_quality: u8,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use miru_common::message::{Msg, QosHint};
    state
        .send_msg(Msg::QosHint(QosHint { mode, max_fps, min_quality }))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn select_display(index: u8, state: State<'_, AppState>) -> Result<(), String> {
    state
        .send_select_display(index)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_trusted_peers(state: State<'_, AppState>) -> Vec<TrustedPeer> {
    state.list_peers()
}

#[tauri::command]
pub fn revoke_peer(device_id: String, state: State<'_, AppState>) -> bool {
    state.revoke_peer(&device_id)
}

#[tauri::command]
pub fn session_stats(state: State<'_, AppState>) -> SessionStats {
    state.stats()
}

#[tauri::command]
pub fn fingerprint(state: tauri::State<'_, AppState>) -> String {
    state.fingerprint()
}

// ─── Agent token issuance ────────────────────────────────────────────────────

use miru_agent::{AgentToken, Capability};
use std::collections::HashSet;

#[derive(serde::Deserialize)]
pub struct IssueTokenArgs {
    pub label: String,
    pub ttl_mins: u64,
    pub capabilities: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct IssuedToken {
    pub token: String,
    pub fingerprint: String,
    pub expires_at: u64,
}

#[tauri::command]
pub fn issue_agent_token(
    args: IssueTokenArgs,
    state: tauri::State<'_, AppState>,
) -> Result<IssuedToken, String> {
    let label = args.label.trim();
    if label.is_empty() {
        return Err("label required".into());
    }
    if label.chars().count() > 64 {
        return Err("label too long (max 64 chars)".into());
    }
    // Hard cap: 15 minutes (see miru_agent::token::MAX_TTL_SECS).
    if args.ttl_mins == 0 {
        return Err("ttl required".into());
    }
    if args.ttl_mins > 15 {
        return Err("ttl exceeds policy cap (max 15 min)".into());
    }
    let caps: HashSet<Capability> = args
        .capabilities
        .iter()
        .filter_map(|s| match s.as_str() {
            "screen_read" => Some(Capability::ScreenRead),
            "pointer_move" => Some(Capability::PointerMove),
            "pointer_click" => Some(Capability::PointerClick),
            "key_type" => Some(Capability::KeyType),
            "key_combo" => Some(Capability::KeyCombo),
            "clipboard_read" => Some(Capability::ClipboardRead),
            "clipboard_write" => Some(Capability::ClipboardWrite),
            "file_read" => Some(Capability::FileRead),
            "file_write" => Some(Capability::FileWrite),
            "shell_exec" => Some(Capability::ShellExec),
            "open_url" => Some(Capability::OpenUrl),
            _ => None,
        })
        .collect();
    if caps.is_empty() {
        return Err("at least one capability required".into());
    }

    let identity = state.identity_signing_key();
    let token = AgentToken::issue(
        identity,
        label.to_string(),
        caps,
        std::time::Duration::from_secs(args.ttl_mins * 60),
        None,
    );
    Ok(IssuedToken {
        token: token.to_string(),
        fingerprint: state.fingerprint(),
        expires_at: token.payload.exp,
    })
}

// ─── Audit log reader ────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct AuditSummary {
    pub total_entries: u64,
    pub chain_intact: bool,
    pub last_entry_seq: u64,
    pub last_entry_ts_ms: u64,
}

#[tauri::command]
pub fn audit_summary(state: tauri::State<'_, AppState>) -> Result<AuditSummary, String> {
    state.audit_summary().map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct AuditEntryView {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub token_jti: String,
    pub capability: String,
    pub action: serde_json::Value,
    pub confirmed: Option<bool>,
    pub outcome: String,
    pub hash_short: String,
}

#[tauri::command]
pub fn audit_entries(
    limit: Option<u64>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AuditEntryView>, String> {
    state
        .audit_entries(limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

// ─── LAN discovery ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct LanPeer {
    pub device_id: String,
    pub friendly_name: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub form_factor: String,
}

#[tauri::command]
pub fn discover_lan_peers(state: tauri::State<'_, AppState>) -> Vec<LanPeer> {
    state.discover_lan_peers()
}

// ─── Constellation ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct ConstellationDevice {
    pub device_id: String,
    pub name: String,
    pub form_factor: String,
    pub status: String,
    pub last_seen_secs_ago: u64,
    pub addresses: Vec<String>,
}

#[tauri::command]
pub fn constellation_devices(state: tauri::State<'_, AppState>) -> Vec<ConstellationDevice> {
    state.constellation_devices()
}

// ─── Recordings (Temporal) ───────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct RecordingSummary {
    pub path: String,
    pub session_id: String,
    pub start_ts_ms: u64,
    pub duration_ms: u64,
    pub frame_count: u64,
    pub size_bytes: u64,
}

#[tauri::command]
pub fn list_recordings(state: tauri::State<'_, AppState>) -> Vec<RecordingSummary> {
    state.list_recordings()
}

#[tauri::command]
pub fn start_recording(state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.start_recording().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn stop_recording(state: tauri::State<'_, AppState>) -> Result<RecordingSummary, String> {
    state.stop_recording().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_recording_frame(
    recording_path: String,
    frame_idx: u64,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    state
        .get_recording_frame(&recording_path, frame_idx)
        .map_err(|e| e.to_string())
}
