//! Miru viewer — Tauri shell.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod session;
mod state;

use state::AppState;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("MIRU_LOG").unwrap_or_else(|_| "miru_client=info,info".to_string()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::disconnect,
            commands::send_input,
            commands::send_clipboard,
            commands::list_trusted_peers,
            commands::revoke_peer,
            commands::session_stats,
            commands::fingerprint,
            commands::issue_agent_token,
            commands::audit_summary,
            commands::audit_entries,
            commands::constellation_devices,
            commands::list_recordings,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri app failed");
}
