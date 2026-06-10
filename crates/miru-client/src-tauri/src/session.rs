//! Viewer session — full lifecycle with real handshake.

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use miru_auth::DeviceIdentity;
use miru_codec::{i420_to_jpeg, Decoder};
use miru_common::{
    message::{AudioCodec, Features, Msg, VideoCodec},
    session::DeviceId,
};
use miru_transport::{
    handshake::viewer_handshake,
    relay::RelayTransport,
    signaling::{SignalClient, SignalEvent},
};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use crate::state::SessionStats;

#[derive(Serialize, Clone)]
pub struct VideoFrameEvent {
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
    pub jpeg_b64: String,
}

#[derive(Serialize, Clone)]
pub struct SessionEvent {
    pub kind: String,
    pub message: Option<String>,
    pub fingerprint: Option<String>,
}

pub async fn run(
    args: crate::commands::ConnectArgs,
    identity: Arc<DeviceIdentity>,
    mut cmd_rx: mpsc::Receiver<Msg>,
    mut cancel_rx: oneshot::Receiver<()>,
    app: AppHandle,
    stats: Arc<Mutex<SessionStats>>,
) -> Result<()> {
    emit_status(&app, "connecting", None, None);

    // 1. Connect to signal server with our viewer device ID
    let viewer_did = DeviceId::new();
    let mut signal = SignalClient::connect(
        &args.signal_url,
        &viewer_did,
        Some(identity.verifying_key.as_bytes()),
    ).await?;

    while let Some(evt) = signal.next_event().await {
        if matches!(evt, SignalEvent::Registered { .. }) { break; }
    }

    // 2. Request connection
    signal.request_connect(&args.device_id, "0.0.0.0").await?;

    // 3. Wait for relay offer
    let (relay_addr, relay_port, token) = loop {
        match signal.next_event().await {
            Some(SignalEvent::IncomingConnection { token, relay_addr, relay_port }) => {
                break (relay_addr, relay_port, token);
            }
            Some(SignalEvent::Error { code, message }) => {
                emit_status(&app, "error", Some(format!("{}: {}", code, message)), None);
                return Err(anyhow::anyhow!("signal: {}", message));
            }
            None => return Err(anyhow::anyhow!("signal disconnected")),
            _ => continue,
        }
    };

    // 4. Connect to relay
    let mut relay = RelayTransport::connect(
        &format!("ws://{}:{}", relay_addr, relay_port),
        &token,
        "viewer",
    ).await?;

    // 5. Handshake
    let viewer_features = Features {
        codecs: vec![VideoCodec::Av1, VideoCodec::H265, VideoCodec::H264, VideoCodec::Vp9, VideoCodec::Vp8],
        audio_codecs: vec![AudioCodec::Opus],
        hw_decode: true,
        clipboard: true,
        file_transfer: true,
        audio: true,
        ..Default::default()
    };

    let result = viewer_handshake(&mut relay, &identity.signing_key, viewer_features).await?;
    let host_fpr = pubkey_fingerprint(&result.peer_identity_pubkey);
    info!("Handshake complete: codec={:?} host_fpr={}",
        result.selected_video_codec, host_fpr);

    // Install ciphers (separate keys per direction, no nonce-reuse risk)
    relay.install_ciphers(result.tx, result.rx).await;

    emit_status(&app, "connected", None, Some(host_fpr));

    // 6. Decoder
    let mut decoder: Option<Decoder> = None;
    let mut frame_count = 0u64;
    let mut last_stats_update = std::time::Instant::now();

    // 7. Main loop
    loop {
        tokio::select! {
            biased;
            _ = &mut cancel_rx => {
                info!("Session cancelled");
                let _ = relay.send_msg(&Msg::Close(miru_common::message::CloseReason {
                    code: 1000, reason: "user".into(),
                })).await;
                break;
            }

            msg = cmd_rx.recv() => {
                match msg {
                    Some(m) => { if relay.send_msg(&m).await.is_err() { break; } }
                    None => break,
                }
            }

            msg = relay.recv_msg() => {
                match msg {
                    Ok(Some(Msg::VideoFrame(vf))) => {
                        // Lazy decoder init: get_or_insert_with can't propagate
                        // errors, so we use an explicit check.
                        if decoder.is_none() {
                            info!("Decoder init: {:?}", vf.codec);
                            match Decoder::new(vf.codec.clone()) {
                                Ok(d) => { decoder = Some(d); }
                                Err(e) => {
                                    tracing::warn!("Decoder init failed for {:?}: {}; skipping frame", vf.codec, e);
                                    continue;
                                }
                            }
                        }
                        let dec = decoder.as_mut().expect("just inserted above");

                        match dec.decode(&vf.data, vf.timestamp_ms) {
                            Ok(Some(frame)) => {
                                frame_count += 1;
                                // Update stats
                                if last_stats_update.elapsed().as_secs() >= 1 {
                                    let mut s = stats.lock();
                                    s.frames_decoded = frame_count;
                                    s.bytes_recv += vf.data.len() as u64;
                                    s.fps = frame_count as f32
                                        / last_stats_update.elapsed().as_secs_f32();
                                    last_stats_update = std::time::Instant::now();
                                }

                                // Encode to JPEG and emit to UI
                                if let Ok(jpeg) = i420_to_jpeg(&frame, 70) {
                                    let _ = app.emit("video-frame", VideoFrameEvent {
                                        width: frame.width,
                                        height: frame.height,
                                        keyframe: vf.keyframe,
                                        jpeg_b64: B64.encode(&jpeg),
                                    });
                                }
                            }
                            Ok(None) => {}
                            Err(e) => warn!("decode: {}", e),
                        }
                    }
                    Ok(Some(Msg::Pong(p))) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default().as_millis() as u64;
                        let rtt = now.saturating_sub(p.ts) as u32;
                        stats.lock().rtt_ms = rtt;
                    }
                    Ok(Some(Msg::Close(reason))) => {
                        info!("Host closed: {} {}", reason.code, reason.reason);
                        break;
                    }
                    Ok(None) => break,
                    Err(e) => { warn!("recv: {}", e); break; }
                    _ => {}
                }
            }
        }
    }

    emit_status(&app, "disconnected", None, None);
    Ok(())
}

fn pubkey_fingerprint(pk: &[u8; 32]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, pk);
    d.as_ref()[..8].iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(":")
}

fn emit_status(app: &AppHandle, kind: &str, message: Option<String>, fingerprint: Option<String>) {
    let _ = app.emit("session-event", SessionEvent {
        kind: kind.to_string(), message, fingerprint,
    });
}
