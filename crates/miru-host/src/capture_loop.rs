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
use miru_codec::color::{bt601_uv, bt601_y};
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
                    match Encoder::new(
                        codec.clone(),
                        frame.width,
                        frame.height,
                        cur_fps,
                        cur_bitrate,
                    ) {
                        Ok(enc) => {
                            encoder = Some(enc);
                        }
                        Err(e) => {
                            error!("Encoder init failed: {}; dropping frame and retrying", e);
                            continue;
                        }
                    }
                }
                let Some(enc) = encoder.as_mut() else {
                    continue; // unreachable: init branch either set Some or continued
                };

                // Convert to I420 if needed.
                // I420 frames are already in the right format — borrow the
                // Bytes slice directly rather than copying into a new Vec.
                let i420_owned: Vec<u8>;
                let i420: &[u8] = match frame.format {
                    PixelFormat::I420 => &frame.data,
                    PixelFormat::Bgra32 => {
                        i420_owned = bgra_to_i420(&frame);
                        &i420_owned
                    }
                    PixelFormat::Nv12 => {
                        i420_owned = nv12_to_i420(&frame);
                        &i420_owned
                    }
                    PixelFormat::Rgba32 => {
                        i420_owned = rgba_to_i420(&frame);
                        &i420_owned
                    }
                };

                // Encode
                match enc.encode(
                    i420,
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

/// Generic packed 32-bit-per-pixel → I420 conversion, parameterized by the
/// byte offset of R/G/B within each pixel (alpha is always ignored). Shared
/// by BGRA32 (r_off=2,g_off=1,b_off=0) and RGBA32 (r_off=0,g_off=1,b_off=2) —
/// the two formats differ only in channel order, so the BT.601 fixed-point
/// math and 2×2 chroma averaging were previously duplicated line-for-line.
/// Two-pass: Y in one linear scan, UV with 2×2 block averaging.
fn packed32_to_i420(frame: &RawFrame, r_off: usize, g_off: usize, b_off: usize) -> Vec<u8> {
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
            let r = src[i + r_off] as i32;
            let g = src[i + g_off] as i32;
            let b = src[i + b_off] as i32;
            y_plane[row * w + col] = bt601_y(r, g, b);
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
            let avg = |off: usize| {
                (src[r0 * stride + c0 * 4 + off] as i32
                    + src[r0 * stride + c1 * 4 + off] as i32
                    + src[r1 * stride + c0 * 4 + off] as i32
                    + src[r1 * stride + c1 * 4 + off] as i32)
                    / 4
            };
            let r = avg(r_off);
            let g = avg(g_off);
            let b = avg(b_off);
            let uv_i = (row / 2) * (w / 2) + col / 2;
            let (u, v) = bt601_uv(r, g, b);
            u_plane[uv_i] = u;
            v_plane[uv_i] = v;
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

/// BGRA32 → I420 (YUV 4:2:0 planar). See [`packed32_to_i420`].
fn bgra_to_i420(frame: &RawFrame) -> Vec<u8> {
    packed32_to_i420(frame, 2, 1, 0)
}

/// RGBA32 → I420 (YUV 4:2:0 planar). See [`packed32_to_i420`].
fn rgba_to_i420(frame: &RawFrame) -> Vec<u8> {
    packed32_to_i420(frame, 0, 1, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bgra_frame(w: u32, h: u32, stride: u32, pixel: [u8; 4]) -> RawFrame {
        let mut data = vec![0u8; (stride * h) as usize];
        for row in 0..h as usize {
            for col in 0..w as usize {
                let off = row * stride as usize + col * 4;
                data[off..off + 4].copy_from_slice(&pixel);
            }
        }
        RawFrame {
            format: PixelFormat::Bgra32,
            data: bytes::Bytes::from(data),
            display_idx: 0,
            width: w,
            height: h,
            stride,
            timestamp_ms: 0,
            dirty_rects: vec![],
        }
    }

    #[test]
    fn bgra_white_produces_correct_yuv() {
        // White: R=255, G=255, B=255 → BGRA = [255, 255, 255, 255]
        let frame = make_bgra_frame(2, 2, 8, [255, 255, 255, 255]);
        let yuv = bgra_to_i420(&frame);
        let y_size = 4; // 2×2
        let uv_size = 1; // 1×1 each
        assert_eq!(yuv.len(), y_size + uv_size * 2);
        // BT.601 limited: white = Y=235, U=128, V=128
        assert!(
            yuv[..y_size].iter().all(|&y| y == 235),
            "Y plane: all white = 235"
        );
        assert_eq!(yuv[y_size], 128, "U for white = 128");
        assert_eq!(yuv[y_size + uv_size], 128, "V for white = 128");
    }

    #[test]
    fn bgra_black_produces_correct_yuv() {
        // Black: R=G=B=0 → BGRA = [0, 0, 0, 255]
        let frame = make_bgra_frame(2, 2, 8, [0, 0, 0, 255]);
        let yuv = bgra_to_i420(&frame);
        let y_size = 4;
        let uv_size = 1;
        // BT.601 limited: black = Y=16, U=128, V=128
        assert!(
            yuv[..y_size].iter().all(|&y| y == 16),
            "Y plane: all black = 16"
        );
        assert_eq!(yuv[y_size], 128, "U for black = 128");
        assert_eq!(yuv[y_size + uv_size], 128, "V for black = 128");
    }

    #[test]
    fn bgra_output_size_is_correct() {
        // 4×4 frame: Y=16, UV=4 each → total 24 bytes
        let frame = make_bgra_frame(4, 4, 16, [0, 0, 0, 255]);
        let yuv = bgra_to_i420(&frame);
        assert_eq!(yuv.len(), 4 * 4 + (4 / 2) * (4 / 2) * 2);
    }

    #[test]
    fn bgra_stride_larger_than_row() {
        // stride = 12 bytes (3 pixels), but width = 2 pixels (8 bytes of active data)
        let w: u32 = 2;
        let h: u32 = 2;
        let stride: u32 = 12; // extra 4-byte padding per row
        let mut data = vec![0u8; (stride * h) as usize];
        // Write white pixels; padding bytes stay 0
        for row in 0..h as usize {
            for col in 0..w as usize {
                let off = row * stride as usize + col * 4;
                data[off..off + 4].copy_from_slice(&[255u8; 4]); // white BGRA
            }
        }
        let frame = RawFrame {
            format: PixelFormat::Bgra32,
            data: bytes::Bytes::from(data),
            display_idx: 0,
            width: w,
            height: h,
            stride,
            timestamp_ms: 0,
            dirty_rects: vec![],
        };
        let yuv = bgra_to_i420(&frame);
        // Despite padding, Y values must be 235 (white), not corrupted by padding zeros
        assert!(
            yuv[..4].iter().all(|&y| y == 235),
            "stride padding must not corrupt Y"
        );
    }

    #[test]
    fn rgba_white_matches_bgra_white() {
        // RGBA white = [255, 255, 255, 255]; BGRA white = [255, 255, 255, 255] — identical for white
        let bgra_frame = make_bgra_frame(2, 2, 8, [255, 255, 255, 255]);
        let rgba_frame = make_bgra_frame(2, 2, 8, [255, 255, 255, 255]);
        // Use a wrapper that treats the same data as RGBA — for all-white, channels R=B so result is same
        let rgba = RawFrame {
            format: PixelFormat::Rgba32,
            data: rgba_frame.data,
            display_idx: 0,
            width: 2,
            height: 2,
            stride: 8,
            timestamp_ms: 0,
            dirty_rects: vec![],
        };
        assert_eq!(bgra_to_i420(&bgra_frame), rgba_to_i420(&rgba));
    }

    #[test]
    fn nv12_output_size_correct() {
        let w: u32 = 4;
        let h: u32 = 4;
        let stride: u32 = 4;
        // NV12: Y plane = stride*h bytes, UV plane = stride*h/2 bytes (interleaved U,V)
        let y_size = (stride * h) as usize;
        let uv_size = (stride * h / 2) as usize;
        let data = vec![0u8; y_size + uv_size];
        let frame = RawFrame {
            format: PixelFormat::Nv12,
            data: bytes::Bytes::from(data),
            display_idx: 0,
            width: w,
            height: h,
            stride,
            timestamp_ms: 0,
            dirty_rects: vec![],
        };
        let yuv = nv12_to_i420(&frame);
        assert_eq!(yuv.len(), (w * h + (w / 2) * (h / 2) * 2) as usize);
    }
}
