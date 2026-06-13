//! Host session: signal → relay → handshake → capture → stream loop.

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use miru_auth::{AclStore, DeviceIdentity, Permission, TrustDecision, TrustedPeer};
use miru_capture::{create_capturer, ScreenCapturer as _};
use miru_common::{
    message::{AudioCodec, ClipboardFormat, ClipboardSync, Features, FileTransfer, Msg, Role},
    session::DeviceId,
};
use miru_transport::{
    handshake::host_handshake,
    nat::discover_public_addr,
    relay::RelayTransport,
    signaling::{SignalClient, SignalEvent},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::time;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    agent_handler::AgentHandler, backpressure::FrameController, capture_loop,
    input_handler::InputHandler, metrics::SessionMetrics, qos_bbr::BbrQos,
    recording::SessionRecorder, safe_fs,
};

/// In-progress file receive state.
struct FileReceive {
    temp_path: PathBuf,
    file: std::fs::File,
    #[allow(dead_code)]
    expected_size: u64,
    expected_hash: String,
}

/// Resolve the directory where received files are saved.
/// Priority: MIRU_FILE_TRANSFER_DIR → ~/Downloads/Miru
fn file_transfer_dir() -> PathBuf {
    if let Ok(d) = std::env::var("MIRU_FILE_TRANSFER_DIR") {
        return PathBuf::from(d);
    }
    dirs::download_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Miru")
}

/// Handle an incoming FileTransfer message from a viewer with Full permission.
fn handle_file_transfer(
    ft: FileTransfer,
    transfers: &mut HashMap<Uuid, FileReceive>,
    ft_cfg: &safe_fs::FileTransferConfig,
) {
    match ft {
        FileTransfer::Start { id, name, size, hash } => {
            if size > ft_cfg.max_file_bytes {
                warn!("FileTransfer {id}: rejected — {size} bytes exceeds limit");
                return;
            }
            let safe_name = safe_fs::sanitize_filename(&name);
            if safe_name.is_empty() {
                warn!("FileTransfer {id}: rejected — empty filename after sanitization");
                return;
            }
            // Resolve the temp path through safe_fs to prevent path-traversal attacks.
            let temp_name = format!(".{id}.tmp");
            let temp_path = match safe_fs::resolve_safe_path(ft_cfg, &temp_name) {
                Ok(p) => p,
                Err(e) => {
                    warn!("FileTransfer {id}: path resolution failed: {e}");
                    return;
                }
            };
            match std::fs::File::create(&temp_path) {
                Ok(f) => {
                    info!("FileTransfer {id}: starting '{safe_name}' ({size} bytes)");
                    transfers.insert(id, FileReceive { temp_path, file: f, expected_size: size, expected_hash: hash });
                }
                Err(e) => warn!("FileTransfer {id}: cannot create temp file: {e}"),
            }
        }
        FileTransfer::Chunk { id, offset, data } => {
            if let Some(rx) = transfers.get_mut(&id) {
                if let Err(e) = rx.file.seek(SeekFrom::Start(offset))
                    .and_then(|_| rx.file.write_all(&data))
                {
                    warn!("FileTransfer {id}: write error at offset {offset}: {e}");
                    let _ = std::fs::remove_file(&rx.temp_path);
                    transfers.remove(&id);
                }
            } else {
                warn!("FileTransfer {id}: chunk for unknown transfer");
            }
        }
        FileTransfer::Done { id } => {
            if let Some(rx) = transfers.remove(&id) {
                drop(rx.file); // flush + close
                // Verify SHA-256
                match std::fs::read(&rx.temp_path) {
                    Ok(bytes) => {
                        let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
                        let actual_hash = digest
                            .as_ref()
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>();
                        if actual_hash != rx.expected_hash {
                            warn!("FileTransfer {id}: hash mismatch (corrupt?), deleting");
                            let _ = std::fs::remove_file(&rx.temp_path);
                            return;
                        }
                        // Rename to final path in the same directory.
                        let dir = rx.temp_path.parent().unwrap_or(std::path::Path::new("."));
                        // Find a non-conflicting name.
                        let name = rx.temp_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("transfer")
                            .trim_start_matches('.')
                            .trim_end_matches(".tmp");
                        let mut dest = dir.join(name);
                        let mut counter = 1u32;
                        while dest.exists() {
                            let stem = std::path::Path::new(name)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("transfer");
                            let ext = std::path::Path::new(name)
                                .extension()
                                .and_then(|s| s.to_str())
                                .map(|e| format!(".{e}"))
                                .unwrap_or_default();
                            dest = dir.join(format!("{stem} ({counter}){ext}"));
                            counter += 1;
                        }
                        match std::fs::rename(&rx.temp_path, &dest) {
                            Ok(_) => info!("FileTransfer {id}: saved to {}", dest.display()),
                            Err(e) => warn!("FileTransfer {id}: rename failed: {e}"),
                        }
                    }
                    Err(e) => {
                        warn!("FileTransfer {id}: cannot read temp file for hash check: {e}");
                        let _ = std::fs::remove_file(&rx.temp_path);
                    }
                }
            } else {
                warn!("FileTransfer {id}: Done for unknown transfer");
            }
        }
        FileTransfer::Abort { id, reason } => {
            if let Some(rx) = transfers.remove(&id) {
                info!("FileTransfer {id}: aborted by peer ({reason})");
                let _ = std::fs::remove_file(&rx.temp_path);
            }
        }
    }
}

#[derive(Clone)]
pub struct HostConfig {
    pub identity: Arc<DeviceIdentity>,
    pub acl: Arc<Mutex<AclStore>>,
    pub config_dir: PathBuf,
}

const SIGNAL_BACKOFF_MAX_SECS: u64 = 120;

pub async fn run(device_id: DeviceId, signal_url: String, config: HostConfig) -> Result<()> {
    // Discover public address once (non-fatal; None → skip direct-path advertisement).
    let pub_addr: Option<(String, u16)> = match tokio::time::timeout(
        Duration::from_secs(5),
        discover_public_addr("0.0.0.0:0".parse().unwrap()),
    )
    .await
    {
        Ok(Ok(addr)) => {
            info!("STUN: public addr = {}", addr);
            Some((addr.ip().to_string(), addr.port()))
        }
        Ok(Err(e)) => {
            info!("STUN: failed ({e}) — relay-only mode");
            None
        }
        Err(_) => {
            info!("STUN: timed out — relay-only mode");
            None
        }
    };

    let mut backoff_secs = 5u64;

    loop {
        let mut signal = match SignalClient::connect_with_pub_addr(
            &signal_url,
            &device_id,
            Some(config.identity.verifying_key.as_bytes()),
            pub_addr.clone(),
        )
        .await
        {
            Ok(s) => {
                backoff_secs = 5;
                s
            }
            Err(e) => {
                warn!("Signal connect failed: {}; retrying in {}s", e, backoff_secs);
                time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(SIGNAL_BACKOFF_MAX_SECS);
                continue;
            }
        };
        info!("Signal: registered as {}", device_id);
        info!("Identity fingerprint: {}", config.identity.pubkey_fingerprint());

        let reconnect = loop {
            match signal.next_event().await {
                Some(SignalEvent::Registered {
                    device_id: did,
                    relay_addr,
                }) => {
                    info!("Device ID: {}  relay: {:?}", did, relay_addr);
                }
                Some(SignalEvent::IncomingConnection {
                    token,
                    relay_addr,
                    relay_port,
                }) => {
                    info!("Incoming → relay {}:{}", relay_addr, relay_port);
                    let url = format!("ws://{relay_addr}:{relay_port}");
                    let cfg = config.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_viewer(url, token, cfg).await {
                            error!("Session: {}", e);
                        }
                    });
                }
                Some(SignalEvent::Disconnected) => {
                    warn!("Signal disconnected — reconnecting in {}s", backoff_secs);
                    break true;
                }
                Some(SignalEvent::Error { code, message }) => {
                    error!("Signal error {}: {}", code, message);
                }
                None => {
                    warn!("Signal stream ended — reconnecting in {}s", backoff_secs);
                    break true;
                }
                _ => {}
            }
        };

        if reconnect {
            time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(SIGNAL_BACKOFF_MAX_SECS);
        } else {
            break;
        }
    }
    Ok(())
}

async fn handle_viewer(relay_url: String, token: String, config: HostConfig) -> Result<()> {
    // 1. Connect to relay
    let mut relay = RelayTransport::connect(&relay_url, &token, "host").await?;

    // 2. Handshake (plaintext until cipher installed)
    let host_features = Features {
        codecs: miru_codec::available_codecs(),
        audio_codecs: vec![AudioCodec::Opus],
        hw_encode: !miru_codec::probe_hw().is_empty(),
        hw_decode: false,
        clipboard: true,
        file_transfer: true,
        audio: true,
        multi_monitor: true,
    };

    let result = time::timeout(
        Duration::from_secs(15),
        host_handshake(&mut relay, &config.identity.signing_key, host_features),
    )
    .await
    .map_err(|_| anyhow::anyhow!("handshake timed out after 15 s"))??;
    info!(
        "Handshake complete: session={} codec={:?}",
        result.session_id, result.selected_video_codec
    );

    // 3. ACL check
    let device_str = B64.encode(result.peer_identity_pubkey);
    let permission = check_or_pair(&config, &device_str, &result.peer_identity_pubkey).await?;

    // 3b. If the peer declared AiAgent role, build an AgentHandler gate.
    //     The token must be embedded in the pubkey field as a 4th segment.
    //     All input events pass through gate_input() before being injected.
    let agent_handler: Option<AgentHandler> = if result.peer_role == Role::AiAgent {
        match crate::agent_handler::extract_agent_token(&result.peer_pubkey_field) {
            Ok(token_str) => {
                use miru_agent::audit::AuditLog;
                use miru_agent::{AgentSession, AgentToken};
                let audit_path = config.config_dir.join("agent_audit.log");
                match AuditLog::open(&audit_path) {
                    Ok(audit) => {
                        let issuer_pub = config.identity.verifying_key;
                        match AgentToken::parse_and_verify(token_str, &issuer_pub) {
                            Ok(token) => {
                                // The headless host has no UI to raise a prompt,
                                // so Dangerous-tier actions (ShellExec, FileWrite,
                                // ClipboardRead, ...) are denied unconditionally;
                                // Normal/Conditional capabilities granted by the
                                // token still work. A native confirmation prompt
                                // is planned for the v0.3 host UI.
                                let confirm = std::sync::Arc::new(
                                    |req: &miru_agent::ConfirmRequest| {
                                        warn!(
                                        "Denying {:?} for agent '{}': requires confirmation, but headless host has no prompt UI",
                                        req.capability, req.agent_label
                                    );
                                        false
                                    },
                                );
                                let session = std::sync::Arc::new(AgentSession::open(
                                    token,
                                    std::sync::Arc::new(audit),
                                    confirm,
                                ));
                                info!("AI agent session opened — all inputs gated (dangerous caps require confirmation, unavailable headless)");
                                Some(AgentHandler::new(session))
                            }
                            Err(e) => {
                                warn!("Agent token invalid, closing AI agent connection: {}", e);
                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to open audit log: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                warn!("AiAgent role but no token in pubkey field: {}", e);
                return Ok(());
            }
        }
    } else {
        None
    };

    // 4. Install ciphers (subsequent traffic is encrypted)
    relay.install_ciphers(result.tx, result.rx).await;

    // 5. Build a fresh capturer for this session
    let capturer = create_capturer()?;

    // Send DisplayList so the viewer knows which displays are available.
    let display_list: Vec<miru_common::message::DisplayInfo> = capturer
        .displays()
        .unwrap_or_default()
        .into_iter()
        .map(|d| miru_common::message::DisplayInfo {
            index: d.index,
            width: d.width,
            height: d.height,
            refresh_hz: d.refresh_hz,
            name: d.name,
            primary: d.primary,
        })
        .collect();
    let _ = relay
        .send_msg(&Msg::DisplayList(miru_common::message::DisplayList {
            displays: display_list,
        }))
        .await;

    // 6. Start capture loop (display 0 by default)
    let qos = Arc::new(Mutex::new(BbrQos::new(60, 5000)));
    let (fps, br) = {
        let q = qos.lock();
        (q.fps(), q.bitrate_kbps())
    };
    // FrameController is seeded with the initial BBR values and updated
    // live via apply_qos() whenever BBR tick fires. The capture loop
    // reads target_fps and target_bitrate_kbps each frame so that QoS
    // changes propagate without restarting the encoder thread.
    let bp = Arc::new(FrameController::new(fps, br));
    let mut current_display_idx: u8 = 0;
    let mut cap_rx = capture_loop::start(
        capture_loop::CaptureConfig {
            codec: result.selected_video_codec.clone(),
            display_idx: current_display_idx,
            target_fps: fps,
            bitrate_kbps: br,
            frame_controller: bp.clone(),
        },
        capturer,
    )?;

    // Optional session recording (MIRU_RECORD_DIR=<dir> enables it).
    let mut recorder: Option<SessionRecorder> = if let Ok(dir) = std::env::var("MIRU_RECORD_DIR") {
        let sid = result.session_id.to_string().replace('-', "");
        match SessionRecorder::create(std::path::Path::new(&dir), &sid) {
            Ok(r) => {
                info!("Session recording enabled → {}", dir);
                Some(r)
            }
            Err(e) => {
                warn!("Recording init failed (non-fatal): {}", e);
                None
            }
        }
    } else {
        None
    };

    // File transfer setup (Full permission only).
    let ft_dir = file_transfer_dir();
    let ft_cfg = if let Permission::Full = permission {
        if let Err(e) = std::fs::create_dir_all(&ft_dir) {
            warn!("File transfer dir unavailable ({e}), transfers disabled");
            None
        } else {
            info!("File transfers → {}", ft_dir.display());
            Some(safe_fs::FileTransferConfig {
                allowed_roots: vec![ft_dir],
                ..Default::default()
            })
        }
    } else {
        None
    };
    let mut file_transfers: HashMap<Uuid, FileReceive> = HashMap::new();

    // 7. Main loop
    let mut input_handler = InputHandler::new();
    let mut ping_ticker = time::interval(Duration::from_secs(1));
    // Bytes sent since last ping tick — used to estimate delivery rate for BBR.
    let mut bytes_since_ping: u64 = 0;
    // Clipboard push: poll every second; push to viewer when content changes.
    // Only enabled for non-view-only peers.
    let clipboard_enabled = matches!(permission, Permission::Control | Permission::Full);
    let mut clip_ticker = time::interval(Duration::from_secs(1));
    let mut last_clip = String::new();
    let metrics = SessionMetrics::new(
        result.session_id,
        B64.encode(config.identity.verifying_key.as_bytes()),
        device_str.clone(),
        format!("{:?}", result.selected_video_codec).to_lowercase(),
        true, // v0.1 always uses relay
    );

    loop {
        tokio::select! {
            // Outgoing: video frames from capture loop
            Ok(msg) = cap_rx.recv_async() => {
                if let Msg::VideoFrame(ref vf) = msg {
                    bytes_since_ping += vf.data.len() as u64;
                    metrics.on_video_frame(vf.data.len() as u64);
                    bp.on_send();
                    if let Some(rec) = recorder.as_mut() {
                        if let Err(e) = rec.record_frame(vf) {
                            warn!("Record frame failed (non-fatal): {}", e);
                        }
                    }
                }
                if relay.send_msg(&msg).await.is_err() { break; }
            }
            // Incoming: input/control
            msg = relay.recv_msg() => {
                match msg? {
                    Some(Msg::InputEvent(evt)) if matches!(permission, Permission::Control | Permission::Full) => {
                        // If this is an AI agent session, every event must pass
                        // through the capability gate (authorize + audit log).
                        let allowed = match &agent_handler {
                            Some(gate) => gate.gate_input(&evt).is_ok(),
                            None => true, // human viewers: unconditional
                        };
                        if allowed {
                            let _ = input_handler.handle_input(&evt);
                        }
                    }
                    Some(Msg::ClipboardSync(s)) if matches!(permission, Permission::Full) => {
                        let _ = input_handler.handle_clipboard(&s);
                    }
                    Some(Msg::SelectDisplay(sel)) => {
                        if sel.index != current_display_idx {
                            info!("Display switch: {} → {}", current_display_idx, sel.index);
                            match create_capturer() {
                                Ok(new_capturer) => {
                                    let (fps, br) = {
                                        let q = qos.lock();
                                        (q.fps(), q.bitrate_kbps())
                                    };
                                    match capture_loop::start(
                                        capture_loop::CaptureConfig {
                                            codec: result.selected_video_codec.clone(),
                                            display_idx: sel.index,
                                            target_fps: fps,
                                            bitrate_kbps: br,
                                            frame_controller: bp.clone(),
                                        },
                                        new_capturer,
                                    ) {
                                        Ok(new_rx) => {
                                            // Drop old cap_rx; capture thread sees send fail and exits.
                                            cap_rx = new_rx;
                                            current_display_idx = sel.index;
                                        }
                                        Err(e) => warn!("Display switch failed: {e}"),
                                    }
                                }
                                Err(e) => warn!("Display switch: capturer create failed: {e}"),
                            }
                        }
                    }
                    Some(Msg::RequestClipboard) if clipboard_enabled => {
                        // Viewer explicitly requested clipboard content — push immediately.
                        if let Ok(text) = miru_input::get_clipboard() {
                            let _ = relay.send_msg(&Msg::ClipboardSync(ClipboardSync {
                                format: miru_common::message::ClipboardFormat::Text,
                                data: text.into_bytes(),
                            })).await;
                        }
                    }
                    Some(Msg::OpenUrl(req)) if matches!(permission, Permission::Full) => {
                        // Only Full-permission sessions may open URLs (same as ShellExec tier).
                        let url = req.url.trim().to_string();
                        if url.starts_with("https://") || url.starts_with("http://") {
                            if let Err(e) = open::that(&url) {
                                warn!("OpenUrl failed for {url}: {e}");
                            } else {
                                info!("Opened URL: {url}");
                            }
                        } else {
                            warn!("OpenUrl rejected non-http URL: {url}");
                        }
                    }
                    Some(Msg::QosHint(hint)) => {
                        // Apply viewer quality preference to QoS floor/ceiling.
                        let mut q = qos.lock();
                        q.apply_hint(&hint);
                        // MutexGuard drops here before any await
                    }
                    Some(Msg::FileTransfer(ft)) if ft_cfg.is_some() => {
                        if let Some(cfg) = ft_cfg.as_ref() {
                            handle_file_transfer(ft, &mut file_transfers, cfg);
                        }
                    }
                    Some(Msg::Pong(p)) => {
                        let rtt = now_ms().saturating_sub(p.ts) as u32;
                        bp.on_ack(1);
                        // Feed RTT into BBR (µs precision).
                        qos.lock().on_rtt(rtt.saturating_mul(1_000));
                    }
                    Some(Msg::Close(reason)) => {
                        info!("Viewer closed: {} {}", reason.code, reason.reason);
                        break;
                    }
                    None => break,
                    _ => {}
                }
            }
            _ = ping_ticker.tick() => {
                // Feed observed delivery rate (bytes in the last second → kbps).
                let kbps = (bytes_since_ping * 8 / 1024) as u32;
                bytes_since_ping = 0;
                let bbr_update = {
                    let mut q = qos.lock();
                    q.on_delivery(kbps.max(100));
                    q.tick()
                    // MutexGuard drops here — before any await
                };
                if let Some(u) = bbr_update {
                    // Push new fps/bitrate to the capture thread via shared atomics.
                    // This is what makes BBR adaptation actually affect the encoder;
                    // previously QosUpdate was only sent to the viewer for display.
                    bp.apply_qos(u.fps, u.bitrate_kbps);
                    let _ = relay.send_msg(&Msg::QosUpdate(u)).await;
                }
                let _ = relay.send_msg(&Msg::Ping(miru_common::message::Ping { ts: now_ms() })).await;
            }
            _ = clip_ticker.tick(), if clipboard_enabled => {
                if let Ok(text) = miru_input::get_clipboard() {
                    if !text.is_empty() && text != last_clip {
                        last_clip = text.clone();
                        let _ = relay.send_msg(&Msg::ClipboardSync(ClipboardSync {
                            format: ClipboardFormat::Text,
                            data: text.into_bytes(),
                        })).await;
                    }
                }
            }
        }
    }

    info!("Session {} ended", result.session_id);

    // Finalize recording (flush + log summary).
    if let Some(rec) = recorder.take() {
        match rec.finalize() {
            Ok(s) => info!(
                "Recording saved: {} ({} frames, {:.1} MB, {:.1}s)",
                s.path.display(),
                s.frame_count,
                s.bytes as f64 / 1024.0 / 1024.0,
                s.duration.as_secs_f64(),
            ),
            Err(e) => warn!("Recording finalize failed: {}", e),
        }
    }

    // 8. Transparency anchoring — record an immutable, signed commitment of
    //    this session's metadata. Posted to Rekor if MIRU_REKOR_URL is set.
    //    Non-fatal: anchoring failure must never break session teardown.
    if let Ok(rekor_url) = std::env::var("MIRU_REKOR_URL") {
        let metadata = metrics.snapshot();
        // Host-only attestation: the viewer's signing key is never available
        // server-side. Co-signing requires a 2-round protocol (planned v1.0).
        let attestation =
            miru_transparency::HostOnlyAttestation::new(&metadata, &config.identity.signing_key);
        match miru_transparency::rekor::submit_host_to_rekor(&rekor_url, &metadata, &attestation)
            .await
        {
            Ok(entry) => info!(
                "Session anchored in Rekor: index={} uuid={}",
                entry.log_index, entry.uuid
            ),
            Err(e) => warn!("Rekor anchoring failed (non-fatal): {}", e),
        }
    }

    Ok(())
}

async fn check_or_pair(
    config: &HostConfig,
    device_str: &str,
    pubkey: &[u8; 32],
) -> Result<Permission> {
    // Base64-encode the *actual* verified Ed25519 pubkey from the handshake.
    // Storing device_str here was a bug: any peer claiming a known device_id
    // would pass the pubkey_b64 == pubkey_b64 comparison (same string both sides),
    // making PubkeyMismatch unreachable and TOFU pinning completely ineffective.
    let pubkey_b64 = B64.encode(pubkey);
    let mut acl = config.acl.lock();
    match acl.check(device_str, &pubkey_b64) {
        TrustDecision::Trusted(p) => Ok(p),
        TrustDecision::Unknown => {
            // v0.1 behavior: auto-accept on first connection (TOFU — Trust On First Use).
            // The fingerprint is printed prominently so the user can verify out-of-band.
            // v0.3 will add a native UI confirmation dialog via Tauri IPC.
            //
            // Security note: TOFU is standard practice for SSH, WireGuard, and Signal.
            // The TOFU fingerprint log (miru-auth::fingerprint_log) records this pin
            // with a hash-chain so any subsequent change is detectable.
            let fp = pubkey_fingerprint(pubkey);
            warn!("┌─ FIRST CONNECTION from new device ─────────────────────────────────┐");
            warn!("│  Fingerprint: {}                      │", fp);
            warn!("│  Auto-accepting (TOFU). Verify this fingerprint out-of-band.        │");
            warn!("│  v0.3 will add a confirmation dialog. See SECURITY.md.              │");
            warn!("└─────────────────────────────────────────────────────────────────────┘");
            acl.trust(TrustedPeer {
                device_id: device_str.to_string(),
                pubkey_b64,
                fingerprint: fp,
                permission: Permission::Control,
                first_seen: now_unix(),
                last_seen: now_unix(),
                friendly_name: None,
            });
            let _ = acl.save(&config.config_dir.join("acl.json"));
            Ok(Permission::Control)
        }
        TrustDecision::PubkeyMismatch => {
            error!("⚠️  Pubkey mismatch for {} — rejecting (possible MITM or impersonation)", device_str);
            Err(anyhow::anyhow!(
                "pubkey mismatch for {device_str} — possible MITM or device re-keyed"
            ))
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_unix() -> u64 {
    now_ms() / 1000
}

fn pubkey_fingerprint(pk: &[u8; 32]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, pk);
    d.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}
