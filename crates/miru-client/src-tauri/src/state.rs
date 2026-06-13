//! Tauri app state.

use anyhow::Result;
use miru_auth::{AclStore, DeviceIdentity, TrustedPeer};
use miru_common::message::{ClipboardFormat, ClipboardSync, InputEvent, Msg, SelectDisplay};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tracing::info;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SessionStats {
    pub fps: f32,
    pub bitrate_kbps: u32,
    pub rtt_ms: u32,
    pub bytes_recv: u64,
    pub frames_decoded: u64,
    pub packet_loss_pct: f32,
}

pub struct AppState {
    identity: Arc<DeviceIdentity>,
    acl: Arc<Mutex<AclStore>>,
    config_dir: PathBuf,
    session: Arc<Mutex<Option<ActiveSession>>>,
    stats: Arc<Mutex<SessionStats>>,
}

struct ActiveSession {
    tx: mpsc::Sender<Msg>,
    cancel: tokio::sync::oneshot::Sender<()>,
}

/// Maximum number of auto-reconnect attempts on unexpected disconnect.
const MAX_RECONNECT_ATTEMPTS: u32 = 3;

impl AppState {
    pub fn new() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("miru-viewer");
        std::fs::create_dir_all(&config_dir).ok();

        let identity =
            DeviceIdentity::load_or_create(&config_dir.join("identity")).unwrap_or_else(|e| {
                // Identity creation failed — typically a filesystem permission error.
                // Log clearly; Tauri will show the main window which can surface this.
                tracing::error!("Identity load failed: {}; using ephemeral identity", e);
                DeviceIdentity::generate()
            });
        let acl = AclStore::load(&config_dir.join("acl.json")).unwrap_or_default();

        info!("Viewer fingerprint: {}", identity.pubkey_fingerprint());

        Self {
            identity: Arc::new(identity),
            acl: Arc::new(Mutex::new(acl)),
            config_dir,
            session: Arc::new(Mutex::new(None)),
            stats: Arc::new(Mutex::new(SessionStats::default())),
        }
    }

    pub fn fingerprint(&self) -> String {
        self.identity.pubkey_fingerprint()
    }

    pub async fn connect(&self, args: crate::commands::ConnectArgs, app: AppHandle) -> Result<()> {
        self.disconnect().await;

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Msg>(64);
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();

        *self.session.lock() = Some(ActiveSession {
            tx: cmd_tx,
            cancel: cancel_tx,
        });

        let identity = Arc::clone(&self.identity);
        let stats = Arc::clone(&self.stats);
        let app_clone = app.clone();
        let session_slot = Arc::clone(&self.session);

        tokio::spawn(async move {
            let mut attempt = 0u32;
            loop {
                let result = crate::session::run(
                    args.clone(),
                    Arc::clone(&identity),
                    cmd_rx,
                    cancel_rx,
                    app_clone.clone(),
                    Arc::clone(&stats),
                )
                .await;

                // Session slot cleared whether we reconnect or not.
                *session_slot.lock() = None;

                match result {
                    Ok(()) => break, // clean exit (user disconnect or host close)
                    Err(e) => {
                        attempt += 1;
                        if attempt > MAX_RECONNECT_ATTEMPTS {
                            tracing::warn!("Session failed after {} attempts: {}", attempt, e);
                            break;
                        }
                        let delay_secs = 1u64 << attempt; // 2, 4, 8
                        tracing::warn!(
                            "Session error (attempt {}/{}): {}; retrying in {}s",
                            attempt, MAX_RECONNECT_ATTEMPTS, e, delay_secs
                        );
                        let _ = app_clone.emit("session-event", crate::session::SessionEvent {
                            kind: "reconnecting".to_string(),
                            message: Some(format!("再接続 ({attempt}/{MAX_RECONNECT_ATTEMPTS})...")),
                            fingerprint: None,
                        });
                        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                        // Re-create cmd channel for the new attempt.
                        let (new_cmd_tx, new_cmd_rx) = mpsc::channel::<Msg>(64);
                        let (new_cancel_tx, new_cancel_rx) = tokio::sync::oneshot::channel();
                        *session_slot.lock() = Some(ActiveSession {
                            tx: new_cmd_tx,
                            cancel: new_cancel_tx,
                        });
                        cmd_rx = new_cmd_rx;
                        cancel_rx = new_cancel_rx;
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn disconnect(&self) {
        if let Some(s) = self.session.lock().take() {
            let _ = s.cancel.send(());
            info!("Session disconnected by UI");
        }
    }

    pub async fn send_input(&self, evt: InputEvent) -> Result<()> {
        let tx = self.session.lock().as_ref().map(|s| s.tx.clone());
        if let Some(tx) = tx {
            tx.send(Msg::InputEvent(evt))
                .await
                .map_err(|_| anyhow::anyhow!("session closed"))?;
        }
        Ok(())
    }

    pub async fn send_select_display(&self, index: u8) -> Result<()> {
        let tx = self.session.lock().as_ref().map(|s| s.tx.clone());
        if let Some(tx) = tx {
            tx.send(Msg::SelectDisplay(SelectDisplay { index }))
                .await
                .map_err(|_| anyhow::anyhow!("session closed"))?;
        }
        Ok(())
    }

    pub async fn send_clipboard(&self, text: String) -> Result<()> {
        let tx = self.session.lock().as_ref().map(|s| s.tx.clone());
        if let Some(tx) = tx {
            tx.send(Msg::ClipboardSync(ClipboardSync {
                format: ClipboardFormat::Text,
                data: text.into_bytes(),
            }))
            .await
            .map_err(|_| anyhow::anyhow!("session closed"))?;
        }
        Ok(())
    }

    pub fn list_peers(&self) -> Vec<TrustedPeer> {
        self.acl.lock().list().into_iter().cloned().collect()
    }

    pub fn revoke_peer(&self, device_id: &str) -> bool {
        let mut acl = self.acl.lock();
        let removed = acl.revoke(device_id);
        if removed {
            let _ = acl.save(&self.config_dir.join("acl.json"));
        }
        removed
    }

    pub fn stats(&self) -> SessionStats {
        self.stats.lock().clone()
    }
}

// ─── Extensions for unique features ──────────────────────────────────────────

use ed25519_dalek::SigningKey;
use miru_agent::{AuditEntry, AuditLog};

impl AppState {
    pub fn identity_signing_key(&self) -> &SigningKey {
        &self.identity.signing_key
    }

    fn audit_log_path(&self) -> std::path::PathBuf {
        self.config_dir.join("agent-audit.log")
    }

    pub fn audit_summary(&self) -> anyhow::Result<crate::commands::AuditSummary> {
        let path = self.audit_log_path();
        if !path.exists() {
            return Ok(crate::commands::AuditSummary {
                total_entries: 0,
                chain_intact: true,
                last_entry_seq: 0,
                last_entry_ts_ms: 0,
            });
        }
        // Read lines, verify chain
        use std::io::BufRead;
        let f = std::fs::File::open(&path)?;
        let r = std::io::BufReader::new(f);
        let mut entries = Vec::new();
        for line in r.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(&line)?;
            entries.push(entry);
        }
        let chain_intact = AuditLog::open(&path).is_ok();
        let last = entries.last();
        Ok(crate::commands::AuditSummary {
            total_entries: entries.len() as u64,
            chain_intact,
            last_entry_seq: last.map(|e| e.seq).unwrap_or(0),
            last_entry_ts_ms: last.map(|e| e.timestamp_ms).unwrap_or(0),
        })
    }

    pub fn audit_entries(
        &self,
        limit: u64,
    ) -> anyhow::Result<Vec<crate::commands::AuditEntryView>> {
        let path = self.audit_log_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        use std::io::BufRead;
        let f = std::fs::File::open(&path)?;
        let r = std::io::BufReader::new(f);
        let mut entries = Vec::new();
        for line in r.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(&line)?;
            let hash = entry.hash();
            entries.push(crate::commands::AuditEntryView {
                seq: entry.seq,
                timestamp_ms: entry.timestamp_ms,
                token_jti: entry.token_jti,
                capability: format!("{:?}", entry.capability),
                action: entry.action,
                confirmed: entry.confirmed,
                outcome: format!("{:?}", entry.outcome).to_lowercase(),
                hash_short: hash[..12].to_string(),
            });
        }
        // Latest first, limited
        entries.reverse();
        entries.truncate(limit as usize);
        Ok(entries)
    }

    pub fn constellation_devices(&self) -> Vec<crate::commands::ConstellationDevice> {
        // Stub: no live discovery yet wired. Returns trusted-peer cache.
        // Real implementation in v0.3 plugs miru-discovery + Constellation.
        self.acl
            .lock()
            .list()
            .iter()
            .map(|p| crate::commands::ConstellationDevice {
                device_id: p.device_id.clone(),
                name: p
                    .friendly_name
                    .clone()
                    .unwrap_or_else(|| p.device_id.clone()),
                form_factor: "desktop".into(),
                status: "offline".into(),
                last_seen_secs_ago: now_unix().saturating_sub(p.last_seen),
                addresses: vec![],
            })
            .collect()
    }

    pub fn list_recordings(&self) -> Vec<crate::commands::RecordingSummary> {
        let dir = self.config_dir.join("recordings");
        if !dir.exists() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("mkv") {
                    continue;
                }
                let meta = match path.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let size = meta.len();
                let start_ts_ms = meta
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                let session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.split('-').nth(1))
                    .unwrap_or("unknown")
                    .to_string();

                out.push(crate::commands::RecordingSummary {
                    path: path.to_string_lossy().to_string(),
                    session_id,
                    start_ts_ms,
                    duration_ms: 0, // requires reading the recording header
                    frame_count: 0, // ditto
                    size_bytes: size,
                });
            }
        }
        out.sort_by(|a, b| b.start_ts_ms.cmp(&a.start_ts_ms));
        out
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
