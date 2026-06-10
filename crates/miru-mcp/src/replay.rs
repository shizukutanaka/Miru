//! Replay protection for MCP tool-call requests.
//!
//! Threat: even with a valid, unexpired capability token, an adversary who can
//! observe or inject on the stdio channel could *replay* a previously-authorized
//! tool call (e.g. resend a `file_write` request). The rate limiter bounds
//! request *frequency* but does not prevent replay of a single captured request.
//!
//! Design follows AttestMCP (Maloyan & Namiot, arXiv:2601.17549) §VI-C
//! "Replay Protection": a sliding window of recent nonces per capability with a
//! bounded validity period. A request carries a `(timestamp, nonce)` pair; the
//! server rejects any request whose timestamp is outside the validity window or
//! whose nonce was already seen.
//!
//! Parameters match the paper's recommendation: 1000-nonce window, 30s validity.

use parking_lot::Mutex;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Maximum number of nonces retained in the sliding window.
const NONCE_WINDOW: usize = 1000;
/// How long a request timestamp is considered valid (clock skew + transit).
const VALIDITY: Duration = Duration::from_secs(30);

/// Outcome of a replay check.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplayDecision {
    /// Request is fresh — accept and record its nonce.
    Fresh,
    /// Nonce already seen within the window — reject as replay.
    DuplicateNonce,
    /// Timestamp outside the validity window (too old or too far future).
    StaleTimestamp,
}

/// Sliding-window replay guard. Cheap, dependency-free, lock-guarded for the
/// single-threaded stdio loop (but safe under contention).
pub struct ReplayGuard {
    inner: Mutex<Inner>,
}

struct Inner {
    /// FIFO of nonces in insertion order, for O(1) eviction of the oldest.
    order: VecDeque<[u8; 16]>,
    /// Set mirror of `order` for O(1) membership tests.
    seen: HashSet<[u8; 16]>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                order: VecDeque::with_capacity(NONCE_WINDOW),
                seen: HashSet::with_capacity(NONCE_WINDOW),
            }),
        }
    }

    /// Check a request's `(timestamp_secs, nonce)`. On `Fresh`, the nonce is
    /// recorded; duplicate or stale requests are rejected without recording.
    pub fn check(&self, timestamp_secs: u64, nonce: [u8; 16]) -> ReplayDecision {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Validity: |now - ts| must be within VALIDITY (handles minor clock skew
        // in both directions).
        let delta = now.abs_diff(timestamp_secs);
        if delta > VALIDITY.as_secs() {
            return ReplayDecision::StaleTimestamp;
        }

        let mut inner = self.inner.lock();
        if inner.seen.contains(&nonce) {
            return ReplayDecision::DuplicateNonce;
        }

        // Record the nonce, evicting the oldest if at capacity.
        if inner.order.len() >= NONCE_WINDOW {
            if let Some(old) = inner.order.pop_front() {
                inner.seen.remove(&old);
            }
        }
        inner.order.push_back(nonce);
        inner.seen.insert(nonce);
        ReplayDecision::Fresh
    }
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn fresh_nonce_accepted() {
        let g = ReplayGuard::new();
        assert_eq!(g.check(now(), [1u8; 16]), ReplayDecision::Fresh);
    }

    #[test]
    fn duplicate_nonce_rejected() {
        let g = ReplayGuard::new();
        let n = [7u8; 16];
        assert_eq!(g.check(now(), n), ReplayDecision::Fresh);
        assert_eq!(g.check(now(), n), ReplayDecision::DuplicateNonce);
    }

    #[test]
    fn stale_timestamp_rejected() {
        let g = ReplayGuard::new();
        // 60s in the past — beyond the 30s window.
        assert_eq!(
            g.check(now() - 60, [2u8; 16]),
            ReplayDecision::StaleTimestamp
        );
        // 60s in the future — also rejected.
        assert_eq!(
            g.check(now() + 60, [3u8; 16]),
            ReplayDecision::StaleTimestamp
        );
    }

    #[test]
    fn distinct_nonces_all_fresh() {
        let g = ReplayGuard::new();
        for i in 0..100u8 {
            assert_eq!(g.check(now(), [i; 16]), ReplayDecision::Fresh);
        }
    }

    #[test]
    fn window_evicts_oldest() {
        let g = ReplayGuard::new();
        // Fill beyond capacity; the very first nonce should be evicted and
        // therefore accepted again (no longer "seen").
        let first = [0u8; 16];
        assert_eq!(g.check(now(), first), ReplayDecision::Fresh);
        for i in 1..=(NONCE_WINDOW as u32) {
            let mut n = [0u8; 16];
            n[0..4].copy_from_slice(&i.to_le_bytes());
            // Avoid colliding with `first` (all-zero).
            n[15] = 1;
            g.check(now(), n);
        }
        // `first` was pushed out of the 1000-entry window → fresh again.
        assert_eq!(g.check(now(), first), ReplayDecision::Fresh);
    }
}
