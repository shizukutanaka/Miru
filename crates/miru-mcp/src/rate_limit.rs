//! Per-capability rate limiter for the MCP server.
//!
//! Per the security playbook (§3 MCP server hardening): "Per-tool rate limits.
//! Use separate buckets per capability: e.g. `ScreenRead 10/s`, `PointerClick
//! 30/s`, `ShellExec 1/min` with a hard daily cap. Exceeding the bucket
//! triggers a 429-equivalent error and an audit-log `RateLimited` entry."
//!
//! This is a simple token-bucket implementation, sized to keep miru-mcp
//! dependency-light. For a richer design (multi-tier, leaky-bucket) consider
//! the `governor` crate at v1.0; for v0.1 the small explicit policy here is
//! easier to audit.

use miru_agent::token::Capability;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Token-bucket policy per capability.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Maximum tokens in the bucket.
    pub burst: u32,
    /// Refill interval — one token added every `refill_every`.
    pub refill_every: Duration,
    /// Hard 24-hour cap. Once reached, the capability is denied for the
    /// remainder of the day regardless of bucket state.
    pub daily_max: u32,
}

impl Policy {
    /// Default policy table per the security playbook recommendation.
    /// Bucket sizes tuned to permit normal interactive use while throttling
    /// runaway agent loops (the canonical "rapid clicker" failure mode).
    pub fn default_for(cap: Capability) -> Self {
        match cap {
            // Cheap reads — generous limits.
            Capability::ScreenRead => Self::per_second(10, 1000),
            Capability::ClipboardRead => Self::per_second(5, 500),
            Capability::FileRead => Self::per_second(5, 500),

            // Pointer / typing — bounded but interactive-feel.
            Capability::PointerMove => Self::per_second(60, 10000),
            Capability::PointerClick => Self::per_second(10, 1000),
            Capability::KeyType => Self::per_second(20, 5000),
            Capability::KeyCombo => Self::per_second(10, 1000),

            // Destructive — strict.
            Capability::ClipboardWrite => Self::per_second(2, 200),
            Capability::FileWrite => Self::per_minute(10, 100),
            Capability::OpenUrl => Self::per_minute(20, 200),
            Capability::ShellExec => Self::per_minute(1, 30),
        }
    }

    fn per_second(burst: u32, daily_max: u32) -> Self {
        let b = burst.max(1) as u64;
        // Ceiling division so the true refill rate never exceeds the
        // configured burst/sec (floor division would round the interval
        // down, letting the bucket refill slightly faster than intended).
        Self {
            burst,
            refill_every: Duration::from_millis((1000 + b - 1) / b),
            daily_max,
        }
    }

    fn per_minute(burst: u32, daily_max: u32) -> Self {
        let b = burst.max(1) as u64;
        Self {
            burst,
            refill_every: Duration::from_secs((60 + b - 1) / b),
            daily_max,
        }
    }
}

/// Per-capability bucket state.
struct Bucket {
    policy: Policy,
    tokens: u32,
    last_refill: Instant,
    /// Total successful consumptions since `day_start`.
    used_today: u32,
    day_start: Instant,
}

impl Bucket {
    fn new(policy: Policy) -> Self {
        Self {
            tokens: policy.burst,
            last_refill: Instant::now(),
            used_today: 0,
            day_start: Instant::now(),
            policy,
        }
    }

    fn try_consume(&mut self) -> Result<(), RateLimitError> {
        let now = Instant::now();

        // Daily cap rollover (24h sliding window).
        if now.duration_since(self.day_start) >= Duration::from_secs(24 * 3600) {
            self.day_start = now;
            self.used_today = 0;
        }
        if self.used_today >= self.policy.daily_max {
            return Err(RateLimitError::DailyCapReached {
                cap_max: self.policy.daily_max,
                resets_at: self.day_start + Duration::from_secs(24 * 3600),
            });
        }

        // Refill tokens that accrued since last attempt.
        // Advance last_refill by exactly (refill_every * refilled), not by the full
        // elapsed time — this carries the sub-token remainder forward so it counts
        // toward the next refill rather than being silently discarded.
        let elapsed = now.duration_since(self.last_refill);
        let refilled_u128 = elapsed.as_nanos() / self.policy.refill_every.as_nanos().max(1);
        // Clamp before the u128 → u32 cast: an extreme clock jump (e.g. system
        // suspend/resume) could otherwise wrap silently instead of just
        // saturating the bucket at its burst size on the next line.
        let refilled = refilled_u128.min(u32::MAX as u128) as u32;
        if refilled > 0 {
            self.tokens = (self.tokens + refilled).min(self.policy.burst);
            self.last_refill += self.policy.refill_every * refilled;
        }

        if self.tokens == 0 {
            let next_token_in = self
                .policy
                .refill_every
                .saturating_sub(now.duration_since(self.last_refill));
            return Err(RateLimitError::BucketEmpty {
                retry_after: next_token_in,
            });
        }

        self.tokens -= 1;
        self.used_today += 1;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RateLimitError {
    #[error("rate limit: bucket empty, retry after {retry_after:?}")]
    BucketEmpty { retry_after: Duration },
    #[error("rate limit: daily cap of {cap_max} reached, resets at {resets_at:?}")]
    DailyCapReached { cap_max: u32, resets_at: Instant },
}

/// Per-capability rate limiter shared across the MCP server's request handlers.
pub struct RateLimiter {
    buckets: Mutex<HashMap<Capability, Bucket>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Try to consume one quota unit for `cap`. Returns Err with retry info
    /// on rate-limit denial; caller MUST emit an audit-log `RateLimited`
    /// entry on Err so denied attempts are forensically visible.
    pub fn try_consume(&self, cap: Capability) -> Result<(), RateLimitError> {
        let mut g = self.buckets.lock();
        let bucket = g
            .entry(cap)
            .or_insert_with(|| Bucket::new(Policy::default_for(cap)));
        bucket.try_consume()
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn shell_exec_limited_to_one_per_minute() {
        let rl = RateLimiter::new();
        // First call OK (burst=1)
        assert!(rl.try_consume(Capability::ShellExec).is_ok());
        // Second call within burst window denied
        assert!(rl.try_consume(Capability::ShellExec).is_err());
    }

    #[test]
    fn screen_read_burst_then_throttle() {
        let rl = RateLimiter::new();
        // First 10 OK (burst=10 for ScreenRead)
        for i in 0..10 {
            assert!(
                rl.try_consume(Capability::ScreenRead).is_ok(),
                "call {i} should succeed"
            );
        }
        // 11th immediately fails
        assert!(rl.try_consume(Capability::ScreenRead).is_err());
        // Wait one refill interval (~100ms for 10/s)
        sleep(Duration::from_millis(120));
        // Should now succeed again
        assert!(rl.try_consume(Capability::ScreenRead).is_ok());
    }

    /// Regression: the old code set last_refill = now after each refill, discarding
    /// the sub-token remainder. Over time this caused the effective refill rate to be
    /// measurably lower than the configured rate (up to ~20% loss).
    #[test]
    fn sub_token_remainder_is_preserved() {
        // Use a policy where the mismatch is easy to observe:
        // burst=3, refill_every=100ms. After exhausting the bucket we wait
        // 250ms. The correct refill is 2 tokens (200ms worth) with 50ms carried
        // forward. A second wait of 60ms should then yield 1 more token (50+60>100).
        // With the buggy code: last_refill = now discards the 50ms, so the second
        // wait of 60ms only adds 0 tokens (60 < 100), leaving the bucket at 2.
        let mut bucket = Bucket::new(Policy {
            burst: 3,
            refill_every: Duration::from_millis(100),
            daily_max: 1000,
        });
        // Drain the bucket.
        for _ in 0..3 {
            assert!(bucket.try_consume().is_ok());
        }
        assert!(bucket.try_consume().is_err());

        // Wait 250ms → should credit 2 tokens (50ms sub-token remainder carried).
        sleep(Duration::from_millis(260));
        assert!(bucket.try_consume().is_ok(), "first refilled token");
        assert!(bucket.try_consume().is_ok(), "second refilled token");
        assert!(bucket.try_consume().is_err(), "third not yet due");

        // Wait 60ms more. With the fix, the 50ms carried + 60ms new = 110ms ≥ 100ms → +1 token.
        sleep(Duration::from_millis(70));
        assert!(bucket.try_consume().is_ok(), "sub-token remainder must carry forward");
    }

    #[test]
    fn caps_are_independent() {
        let rl = RateLimiter::new();
        // Exhaust ShellExec
        let _ = rl.try_consume(Capability::ShellExec);
        // ScreenRead should still work
        assert!(rl.try_consume(Capability::ScreenRead).is_ok());
    }

    #[test]
    fn daily_cap_eventually_blocks() {
        // Use a policy with very low daily_max for fast testing.
        // Inline a small bucket directly since the public Policy::default_for
        // values are sized for production use.
        let mut bucket = Bucket::new(Policy {
            burst: 1000,
            refill_every: Duration::from_micros(1),
            daily_max: 5,
        });
        for _ in 0..5 {
            assert!(bucket.try_consume().is_ok());
        }
        let err = bucket.try_consume().unwrap_err();
        match err {
            RateLimitError::DailyCapReached { cap_max, .. } => assert_eq!(cap_max, 5),
            other => panic!("expected DailyCapReached, got {other:?}"),
        }
    }
}
