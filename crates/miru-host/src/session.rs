//! Host session: signal → relay → handshake → capture → stream loop.

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use miru_auth::{AclStore, DeviceIdentity, Permission, TrustDecision, TrustedPeer};
use miru_capture::create_capturer;
use miru_common::{
    message::{AudioCodec, Features, Msg, Role},
    session::DeviceId,
};
use miru_transport::{
    handshake::host_handshake,
    relay::RelayTransport,
    signaling::{SignalClient, SignalEvent},
};
use parking_lot::Mutex;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::time;
use tracing::{error, info, warn};

use crate::{
    agent_handler::AgentHandler, capture_loop, input_handler::InputHandler, qos::QosController,
};

#[derive(Clone)]
pub struct HostConfig {
    pub identity: Arc<DeviceIdentity>,
    pub acl: Arc<Mutex<AclStore>>,
    pub config_dir: PathBuf,
}

pub async fn run(device_id: DeviceId, signal_url: String, config: HostConfig) -> Result<()> {
    let mut signal = SignalClient::connect(
        &signal_url,
        &device_id,
        Some(config.identity.verifying_key.as_bytes()),
    )
    .await?;
    info!("Signal: registered as {}", device_id);
    info!(
        "Identity fingerprint: {}",
        config.identity.pubkey_fingerprint()
    );

    loop {
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
                warn!("Signal disconnected — retry in 5s");
                time::sleep(Duration::from_secs(5)).await;
                break;
            }
            Some(SignalEvent::Error { code, message }) => {
                error!("Signal error {}: {}", code, message);
            }
            None => break,
            _ => {}
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

    let result = host_handshake(&mut relay, &config.identity.signing_key, host_features).await?;
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

    // 6. Start capture loop
    let qos = Arc::new(Mutex::new(QosController::new(60, 5000)));
    let (fps, br) = {
        let q = qos.lock();
        (q.fps(), q.bitrate_kbps())
    };
    let cap_rx = capture_loop::start(
        capture_loop::CaptureConfig {
            codec: result.selected_video_codec.clone(),
            display_idx: 0,
            target_fps: fps,
            bitrate_kbps: br,
        },
        capturer,
    )?;

    // 7. Main loop
    let mut input_handler = InputHandler::new();
    let mut ping_ticker = time::interval(Duration::from_secs(1));
    let session_started = now_unix();
    let mut video_frames: u64 = 0;
    let mut total_bytes: u64 = 0;

    loop {
        tokio::select! {
            // Outgoing: video frames from capture loop
            Ok(msg) = cap_rx.recv_async() => {
                if let Msg::VideoFrame(ref vf) = msg {
                    video_frames += 1;
                    total_bytes += vf.data.len() as u64;
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
                    Some(Msg::Pong(p)) => {
                        let rtt = now_ms().saturating_sub(p.ts) as u32;
                        // Scope the lock so the MutexGuard is dropped before .await.
                        // parking_lot::MutexGuard is !Send — holding it across await
                        // would prevent tokio::spawn from accepting this future.
                        let update = qos.lock().update(rtt, 0.0);
                        if let Some(u) = update {
                            let _ = relay.send_msg(&Msg::QosUpdate(u)).await;
                        }
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
                let _ = relay.send_msg(&Msg::Ping(miru_common::message::Ping { ts: now_ms() })).await;
            }
        }
    }

    info!("Session {} ended", result.session_id);

    // 8. Transparency anchoring — record an immutable, signed commitment of
    //    this session's metadata. Posted to Rekor if MIRU_REKOR_URL is set.
    //    Non-fatal: anchoring failure must never break session teardown.
    if let Ok(rekor_url) = std::env::var("MIRU_REKOR_URL") {
        let metadata = miru_transparency::SessionMetadata {
            session_id: result.session_id,
            host_pubkey_b64: B64.encode(config.identity.verifying_key.as_bytes()),
            viewer_pubkey_b64: device_str.clone(),
            codec: format!("{:?}", result.selected_video_codec).to_lowercase(),
            started_at: session_started,
            ended_at: now_unix(),
            video_frames,
            total_bytes,
            relayed: true, // v0.1 always uses relay
        };
        // Host-only commitment (single-signer for v0.1; co-signing in v1.0).
        let commitment = miru_transparency::CoSignedCommitment::new(
            &metadata,
            &config.identity.signing_key,
            &config.identity.signing_key,
        );
        match miru_transparency::rekor::submit_to_rekor(&rekor_url, &metadata, &commitment).await {
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
    let mut acl = config.acl.lock();
    match acl.check(device_str, device_str) {
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
                pubkey_b64: device_str.to_string(),
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
            error!("⚠️  Pubkey mismatch for {} — rejecting", device_str);
            Err(anyhow::anyhow!(
                "pubkey mismatch — possible MITM or device re-keyed"
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
