//! Hardware-accelerated encoder via FFmpeg.
//!
//! Backend selection (per OS):
//!   Windows    : NVENC > AMF > QSV > software
//!   macOS      : VideoToolbox (h264_videotoolbox, hevc_videotoolbox)
//!   Linux      : NVENC > VAAPI (Intel/AMD) > software
//!
//! **This module is diagnostics only.** It reports which encoder silicon is
//! present; it does not encode anything. The encoding backend lives in
//! ffmpeg_enc.rs behind the `ffmpeg` feature — which is a skeleton, not a
//! backend: its encode() bails, and the `ffmpeg` feature pulls in no crate. The
//! only working encode path is vpx.rs. Do not derive a capability advertised to peers from `probe()` —
//! silicon being present says nothing about this build being able to use it.
//! Use `crate::has_hw_encode()` for that.

use miru_common::message::VideoCodec;

/// Probe hardware encoder silicon present on this system. Diagnostics only —
/// see the module docs before using this for anything a peer can observe.
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
        // QSV was pushed unconditionally, so every Windows host — AMD-only
        // machines included — reported Intel QuickSync. The runtime is what
        // actually has to be there: libmfxhw64.dll is the classic Media SDK
        // dispatcher, libvpl the oneVPL successor shipped with newer drivers.
        if has_dll("libmfxhw64.dll") || has_dll("libvpl.dll") {
            encoders.push(HwEncoder::Qsv);
        }
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


#[cfg(test)]
mod tests {
    use super::*;

    /// `codecs()` feeds a log line and, eventually, backend selection. An empty
    /// list would silently mean "this silicon can encode nothing".
    #[test]
    fn every_encoder_lists_only_hardware_codecs() {
        let all = [
            HwEncoder::Nvenc,
            HwEncoder::Amf,
            HwEncoder::Qsv,
            HwEncoder::VideoToolbox,
            HwEncoder::Vaapi,
        ];
        for hw in all {
            let codecs = hw.codecs();
            assert!(!codecs.is_empty(), "{hw:?} advertises no codecs");
            for c in codecs {
                assert!(
                    matches!(c, VideoCodec::Av1 | VideoCodec::H264 | VideoCodec::H265),
                    "{hw:?} lists {c:?}, which is a software codec in this project"
                );
            }
            assert!(!hw.name().is_empty());
        }
    }

    /// H.264 is the floor: every backend here predates the codecs above it, so
    /// a list missing it means the entry was mistyped.
    #[test]
    fn every_encoder_supports_h264() {
        for hw in [
            HwEncoder::Nvenc,
            HwEncoder::Amf,
            HwEncoder::Qsv,
            HwEncoder::VideoToolbox,
            HwEncoder::Vaapi,
        ] {
            assert!(hw.codecs().contains(&VideoCodec::H264), "{hw:?}");
        }
    }

    /// Duplicates would double-log and, once selection exists, make ordering
    /// ambiguous. Also pins that probe never panics on this platform.
    #[test]
    fn probe_returns_no_duplicates() {
        let found = probe();
        for (i, a) in found.iter().enumerate() {
            assert!(
                !found[i + 1..].contains(a),
                "probe() reported {a:?} more than once"
            );
        }
    }
}
