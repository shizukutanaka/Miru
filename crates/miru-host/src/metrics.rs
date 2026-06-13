//! Session metadata accumulator.
//!
//! Tracks the values that go into a transparency commitment, updated as
//! the session progresses. At session end, both peers exchange and sign
//! the resulting `SessionMetadata`.

use miru_transparency::SessionMetadata;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;
use uuid::Uuid;

pub struct SessionMetrics {
    started_at: u64,
    session_id: Uuid,
    host_pubkey_b64: String,
    viewer_pubkey_b64: String,
    codec: String,
    video_frames: AtomicU64,
    total_bytes: AtomicU64,
    relayed: AtomicBool,
}

impl SessionMetrics {
    pub fn new(
        session_id: Uuid,
        host_pubkey_b64: String,
        viewer_pubkey_b64: String,
        codec: String,
        relayed: bool,
    ) -> Self {
        let started_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            started_at,
            session_id,
            host_pubkey_b64,
            viewer_pubkey_b64,
            codec,
            video_frames: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            relayed: AtomicBool::new(relayed),
        }
    }

    pub fn on_video_frame(&self, encoded_bytes: u64) {
        self.video_frames.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_add(encoded_bytes, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn on_audio_frame(&self, encoded_bytes: u64) {
        self.total_bytes.fetch_add(encoded_bytes, Ordering::Relaxed);
    }

    /// Convert to a SessionMetadata for transparency commitment.
    pub fn snapshot(&self) -> SessionMetadata {
        let ended_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        SessionMetadata {
            session_id: self.session_id,
            host_pubkey_b64: self.host_pubkey_b64.clone(),
            viewer_pubkey_b64: self.viewer_pubkey_b64.clone(),
            codec: self.codec.clone(),
            started_at: self.started_at,
            ended_at,
            video_frames: self.video_frames.load(Ordering::Relaxed),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            relayed: self.relayed.load(Ordering::Relaxed),
        }
    }
}
