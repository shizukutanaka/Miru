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
    io::{BufReader, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::time;
use tracing::{error, info, warn};
use uuid::Uuid;

#[cfg(feature = "agent")]
use crate::agent_handler::AgentHandler;
use crate::{
    backpressure::FrameController, capture_loop, input_handler::InputHandler,
    metrics::SessionMetrics, qos_bbr::BbrQos, recording::SessionRecorder, safe_fs,
};

/// In-progress file receive state.
struct FileReceive {
    temp_path: PathBuf,
    file: std::fs::File,
    expected_size: u64,
    expected_hash: String,
    /// Sanitized filename from the Start message — used as the final save name.
    name: String,
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
        FileTransfer::Start {
            id,
            name,
            size,
            hash,
        } => {
            // Cap concurrent in-flight transfers to bound open file descriptor usage.
            const MAX_CONCURRENT_TRANSFERS: usize = 8;
            if transfers.len() >= MAX_CONCURRENT_TRANSFERS {
                warn!("FileTransfer {id}: rejected — too many concurrent transfers (limit {MAX_CONCURRENT_TRANSFERS})");
                return;
            }
            if size > ft_cfg.max_file_bytes {
                warn!("FileTransfer {id}: rejected — {size} bytes exceeds limit");
                return;
            }
            let safe_name = safe_fs::sanitize_filename(&name);
            if safe_name.is_empty() {
                warn!("FileTransfer {id}: rejected — empty filename after sanitization");
                return;
            }
            // SHA-256 as hex is exactly 64 lowercase hex chars.
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                warn!("FileTransfer {id}: rejected — invalid hash format");
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
            let temp_file_result = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&temp_path)
                }
                #[cfg(not(unix))]
                {
                    std::fs::File::create(&temp_path)
                }
            };
            match temp_file_result {
                Ok(f) => {
                    info!("FileTransfer {id}: starting '{safe_name}' ({size} bytes)");
                    transfers.insert(
                        id,
                        FileReceive {
                            temp_path,
                            file: f,
                            expected_size: size,
                            expected_hash: hash,
                            name: safe_name,
                        },
                    );
                }
                Err(e) => warn!("FileTransfer {id}: cannot create temp file: {e}"),
            }
        }
        FileTransfer::Chunk { id, offset, data } => {
            if let Some(rx) = transfers.get_mut(&id) {
                // Reject chunks that would write past the declared file size.
                // Without this check a sender can seek the temp file to an
                // arbitrary offset, creating a sparse file that uses disk space
                // far beyond max_file_bytes while still passing the Start size check.
                let chunk_end = offset.saturating_add(data.len() as u64);
                if chunk_end > rx.expected_size {
                    warn!("FileTransfer {id}: chunk at {offset}+{} exceeds declared size {} — aborting", data.len(), rx.expected_size);
                    let _ = std::fs::remove_file(&rx.temp_path);
                    transfers.remove(&id);
                    return;
                }
                if let Err(e) = rx
                    .file
                    .seek(SeekFrom::Start(offset))
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
                               // Stream SHA-256 rather than loading the entire file into memory —
                               // std::fs::read() on a 100 MB file would double peak RSS.
                match hash_file_streaming(&rx.temp_path) {
                    Ok(actual_hash) => {
                        if actual_hash != rx.expected_hash {
                            warn!("FileTransfer {id}: hash mismatch (corrupt?), deleting");
                            let _ = std::fs::remove_file(&rx.temp_path);
                            return;
                        }
                        // Rename to final path in the same directory.
                        let dir = rx.temp_path.parent().unwrap_or(std::path::Path::new("."));
                        // Use the sanitized name stored at Start time — deriving from
                        // temp_path would yield the UUID, not the original filename.
                        let name = &rx.name;
                        let mut dest = dir.join(name);
                        let mut counter = 1u32;
                        const MAX_DEDUP: u32 = 9_999;
                        while dest.exists() && counter <= MAX_DEDUP {
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
                        if counter > MAX_DEDUP {
                            warn!("FileTransfer {id}: too many files with same name — aborting");
                            let _ = std::fs::remove_file(&rx.temp_path);
                            return;
                        }
                        match std::fs::rename(&rx.temp_path, &dest) {
                            Ok(_) => info!("FileTransfer {id}: saved to {}", dest.display()),
                            Err(e) => {
                                warn!("FileTransfer {id}: rename failed: {e}");
                                let _ = std::fs::remove_file(&rx.temp_path);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("FileTransfer {id}: hash check failed: {e}");
                        let _ = std::fs::remove_file(&rx.temp_path);
                    }
                }
            } else {
                warn!("FileTransfer {id}: Done for unknown transfer");
            }
        }
        FileTransfer::Abort { id, reason } => {
            if let Some(rx) = transfers.remove(&id) {
                let reason_trunc: String = reason.chars().take(200).collect();
                info!("FileTransfer {id}: aborted by peer ({reason_trunc})");
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
    /// Opt-in strict mode: refuse unapproved (first-connection) devices
    /// instead of auto-accepting via TOFU. Read once from
    /// `MIRU_REQUIRE_PAIRING_CONFIRM` at startup (see main.rs) rather than
    /// inside `check_or_pair`, matching how other env-derived settings
    /// (MIRU_RDV_PORT, MIRU_FRIENDLY_NAME) are handled in this binary.
    pub require_pairing_confirm: bool,
}

const SIGNAL_BACKOFF_MAX_SECS: u64 = 120;

pub async fn run(device_id: DeviceId, signal_url: String, config: HostConfig) -> Result<()> {
    // Discover public address once (non-fatal; None → skip direct-path advertisement).
    let pub_addr: Option<(String, u16)> = match tokio::time::timeout(
        Duration::from_secs(5),
        discover_public_addr(std::net::SocketAddr::from(([0, 0, 0, 0], 0))),
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

    // Attempt counter for jittered backoff. Plain doubling synchronised every
    // host pointed at the same signal server: they all disconnect together on a
    // restart and then retry in lockstep. See miru_common::backoff.
    const SIGNAL_BACKOFF_BASE_SECS: u64 = 5;
    let mut attempt: u32 = 0;

    loop {
        let mut signal = match SignalClient::connect_with_pub_addr_signed(
            &signal_url,
            &device_id,
            Some(config.identity.verifying_key.as_bytes()),
            Some(&config.identity.signing_key),
            pub_addr.clone(),
        )
        .await
        {
            Ok(s) => {
                attempt = 0;
                s
            }
            Err(e) => {
                let delay = miru_common::backoff::next_delay(
                    attempt,
                    SIGNAL_BACKOFF_BASE_SECS,
                    SIGNAL_BACKOFF_MAX_SECS,
                );
                warn!(
                    "Signal connect failed: {}; retrying in {}s",
                    e,
                    delay.as_secs()
                );
                time::sleep(delay).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
        };
        info!("Signal: registered as {}", device_id);
        info!(
            "Identity fingerprint: {}",
            config.identity.pubkey_fingerprint()
        );

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
                    warn!("Signal disconnected — reconnecting");
                    break true;
                }
                Some(SignalEvent::Error { code, message }) => {
                    error!("Signal error {}: {}", code, message);
                }
                None => {
                    warn!("Signal stream ended — reconnecting");
                    break true;
                }
                _ => {}
            }
        };

        if reconnect {
            let delay = miru_common::backoff::next_delay(
                attempt,
                SIGNAL_BACKOFF_BASE_SECS,
                SIGNAL_BACKOFF_MAX_SECS,
            );
            warn!("Reconnecting to signal in {}s", delay.as_secs());
            time::sleep(delay).await;
            attempt = attempt.saturating_add(1);
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
        // Advertise hardware encoding only if this build can actually do it.
        // probe_hw() reports silicon, which is true on nearly every desktop and
        // was telling viewers we had an encoder we never compiled in.
        hw_encode: miru_codec::has_hw_encode(),
        hw_decode: false,
        clipboard: true,
        file_transfer: true,
        // Advertise audio only when a loopback/monitor capture device actually
        // exists (PulseAudio/PipeWire monitor source, or a virtual loopback
        // device). Capturing the microphone and calling it "system audio" would
        // be dishonest, so audio_loop uses monitor devices only; if none is
        // present we advertise false and never send AudioFrame.
        audio: miru_audio::capture::loopback_available(),
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
    // Without the `agent` feature there is no capability gate compiled in, so a
    // peer claiming AiAgent must be refused outright — falling through would
    // hand it the ungated human path (see ADR 0022).
    #[cfg(not(feature = "agent"))]
    if result.peer_role == Role::AiAgent {
        warn!(
            "Peer requested AiAgent role, but this build has the agent feature disabled — refusing"
        );
        return Ok(());
    }

    #[cfg(feature = "agent")]
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
                        // Audit log is mandatory for AI agent sessions: proceed
                        // without it would make the None => true fallback grant
                        // all inputs unconditionally, bypassing capability gating.
                        warn!(
                            "Cannot open audit log for AI agent session — rejecting connection: {}",
                            e
                        );
                        return Err(anyhow::anyhow!("audit log unavailable: {e}"));
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
            x: d.x,
            y: d.y,
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

    // Optional system-audio capture. Only when the viewer negotiated Opus and a
    // loopback/monitor device exists; otherwise degrade silently to no audio
    // (the viewer already knows from Features.audio whether to expect it).
    let mut audio_rx: Option<flume::Receiver<Msg>> =
        if result.selected_audio_codec == AudioCodec::Opus {
            match audio_loop::start(128) {
                Ok(rx) => Some(rx),
                Err(e) => {
                    warn!("Audio capture unavailable (non-fatal): {e}");
                    None
                }
            }
        } else {
            None
        };

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
                    if let Some(rec) = recorder.as_mut() {
                        if let Err(e) = rec.record_frame(vf) {
                            warn!("Record frame failed (non-fatal): {}", e);
                        }
                    }
                    // AI agent without ScreenRead capability: discard frame instead of transmitting.
                    #[cfg(feature = "agent")]
                    if agent_handler.as_ref().is_some_and(|h| !h.allow_screen_send()) {
                        continue;
                    }
                    bytes_since_ping += vf.data.len() as u64;
                    metrics.on_video_frame(vf.data.len() as u64);
                    bp.on_send();
                }
                if relay.send_msg(&msg).await.is_err() { break; }
            }
            // Outgoing: encoded system-audio frames (when audio is active).
            // recv_audio stays pending forever once audio_rx is None, so this
            // arm never busy-spins when audio is disabled or the thread exits.
            audio = recv_audio(&audio_rx) => {
                match audio {
                    Some(msg) => {
                        bytes_since_ping += msg_audio_len(&msg);
                        if relay.send_msg(&msg).await.is_err() { break; }
                    }
                    // Capture thread exited — stop polling to avoid a busy loop.
                    None => audio_rx = None,
                }
            }
            // Incoming: input/control
            msg = relay.recv_msg() => {
                match msg? {
                    Some(Msg::InputEvent(evt)) if matches!(permission, Permission::Control | Permission::Full) => {
                        // If this is an AI agent session, every event must pass
                        // through the capability gate (authorize + audit log).
                        #[cfg(feature = "agent")]
                        let allowed = match &agent_handler {
                            Some(gate) => gate.gate_input(&evt).is_ok(),
                            None => true, // human viewers: unconditional
                        };
                        // Default build serves human viewers only; the AiAgent
                        // role was already refused above, so there is nothing
                        // to gate and no branch on the input hot path.
                        #[cfg(not(feature = "agent"))]
                        let allowed = true;
                        if allowed {
                            let _ = input_handler.handle_input(&evt);
                        }
                    }
                    Some(Msg::ClipboardSync(s)) if matches!(permission, Permission::Full) => {
                        // Cap inbound clipboard to prevent the host from allocating an
                        // arbitrarily large OS clipboard buffer. WebSocket frames can be up
                        // to 64 MiB; a cap here is the last defence before the OS API.
                        const MAX_VIEWER_CLIP_BYTES: usize = 16 * 1024 * 1024; // 16 MiB
                        if s.data.len() <= MAX_VIEWER_CLIP_BYTES {
                            let _ = input_handler.handle_clipboard(&s);
                        } else {
                            warn!("ClipboardSync from viewer too large ({} bytes) — dropped", s.data.len());
                        }
                    }
                    Some(Msg::SelectDisplay(sel)) if matches!(permission, Permission::Control | Permission::Full) => {
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
                            let text = clip_truncate(text);
                            let _ = relay.send_msg(&Msg::ClipboardSync(ClipboardSync {
                                format: miru_common::message::ClipboardFormat::Text,
                                data: text.into_bytes(),
                            })).await;
                        }
                    }
                    Some(Msg::OpenUrl(req)) if matches!(permission, Permission::Full) => {
                        // Only Full-permission sessions may open URLs (same as ShellExec tier).
                        let url = req.url.trim().to_string();
                        // Cap before logging and passing to the OS shell launcher.
                        const MAX_URL_BYTES: usize = 2048;
                        if url.len() > MAX_URL_BYTES {
                            warn!("OpenUrl rejected: URL too long ({} bytes)", url.len());
                        } else if url.starts_with("https://") || url.starts_with("http://") {
                            if let Err(e) = open::that(&url) {
                                warn!("OpenUrl failed for {url}: {e}");
                            } else {
                                info!("Opened URL: {url}");
                            }
                        } else {
                            warn!("OpenUrl rejected non-http URL: {url}");
                        }
                    }
                    Some(Msg::QosHint(hint)) if matches!(permission, Permission::Control | Permission::Full) => {
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
                        bp.on_pong();
                        // Feed RTT into BBR (µs precision).
                        qos.lock().on_rtt(rtt.saturating_mul(1_000));
                    }
                    Some(Msg::Close(reason)) => {
                        let reason_trunc: String = reason.reason.chars().take(200).collect();
                        info!("Viewer closed: {} {}", reason.code, reason_trunc);
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
                if let Ok(raw) = miru_input::get_clipboard() {
                    let text = clip_truncate(raw);
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

    // Un-stick any key the viewer pressed but never released. Every `break`
    // out of the loop above lands here, so a crashed/disconnected viewer can't
    // leave a modifier latched on the host.
    input_handler.release_all_keys();

    // Clean up any file transfers that never completed (peer disconnected mid-transfer).
    for (id, rx) in file_transfers.drain() {
        warn!(
            "FileTransfer {}: session ended without completion — removing temp file",
            id
        );
        let _ = std::fs::remove_file(&rx.temp_path);
    }

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
    #[cfg(feature = "agent")]
    if let Ok(rekor_url) = std::env::var("MIRU_REKOR_URL") {
        let metadata = metrics.snapshot();
        // Host-only attestation: the viewer's signing key is never available
        // server-side. Co-signing requires a 2-round protocol (planned v1.0).
        match miru_transparency::HostOnlyAttestation::new(&metadata, &config.identity.signing_key) {
            Err(e) => warn!("Attestation creation failed (non-fatal): {e}"),
            Ok(attestation) => {
                match miru_transparency::rekor::submit_host_to_rekor(
                    &rekor_url,
                    &metadata,
                    &attestation,
                )
                .await
                {
                    Ok(entry) => info!(
                        "Session anchored in Rekor: index={} uuid={}",
                        entry.log_index, entry.uuid
                    ),
                    Err(e) => warn!("Rekor anchoring failed (non-fatal): {}", e),
                }
            }
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
        TrustDecision::Trusted(p) => {
            acl.touch(device_str, now_unix());
            let _ = acl.save(&config.config_dir.join("acl.json"));
            Ok(p)
        }
        TrustDecision::Unknown => {
            // v0.1 default: auto-accept on first connection (TOFU — Trust On First Use).
            // The fingerprint is printed prominently so the user can verify out-of-band.
            // v0.3 will add a native UI confirmation dialog via Tauri IPC.
            //
            // Security note: TOFU is standard practice for SSH, WireGuard, and Signal.
            // The TOFU fingerprint log (miru-auth::fingerprint_log) records this pin
            // with a hash-chain so any subsequent change is detectable.
            //
            // Operators who want a hard stop instead of auto-accept (e.g. a host
            // exposed to an untrusted network, or one where every legitimate
            // device should be pre-approved out of band) can opt into strict mode
            // with MIRU_REQUIRE_PAIRING_CONFIRM=1. This does not change the
            // default — existing deployments relying on the current
            // first-connection-just-works behavior are unaffected.
            let fp = pubkey_fingerprint(pubkey);
            if config.require_pairing_confirm {
                error!("┌─ FIRST CONNECTION from new device REJECTED ─────────────────────────┐");
                error!("│  Fingerprint: {}                      │", fp);
                error!("│  MIRU_REQUIRE_PAIRING_CONFIRM is set — refusing unapproved devices. │");
                error!("│  Pre-approve this device in acl.json, or unset the env var to       │");
                error!("│  restore auto-accept (TOFU) for first connections.                  │");
                error!("└─────────────────────────────────────────────────────────────────────┘");
                return Err(anyhow::anyhow!(
                    "unknown device {device_str} rejected: MIRU_REQUIRE_PAIRING_CONFIRM is set"
                ));
            }
            warn!("┌─ FIRST CONNECTION from new device ─────────────────────────────────┐");
            warn!("│  Fingerprint: {}                      │", fp);
            warn!("│  Auto-accepting (TOFU). Verify this fingerprint out-of-band.        │");
            warn!("│  Set MIRU_REQUIRE_PAIRING_CONFIRM=1 to require pre-approval instead. │");
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
            error!(
                "⚠️  Pubkey mismatch for {} — rejecting (possible MITM or impersonation)",
                device_str
            );
            Err(anyhow::anyhow!(
                "pubkey mismatch for {device_str} — possible MITM or device re-keyed"
            ))
        }
    }
}

/// Truncate clipboard text to 1 MB at a valid UTF-8 char boundary.
///
/// The system clipboard can hold arbitrarily large content (e.g. "Copy as
/// text" on a large file). Forwarding it without a cap could OOM the viewer's
/// JS frontend. We truncate before sending; the comparison in the poll loop
/// uses the truncated form so we don't resend on every tick.
fn clip_truncate(text: String) -> String {
    const MAX_BYTES: usize = 1024 * 1024; // 1 MiB
    if text.len() <= MAX_BYTES {
        return text;
    }
    let mut end = MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    tracing::warn!(
        "clipboard content truncated to {} bytes (was {})",
        end,
        text.len()
    );
    text[..end].to_string()
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

/// Await the next audio frame from an optional capture receiver.
///   `Some(msg)` — an `AudioFrame` to forward to the viewer.
///   `None`      — the capture channel closed (thread exited); the caller
///                 should stop polling.
/// When `rx` is `None` this future stays pending forever, so the `select!` arm
/// is effectively disabled and never busy-spins.
async fn recv_audio(rx: &Option<flume::Receiver<Msg>>) -> Option<Msg> {
    match rx {
        Some(r) => r.recv_async().await.ok(),
        None => std::future::pending::<Option<Msg>>().await,
    }
}

/// Payload length of an `AudioFrame` message, for delivery-rate accounting.
/// Zero for any non-audio message (never expected on the audio channel).
fn msg_audio_len(msg: &Msg) -> u64 {
    if let Msg::AudioFrame(af) = msg {
        af.data.len() as u64
    } else {
        0
    }
}

fn pubkey_fingerprint(pk: &[u8; 32]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, pk);
    d.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Stream a SHA-256 digest over a file without loading the entire content into memory.
fn hash_file_streaming(path: &std::path::Path) -> std::io::Result<String> {
    let f = std::fs::File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    let digest = ctx.finish();
    Ok(digest.as_ref().iter().map(|b| format!("{b:02x}")).collect())
}
