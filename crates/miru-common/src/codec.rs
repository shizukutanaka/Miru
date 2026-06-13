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
    CODEC_PRIORITY
        .iter()
        .find(|c| host.contains(c) && viewer.contains(c))
        .cloned()
}

/// Audio codec negotiation priority: Opus (low-latency, great quality)
/// over raw PCM (always decodable, high bandwidth).
pub const AUDIO_CODEC_PRIORITY: &[crate::message::AudioCodec] = &[
    crate::message::AudioCodec::Opus,
    crate::message::AudioCodec::Pcm,
];

/// Select best common audio codec, mirroring [`negotiate`] for video.
///
/// When either side advertises no audio codecs (empty slice) we fall back to
/// PCM — the universally-decodable baseline.  This preserves backward
/// compatibility with older peers that pre-date the `audio_codecs` field.
pub fn negotiate_audio(
    host: &[crate::message::AudioCodec],
    viewer: &[crate::message::AudioCodec],
) -> Option<crate::message::AudioCodec> {
    if host.is_empty() || viewer.is_empty() {
        return Some(crate::message::AudioCodec::Pcm);
    }
    AUDIO_CODEC_PRIORITY
        .iter()
        .find(|c| host.contains(c) && viewer.contains(c))
        .cloned()
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

    #[test]
    fn audio_prefers_opus() {
        use crate::message::AudioCodec::*;
        assert_eq!(negotiate_audio(&[Opus, Pcm], &[Pcm, Opus]), Some(Opus));
        assert_eq!(negotiate_audio(&[Opus, Pcm], &[Pcm]), Some(Pcm));
        assert_eq!(negotiate_audio(&[Opus], &[Pcm]), None);
    }

    #[test]
    fn audio_empty_list_falls_back_to_pcm() {
        use crate::message::AudioCodec::*;
        // Either side omitting audio_codecs (backward compat) → PCM baseline.
        assert_eq!(negotiate_audio(&[], &[Opus]), Some(Pcm));
        assert_eq!(negotiate_audio(&[Opus], &[]), Some(Pcm));
        assert_eq!(negotiate_audio(&[], &[]), Some(Pcm));
    }
}
