//! BBR-style congestion controller for adaptive bitrate.
//!
//! Inspired by:
//!   - Parsec's BUD: predict congestion BEFORE packet loss occurs
//!   - Google BBR: model the bottleneck bandwidth and propagation delay
//!
//! The classic AIMD ("AIMDController") below reacts to loss after it happens.
//! BBR tracks the minimum RTT (best path) and the maximum delivery rate, and
//! only saturates when delivery rate stops growing — which signals the
//! bottleneck queue is filling up before the queue overflows into loss.
//!
//! For remote desktop the inputs are noisier than for raw data transfer
//! (frame-rate jitter, encoder bitrate variation), so we smooth aggressively.

use miru_common::message::QosUpdate;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Window for tracking minimum RTT (10s rolling window).
const RTT_WINDOW: Duration = Duration::from_secs(10);
/// Window for tracking maximum delivery rate.
const BW_WINDOW: Duration = Duration::from_secs(10);
/// How often we adjust bitrate (ProbeBW phase pacing).
const ADJUST_INTERVAL: Duration = Duration::from_millis(200);

/// BBR-style controller.
pub struct BbrQos {
    /// Best (minimum) RTT we've seen recently — proxy for propagation delay.
    rtt_min_us: u32,
    /// Max delivery rate we've seen recently — proxy for bottleneck bandwidth.
    bw_max_kbps: u32,

    /// Sliding-window samples (timestamp, value).
    rtt_samples: VecDeque<(Instant, u32)>,
    bw_samples: VecDeque<(Instant, u32)>,

    /// Current send rate.
    cur_bitrate_kbps: u32,
    cur_fps: u8,

    /// Phase of the BBR cycle (Startup → Drain → ProbeBW → ProbeRTT).
    phase: Phase,
    last_adjust: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Exponential growth until first BW plateau.
    Startup,
    /// Drain queue formed during startup.
    Drain,
    /// Cycle gain factors around bottleneck rate.
    ProbeBw { gain_idx: u8 },
    /// Periodically probe minimum RTT (drain queue, observe true RTT).
    ProbeRtt,
}

impl Phase {
    fn pacing_gain(self) -> f32 {
        match self {
            Phase::Startup => 2.89,
            Phase::Drain => 1.0 / 2.89,
            Phase::ProbeBw { gain_idx } => {
                // 8-step cycle: 1.25, 0.75, 1, 1, 1, 1, 1, 1
                const CYCLE: [f32; 8] = [1.25, 0.75, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
                CYCLE[gain_idx as usize % CYCLE.len()]
            }
            Phase::ProbeRtt => 1.0,
        }
    }
}

impl BbrQos {
    pub fn new(initial_fps: u8, initial_kbps: u32) -> Self {
        Self {
            rtt_min_us: u32::MAX,
            bw_max_kbps: 1,
            rtt_samples: VecDeque::new(),
            bw_samples: VecDeque::new(),
            cur_bitrate_kbps: initial_kbps,
            cur_fps: initial_fps,
            phase: Phase::Startup,
            last_adjust: Instant::now(),
        }
    }

    /// Feed a new RTT sample (microseconds).
    pub fn on_rtt(&mut self, rtt_us: u32) {
        let now = Instant::now();
        self.rtt_samples.push_back((now, rtt_us));
        // Evict old
        while self
            .rtt_samples
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > RTT_WINDOW)
        {
            self.rtt_samples.pop_front();
        }
        self.rtt_min_us = self
            .rtt_samples
            .iter()
            .map(|(_, v)| *v)
            .min()
            .unwrap_or(rtt_us);
    }

    /// Feed an observed delivery rate (bytes acknowledged / interval).
    pub fn on_delivery(&mut self, kbps: u32) {
        let now = Instant::now();
        self.bw_samples.push_back((now, kbps));
        while self
            .bw_samples
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > BW_WINDOW)
        {
            self.bw_samples.pop_front();
        }
        self.bw_max_kbps = self
            .bw_samples
            .iter()
            .map(|(_, v)| *v)
            .max()
            .unwrap_or(kbps)
            .max(1);
    }

    /// Run a control step. Returns a QoS update if anything changed.
    pub fn tick(&mut self) -> Option<QosUpdate> {
        let now = Instant::now();
        if now.duration_since(self.last_adjust) < ADJUST_INTERVAL {
            return None;
        }
        self.last_adjust = now;

        // Phase transitions
        self.advance_phase();

        // Target bitrate = pacing_gain × bottleneck bandwidth
        let target = (self.bw_max_kbps as f32 * self.phase.pacing_gain()) as u32;
        let target = target.clamp(500, 50_000); // 0.5–50 Mbps cap

        // FPS adjusts gently — drop FPS first (drops bitrate need too) on high RTT
        let target_fps = if self.rtt_min_us > 100_000 {
            self.cur_fps.saturating_sub(5).max(15)
        } else if self.rtt_min_us < 30_000 && self.cur_fps < 60 {
            (self.cur_fps + 5).min(60)
        } else {
            self.cur_fps
        };

        let changed = target != self.cur_bitrate_kbps || target_fps != self.cur_fps;
        self.cur_bitrate_kbps = target;
        self.cur_fps = target_fps;

        if changed {
            Some(QosUpdate {
                fps: self.cur_fps,
                bitrate_kbps: self.cur_bitrate_kbps,
                quality: self.quality_estimate(),
            })
        } else {
            None
        }
    }

    fn advance_phase(&mut self) {
        match self.phase {
            Phase::Startup => {
                // Exit when BW growth plateaus (last 3 samples within 10%)
                if let (Some(&(_, last)), Some(&(_, third_last))) =
                    (self.bw_samples.back(), self.bw_samples.iter().rev().nth(2))
                {
                    if (last as f32) < third_last as f32 * 1.1 {
                        self.phase = Phase::Drain;
                    }
                }
            }
            Phase::Drain => {
                self.phase = Phase::ProbeBw { gain_idx: 0 };
            }
            Phase::ProbeBw { gain_idx } => {
                // Periodically dip to ProbeRtt to refresh rtt_min.
                // No samples at all also counts as "stale".
                let window_stale = match (self.rtt_samples.front(), self.rtt_samples.back()) {
                    (Some((first, _)), Some((last, _))) => last.duration_since(*first) > RTT_WINDOW,
                    _ => true,
                };
                if window_stale {
                    self.phase = Phase::ProbeRtt;
                } else {
                    self.phase = Phase::ProbeBw {
                        gain_idx: gain_idx.wrapping_add(1),
                    };
                }
            }
            Phase::ProbeRtt => {
                self.phase = Phase::ProbeBw { gain_idx: 0 };
            }
        }
    }

    fn quality_estimate(&self) -> u8 {
        // Heuristic: high BW + low RTT → high quality
        let bw_score = (self.cur_bitrate_kbps as f32 / 100.0).min(50.0);
        let rtt_penalty = (self.rtt_min_us as f32 / 1000.0 / 10.0).min(50.0);
        (50.0 + bw_score - rtt_penalty).clamp(20.0, 95.0) as u8
    }

    pub fn fps(&self) -> u8 {
        self.cur_fps
    }
    pub fn bitrate_kbps(&self) -> u32 {
        self.cur_bitrate_kbps
    }
    #[allow(dead_code)]
    pub fn rtt_min_ms(&self) -> u32 {
        self.rtt_min_us / 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_grows_bandwidth() {
        let mut bbr = BbrQos::new(60, 2000);
        for _ in 0..5 {
            bbr.on_delivery(5000);
            bbr.on_rtt(20_000);
            std::thread::sleep(Duration::from_millis(210));
            bbr.tick();
        }
        // Should have explored higher bitrates than initial 2000
        assert!(bbr.bitrate_kbps() >= 2000);
    }

    #[test]
    fn high_rtt_reduces_fps() {
        let mut bbr = BbrQos::new(60, 5000);
        for _ in 0..3 {
            bbr.on_rtt(150_000); // 150ms
            bbr.on_delivery(5000);
            std::thread::sleep(Duration::from_millis(210));
            bbr.tick();
        }
        assert!(bbr.fps() < 60);
    }
}
