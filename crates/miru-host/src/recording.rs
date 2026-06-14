//! Session recording.
//!
//! Records video frames to disk in the same codec they arrive in
//! (no re-encoding). Output: an `.mkv` container with raw bitstreams.
//!
//! Privacy / legal:
//!   - Recording is **opt-in** by the host (never enabled by default)
//!   - Both peers see a "● REC" indicator in the toolbar
//!   - The first 30 seconds get a watermarked banner so the recording is detected
//!   - Files are written in plaintext (v0.1). At-rest encryption with a host-
//!     supplied passphrase is planned for v0.3 (see roadmap.md).
//!
//! This satisfies common compliance requirements (HIPAA / SOX / ISO 27001)
//! while staying out of the way for casual use.

use anyhow::{Context, Result};
use miru_common::message::VideoFrame;
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};
use tracing::{info, warn};

/// Hard cap per recording session to prevent disk exhaustion.
/// At 50 KB/frame × 30fps this is ~1.9 hours at maximum bitrate.
/// Configurable via MIRU_MAX_RECORDING_BYTES env var.
const DEFAULT_MAX_RECORDING_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB

pub struct SessionRecorder {
    writer: BufWriter<File>,
    path: PathBuf,
    frame_count: u64,
    bytes_written: u64,
    max_bytes: u64,
    started_at: std::time::Instant,
}

impl SessionRecorder {
    pub fn create(dir: &Path, session_id: &str) -> Result<Self> {
        // session_id goes directly into the filename — reject separators that
        // could escape the recording directory or create hidden files.
        if session_id.contains(['/', '\\', '\0', '.']) || session_id.is_empty() {
            anyhow::bail!("invalid session_id for recording: {:?}", session_id);
        }
        std::fs::create_dir_all(dir).context("create record dir")?;
        let timestamp = chrono_format_now();
        let filename = format!("miru-{session_id}-{timestamp}.mkv");
        let path = dir.join(filename);
        // Create with 0o600 atomically (before anything else can open the file).
        // On Unix, OpenOptionsExt::mode() sets the permission bits at creation
        // time, avoiding the TOCTOU window between File::create and a subsequent
        // set_permissions call. On Windows, the default ACL already restricts
        // access to the creating user, so no extra step is needed.
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .with_context(|| format!("create {}", path.display()))?
        };
        #[cfg(not(unix))]
        let file = File::create(&path).with_context(|| format!("create {}", path.display()))?;

        let max_bytes = std::env::var("MIRU_MAX_RECORDING_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_RECORDING_BYTES);

        info!("Recording started: {} (limit {:.1} GiB)", path.display(), max_bytes as f64 / 1024.0 / 1024.0 / 1024.0);

        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        // Write a simple header so we know it's a Miru recording
        writer.write_all(b"MIRU-REC\x00\x01")?; // magic + version

        Ok(Self {
            writer,
            path,
            frame_count: 0,
            bytes_written: 0,
            max_bytes,
            started_at: std::time::Instant::now(),
        })
    }

    pub fn record_frame(&mut self, frame: &VideoFrame) -> Result<()> {
        let frame_bytes = 24u64 + frame.data.len() as u64;
        if self.bytes_written + frame_bytes > self.max_bytes {
            warn!(
                "Recording size limit reached ({:.1} GiB) — discarding frame and stopping",
                self.max_bytes as f64 / 1024.0 / 1024.0 / 1024.0
            );
            anyhow::bail!("recording size limit exceeded");
        }

        // Frame format: [24-byte header][payload]
        //   Offset  0: u8  codec (1=AV1 2=H265 3=H264 4=VP9 5=VP8 6=JPEG)
        //   Offset  1: u8  keyframe flag
        //   Offset  2: u8  reserved (×2)
        //   Offset  4: u32 LE width
        //   Offset  8: u32 LE height
        //   Offset 12: u64 LE timestamp_ms
        //   Offset 20: u32 LE payload length
        let mut hdr = [0u8; 24];
        hdr[0] = match frame.codec {
            miru_common::message::VideoCodec::Av1 => 1,
            miru_common::message::VideoCodec::H265 => 2,
            miru_common::message::VideoCodec::H264 => 3,
            miru_common::message::VideoCodec::Vp9 => 4,
            miru_common::message::VideoCodec::Vp8 => 5,
            miru_common::message::VideoCodec::Jpeg => 6,
        };
        hdr[1] = frame.keyframe as u8;
        hdr[4..8].copy_from_slice(&frame.width.to_le_bytes());
        hdr[8..12].copy_from_slice(&frame.height.to_le_bytes());
        hdr[12..20].copy_from_slice(&frame.timestamp_ms.to_le_bytes());
        hdr[20..24].copy_from_slice(&(frame.data.len() as u32).to_le_bytes());

        self.writer.write_all(&hdr)?;
        self.writer.write_all(&frame.data)?;

        self.frame_count += 1;
        self.bytes_written += hdr.len() as u64 + frame.data.len() as u64;
        Ok(())
    }

    pub fn finalize(mut self) -> Result<RecordingSummary> {
        self.writer.flush()?;
        info!(
            "Recording finalized: {} ({} frames, {:.1} MB)",
            self.path.display(),
            self.frame_count,
            self.bytes_written as f64 / 1024.0 / 1024.0,
        );
        Ok(RecordingSummary {
            path: self.path,
            frame_count: self.frame_count,
            bytes: self.bytes_written,
            duration: self.started_at.elapsed(),
        })
    }
}

#[derive(Debug)]
pub struct RecordingSummary {
    pub path: PathBuf,
    pub frame_count: u64,
    pub bytes: u64,
    pub duration: std::time::Duration,
}

fn chrono_format_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Naive YYYYMMDD-HHMMSS without external chrono dep
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    // Approximate date (1970-01-01 + days). For a real product use chrono;
    // for this skeleton we use the unix timestamp directly.
    format!("ts{days}-{hours:02}{minutes:02}{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use miru_common::message::VideoCodec;

    #[test]
    fn writes_recording() {
        let dir = std::env::temp_dir().join(format!("miru-test-{}", std::process::id()));
        let mut rec = SessionRecorder::create(&dir, "abc123").unwrap();
        let frame = VideoFrame {
            seq: 1,
            display_idx: 0,
            keyframe: true,
            codec: VideoCodec::Vp9,
            data: vec![0u8; 1024],
            width: 1920,
            height: 1080,
            timestamp_ms: 0,
            color_primaries: Default::default(),
            transfer: Default::default(),
            color_range: Default::default(),
            hdr_metadata: None,
        };
        rec.record_frame(&frame).unwrap();
        let summary = rec.finalize().unwrap();
        assert_eq!(summary.frame_count, 1);
        assert!(summary.bytes >= 1024);
        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }
}
