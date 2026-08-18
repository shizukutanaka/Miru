//! FFmpeg-backed hardware encoder (AV1/H264/H265).
//!
//! Build with: cargo build --features ffmpeg
//!
//! Codec selection (auto-resolved at runtime):
//!   AV1 :  av1_nvenc | av1_qsv | av1_amf  (RTX 40+ / Arc / RX 7000+)
//!   H265:  hevc_nvenc | hevc_qsv | hevc_videotoolbox | hevc_vaapi
//!   H264:  h264_nvenc | h264_qsv | h264_videotoolbox | h264_vaapi
//!
//! NOTE: requires ffmpeg dev libraries installed:
//!   apt: libavcodec-dev libavutil-dev libavformat-dev
//!   brew: ffmpeg
//!   choco: ffmpeg-shared

#![cfg(feature = "ffmpeg")]

use anyhow::{bail, Context, Result};
use miru_common::message::VideoCodec;

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
    // Real impl: ffmpeg::codec::context::Context, ffmpeg::frame::Video, etc.
    hw: HwEncoder,
    codec: VideoCodec,
    width: u32,
    height: u32,
}

impl FFmpegEncoder {
    pub fn new(hw: HwEncoder, codec: VideoCodec, width: u32, height: u32, fps: u8, bitrate_kbps: u32) -> Result<Self> {
        let ffmpeg_name = ffmpeg_codec_name(hw, &codec)
            .ok_or_else(|| anyhow::anyhow!("No FFmpeg encoder for {:?}/{:?}", hw, codec))?;

        // TODO real init flow:
        //   ffmpeg::init()?;
        //   let codec = ffmpeg::encoder::find_by_name(ffmpeg_name)?;
        //   let mut ctx = ffmpeg::codec::Context::new();
        //   let mut enc = ctx.encoder().video()?;
        //   enc.set_width(width); enc.set_height(height);
        //   enc.set_format(ffmpeg::format::Pixel::YUV420P);
        //   enc.set_time_base((1, fps as i32));
        //   enc.set_bit_rate((bitrate_kbps * 1000) as usize);
        //   enc.set_max_bit_rate((bitrate_kbps * 1500) as usize);
        //   // GOP: 10 seconds
        //   enc.set_gop((fps as i32) * 10);
        //   let opened = enc.open_as(codec)?;

        tracing::info!("FFmpeg encoder requested: {} ({:?})", ffmpeg_name, hw);
        Ok(Self { hw, codec, width, height })
    }
}

impl EncoderBackend for FFmpegEncoder {
    fn encode(&mut self, _i420: &[u8], _w: u32, _h: u32, _ts: u64, _kf: bool) -> Result<Option<EncodedPacket>> {
        // TODO:
        //   let mut frame = Video::new(format::Pixel::YUV420P, w, h);
        //   // copy planes from i420 into frame.data(0..2)
        //   frame.set_pts(Some(ts as i64));
        //   if kf { frame.set_kind(picture::Type::I); }
        //   self.encoder.send_frame(&frame)?;
        //   let mut packet = Packet::empty();
        //   while self.encoder.receive_packet(&mut packet).is_ok() {
        //       return Ok(Some(EncodedPacket {
        //           keyframe: packet.is_key(),
        //           data: packet.data().unwrap().to_vec(),
        //           timestamp_ms: ts,
        //           duration_ms: 0,
        //       }));
        //   }
        bail!("FFmpegEncoder not yet wired — see TODO in source")
    }

    fn request_keyframe(&mut self) {}
    fn update_bitrate(&mut self, _kbps: u32) {}
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
