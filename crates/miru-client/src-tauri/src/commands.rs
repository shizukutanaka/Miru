//! Tauri commands — invoked from frontend via window.__TAURI__.invoke().

use crate::state::{AppState, SessionStats};
use miru_auth::TrustedPeer;
use miru_common::message::{InputEvent, InputKind, MouseButton};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tracing::info;

#[derive(Serialize, Deserialize)]
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
        "text" => InputKind::Text {
            text: args.text.unwrap_or_default(),
        },
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
    state.send_clipboard(text).await.map_err(|e| e.to_string())
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
    pub ttl_hours: u64,
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
    // Hard cap: 15 minutes. See miru_agent::token::MAX_TTL_SECS.
    // We accept input in minutes (not hours) to match the cap granularity.
    if args.ttl_hours == 0 {
        return Err("ttl required".into());
    }
    if args.ttl_hours > 1 {
        return Err("ttl exceeds policy cap (max 15 min — please use shorter sessions)".into());
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
        std::time::Duration::from_secs(args.ttl_hours * 3600),
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
