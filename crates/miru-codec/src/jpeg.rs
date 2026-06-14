//! JPEG encoder for the viewer's WebView path.

use anyhow::Result;
use image::{codecs::jpeg::JpegEncoder, ColorType, ImageEncoder};

use crate::DecodedFrame;

/// Encode I420 → JPEG from a DecodedFrame. quality: 1–100.
pub fn i420_to_jpeg(frame: &DecodedFrame, quality: u8) -> Result<Vec<u8>> {
    i420_to_jpeg_raw(
        &frame.y_plane,
        &frame.u_plane,
        &frame.v_plane,
        frame.width,
        frame.height,
        quality,
    )
}

/// Encode raw I420 planes → JPEG. Used by the JPEG fallback encoder.
pub fn i420_to_jpeg_raw(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    let rgb = i420_planes_to_rgb(y, u, v, w, h);
    let mut out = Vec::with_capacity(w * h / 4);
    let encoder = JpegEncoder::new_with_quality(&mut out, quality);
    encoder.write_image(&rgb, width, height, ColorType::Rgb8.into())?;
    Ok(out)
}

/// I420 (YUV planar) → RGB packed. BT.601 limited-range.
pub fn i420_to_rgb(frame: &DecodedFrame) -> Vec<u8> {
    i420_planes_to_rgb(
        &frame.y_plane,
        &frame.u_plane,
        &frame.v_plane,
        frame.width as usize,
        frame.height as usize,
    )
}

fn i420_planes_to_rgb(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    w: usize,
    h: usize,
) -> Vec<u8> {
    // Guard against mismatched plane sizes — return empty rather than panic.
    // Callers that validated input (VPX/JPEG decoder) never hit this; it
    // protects against a hypothetical future path that constructs a DecodedFrame
    // with wrong plane lengths.
    let y_needed = w.saturating_mul(h);
    let uv_needed = (w / 2).saturating_mul(h / 2);
    if y_plane.len() < y_needed || u_plane.len() < uv_needed || v_plane.len() < uv_needed {
        return Vec::new();
    }

    let mut rgb = vec![0u8; y_needed.saturating_mul(3)];

    for row in 0..h {
        for col in 0..w {
            let y = y_plane[row * w + col] as i32;
            let uv_idx = (row / 2) * (w / 2) + col / 2;
            let u = u_plane[uv_idx] as i32;
            let v = v_plane[uv_idx] as i32;

            let c = y - 16;
            let d = u - 128;
            let e = v - 128;

            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255);
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255);
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255);

            let i = (row * w + col) * 3;
            rgb[i] = r as u8;
            rgb[i + 1] = g as u8;
            rgb[i + 2] = b as u8;
        }
    }
    rgb
}
