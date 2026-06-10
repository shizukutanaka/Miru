//! QoS controller — adapts fps/bitrate to network conditions.
//!
//! Algorithm (simple AIMD):
//!   RTT > 200ms  → quality step down (fps ÷2, bitrate ×0.7)
//!   RTT > 100ms  → fps cap 30
//!   loss > 5%    → bitrate ×0.8
//!   stable 3s    → bitrate ×1.1 (additive increase, capped at max)

use miru_common::message::QosUpdate;
use std::time::{Duration, Instant};
use tracing::info;

pub struct QosController {
    fps: u8,
    bitrate_kbps: u32,
    quality: u8, // 0–100
    last_adjust: Instant,
    stable_streak: u32, // consecutive stable measurements
}

impl QosController {
    pub fn new(fps: u8, bitrate_kbps: u32) -> Self {
        Self {
            fps,
            bitrate_kbps,
            quality: 80,
            last_adjust: Instant::now(),
            stable_streak: 0,
        }
    }

    pub fn update(&mut self, rtt_ms: u32, packet_loss_pct: f32) -> Option<QosUpdate> {
        if self.last_adjust.elapsed() < Duration::from_secs(1) {
            return None; // cooldown
        }
        self.last_adjust = Instant::now();

        let prev_fps = self.fps;
        let prev_bitrate = self.bitrate_kbps;

        if rtt_ms > 200 || packet_loss_pct > 10.0 {
            // Severe degradation
            self.fps = (self.fps / 2).max(5);
            self.bitrate_kbps = (self.bitrate_kbps as f32 * 0.6) as u32;
            self.quality = (self.quality - 10).max(30);
            self.stable_streak = 0;
        } else if rtt_ms > 100 || packet_loss_pct > 5.0 {
            // Moderate degradation
            self.fps = self.fps.min(30);
            self.bitrate_kbps = (self.bitrate_kbps as f32 * 0.8) as u32;
            self.quality = (self.quality - 5).max(50);
            self.stable_streak = 0;
        } else {
            // Good conditions — additive increase
            self.stable_streak += 1;
            if self.stable_streak >= 3 {
                self.fps = (self.fps + 5).min(60);
                self.bitrate_kbps = (self.bitrate_kbps as f32 * 1.1) as u32;
                self.quality = (self.quality + 5).min(100);
            }
        }

        // Clamp
        self.bitrate_kbps = self.bitrate_kbps.clamp(200, 50_000);

        if self.fps != prev_fps || self.bitrate_kbps != prev_bitrate {
            info!(
                "QoS: rtt={}ms loss={:.1}% → fps={} bitrate={}kbps q={}",
                rtt_ms, packet_loss_pct, self.fps, self.bitrate_kbps, self.quality
            );
            Some(QosUpdate {
                fps: self.fps,
                bitrate_kbps: self.bitrate_kbps,
                quality: self.quality,
            })
        } else {
            None
        }
    }

    pub fn fps(&self) -> u8 {
        self.fps
    }
    pub fn bitrate_kbps(&self) -> u32 {
        self.bitrate_kbps
    }
    #[allow(dead_code)]
    pub fn quality(&self) -> u8 {
        self.quality
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrades_under_high_rtt() {
        let mut qos = QosController::new(60, 5000);
        // Force cooldown bypass
        qos.last_adjust = Instant::now() - Duration::from_secs(2);
        let update = qos.update(250, 0.0);
        assert!(update.is_some());
        assert!(qos.fps < 60);
        assert!(qos.bitrate_kbps < 5000);
    }

    #[test]
    fn recovers_under_good_conditions() {
        let mut qos = QosController::new(30, 2000);
        for _ in 0..4 {
            qos.last_adjust = Instant::now() - Duration::from_secs(2);
            qos.update(50, 0.0);
        }
        assert!(qos.fps > 30 || qos.bitrate_kbps > 2000);
    }
}
