#![allow(dead_code)]
//! Hardware-accelerated encoder via FFmpeg.
//!
//! Backend selection (per OS):
//!   Windows    : NVENC > AMF > QSV > software
//!   macOS      : VideoToolbox (h264_videotoolbox, hevc_videotoolbox)
//!   Linux      : NVENC > VAAPI (Intel/AMD) > software
//!
//! Currently a stub — ffmpeg-next integration deferred for v0.2.
//! See also: miru-codec/src/vpx.rs for the working software path.

use anyhow::{bail, Result};
use miru_common::message::VideoCodec;

use crate::{encoder::EncoderBackend, EncodedPacket};

/// Probe available hardware encoders on this system.
pub fn probe() -> Vec<HwEncoder> {
    let mut encoders = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if has_dll("nvEncodeAPI64.dll") {
            encoders.push(HwEncoder::Nvenc);
        }
        if has_dll("amfrt64.dll") {
            encoders.push(HwEncoder::Amf);
        }
        // QSV is available on most modern Intel CPUs
        encoders.push(HwEncoder::Qsv);
    }

    #[cfg(target_os = "macos")]
    {
        encoders.push(HwEncoder::VideoToolbox);
    }

    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/dev/dri/renderD128").exists() {
            encoders.push(HwEncoder::Vaapi);
        }
        if std::path::Path::new("/dev/nvidia0").exists() {
            encoders.push(HwEncoder::Nvenc);
        }
    }

    encoders
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwEncoder {
    Nvenc,        // NVIDIA
    Amf,          // AMD
    Qsv,          // Intel QuickSync
    VideoToolbox, // macOS
    Vaapi,        // Linux Intel/AMD
}

impl HwEncoder {
    /// Codecs supported by this hardware encoder.
    pub fn codecs(self) -> &'static [VideoCodec] {
        match self {
            // RTX 40-series adds AV1 encode; RTX 30+ for HEVC; older for H.264 only.
            HwEncoder::Nvenc => &[VideoCodec::Av1, VideoCodec::H265, VideoCodec::H264],
            HwEncoder::Amf => &[VideoCodec::Av1, VideoCodec::H265, VideoCodec::H264],
            HwEncoder::Qsv => &[VideoCodec::Av1, VideoCodec::H265, VideoCodec::H264],
            // Apple Silicon: full H.264/H.265; AV1 decode only on M3+.
            HwEncoder::VideoToolbox => &[VideoCodec::H265, VideoCodec::H264],
            HwEncoder::Vaapi => &[VideoCodec::H265, VideoCodec::H264],
        }
    }

    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            HwEncoder::Nvenc => "NVIDIA NVENC",
            HwEncoder::Amf => "AMD AMF",
            HwEncoder::Qsv => "Intel QuickSync",
            HwEncoder::VideoToolbox => "Apple VideoToolbox",
            HwEncoder::Vaapi => "Linux VAAPI",
        }
    }
}

#[cfg(target_os = "windows")]
fn has_dll(name: &str) -> bool {
    use std::path::PathBuf;
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(system).join("System32").join(name).exists()
}

/// Select best hardware encoder for the requested codec.
/// Returns None if no HW backend supports it.
pub fn select(codec: &VideoCodec) -> Option<HwEncoder> {
    probe().into_iter().find(|hw| hw.codecs().contains(codec))
}

// ─── Stub encoder backend (real impl uses ffmpeg-next) ────────────────────────

pub struct HwEncoderBackend {
    hw: HwEncoder,
    codec: VideoCodec,
}

impl HwEncoderBackend {
    pub fn new(
        hw: HwEncoder,
        codec: VideoCodec,
        _w: u32,
        _h: u32,
        _fps: u8,
        _bps: u32,
    ) -> Result<Self> {
        if !hw.codecs().contains(&codec) {
            bail!("{hw:?} does not support codec {codec:?}");
        }
        // TODO: ffmpeg-next AVCodecContext init with hw_device_ctx
        // hwaccel name = match hw { Nvenc => "cuda", VideoToolbox => "videotoolbox", ... }
        Ok(Self { hw, codec })
    }
}

impl EncoderBackend for HwEncoderBackend {
    fn encode(
        &mut self,
        _i420: &[u8],
        _w: u32,
        _h: u32,
        _ts: u64,
        _kf: bool,
    ) -> Result<Option<EncodedPacket>> {
        // TODO: avcodec_send_frame → avcodec_receive_packet
        bail!("HW encoder not yet implemented — fall back to VpxEncoder")
    }
    fn request_keyframe(&mut self) {}
    fn update_bitrate(&mut self, _kbps: u32) {}
}
