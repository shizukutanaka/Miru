//! Frame backpressure.
//!
//! Inspired by RustDesk's VideoFrameController.
//!
//! Problem: capture runs at 60fps, encode runs at 60fps, but the network or
//! the viewer's decoder may be slower. Without backpressure we'd buffer
//! frames forever, growing memory and adding latency.
//!
//! Strategy:
//!   - Track in-flight frames (sent but not ACK'd via heartbeat)
//!   - If in-flight >= MAX, skip new frames at capture time (cheapest)
//!   - On significant lag, request a keyframe and reset
//!
//! Skip-at-capture is critical: encoding a frame we'll throw away wastes
//! GPU cycles (5-50ms each). Skip-at-encoder is second-best. Skip-at-network
//! is worst (we already paid encode cost).

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Maximum number of in-flight frames before backpressure kicks in.
/// Tuned for ~1 RTT @ 60fps + small buffer = 3-5 frames.
const MAX_IN_FLIGHT: i32 = 3;
/// Frames to skip during sustained lag (panic mode).
#[allow(dead_code)]
const PANIC_THRESHOLD: i32 = 8;

pub struct FrameController {
    /// Frames sent minus frames ACK'd. Negative is impossible but we use i32 for safety.
    in_flight: AtomicI32,
    /// Total frames captured (for diagnostics).
    captured: AtomicU64,
    /// Total frames skipped due to backpressure.
    skipped: AtomicU64,
    /// Total frames sent.
    sent: AtomicU64,
    /// Last keyframe-request timestamp (so we don't spam).
    last_keyframe_request: Mutex<Option<Instant>>,
    /// BBR-controlled target FPS — written by session loop, read by capture thread.
    /// 0 means "use initial value" (not yet set by BBR).
    pub target_fps: AtomicU8,
    /// BBR-controlled target bitrate (kbps) — written by session loop, read by capture thread.
    /// 0 means "use initial value".
    pub target_bitrate_kbps: AtomicU32,
}

impl FrameController {
    pub fn new(initial_fps: u8, initial_bitrate_kbps: u32) -> Self {
        Self {
            in_flight: AtomicI32::new(0),
            captured: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            sent: AtomicU64::new(0),
            last_keyframe_request: Mutex::new(None),
            target_fps: AtomicU8::new(initial_fps),
            target_bitrate_kbps: AtomicU32::new(initial_bitrate_kbps),
        }
    }

    /// Update QoS parameters from BBR tick. Called from the session loop;
    /// the capture thread picks these up on the next frame iteration.
    pub fn apply_qos(&self, fps: u8, bitrate_kbps: u32) {
        let old_fps = self.target_fps.swap(fps, Ordering::Relaxed);
        let old_br = self.target_bitrate_kbps.swap(bitrate_kbps, Ordering::Relaxed);
        if old_fps != fps || old_br != bitrate_kbps {
            info!(
                "QoS applied to capture: {}fps {}kbps → {}fps {}kbps",
                old_fps, old_br, fps, bitrate_kbps
            );
        }
    }

    /// Should we capture this frame? Returns false if we should skip
    /// (i.e. the viewer hasn't kept up).
    pub fn should_capture(&self) -> bool {
        self.captured.fetch_add(1, Ordering::Relaxed);
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        if in_flight >= MAX_IN_FLIGHT {
            self.skipped.fetch_add(1, Ordering::Relaxed);
            debug!("backpressure: skip (in_flight={})", in_flight);
            return false;
        }
        true
    }

    /// Called when a frame goes onto the wire.
    pub fn on_send(&self) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Called when the client ACKs a frame (via Pong with matching seq, or
    /// via a periodic heartbeat reporting received-frame count).
    pub fn on_ack(&self, n: u32) {
        self.in_flight.fetch_sub(n as i32, Ordering::Relaxed);
        // Clamp to >= 0 (defensive)
        let cur = self.in_flight.load(Ordering::Relaxed);
        if cur < 0 {
            self.in_flight.store(0, Ordering::Relaxed);
        }
    }

    /// Returns true if the controller wants a keyframe (to recover from a stall).
    /// Cooldown of 1 second to avoid spam.
    #[allow(dead_code)]
    pub fn want_keyframe(&self) -> bool {
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        if in_flight < PANIC_THRESHOLD {
            return false;
        }
        let mut last = self
            .last_keyframe_request
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        match *last {
            Some(t) if now.duration_since(t) < Duration::from_secs(1) => false,
            _ => {
                *last = Some(now);
                warn!(
                    "backpressure: requesting keyframe (in_flight={})",
                    in_flight
                );
                // Reset in_flight assuming the keyframe will resync
                self.in_flight.store(MAX_IN_FLIGHT, Ordering::Relaxed);
                true
            }
        }
    }

    #[allow(dead_code)]
    pub fn stats(&self) -> FrameStats {
        FrameStats {
            captured: self.captured.load(Ordering::Relaxed),
            sent: self.sent.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            in_flight: self.in_flight.load(Ordering::Relaxed).max(0) as u32,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct FrameStats {
    pub captured: u64,
    pub sent: u64,
    pub skipped: u64,
    pub in_flight: u32,
}

impl FrameStats {
    #[allow(dead_code)]
    pub fn skip_rate(&self) -> f32 {
        if self.captured == 0 {
            return 0.0;
        }
        self.skipped as f32 / self.captured as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_skip_when_keeping_up() {
        let c = FrameController::new(30, 2000);
        for _ in 0..10 {
            assert!(c.should_capture());
            c.on_send();
            c.on_ack(1);
        }
        assert_eq!(c.stats().skipped, 0);
        assert_eq!(c.stats().sent, 10);
    }

    #[test]
    fn skip_when_lagging() {
        let c = FrameController::new(30, 2000);
        // Fill in-flight
        for _ in 0..MAX_IN_FLIGHT {
            assert!(c.should_capture());
            c.on_send();
        }
        // Now we should skip
        assert!(!c.should_capture());
        assert!(!c.should_capture());
        assert!(c.stats().skipped >= 2);
    }

    #[test]
    fn keyframe_request_cooldown() {
        let c = FrameController::new(30, 2000);
        for _ in 0..10 {
            c.on_send();
        }
        assert!(c.want_keyframe());
        // Immediate second call within cooldown
        for _ in 0..5 {
            c.on_send();
        }
        assert!(!c.want_keyframe());
    }
}
