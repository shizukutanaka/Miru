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

    /// JPEG codec encodes a synthetic I420 frame and returns a valid JPEG.
    /// Verifies buffer sizing and JPEG magic bytes.
    #[test]
    fn jpeg_encode_produces_valid_jpeg() {
        let (w, h) = (64u32, 48u32);
        let y_size = (w * h) as usize;
        let uv_size = y_size / 4;
        let y = vec![128u8; y_size];
        let u = vec![128u8; uv_size];
        let v = vec![128u8; uv_size];

        let jpeg =
            crate::jpeg::i420_to_jpeg_raw(&y, &u, &v, w, h, 80).expect("JPEG encode");
        assert!(
            jpeg.len() > 4 && jpeg[0] == 0xFF && jpeg[1] == 0xD8 && jpeg[2] == 0xFF,
            "output does not start with JPEG SOI marker"
        );
    }

    /// JPEG encode+Encoder roundtrip: Encoder wraps i420_to_jpeg and returns an EncodedPacket.
    #[test]
    fn jpeg_encoder_roundtrip() {
        use miru_common::message::VideoCodec;
        let (w, h) = (64u32, 48u32);
        let mut enc = Encoder::new(VideoCodec::Jpeg, w, h, 30, 500).expect("create encoder");
        let i420 = vec![128u8; (w * h + w * h / 2) as usize];
        let packet = enc
            .encode(&i420, w, h, 0, true)
            .expect("encode")
            .expect("packet");
        assert!(packet.keyframe, "first JPEG frame must be keyframe");
        assert!(
            !packet.data.is_empty(),
            "encoded packet must be non-empty"
        );
        assert_eq!(packet.data[0], 0xFF, "packet must start with JPEG SOI");
    }
}
