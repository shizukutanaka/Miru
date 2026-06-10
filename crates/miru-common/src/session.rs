use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceId {
    pub fn new() -> Self {
        // Format: XXXX-XXXX (8 digits, easy to read aloud)
        let id = uuid::Uuid::new_v4().simple().to_string();
        let s = id.chars().take(8).collect::<String>().to_uppercase();
        Self(format!("{}-{}", &s[..4], &s[4..]))
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Runtime session statistics.
#[derive(Debug, Default)]
pub struct SessionStats {
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub rtt_ms: u32,
    pub fps_actual: f32,
    pub bitrate_kbps: u32,
    pub last_keyframe: Option<Instant>,
}

impl SessionStats {
    pub fn packet_loss_pct(&self) -> f32 {
        let total = self.frames_sent + self.frames_dropped;
        if total == 0 {
            return 0.0;
        }
        self.frames_dropped as f32 / total as f32 * 100.0
    }
}
