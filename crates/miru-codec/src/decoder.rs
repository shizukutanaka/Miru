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
pub(crate) fn rgb_to_i420(rgb: &image::RgbImage, w: u32, h: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
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

    // Chroma subsampled 2x2: average the four pixels in each block (BT.601).
    // Sampling only the top-left pixel causes chroma aliasing on fine detail.
    let h2 = h / 2;
    let w2 = w / 2;
    for row in 0..h2 {
        for col in 0..w2 {
            let x0 = col * 2;
            let y0 = row * 2;
            let x1 = (x0 + 1).min(w - 1);
            let y1 = (y0 + 1).min(h - 1);

            let avg_channel = |c: usize| {
                (rgb.get_pixel(x0, y0)[c] as f64
                    + rgb.get_pixel(x1, y0)[c] as f64
                    + rgb.get_pixel(x0, y1)[c] as f64
                    + rgb.get_pixel(x1, y1)[c] as f64)
                    / 4.0
            };
            let r = avg_channel(0);
            let g = avg_channel(1);
            let b = avg_channel(2);

            let cb = (-0.169 * r - 0.331 * g + 0.500 * b + 128.0).clamp(0.0, 255.0);
            let cr = (0.500 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0);
            u.push(cb as u8);
            v.push(cr as u8);
        }
    }

    (y, u, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 block with one red and one blue pixel (plus two greys) should
    /// yield a chroma value that reflects the *average*, not just the top-left
    /// pixel.  Before the fix, sampling only (0,0) = red gave Cr ≈ 212 and
    /// Cb ≈ 85.  After the fix (average), Cr and Cb should be much closer to
    /// neutral 128.
    #[test]
    fn chroma_averages_2x2_block() {
        let mut img = image::RgbImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0])); // red
        img.put_pixel(1, 0, image::Rgb([0, 0, 255])); // blue
        img.put_pixel(0, 1, image::Rgb([128, 128, 128])); // grey
        img.put_pixel(1, 1, image::Rgb([128, 128, 128])); // grey

        let (_y, u, v) = rgb_to_i420(&img, 2, 2);
        assert_eq!(u.len(), 1);
        assert_eq!(v.len(), 1);

        // When only top-left (red) was sampled: Cr ≈ 212, Cb ≈ 85.
        // With correct 4-pixel average the values are much closer to 128.
        let cb = u[0] as i32;
        let cr = v[0] as i32;
        assert!(
            (cb - 128).abs() < 50,
            "Cb={cb} too far from neutral (top-left-only sampling bug)"
        );
        assert!(
            (cr - 128).abs() < 50,
            "Cr={cr} too far from neutral (top-left-only sampling bug)"
        );
    }
}
