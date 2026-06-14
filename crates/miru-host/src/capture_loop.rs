//! Capture loop — runs on a dedicated OS thread.
//!
//! Pipeline (per display):
//!   ScreenCapturer.next_frame()
//!   → ColorConvert (BGRA→I420 if needed)
//!   → Encoder (AV1/H265/H264/VP9)
//!   → flume channel → transport task → network

use anyhow::Result;
use flume::Receiver;
use miru_capture::{
    frame::{PixelFormat, RawFrame},
    ScreenCapturer,
};
use miru_codec::encoder::Encoder;
use miru_common::message::{Msg, VideoCodec, VideoFrame};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::backpressure::FrameController;

/// Configuration for the capture loop.
pub struct CaptureConfig {
    pub codec: VideoCodec,
    pub display_idx: u8,
    pub target_fps: u8,
    pub bitrate_kbps: u32,
    /// Shared backpressure controller — skip frames when the viewer falls behind.
    pub frame_controller: Arc<FrameController>,
}

/// Start the capture loop in a dedicated thread.
/// Returns a channel receiver that produces VideoFrame messages.
pub fn start(
    config: CaptureConfig,
    mut capturer: impl ScreenCapturer + 'static,
) -> Result<Receiver<Msg>> {
    let (tx, rx) = flume::bounded::<Msg>(8); // backpressure: max 8 frames queued
    let codec = config.codec.clone();
    let fps = config.target_fps;
    let bitrate = config.bitrate_kbps;
    let display_idx = config.display_idx;
    let fc = config.frame_controller;

    std::thread::Builder::new()
        .name("miru-capture".to_string())
        .spawn(move || {
            use std::sync::atomic::Ordering;

            // Select display
            if let Err(e) = capturer.select_display(display_idx) {
                warn!("select display {}: {}", display_idx, e);
            }

            let mut encoder: Option<Encoder> = None;
            let mut seq: u64 = 0;
            let mut last_frame = Instant::now();
            let mut force_keyframe = true;
            // Track last-seen QoS values so we only call update_bitrate on change.
            let mut cur_fps = fps;
            let mut cur_bitrate = bitrate;

            info!(
                "Capture loop started: display={} fps={} bitrate={}kbps codec={:?}",
                display_idx, fps, bitrate, codec
            );

            loop {
                // Re-read QoS targets every frame — written by session loop via apply_qos().
                // This is how BBR tick results actually reach the encoder.
                let new_fps = fc.target_fps.load(Ordering::Relaxed);
                let new_bitrate = fc.target_bitrate_kbps.load(Ordering::Relaxed);
                if new_fps > 0 && new_fps != cur_fps {
                    cur_fps = new_fps;
                }
                if new_bitrate > 0 && new_bitrate != cur_bitrate {
                    cur_bitrate = new_bitrate;
                    // Update the live encoder without re-initialising it
                    // (avoids a keyframe disruption just to change bitrate).
                    if let Some(ref mut enc) = encoder {
                        enc.update_bitrate(cur_bitrate);
                    }
                }

                // Rate limiting — uses dynamic cur_fps set by BBR.
                let frame_interval = Duration::from_secs_f64(1.0 / cur_fps.max(1) as f64);
                let elapsed = last_frame.elapsed();
                if elapsed < frame_interval {
                    std::thread::sleep(frame_interval - elapsed);
                }

                // Skip-at-capture when the viewer hasn't kept up (cheapest skip point).
                if !fc.should_capture() {
                    continue;
                }

                let frame = match capturer.next_frame() {
                    Ok(Some(f)) => f,
                    Ok(None) => continue, // timeout / no change
                    Err(e) => {
                        warn!("capture error: {}", e);
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                };

                last_frame = Instant::now();

                // Lazy encoder init (need frame dimensions from first frame).
                if encoder.is_none() {
                    info!("Encoder init: {}×{} {:?}", frame.width, frame.height, codec);
                    match Encoder::new(codec.clone(), frame.width, frame.height, cur_fps, cur_bitrate) {
                        Ok(enc) => {
                            encoder = Some(enc);
                        }
                        Err(e) => {
                            error!("Encoder init failed: {}; dropping frame and retrying", e);
                            continue;
                        }
                    }
                }
                let enc = encoder.as_mut().expect("just inserted above");

                // Convert to I420 if needed
                let i420 = match frame.format {
                    PixelFormat::I420 => frame.data.to_vec(),
                    PixelFormat::Bgra32 => bgra_to_i420(&frame),
                    PixelFormat::Nv12 => nv12_to_i420(&frame),
                    PixelFormat::Rgba32 => rgba_to_i420(&frame),
                };

                // Encode
                match enc.encode(
                    &i420,
                    frame.width,
                    frame.height,
                    frame.timestamp_ms,
                    force_keyframe,
                ) {
                    Ok(Some(packet)) => {
                        force_keyframe = false;
                        seq += 1;

                        let msg = Msg::VideoFrame(VideoFrame {
                            seq,
                            display_idx,
                            keyframe: packet.keyframe,
                            codec: enc.codec().clone(),
                            data: packet.data,
                            width: frame.width,
                            height: frame.height,
                            timestamp_ms: packet.timestamp_ms,
                            // HDR / color metadata: SDR defaults for v0.1.
                            // v0.3 will populate from capture surface format.
                            color_primaries: Default::default(),
                            transfer: Default::default(),
                            color_range: Default::default(),
                            hdr_metadata: None,
                        });

                        match tx.try_send(msg) {
                            Ok(()) => {}
                            Err(flume::TrySendError::Full(_)) => {
                                debug!("frame dropped (viewer backpressure)");
                                force_keyframe = true;
                            }
                            Err(flume::TrySendError::Disconnected(_)) => {
                                info!("Capture channel closed — stopping capture loop");
                                return;
                            }
                        }
                    }
                    Ok(None) => {} // encoder buffering
                    Err(e) => warn!("encode error: {}", e),
                }
            }
        })?;

    Ok(rx)
}

// ─── Color conversion ─────────────────────────────────────────────────────────

/// BGRA32 → I420 (YUV 4:2:0 planar).
/// Two-pass: Y in one linear scan, UV with 2×2 block averaging (BT.601).
fn bgra_to_i420(frame: &RawFrame) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let src = &frame.data;
    let stride = frame.stride as usize;

    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);
    let mut out = vec![0u8; y_size + uv_size * 2];
    let (y_plane, uv_plane) = out.split_at_mut(y_size);
    let (u_plane, v_plane) = uv_plane.split_at_mut(uv_size);

    // Pass 1: Y — single linear scan, cache-friendly.
    for row in 0..h {
        for col in 0..w {
            let i = row * stride + col * 4;
            let b = src[i] as i32;
            let g = src[i + 1] as i32;
            let r = src[i + 2] as i32;
            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            y_plane[row * w + col] = y.clamp(16, 235) as u8;
        }
    }

    // Pass 2: UV — average all four pixels in each 2×2 block (correct chroma).
    // Sampling only (row*2, col*2) causes visible chroma aliasing on text/edges.
    for row in (0..h).step_by(2) {
        let r0 = row;
        let r1 = (row + 1).min(h - 1);
        for col in (0..w).step_by(2) {
            let c0 = col;
            let c1 = (col + 1).min(w - 1);
            let avg = |ch: usize| {
                (src[r0 * stride + c0 * 4 + ch] as i32
                    + src[r0 * stride + c1 * 4 + ch] as i32
                    + src[r1 * stride + c0 * 4 + ch] as i32
                    + src[r1 * stride + c1 * 4 + ch] as i32)
                    / 4
            };
            let b = avg(0);
            let g = avg(1);
            let r = avg(2);
            let uv_i = (row / 2) * (w / 2) + col / 2;
            let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
            let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
            u_plane[uv_i] = u.clamp(16, 240) as u8;
            v_plane[uv_i] = v.clamp(16, 240) as u8;
        }
    }
    out
}

fn nv12_to_i420(frame: &RawFrame) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.stride as usize;
    let src = &frame.data;
    let y_size = w * h;
    let uv_w = w / 2;
    let uv_h = h / 2;
    let uv_size = uv_w * uv_h;
    let mut out = vec![0u8; y_size + uv_size * 2];

    // Y plane: copy row by row to strip stride padding. Using a flat copy of
    // w*h bytes is wrong when stride > w (DXGI NV12 always pads to alignment).
    for row in 0..h {
        let src_off = row * stride;
        let dst_off = row * w;
        out[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
    }

    // NV12 UV plane starts at stride*h (not w*h).
    let nv12_uv = &src[stride * h..];
    let (u_out, v_out) = out[y_size..].split_at_mut(uv_size);
    for row in 0..uv_h {
        let src_row = &nv12_uv[row * stride..];
        for col in 0..uv_w {
            u_out[row * uv_w + col] = src_row[col * 2];
            v_out[row * uv_w + col] = src_row[col * 2 + 1];
        }
    }
    out
}

fn rgba_to_i420(frame: &RawFrame) -> Vec<u8> {
    // Same as BGRA but swap R and B channels
    let mut swapped = frame.data.to_vec();
    for i in (0..swapped.len()).step_by(4) {
        swapped.swap(i, i + 2); // R ↔ B
    }
    let frame2 = RawFrame {
        format: PixelFormat::Bgra32,
        data: bytes::Bytes::from(swapped),
        display_idx: frame.display_idx,
        width: frame.width,
        height: frame.height,
        stride: frame.stride,
        timestamp_ms: frame.timestamp_ms,
        dirty_rects: frame.dirty_rects.clone(),
    };
    bgra_to_i420(&frame2)
}
