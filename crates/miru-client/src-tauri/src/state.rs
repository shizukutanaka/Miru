//! Tauri app state.

use anyhow::Result;
use miru_auth::{AclStore, DeviceIdentity, TrustedPeer};
use miru_common::message::{
    ClipboardFormat, ClipboardSync, FileTransfer, InputEvent, Msg, SelectDisplay,
};
use uuid::Uuid;
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

pub struct RecordingState {
    pub dir: PathBuf,
    pub session_id: String,
    pub start_ts_ms: u64,
    pub frame_count: u64,
}

pub struct AppState {
    identity: Arc<DeviceIdentity>,
    acl: Arc<Mutex<AclStore>>,
    config_dir: PathBuf,
    session: Arc<Mutex<Option<ActiveSession>>>,
    stats: Arc<Mutex<SessionStats>>,
    pub recording: Arc<Mutex<Option<RecordingState>>>,
    discovery: Arc<Mutex<Option<miru_discovery::Discovery>>>,
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

        let fingerprint = identity.pubkey_fingerprint();
        info!("Viewer fingerprint: {}", fingerprint);

        let discovery = start_discovery(&fingerprint);

        Self {
            identity: Arc::new(identity),
            acl: Arc::new(Mutex::new(acl)),
            config_dir,
            session: Arc::new(Mutex::new(None)),
            stats: Arc::new(Mutex::new(SessionStats::default())),
            recording: Arc::new(Mutex::new(None)),
            discovery: Arc::new(Mutex::new(discovery)),
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
        let recording = Arc::clone(&self.recording);
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
                    Arc::clone(&recording),
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

    pub async fn send_msg(&self, msg: Msg) -> Result<()> {
        let tx = self.session.lock().as_ref().map(|s| s.tx.clone());
        if let Some(tx) = tx {
            tx.send(msg).await.map_err(|_| anyhow::anyhow!("session closed"))?;
        }
        Ok(())
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

    pub async fn send_file_transfer(&self, name: String, data: Vec<u8>) -> Result<()> {
        let tx = self.session.lock().as_ref().map(|s| s.tx.clone());
        let tx = match tx {
            Some(t) => t,
            None => anyhow::bail!("no active session"),
        };

        // Compute SHA-256 hash
        let digest = ring::digest::digest(&ring::digest::SHA256, &data);
        let hash = digest
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();

        let id = Uuid::new_v4();
        let size = data.len() as u64;

        tx.send(Msg::FileTransfer(FileTransfer::Start {
            id,
            name,
            size,
            hash,
        }))
        .await
        .map_err(|_| anyhow::anyhow!("session closed"))?;

        const CHUNK_SIZE: usize = 256 * 1024;
        for (i, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
            tx.send(Msg::FileTransfer(FileTransfer::Chunk {
                id,
                offset: (i * CHUNK_SIZE) as u64,
                data: chunk.to_vec(),
            }))
            .await
            .map_err(|_| anyhow::anyhow!("session closed"))?;
        }

        tx.send(Msg::FileTransfer(FileTransfer::Done { id }))
            .await
            .map_err(|_| anyhow::anyhow!("session closed"))?;

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

    pub fn discover_lan_peers(&self) -> Vec<crate::commands::LanPeer> {
        let guard = self.discovery.lock();
        let Some(ref disc) = *guard else { return vec![] };
        disc.snapshot(None)
            .into_iter()
            .map(|p| crate::commands::LanPeer {
                device_id: p.device_id,
                friendly_name: p.friendly_name,
                addresses: p.addresses.iter().map(|a| a.to_string()).collect(),
                port: p.port,
                form_factor: match p.form_factor {
                    miru_constellation::FormFactor::Laptop => "laptop",
                    miru_constellation::FormFactor::Phone => "phone",
                    miru_constellation::FormFactor::Tablet => "tablet",
                    miru_constellation::FormFactor::Server => "server",
                    _ => "desktop",
                }
                .into(),
            })
            .collect()
    }

    pub fn start_recording(&self) -> Result<String> {
        let session_id = Uuid::new_v4().to_string();
        let dir = self.config_dir.join("recordings").join(&session_id);
        std::fs::create_dir_all(dir.join("frames"))?;
        let start_ts_ms = now_unix() * 1000;
        *self.recording.lock() = Some(RecordingState {
            dir,
            session_id: session_id.clone(),
            start_ts_ms,
            frame_count: 0,
        });
        info!("Recording started: {}", session_id);
        Ok(session_id)
    }

    pub fn stop_recording(&self) -> Result<crate::commands::RecordingSummary> {
        let rec = self.recording.lock().take();
        let rec = match rec {
            Some(r) => r,
            None => anyhow::bail!("no active recording"),
        };
        let duration_ms = now_unix() * 1000 - rec.start_ts_ms;
        let frame_count = rec.frame_count;
        let size_bytes = dir_size(&rec.dir);

        // Write metadata
        let meta = serde_json::json!({
            "session_id": rec.session_id,
            "start_ts_ms": rec.start_ts_ms,
            "duration_ms": duration_ms,
            "frame_count": frame_count,
        });
        std::fs::write(rec.dir.join("meta.json"), meta.to_string()).ok();
        info!("Recording stopped: {} ({} frames)", rec.session_id, frame_count);

        Ok(crate::commands::RecordingSummary {
            path: rec.dir.to_string_lossy().to_string(),
            session_id: rec.session_id,
            start_ts_ms: rec.start_ts_ms,
            duration_ms,
            frame_count,
            size_bytes,
        })
    }

    pub fn get_recording_frame(&self, recording_path: &str, frame_idx: u64) -> Result<String> {
        use base64::{engine::general_purpose::STANDARD as B64, Engine};
        let path = std::path::Path::new(recording_path)
            .join("frames")
            .join(format!("{:08}.jpg", frame_idx));
        let data = std::fs::read(&path)?;
        Ok(B64.encode(&data))
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
                if !path.is_dir() {
                    continue;
                }
                let meta_path = path.join("meta.json");
                if !meta_path.exists() {
                    continue;
                }
                let meta: serde_json::Value = std::fs::read_to_string(&meta_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::Value::Null);

                let session_id = meta["session_id"].as_str().unwrap_or("").to_string();
                let start_ts_ms = meta["start_ts_ms"].as_u64().unwrap_or(0);
                let duration_ms = meta["duration_ms"].as_u64().unwrap_or(0);
                let frame_count = meta["frame_count"].as_u64().unwrap_or(0);
                let size_bytes = dir_size(&path);

                out.push(crate::commands::RecordingSummary {
                    path: path.to_string_lossy().to_string(),
                    session_id,
                    start_ts_ms,
                    duration_ms,
                    frame_count,
                    size_bytes,
                });
            }
        }
        out.sort_by(|a, b| b.start_ts_ms.cmp(&a.start_ts_ms));
        out
    }
}

/// Start mDNS discovery (browse + advertise). Non-fatal if unavailable.
fn start_discovery(fingerprint: &str) -> Option<miru_discovery::Discovery> {
    use miru_constellation::{DeviceCapabilities, FormFactor};
    use miru_discovery::{Discovery, LocalAdvertisement};

    let advert = LocalAdvertisement {
        device_id: fingerprint.to_string(),
        constellation_pubkey: String::new(),
        friendly_name: format!("Miru Viewer ({})", fingerprint),
        form_factor: FormFactor::Desktop,
        port: 0, // viewer doesn't serve inbound connections
        capabilities: DeviceCapabilities {
            displays: vec![],
            has_microphone: false,
            has_speakers: true,
            has_camera: false,
            has_hw_encode: false,
            battery_pct: None,
            form_factor: FormFactor::Desktop,
            friendly_name: String::new(),
            os_family: std::env::consts::OS.to_string(),
        },
    };
    match Discovery::start(advert) {
        Ok(d) => {
            info!("mDNS discovery started");
            Some(d)
        }
        Err(e) => {
            tracing::warn!("mDNS discovery unavailable (non-fatal): {}", e);
            None
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else { return 0 };
    let mut total = 0u64;
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(m) = p.metadata() {
            total += m.len();
        }
    }
    total
}
