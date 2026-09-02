//! Video codec abstraction.

pub mod color;
pub mod decoder;
pub mod encoder;
pub mod hw;
pub mod jpeg;

#[cfg(feature = "vpx")]
pub mod vpx;

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg_enc;

/// Platform/FFI code. Per the workspace rule, all `unsafe` in this crate lives
/// here.
#[cfg(feature = "ffmpeg")]
pub mod platform {
    pub mod ffmpeg_ffi;
}

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
/// The rule is that everything listed here must be constructible by
/// `Encoder::new` right now — a viewer that negotiates a codec from this list
/// must not get a session that dies on the first frame. Hardware codecs are
/// therefore gated on an encoder having genuinely opened, not on the silicon
/// being detected; see `hw_codecs_that_open()`.
pub fn available_codecs() -> Vec<VideoCodec> {
    // JPEG fallback always available (no external codec dep).
    // vpx feature adds VP9/VP8 (royalty-free, software-encoded).
    #[allow(unused_mut)]
    let mut v = Vec::new();
    // Hardware codecs first: negotiation prefers earlier entries, and they are
    // only listed when an encoder for them genuinely opened on this machine.
    #[cfg(feature = "ffmpeg")]
    v.extend_from_slice(hw_codecs_that_open());
    #[cfg(feature = "vpx")]
    v.extend_from_slice(&[VideoCodec::Vp9, VideoCodec::Vp8]);
    v.push(VideoCodec::Jpeg);
    v
}

/// Hardware codecs for which an encoder actually opened on this machine.
///
/// Probed once by really opening each candidate at a small resolution, because
/// `h264_nvenc` being compiled into libavcodec says nothing about a GPU being
/// present. Cached: opening an encoder is far too expensive to repeat, and the
/// answer cannot change within a process.
#[cfg(feature = "ffmpeg")]
fn hw_codecs_that_open() -> &'static [VideoCodec] {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Vec<VideoCodec>> = OnceLock::new();
    CACHE.get_or_init(|| {
        [VideoCodec::Av1, VideoCodec::H265, VideoCodec::H264]
            .into_iter()
            .filter(|c| encoder::Encoder::new(c.clone(), 640, 480, 30, 2000).is_ok())
            .collect()
    })
}

/// Whether this build can actually encode using hardware.
///
/// This is what gets advertised to peers as `Features::hw_encode`, so it must
/// describe **this binary's ability**, not the machine's silicon. `probe_hw()`
/// answers the latter and returns true on essentially every modern desktop;
/// wiring it to the handshake told viewers a build with no hardware encoder
/// compiled in had one. The same honesty rule is already applied to audio
/// (see `Features::audio` in miru-host/src/session.rs).
///
/// Derived from `available_codecs()` so it becomes true on its own the moment
/// the FFmpeg backend starts advertising a hardware codec — there is no second
/// place to remember to update.
pub fn has_hw_encode() -> bool {
    available_codecs().iter().any(is_hardware_codec)
}

/// Codecs that in practice require a hardware encoder in this project.
/// VP8/VP9 are software (libvpx) and JPEG is trivially software.
fn is_hardware_codec(c: &VideoCodec) -> bool {
    matches!(c, VideoCodec::Av1 | VideoCodec::H264 | VideoCodec::H265)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hw_encode is a claim about this build, not about the machine. Until a
    /// hardware backend is wired into `Encoder::new`, advertising it is a lie
    /// that makes a viewer negotiate a codec whose session dies on frame one.
    #[test]
    fn hw_encode_claim_matches_what_the_build_can_encode() {
        for codec in available_codecs() {
            if is_hardware_codec(&codec) {
                assert!(
                    has_hw_encode(),
                    "advertising HW codec {codec:?} while has_hw_encode() is false"
                );
            }
        }
        if has_hw_encode() {
            assert!(
                available_codecs().iter().any(is_hardware_codec),
                "claiming hw_encode with no hardware codec advertised"
            );
        }
    }

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

        let jpeg = crate::jpeg::i420_to_jpeg_raw(&y, &u, &v, w, h, 80).expect("JPEG encode");
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
        assert!(!packet.data.is_empty(), "encoded packet must be non-empty");
        assert_eq!(packet.data[0], 0xFF, "packet must start with JPEG SOI");
    }
}
