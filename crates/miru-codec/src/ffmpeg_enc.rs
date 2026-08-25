//! FFmpeg-backed hardware encoder (AV1/H264/H265).
//!
//! Build with: cargo build --features ffmpeg
//!
//! Codec selection (auto-resolved at runtime):
//!   AV1 :  av1_nvenc | av1_qsv | av1_amf  (RTX 40+ / Arc / RX 7000+)
//!   H265:  hevc_nvenc | hevc_qsv | hevc_videotoolbox | hevc_vaapi
//!   H264:  h264_nvenc | h264_qsv | h264_videotoolbox | h264_vaapi
//!
//! Requires the ffmpeg dev libraries at build time:
//!   apt: libavcodec-dev libavutil-dev
//!   brew: ffmpeg
//!   choco: ffmpeg-shared
//!
//! There is no Rust dependency behind this. `crates/miru-codec/csrc/miru_ffmpeg.c`
//! is a small C surface over libavcodec, compiled by build.rs and wrapped by
//! `platform/ffmpeg_ffi.rs`; see the shim header for why that beats a binding
//! crate. Consequently the `ffmpeg` feature adds no crate to the graph.

#![cfg(feature = "ffmpeg")]

use anyhow::{bail, Result};
use miru_common::message::VideoCodec;

use crate::platform::ffmpeg_ffi::FfmpegEncoder as Raw;
use crate::{encoder::EncoderBackend, hw::HwEncoder, EncodedPacket};

/// FFmpeg encoder name lookup table.
fn ffmpeg_codec_name(hw: HwEncoder, codec: &VideoCodec) -> Option<&'static str> {
    match (hw, codec) {
        (HwEncoder::Nvenc, VideoCodec::Av1)  => Some("av1_nvenc"),
        (HwEncoder::Nvenc, VideoCodec::H265) => Some("hevc_nvenc"),
        (HwEncoder::Nvenc, VideoCodec::H264) => Some("h264_nvenc"),
        (HwEncoder::Qsv,   VideoCodec::Av1)  => Some("av1_qsv"),
        (HwEncoder::Qsv,   VideoCodec::H265) => Some("hevc_qsv"),
        (HwEncoder::Qsv,   VideoCodec::H264) => Some("h264_qsv"),
        (HwEncoder::Amf,   VideoCodec::Av1)  => Some("av1_amf"),
        (HwEncoder::Amf,   VideoCodec::H265) => Some("hevc_amf"),
        (HwEncoder::Amf,   VideoCodec::H264) => Some("h264_amf"),
        (HwEncoder::VideoToolbox, VideoCodec::H265) => Some("hevc_videotoolbox"),
        (HwEncoder::VideoToolbox, VideoCodec::H264) => Some("h264_videotoolbox"),
        (HwEncoder::Vaapi, VideoCodec::H265) => Some("hevc_vaapi"),
        (HwEncoder::Vaapi, VideoCodec::H264) => Some("h264_vaapi"),
        _ => None,
    }
}

pub struct FFmpegEncoder {
    raw: Raw,
    hw: HwEncoder,
    codec: VideoCodec,
    /// Set when a keyframe was requested out of band; consumed by the next encode.
    force_keyframe: bool,
}

impl FFmpegEncoder {
    pub fn new(
        hw: HwEncoder,
        codec: VideoCodec,
        width: u32,
        height: u32,
        fps: u8,
        bitrate_kbps: u32,
    ) -> Result<Self> {
        let name = ffmpeg_codec_name(hw, &codec)
            .ok_or_else(|| anyhow::anyhow!("No FFmpeg encoder for {hw:?}/{codec:?}"))?;

        // Opening is the capability test. The encoder being compiled into
        // libavcodec says nothing about the silicon being present, so the two
        // failures are reported differently: a missing encoder is a build
        // problem, a refusal to open is "this machine has no such hardware".
        let raw = Raw::open(name, width, height, fps, bitrate_kbps).ok_or_else(|| {
            if crate::platform::ffmpeg_ffi::encoder_compiled_in(name) {
                anyhow::anyhow!(
                    "{name} is present but would not open at {width}x{height} — \
                     the hardware it needs is most likely absent"
                )
            } else {
                anyhow::anyhow!("{name} is not compiled into the linked libavcodec")
            }
        })?;

        tracing::info!("FFmpeg encoder open: {name} ({hw:?}) {width}x{height}@{fps}");
        Ok(Self { raw, hw, codec, force_keyframe: false })
    }

    /// Which hardware this encoder is running on.
    pub fn hw(&self) -> HwEncoder {
        self.hw
    }

    /// Which codec it was opened for.
    pub fn codec(&self) -> &VideoCodec {
        &self.codec
    }
}

impl EncoderBackend for FFmpegEncoder {
    fn encode(
        &mut self,
        i420: &[u8],
        _w: u32,
        _h: u32,
        ts_ms: u64,
        keyframe: bool,
    ) -> Result<Option<EncodedPacket>> {
        let want_key = keyframe || std::mem::take(&mut self.force_keyframe);
        if let Err(e) = self.raw.send(i420, want_key) {
            bail!("ffmpeg send_frame failed: {e}");
        }
        match self.raw.receive() {
            Err(e) => bail!("ffmpeg receive_packet failed: {e}"),
            // The encoder is still filling its pipeline; not an error.
            Ok(None) => Ok(None),
            Ok(Some(p)) => Ok(Some(EncodedPacket {
                keyframe: p.keyframe,
                data: p.data,
                timestamp_ms: ts_ms,
                duration_ms: 0,
            })),
        }
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    fn update_bitrate(&mut self, kbps: u32) {
        self.raw.set_bitrate(kbps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HW: &[HwEncoder] = &[
        HwEncoder::Nvenc,
        HwEncoder::Amf,
        HwEncoder::Qsv,
        HwEncoder::VideoToolbox,
        HwEncoder::Vaapi,
    ];

    /// The two tables must agree. `HwEncoder::codecs()` is what negotiation
    /// offers to the viewer; `ffmpeg_codec_name` is what we can actually
    /// instantiate. If the first promises a codec the second cannot name, the
    /// session negotiates a codec and then fails to start the encoder.
    #[test]
    fn every_advertised_codec_has_an_ffmpeg_name() {
        for &hw in HW {
            for codec in hw.codecs() {
                assert!(
                    ffmpeg_codec_name(hw, codec).is_some(),
                    "{hw:?} advertises {codec:?} but ffmpeg_codec_name has no entry"
                );
            }
        }
    }

    /// And the reverse: naming an encoder we never advertise is dead weight at
    /// best, and at worst means codecs() is missing a capability we have.
    #[test]
    fn every_ffmpeg_name_is_advertised() {
        let all = [
            VideoCodec::Av1,
            VideoCodec::H265,
            VideoCodec::H264,
            VideoCodec::Vp9,
            VideoCodec::Vp8,
            VideoCodec::Jpeg,
        ];
        for &hw in HW {
            for codec in &all {
                if ffmpeg_codec_name(hw, codec).is_some() {
                    assert!(
                        hw.codecs().contains(codec),
                        "ffmpeg_codec_name maps {hw:?}/{codec:?} but codecs() omits it"
                    );
                }
            }
        }
    }

    /// Software-only codecs must never resolve to a hardware encoder — VP9/VP8
    /// go through libvpx and JPEG through the always-available fallback.
    #[test]
    fn software_codecs_have_no_hardware_entry() {
        for &hw in HW {
            for codec in [VideoCodec::Vp9, VideoCodec::Vp8, VideoCodec::Jpeg] {
                assert_eq!(ffmpeg_codec_name(hw, &codec), None, "{hw:?}/{codec:?}");
            }
        }
    }
}
