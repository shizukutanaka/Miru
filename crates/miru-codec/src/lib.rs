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

pub fn available_codecs() -> Vec<VideoCodec> {
    let mut codecs: Vec<VideoCodec> = Vec::new();
    #[cfg(feature = "vpx")]
    {
        codecs.push(VideoCodec::Vp9);
        codecs.push(VideoCodec::Vp8);
    }
    for hw in probe_hw() {
        for c in hw.codecs() {
            if !codecs.contains(c) {
                codecs.push(c.clone());
            }
        }
    }
    // JPEG fallback always available (no external codec dep).
    codecs.push(VideoCodec::Jpeg);
    codecs
}
