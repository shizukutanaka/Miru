/// Codec negotiation priority (higher = preferred).
///
/// Royalty-free codecs are preferred over patent-encumbered ones, aligning
/// with Miru's open-source / buy-once ethos (no per-seat licensing surprises):
///   AV1 > VP9 > VP8  (royalty-free, AOMedia)
///   > H265 > H264    (patent-encumbered; kept as HW-accel fallback)
///   > JPEG           (always-available last resort)
///
/// AV1 leads because it is both royalty-free AND the best compressor.
pub const CODEC_PRIORITY: &[crate::message::VideoCodec] = &[
    crate::message::VideoCodec::Av1,
    crate::message::VideoCodec::Vp9,
    crate::message::VideoCodec::Vp8,
    crate::message::VideoCodec::H265,
    crate::message::VideoCodec::H264,
    crate::message::VideoCodec::Jpeg,
];

/// Select best common codec between host capabilities and viewer capabilities.
pub fn negotiate(
    host: &[crate::message::VideoCodec],
    viewer: &[crate::message::VideoCodec],
) -> Option<crate::message::VideoCodec> {
    CODEC_PRIORITY.iter().find(|c| host.contains(c) && viewer.contains(c)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::VideoCodec::*;

    #[test]
    fn prefers_royalty_free_vp9_over_h264() {
        // When both peers support VP9 and H264, the royalty-free VP9 wins.
        let chosen = negotiate(&[Vp9, H264], &[Vp9, H264]);
        assert_eq!(chosen, Some(Vp9));
    }

    #[test]
    fn av1_beats_everything() {
        let chosen = negotiate(&[Av1, Vp9, H264], &[Av1, Vp9, H264]);
        assert_eq!(chosen, Some(Av1));
    }

    #[test]
    fn falls_back_to_h264_when_only_common() {
        // If the only shared codec is H264, it's still selected.
        let chosen = negotiate(&[H264, Jpeg], &[H264, Vp9]);
        assert_eq!(chosen, Some(H264));
    }

    #[test]
    fn jpeg_is_last_resort() {
        let chosen = negotiate(&[Jpeg], &[Jpeg, Vp9]);
        assert_eq!(chosen, Some(Jpeg));
    }

    #[test]
    fn no_common_codec_returns_none() {
        assert_eq!(negotiate(&[Av1], &[H264]), None);
    }
}
