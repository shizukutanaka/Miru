//! Video decoder — selects backend by codec.
//!
//! VP9/VP8 via libvpx (behind the `vpx` feature gate).
//! JPEG via the `image` crate (always available).
//! AV1 → dav1d, H264/H265 → ffmpeg (TODO v0.3).

use crate::DecodedFrame;
use anyhow::{bail, Result};
use miru_common::message::VideoCodec;

pub struct Decoder {
    inner: Box<dyn DecoderBackend>,
    codec: VideoCodec,
}

pub trait DecoderBackend: Send {
    fn decode(&mut self, data: &[u8], ts_ms: u64) -> Result<Option<DecodedFrame>>;
}

impl Decoder {
    pub fn new(codec: VideoCodec) -> Result<Self> {
        let inner: Box<dyn DecoderBackend> = match &codec {
            #[cfg(feature = "vpx")]
            VideoCodec::Vp9 | VideoCodec::Vp8 => {
                Box::new(crate::vpx::VpxDecoder::new(codec.clone())?)
            }
            #[cfg(not(feature = "vpx"))]
            VideoCodec::Vp9 | VideoCodec::Vp8 => {
                bail!("VP9/VP8 decoding requires the `vpx` feature (install libvpx-dev)")
            }
            VideoCodec::Jpeg => Box::new(JpegDecoder),
            VideoCodec::Av1 | VideoCodec::H264 | VideoCodec::H265 => {
                bail!("{codec:?} decoding not yet supported (planned for v0.3)")
            }
        };
        Ok(Self { inner, codec })
    }

    pub fn decode(&mut self, data: &[u8], ts_ms: u64) -> Result<Option<DecodedFrame>> {
        self.inner.decode(data, ts_ms)
    }

    pub fn codec(&self) -> &VideoCodec {
        &self.codec
    }
}

/// JPEG decoder — decompresses to I420 planes via the `image` crate.
struct JpegDecoder;

impl DecoderBackend for JpegDecoder {
    fn decode(&mut self, data: &[u8], ts_ms: u64) -> Result<Option<DecodedFrame>> {
        use image::ImageReader;
        use std::io::Cursor;

        let img = ImageReader::new(Cursor::new(data))
            .with_guessed_format()?
            .decode()
            .map_err(|e| anyhow::anyhow!("JPEG decode: {e}"))?;

        let rgb = img.to_rgb8();
        let w = rgb.width();
        let h = rgb.height();

        // RGB → I420 conversion
        let (y_plane, u_plane, v_plane) = rgb_to_i420(&rgb, w, h);

        Ok(Some(DecodedFrame {
            width: w,
            height: h,
            y_plane,
            u_plane,
            v_plane,
            timestamp_ms: ts_ms,
        }))
    }
}

/// Convert an RGB image to I420 (YUV 4:2:0) planes.
fn rgb_to_i420(rgb: &image::RgbImage, w: u32, h: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = Vec::with_capacity((w * h) as usize);
    let mut u = Vec::with_capacity(((w / 2) * (h / 2)) as usize);
    let mut v = Vec::with_capacity(((w / 2) * (h / 2)) as usize);

    for row in 0..h {
        for col in 0..w {
            let px = rgb.get_pixel(col, row);
            let r = px[0] as f64;
            let g = px[1] as f64;
            let b = px[2] as f64;

            // BT.601 coefficients
            let yy = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0);
            y.push(yy as u8);
        }
    }

    // Chroma subsampled 2x2
    let h2 = h / 2;
    let w2 = w / 2;
    for row in 0..h2 {
        for col in 0..w2 {
            let px = rgb.get_pixel(col * 2, row * 2);
            let r = px[0] as f64;
            let g = px[1] as f64;
            let b = px[2] as f64;

            let cb = (-0.169 * r - 0.331 * g + 0.500 * b + 128.0).clamp(0.0, 255.0);
            let cr = (0.500 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0);
            u.push(cb as u8);
            v.push(cr as u8);
        }
    }

    (y, u, v)
}
