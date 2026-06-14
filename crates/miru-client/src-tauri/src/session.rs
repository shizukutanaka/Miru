//! Viewer session — full lifecycle with real handshake.

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use miru_audio::{playback::AudioPlayer, AudioDecoder, Layout};
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
use tokio::{sync::{mpsc, oneshot}, time};
use tracing::{info, warn};

use crate::state::{RecordingState, SessionStats};

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
    /// STUN-discovered public address of the host, if known ("直接" path possible).
    pub host_pub_addr: Option<String>,
}

pub async fn run(
    args: crate::commands::ConnectArgs,
    identity: Arc<DeviceIdentity>,
    mut cmd_rx: mpsc::Receiver<Msg>,
    mut cancel_rx: oneshot::Receiver<()>,
    app: AppHandle,
    stats: Arc<Mutex<SessionStats>>,
    recording: Arc<Mutex<Option<RecordingState>>>,
) -> Result<()> {
    emit_status(&app, "connecting", None, None);

    // 1. Connect to signal server with our viewer device ID
    let viewer_did = DeviceId::new();
    let mut signal = SignalClient::connect_signed(
        &args.signal_url,
        &viewer_did,
        Some(identity.verifying_key.as_bytes()),
        Some(&identity.signing_key),
    )
    .await?;

    while let Some(evt) = signal.next_event().await {
        if matches!(evt, SignalEvent::Registered { .. }) {
            break;
        }
    }

    // 2. Request connection
    signal.request_connect(&args.device_id, "0.0.0.0").await?;

    // 3. Wait for relay offer; also capture host public addr from ConnectAck.
    let mut host_pub_addr: Option<String> = None;
    let (relay_addr, relay_port, token) = loop {
        match signal.next_event().await {
            Some(SignalEvent::ConnectAck { host_addr, host_port, relay, .. }) => {
                // Compose addr:port for a directly usable socket address string.
                host_pub_addr = match (host_addr, host_port) {
                    (Some(a), Some(p)) => Some(format!("{}:{}", a, p)),
                    (Some(a), None) => Some(a),
                    _ => None,
                };
                if let Some(ref a) = host_pub_addr {
                    info!(
                        "Host public addr: {} ({})",
                        a,
                        if relay { "relay required" } else { "direct path may be possible — not yet wired" }
                    );
                }
            }
            Some(SignalEvent::IncomingConnection {
                token,
                relay_addr,
                relay_port,
            }) => {
                break (relay_addr, relay_port, token);
            }
            Some(SignalEvent::Error { code, message }) => {
                emit_status(&app, "error", Some(format!("{code}: {message}")), None);
                return Err(anyhow::anyhow!("signal: {message}"));
            }
            None => return Err(anyhow::anyhow!("signal disconnected")),
            _ => continue,
        }
    };

    // 4. Connect to relay
    let mut relay =
        RelayTransport::connect(&format!("ws://{relay_addr}:{relay_port}"), &token, "viewer")
            .await?;

    // 5. Handshake
    // Advertise only codecs the Rust-side decoder can actually handle.
    // HW codecs (AV1/H264/H265) are listed only when the `vpx` feature is
    // enabled; the WebCodecs fallback (v0.3) will broaden this.
    let viewer_features = Features {
        codecs: miru_codec::available_codecs(),
        audio_codecs: vec![AudioCodec::Opus, AudioCodec::Pcm],
        hw_decode: false,
        hw_encode: false,
        clipboard: true,
        file_transfer: true,
        audio: true,
        multi_monitor: true,
        ..Default::default()
    };

    let result = time::timeout(
        std::time::Duration::from_secs(15),
        viewer_handshake(&mut relay, &identity.signing_key, viewer_features),
    )
    .await
    .map_err(|_| anyhow::anyhow!("handshake timed out after 15 s"))??;
    let host_fpr = pubkey_fingerprint(&result.peer_identity_pubkey);
    info!(
        "Handshake complete: codec={:?} host_fpr={}",
        result.selected_video_codec, host_fpr
    );

    // Install ciphers (separate keys per direction, no nonce-reuse risk)
    relay.install_ciphers(result.tx, result.rx).await;

    emit_status_full(&app, "connected", None, Some(host_fpr), host_pub_addr);

    // 6. Video decoder + audio pipeline
    let mut decoder: Option<Decoder> = None;
    // Audio: cpal::Stream is !Send, so the decoder+player run on a dedicated
    // std::thread. The session loop sends AudioFrame payloads via a channel.
    let audio_enabled = result.selected_audio_codec == AudioCodec::Opus;
    let audio_tx: Option<std::sync::mpsc::SyncSender<miru_common::message::AudioFrame>> =
        if audio_enabled {
            let (tx, rx) = std::sync::mpsc::sync_channel::<miru_common::message::AudioFrame>(16);
            std::thread::spawn(move || {
                let mut audio_decoder: Option<AudioDecoder> = None;
                let mut audio_player: Option<AudioPlayer> = None;
                for af in rx {
                    let channels = af.channels;
                    if audio_decoder.is_none() {
                        if let Ok(layout) = Layout::from_channels(channels) {
                            match (AudioDecoder::new(layout), AudioPlayer::new(channels, 48_000)) {
                                (Ok(d), Ok(p)) => {
                                    audio_decoder = Some(d);
                                    audio_player = Some(p);
                                }
                                (Err(e), _) | (_, Err(e)) => {
                                    warn!("Audio init failed (non-fatal): {e}");
                                }
                            }
                        }
                    }
                    if let (Some(dec), Some(player)) = (audio_decoder.as_mut(), audio_player.as_ref()) {
                        match dec.decode(&af) {
                            Ok(samples) => player.push(samples),
                            Err(e) => warn!("audio decode: {e}"),
                        }
                    }
                }
            });
            Some(tx)
        } else {
            None
        };
    let mut frame_count = 0u64;
    let mut frames_since_update = 0u64;
    let mut last_stats_update = std::time::Instant::now();
    let mut bytes_since_update: u64 = 0;
    let mut last_seq: Option<u64> = None;
    let mut seq_gaps: u64 = 0;
    let mut seq_total: u64 = 0;

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
                        // Track sequence for packet loss estimation
                        bytes_since_update += vf.data.len() as u64;
                        if let Some(prev) = last_seq {
                            let gap = vf.seq.saturating_sub(prev + 1);
                            seq_gaps += gap;
                            seq_total += gap + 1;
                        }
                        last_seq = Some(vf.seq);

                        // JPEG fast path: skip decode+re-encode entirely.
                        // Passing host JPEG bytes straight to the frontend preserves
                        // original quality and avoids double-compression loss.
                        if vf.codec == VideoCodec::Jpeg {
                            frame_count += 1;
                            frames_since_update += 1;
                            let elapsed = last_stats_update.elapsed();
                            if elapsed.as_secs() >= 1 {
                                let elapsed_secs = elapsed.as_secs_f32();
                                let mut s = stats.lock();
                                s.frames_decoded = frame_count;
                                s.bytes_recv += bytes_since_update;
                                s.fps = frames_since_update as f32 / elapsed_secs;
                                s.bitrate_kbps = ((bytes_since_update * 8) as f32
                                    / elapsed_secs / 1000.0) as u32;
                                s.packet_loss_pct = if seq_total > 0 {
                                    (seq_gaps as f32 / seq_total as f32) * 100.0
                                } else {
                                    0.0
                                };
                                bytes_since_update = 0;
                                frames_since_update = 0;
                                seq_gaps = 0;
                                seq_total = 0;
                                last_stats_update = std::time::Instant::now();
                            }
                            // Dimensions from JPEG header (parse width/height from SOF marker)
                            let (w, h) = jpeg_dimensions(&vf.data).unwrap_or((0, 0));
                            write_recording_frame(&recording, &vf.data);
                            let _ = app.emit("video-frame", VideoFrameEvent {
                                width: w,
                                height: h,
                                keyframe: vf.keyframe,
                                jpeg_b64: B64.encode(&vf.data),
                            });
                            continue;
                        }

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
                        let Some(dec) = decoder.as_mut() else { continue };

                        match dec.decode(&vf.data, vf.timestamp_ms) {
                            Ok(Some(frame)) => {
                                frame_count += 1;
                                frames_since_update += 1;
                                // Update stats every second
                                let elapsed = last_stats_update.elapsed();
                                if elapsed.as_secs() >= 1 {
                                    let elapsed_secs = elapsed.as_secs_f32();
                                    let mut s = stats.lock();
                                    s.frames_decoded = frame_count;
                                    s.bytes_recv += bytes_since_update;
                                    s.fps = frames_since_update as f32 / elapsed_secs;
                                    s.bitrate_kbps = ((bytes_since_update * 8) as f32
                                        / elapsed_secs / 1000.0) as u32;
                                    s.packet_loss_pct = if seq_total > 0 {
                                        (seq_gaps as f32 / seq_total as f32) * 100.0
                                    } else {
                                        0.0
                                    };
                                    bytes_since_update = 0;
                                    frames_since_update = 0;
                                    seq_gaps = 0;
                                    seq_total = 0;
                                    last_stats_update = std::time::Instant::now();
                                }

                                // Re-encode to JPEG at higher quality (85) for VP9/VP8 frames.
                                if let Ok(jpeg) = i420_to_jpeg(&frame, 85) {
                                    write_recording_frame(&recording, &jpeg);
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
                    Ok(Some(Msg::Ping(p))) => {
                        // Echo Pong back so the host can measure RTT.
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default().as_millis() as u64;
                        let rtt = now.saturating_sub(p.ts) as u32;
                        stats.lock().rtt_ms = rtt;
                        let _ = relay.send_msg(&Msg::Pong(miru_common::message::Pong {
                            ts: p.ts,
                            server_ts: now,
                        })).await;
                    }
                    Ok(Some(Msg::AudioFrame(af))) if audio_enabled => {
                        if let Some(tx) = &audio_tx {
                            // Non-blocking: drop frame on full queue (avoid latency drift)
                            let _ = tx.try_send(af);
                        }
                    }
                    Ok(Some(Msg::DisplayList(mut dl))) => {
                        const MAX_DISPLAYS: usize = 16;
                        if dl.displays.len() > MAX_DISPLAYS {
                            warn!("DisplayList: {} displays from host, capping at {MAX_DISPLAYS}", dl.displays.len());
                            dl.displays.truncate(MAX_DISPLAYS);
                        }
                        for d in &mut dl.displays {
                            // Limit display name to 64 Unicode chars to bound Tauri event payload.
                            if d.name.chars().count() > 64 {
                                d.name = d.name.chars().take(64).collect();
                            }
                        }
                        let _ = app.emit("display-list", &dl);
                    }
                    Ok(Some(Msg::QosUpdate(u))) => {
                        // Log host-side QoS adjustments; UI can display these.
                        info!(
                            "QoS update from host: fps={} bitrate={}kbps quality={}",
                            u.fps, u.bitrate_kbps, u.quality
                        );
                        let _ = app.emit("qos-update", serde_json::json!({
                            "fps": u.fps,
                            "bitrate_kbps": u.bitrate_kbps,
                            "quality": u.quality,
                        }));
                    }
                    Ok(Some(Msg::ClipboardSync(cs))) => {
                        if cs.format == miru_common::message::ClipboardFormat::Text {
                            const MAX_CLIP_BYTES: usize = 1024 * 1024; // 1 MiB
                            if cs.data.len() <= MAX_CLIP_BYTES {
                                if let Ok(text) = String::from_utf8(cs.data) {
                                    let _ = app.emit("clipboard-sync", text);
                                }
                            } else {
                                tracing::warn!(
                                    "clipboard-sync too large ({} bytes), ignored",
                                    cs.data.len()
                                );
                            }
                        }
                    }
                    Ok(Some(Msg::Close(reason))) => {
                        let reason_trunc: String = reason.reason.chars().take(200).collect();
                        info!("Host closed: {} {}", reason.code, reason_trunc);
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

/// Append a JPEG frame to the active recording's binary container.
///
/// Container format:
///   frames.bin  — raw JPEG frames concatenated back-to-back
///   offsets.bin — [(offset: u64 LE, size: u32 LE); frame_count] (12 bytes/frame)
///
/// Both files are kept open for the session; O(1) random-access is enabled
/// by offsets.bin. Eliminates the per-frame inode overhead of the old
/// individual-file approach (~144MB of filesystem overhead at 30fps for 10min).
fn write_recording_frame(recording: &Arc<Mutex<Option<RecordingState>>>, jpeg: &[u8]) {
    use std::io::Write;
    let mut guard = recording.lock();
    let Some(ref mut rec) = *guard else { return };

    // Compute byte offset where this frame starts.
    let offset: u64 = rec
        .frames_file
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0);
    let size = jpeg.len() as u32;

    // Write frame data first — if we crash here no index entry points to the
    // partial write so the index stays consistent (the frame is simply absent).
    // Writing the index first would leave a dangling entry pointing to data
    // that may never be fully written, corrupting the index on crash.
    if rec.frames_file.write_all(jpeg).is_err() {
        return;
    }

    let mut entry = [0u8; 12];
    entry[..8].copy_from_slice(&offset.to_le_bytes());
    entry[8..12].copy_from_slice(&size.to_le_bytes());
    if rec.offsets_file.write_all(&entry).is_err() {
        return;
    }

    rec.frame_count += 1;
}

/// Extract (width, height) from a JPEG SOF0/SOF2 marker without fully decoding.
fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            break;
        }
        let marker = data[i + 1];
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        // SOF0 (0xC0), SOF1 (0xC1), SOF2 (0xC2) contain image dimensions
        if matches!(marker, 0xC0 | 0xC1 | 0xC2) && i + 9 <= data.len() {
            let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

fn pubkey_fingerprint(pk: &[u8; 32]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, pk);
    d.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn emit_status(app: &AppHandle, kind: &str, message: Option<String>, fingerprint: Option<String>) {
    emit_status_full(app, kind, message, fingerprint, None);
}

fn emit_status_full(
    app: &AppHandle,
    kind: &str,
    message: Option<String>,
    fingerprint: Option<String>,
    host_pub_addr: Option<String>,
) {
    let _ = app.emit(
        "session-event",
        SessionEvent {
            kind: kind.to_string(),
            message,
            fingerprint,
            host_pub_addr,
        },
    );
}
