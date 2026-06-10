//! 5-stage performance instrumentation.
//!
//! Inspired by Sunshine's performance overlay:
//!   1. Capture latency  — frame ready → buffer captured
//!   2. Encode latency   — buffer → compressed packet
//!   3. Network latency  — packet sent → ACK received (RTT/2)
//!   4. Decode latency   — packet received → frame decoded (viewer side)
//!   5. Display latency  — decoded frame → on-screen (viewer side)
//!
//! Total latency = sum of all five. Knowing the breakdown tells the user
//! which stage to attack: GPU upgrade, network change, or decoder switch.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Atomic latency counter with EWMA smoothing.
pub struct LatencyMeter {
    /// Smoothed value in microseconds.
    smoothed_us: AtomicU32,
    /// Last raw sample.
    last_us: AtomicU32,
    /// Sample count.
    samples: AtomicU64,
    /// Smoothing factor in 1/1024 units (alpha = 1/8 default → fast response).
    alpha_1024: u32,
}

impl Default for LatencyMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyMeter {
    pub const fn new() -> Self {
        Self {
            smoothed_us: AtomicU32::new(0),
            last_us: AtomicU32::new(0),
            samples: AtomicU64::new(0),
            alpha_1024: 128, // 1/8
        }
    }

    pub fn record(&self, dur: Duration) {
        let us = dur.as_micros().min(u32::MAX as u128) as u32;
        self.last_us.store(us, Ordering::Relaxed);

        // EWMA: smoothed = (1-α) × smoothed + α × sample
        let prev = self.smoothed_us.load(Ordering::Relaxed);
        let new = if prev == 0 {
            us
        } else {
            let a = self.alpha_1024;
            (((1024 - a) as u64 * prev as u64 + a as u64 * us as u64) / 1024) as u32
        };
        self.smoothed_us.store(new, Ordering::Relaxed);
        self.samples.fetch_add(1, Ordering::Relaxed);
    }

    pub fn smoothed_ms(&self) -> f32 {
        self.smoothed_us.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn last_ms(&self) -> f32 {
        self.last_us.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }
}

/// All five latency meters in one struct, shared via Arc.
pub struct PerfMeters {
    pub capture: LatencyMeter,
    pub encode: LatencyMeter,
    pub network: LatencyMeter,
    pub decode: LatencyMeter,
    pub display: LatencyMeter,
}

impl Default for PerfMeters {
    fn default() -> Self {
        Self::new()
    }
}

impl PerfMeters {
    pub const fn new() -> Self {
        Self {
            capture: LatencyMeter::new(),
            encode: LatencyMeter::new(),
            network: LatencyMeter::new(),
            decode: LatencyMeter::new(),
            display: LatencyMeter::new(),
        }
    }

    pub fn snapshot(&self) -> PerfSnapshot {
        PerfSnapshot {
            capture_ms: self.capture.smoothed_ms(),
            encode_ms: self.encode.smoothed_ms(),
            network_ms: self.network.smoothed_ms(),
            decode_ms: self.decode.smoothed_ms(),
            display_ms: self.display.smoothed_ms(),
            total_ms: self.capture.smoothed_ms()
                + self.encode.smoothed_ms()
                + self.network.smoothed_ms()
                + self.decode.smoothed_ms()
                + self.display.smoothed_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfSnapshot {
    pub capture_ms: f32,
    pub encode_ms: f32,
    pub network_ms: f32,
    pub decode_ms: f32,
    pub display_ms: f32,
    pub total_ms: f32,
}

/// RAII timer — records elapsed on drop.
pub struct StageTimer<'a> {
    meter: &'a LatencyMeter,
    start: Instant,
}

impl<'a> StageTimer<'a> {
    pub fn start(meter: &'a LatencyMeter) -> Self {
        Self { meter, start: Instant::now() }
    }
}

impl<'a> Drop for StageTimer<'a> {
    fn drop(&mut self) {
        self.meter.record(self.start.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_smooths_samples() {
        let m = LatencyMeter::new();
        m.record(Duration::from_millis(10));
        m.record(Duration::from_millis(10));
        assert!((m.smoothed_ms() - 10.0).abs() < 0.5);

        // Spike samples — smoothed should not jump fully
        m.record(Duration::from_millis(50));
        let after_spike = m.smoothed_ms();
        assert!(after_spike < 30.0, "got {}", after_spike); // damped
    }

    #[test]
    fn stage_timer_records_on_drop() {
        let m = LatencyMeter::new();
        {
            let _t = StageTimer::start(&m);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(m.last_ms() >= 5.0);
        assert_eq!(m.samples(), 1);
    }

    #[test]
    fn perf_snapshot_sums() {
        let p = PerfMeters::new();
        p.capture.record(Duration::from_millis(2));
        p.encode.record(Duration::from_millis(8));
        p.network.record(Duration::from_millis(15));
        p.decode.record(Duration::from_millis(3));
        p.display.record(Duration::from_millis(5));
        let snap = p.snapshot();
        assert!((snap.total_ms - 33.0).abs() < 1.0, "{:?}", snap);
    }
}
