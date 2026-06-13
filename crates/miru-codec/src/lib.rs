//! Video codec abstraction.

pub mod decoder;
pub mod encoder;
pub mod hw;
pub mod jpeg;

#[cfg(feature = "vpx")]
pub mod vpx;

pub use decoder::Decoder;
pub use encoder::Encoder;
pub use hw::{probe as probe_hw, HwEncoder};
pub use jpeg::{i420_to_jpeg, i420_to_rgb};

use miru_common::message::VideoCodec;

pub struct EncodedPacket {
    pub keyframe: bool,
    pub data: Vec<u8>,
    pub timestamp_ms: u64,
    pub duration_ms: u32,
}

pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub y_plane: Vec<u8>,
    pub u_plane: Vec<u8>,
    pub v_plane: Vec<u8>,
    pub timestamp_ms: u64,
}

/// Codecs this build can actually ENCODE, in negotiation priority order.
///
/// Deliberately excludes hardware codecs (H264/H265/AV1): `probe_hw()` can
/// detect the silicon, but `HwEncoderBackend::encode` is still a stub, so
/// advertising them would let a viewer negotiate a codec whose session dies
/// on the first frame. Re-add them when the FFmpeg backend lands (v0.2+).
pub fn available_codecs() -> Vec<VideoCodec> {
    // JPEG fallback always available (no external codec dep).
    // vpx feature adds VP9/VP8 (royalty-free, software-encoded).
    // HW codecs (H264/H265/AV1) excluded until the FFmpeg backend lands.
    #[cfg(feature = "vpx")]
    return vec![VideoCodec::Vp9, VideoCodec::Vp8, VideoCodec::Jpeg];
    #[cfg(not(feature = "vpx"))]
    vec![VideoCodec::Jpeg]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every advertised codec must be constructible by Encoder::new —
    /// otherwise codec negotiation can select a codec that fails at the
    /// first frame.
    #[test]
    fn advertised_codecs_are_encodable() {
        for codec in available_codecs() {
            assert!(
                Encoder::new(codec.clone(), 640, 480, 30, 2000).is_ok(),
                "advertised codec {codec:?} cannot actually be constructed"
            );
        }
    }
}
