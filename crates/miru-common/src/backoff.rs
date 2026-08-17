//! Reconnect backoff with jitter.
//!
//! # Why jitter
//!
//! Both reconnect loops used plain doubling — the host's signal reconnect
//! (`miru-host/src/session.rs`) and the viewer's session retry
//! (`miru-client/src-tauri/src/state.rs`) — with no randomisation at all.
//!
//! Deterministic backoff synchronises clients. In a self-hosted deployment
//! every device points at one `MIRU_SIGNAL` server, so when that server
//! restarts (or a network partition heals) all of them are disconnected at the
//! same instant and then retry at exactly the same offsets: 5s, 10s, 20s, 40s…
//! The retries arrive as spikes rather than spread out, which is precisely the
//! thundering-herd behaviour that can knock the server over again as it comes
//! up. Randomising each client's schedule is what actually decorrelates the
//! herd — see Marc Brooker, "Exponential Backoff And Jitter" (AWS Architecture
//! Blog, 2015), and the Amazon Builders' Library article "Timeouts, retries,
//! and backoff with jitter".
//!
//! # Which jitter
//!
//! That post evaluates three variants. This uses **equal jitter**:
//!
//! ```text
//! temp  = min(cap, base * 2^attempt)
//! sleep = temp/2 + random_between(0, temp/2)
//! ```
//!
//! Full jitter (`random_between(0, temp)`) decorrelates slightly better but can
//! draw a near-zero sleep repeatedly. For a *reconnect* loop against a server
//! that is plainly down, that degenerates into hammering. Equal jitter keeps a
//! guaranteed floor of half the computed delay while still spreading clients
//! across the second half, which is the property this code needs.
//!
//! The pure function takes the random draw as an argument so the schedule can
//! be unit-tested exactly; `next_delay` supplies it from the thread RNG.

use std::time::Duration;

/// Compute the equal-jitter delay in seconds.
///
/// `attempt` is 0-based (0 = the first retry). `rand_unit` must be in `[0, 1)`;
/// values outside that range are clamped so a bad caller cannot produce a
/// negative or over-long sleep.
///
/// Returns a value in `[temp/2, temp]` where `temp = min(cap, base << attempt)`.
pub fn equal_jitter_secs(attempt: u32, base_secs: u64, cap_secs: u64, rand_unit: f64) -> u64 {
    let base = base_secs.max(1);
    let cap = cap_secs.max(base);

    // Saturating shift: `attempt` can grow without bound on a long outage, and
    // `base << 64` is undefined-ish (it panics in debug). Clamp to the cap.
    let doubled = if attempt >= 63 {
        cap
    } else {
        base.saturating_mul(1u64 << attempt).min(cap)
    };

    let half = doubled / 2;
    let unit = if rand_unit.is_finite() {
        rand_unit.clamp(0.0, 1.0)
    } else {
        0.0
    };
    half + (half as f64 * unit) as u64
}

/// Equal-jitter delay for `attempt`, drawing randomness from the thread RNG.
pub fn next_delay(attempt: u32, base_secs: u64, cap_secs: u64) -> Duration {
    Duration::from_secs(equal_jitter_secs(
        attempt,
        base_secs,
        cap_secs,
        rand::random::<f64>(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 5;
    const CAP: u64 = 300;

    /// The floor is what stops a near-zero draw from hammering a downed server.
    #[test]
    fn never_returns_less_than_half_the_computed_delay() {
        for attempt in 0..10 {
            let lo = equal_jitter_secs(attempt, BASE, CAP, 0.0);
            let expected = (BASE.saturating_mul(1 << attempt)).min(CAP);
            assert_eq!(lo, expected / 2, "attempt {attempt}");
        }
    }

    #[test]
    fn never_exceeds_the_computed_delay() {
        for attempt in 0..10 {
            let hi = equal_jitter_secs(attempt, BASE, CAP, 1.0);
            let expected = (BASE.saturating_mul(1 << attempt)).min(CAP);
            assert!(hi <= expected, "attempt {attempt}: {hi} > {expected}");
        }
    }

    #[test]
    fn grows_exponentially_until_the_cap() {
        // Compare the deterministic halves so the assertion is exact.
        let d = |a| equal_jitter_secs(a, BASE, CAP, 0.0);
        assert_eq!(d(0), 2); // 5/2
        assert_eq!(d(1), 5); // 10/2
        assert_eq!(d(2), 10); // 20/2
        assert_eq!(d(3), 20); // 40/2
    }

    #[test]
    fn saturates_at_the_cap() {
        for attempt in [8, 20, 62, 63, 64, u32::MAX] {
            let hi = equal_jitter_secs(attempt, BASE, CAP, 1.0);
            assert!(hi <= CAP, "attempt {attempt} exceeded cap: {hi}");
            assert_eq!(equal_jitter_secs(attempt, BASE, CAP, 0.0), CAP / 2);
        }
    }

    /// The whole point: two clients that failed at the same instant must not
    /// pick the same delay.
    #[test]
    fn different_draws_give_different_delays() {
        let a = equal_jitter_secs(4, BASE, CAP, 0.0);
        let b = equal_jitter_secs(4, BASE, CAP, 0.99);
        assert_ne!(a, b);
        assert!(b > a);
    }

    #[test]
    fn spreads_across_the_upper_half() {
        let temp = (BASE * 8).min(CAP); // attempt 3 → 40
        let mut seen = std::collections::HashSet::new();
        for i in 0..100 {
            seen.insert(equal_jitter_secs(3, BASE, CAP, i as f64 / 100.0));
        }
        // 20..=40 inclusive is 21 distinct values; require a broad spread.
        assert!(seen.len() > 10, "only {} distinct delays", seen.len());
        assert!(seen.iter().all(|&d| d >= temp / 2 && d <= temp));
    }

    #[test]
    fn clamps_out_of_range_or_nan_draws() {
        let lo = equal_jitter_secs(3, BASE, CAP, 0.0);
        let hi = equal_jitter_secs(3, BASE, CAP, 1.0);
        assert_eq!(equal_jitter_secs(3, BASE, CAP, -5.0), lo);
        assert_eq!(equal_jitter_secs(3, BASE, CAP, 5.0), hi);
        assert_eq!(equal_jitter_secs(3, BASE, CAP, f64::NAN), lo);
    }

    #[test]
    fn degenerate_parameters_do_not_panic_or_return_zero_forever() {
        // base 0 is treated as 1; cap below base is raised to base.
        assert!(equal_jitter_secs(0, 0, 0, 0.5) <= 1);
        assert_eq!(equal_jitter_secs(5, 10, 1, 0.0), 5); // cap raised to base=10
    }

    #[test]
    fn next_delay_stays_within_bounds() {
        for attempt in 0..8 {
            let d = next_delay(attempt, BASE, CAP).as_secs();
            let temp = (BASE.saturating_mul(1 << attempt)).min(CAP);
            assert!(d >= temp / 2 && d <= temp, "attempt {attempt}: {d}");
        }
    }
}
