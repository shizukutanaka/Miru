//! Video encoder — selects backend by codec.

use crate::EncodedPacket;
use anyhow::{bail, Result};
use miru_common::message::VideoCodec;

#[cfg(feature = "vpx")]
use crate::vpx::VpxEncoder;

pub struct Encoder {
    inner: Box<dyn EncoderBackend>,
    codec: VideoCodec,
}

pub trait EncoderBackend: Send {
    fn encode(
        &mut self,
        i420: &[u8],
        width: u32,
        height: u32,
        ts_ms: u64,
        keyframe: bool,
    ) -> Result<Option<EncodedPacket>>;
    fn request_keyframe(&mut self);
    fn update_bitrate(&mut self, kbps: u32);
}

impl Encoder {
    pub fn new(
        codec: VideoCodec,
        width: u32,
        height: u32,
        fps: u8,
        bitrate_kbps: u32,
    ) -> Result<Self> {
        // fps / bitrate_kbps are only consumed by the vpx backend.
        #[cfg(not(feature = "vpx"))]
        let _ = (fps, bitrate_kbps);
        let inner: Box<dyn EncoderBackend> = match &codec {
            #[cfg(feature = "vpx")]
            VideoCodec::Vp9 | VideoCodec::Vp8 => Box::new(VpxEncoder::new(
                if matches!(codec, VideoCodec::Vp8) {
                    &VideoCodec::Vp8
                } else {
                    &VideoCodec::Vp9
                },
                width,
                height,
                fps,
                bitrate_kbps,
            )?),
            #[cfg(not(feature = "vpx"))]
            VideoCodec::Vp9 | VideoCodec::Vp8 => {
                bail!("VP9/VP8 encoding requires the `vpx` feature (install libvpx-dev)")
            }
            VideoCodec::Jpeg => Box::new(JpegEncoder::new(width, height)),
            VideoCodec::Av1 | VideoCodec::H264 | VideoCodec::H265 => {
                bail!("{codec:?} encoding not yet implemented (planned for v0.3)")
            }
        };
        Ok(Self { inner, codec })
    }

    pub fn encode(
        &mut self,
        i420: &[u8],
        w: u32,
        h: u32,
        ts_ms: u64,
        kf: bool,
    ) -> Result<Option<EncodedPacket>> {
        self.inner.encode(i420, w, h, ts_ms, kf)
    }

    pub fn request_keyframe(&mut self) {
        self.inner.request_keyframe();
    }
    pub fn update_bitrate(&mut self, kbps: u32) {
        self.inner.update_bitrate(kbps);
    }
    pub fn codec(&self) -> &VideoCodec {
        &self.codec
    }
}

/// JPEG fallback encoder — always available, zero native deps.
/// Every frame is an independent JPEG; no inter-frame compression.
/// Higher bandwidth than VP9 but useful for:
///   - build environments without libvpx
///   - initial "something on screen" before codec negotiation
///   - screenshot-style workflows (AI agent ScreenRead)
struct JpegEncoder {
    width: u32,
    height: u32,
    quality: u8,
}

impl JpegEncoder {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            quality: 80,
        }
    }
}

impl EncoderBackend for JpegEncoder {
    fn encode(
        &mut self,
        i420: &[u8],
        width: u32,
        height: u32,
        ts_ms: u64,
        _keyframe: bool,
    ) -> Result<Option<EncodedPacket>> {
        const MAX_FRAME_DIM: u32 = 32768;
        if width > MAX_FRAME_DIM || height > MAX_FRAME_DIM {
            bail!("JPEG encoder: frame dimensions {width}×{height} exceed limit {MAX_FRAME_DIM}");
        }
        let w = width as usize;
        let h = height as usize;
        let y_size = w * h;
        let uv_size = (w / 2) * (h / 2);

        if i420.len() < y_size + 2 * uv_size {
            bail!(
                "JPEG encoder: I420 buffer too small ({} < {})",
                i420.len(),
                y_size + 2 * uv_size
            );
        }

        let y = &i420[..y_size];
        let u = &i420[y_size..y_size + uv_size];
        let v = &i420[y_size + uv_size..y_size + 2 * uv_size];

        let jpeg_bytes = crate::jpeg::i420_to_jpeg_raw(y, u, v, width, height, self.quality)?;
        Ok(Some(EncodedPacket {
            keyframe: true, // every JPEG frame is a keyframe
            data: jpeg_bytes,
            timestamp_ms: ts_ms,
            duration_ms: 0,
        }))
    }

    fn request_keyframe(&mut self) { /* every frame is a keyframe */
    }

    fn update_bitrate(&mut self, kbps: u32) {
        // Approximate quality from target bitrate — very rough heuristic.
        // At 1080p30, ~9 Mbps corresponds to quality 30 (minimum); quality
        // increases above that. For AI-agent screenshot use cases (lower
        // resolution, infrequent frames) this is sufficient.
        let pixels = (self.width * self.height) as f64;
        let bits_per_pixel = (kbps as f64 * 1000.0) / (pixels * 30.0);
        self.quality = (bits_per_pixel * 200.0).clamp(30.0, 95.0) as u8;
    }
}
