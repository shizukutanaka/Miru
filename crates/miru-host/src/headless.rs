//! Headless mode — run as a system service.
//!
//! Used for:
//!   - Servers without monitors (with virtual display via `xvfb-run` or a dummy HDMI plug)
//!   - CI/CD environments
//!   - Background install via system-service installer
//!
//! Differences from interactive mode:
//!   - No tray icon, no notification popups
//!   - All log to syslog/journald/EventLog (not stdout)
//!   - Auto-accept ACL = false: requires pre-pairing via CLI
//!   - Listen-only socket on localhost for IPC (control via `miru-host ctl`)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadlessConfig {
    /// Listen address for the local IPC socket (control plane).
    pub ipc_socket: String,
    /// Path to a JSON of pre-paired peers (no interactive accept).
    pub paired_peers: PathBuf,
    /// Whether to auto-launch on system boot.
    pub launch_on_boot: bool,
    /// Maximum concurrent sessions (0 = unlimited).
    pub max_sessions: u32,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            #[cfg(unix)]
            ipc_socket: "/var/run/miru.sock".to_string(),
            #[cfg(not(unix))]
            ipc_socket: r"\\.\pipe\miru".to_string(),
            paired_peers: PathBuf::from("/etc/miru/peers.json"),
            launch_on_boot: false,
            max_sessions: 0,
        }
    }
}

/// Detect whether we're running headless. Heuristics:
///   - Linux: no $DISPLAY and no $WAYLAND_DISPLAY → headless
///   - Windows: SERVICES session id (0) or no console window
///   - macOS: launchd-managed (no controlling terminal)
pub fn detect_headless() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err()
    }
    #[cfg(target_os = "macos")]
    {
        // launchctl-spawned processes have no controlling terminal
        unsafe { libc::isatty(0) == 0 && libc::isatty(1) == 0 }
    }
    #[cfg(target_os = "windows")]
    {
        // Check if running as a service (session 0 isolation)
        use std::os::windows::ffi::OsStringExt;
        // Defer to env var; reliable detection requires winapi
        std::env::var("MIRU_HEADLESS").is_ok()
    }
}

/// systemd unit file content.
pub fn systemd_unit() -> &'static str {
    include_str!("../res/miru-host.service")
}

/// launchd plist (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn launchd_plist() -> &'static str {
    include_str!("../res/app.miru.host.plist")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_sane() {
        let cfg = HeadlessConfig::default();
        assert!(!cfg.ipc_socket.is_empty());
        assert!(!cfg.launch_on_boot);
    }
}
