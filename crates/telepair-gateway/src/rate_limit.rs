//! Per-IP rate limiting for public unauthenticated endpoints.
//!
//! The only surface wired up here is `POST /api/auth/register`: each
//! call runs Argon2 password hashing and (when SMTP is configured)
//! sends an outbound email. The existing per-email throttle inside
//! `AuthService::register` stops the *same* pending row from being
//! refreshed faster than `OTP_RATE_LIMIT_SECS`, but an attacker
//! rotating email addresses from a single host bypasses that check
//! entirely — every unique address looks like a fresh pending row.
//! This module adds the matching per-IP gate so "one host, many
//! emails" also costs at most one slow hash + one SMTP send per
//! `min_interval`.
//!
//! The implementation is a short-held `Mutex<HashMap>` rather than
//! a lock-free crate (`dashmap`, `governor`, etc.). Register traffic
//! is inherently low (bounded by real signup rate), the critical
//! section is O(1), and keeping the dependency surface small is a
//! net win here.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Minimum interval between register attempts from the same source
/// IP. Chosen as a multiple of `OTP_RATE_LIMIT_SECS` (60 s per
/// email) so a legitimate signup that legitimately retries after an
/// email typo still succeeds, while a script pumping unique emails
/// pays 30 s per attempt. Operators behind a reverse proxy that
/// terminates the real client IP must still forward it — otherwise
/// every connection appears to come from the proxy and one bad
/// actor would lock out the whole fleet.
pub const DEFAULT_REGISTER_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Per-IP minimum-interval limiter. See module-level docs.
pub struct RegisterRateLimiter {
    min_interval: Duration,
    last_seen: Mutex<HashMap<IpAddr, Instant>>,
}

/// Outcome of a [`RegisterRateLimiter::check`] call. Separated from
/// a bare `Result<(), Duration>` so callers reading the returned
/// value on the 429 path do not need a comment to explain what the
/// `Duration` means.
#[derive(Debug, PartialEq, Eq)]
pub enum RateLimitDecision {
    /// Under the limit — call was recorded, caller may proceed.
    Allowed,
    /// Caller hit the limit; `retry_after` is the time remaining
    /// until the next attempt from this IP will be allowed.
    Throttled { retry_after: Duration },
}

impl RegisterRateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_seen: Mutex::new(HashMap::new()),
        }
    }

    /// `expect` on a poisoned mutex is the right thing here: the only
    /// way to poison is a panic while holding the lock, and the
    /// critical section is a single HashMap insert with no allocations
    /// or user code — a poison means the process is already in an
    /// unrecoverable state.
    fn lock(&self) -> MutexGuard<'_, HashMap<IpAddr, Instant>> {
        self.last_seen
            .lock()
            .expect("register limiter mutex poisoned")
    }

    /// Record an attempt from `ip`. On [`RateLimitDecision::Allowed`]
    /// the stored instant is refreshed so the next call within the
    /// window throttles. On [`RateLimitDecision::Throttled`] the
    /// existing instant is **not** touched — a throttled caller
    /// does not extend their own cooldown by retrying.
    pub fn check(&self, ip: IpAddr) -> RateLimitDecision {
        let now = Instant::now();
        let mut map = self.lock();
        if let Some(&prev) = map.get(&ip) {
            let elapsed = now.duration_since(prev);
            if elapsed < self.min_interval {
                return RateLimitDecision::Throttled {
                    retry_after: self.min_interval - elapsed,
                };
            }
        }
        map.insert(ip, now);
        RateLimitDecision::Allowed
    }

    /// Drop entries older than `min_interval` so the map does not
    /// grow unbounded under churn. Safe to call from a background
    /// loop; the contended lock is held only while draining.
    pub fn purge_expired(&self) {
        let now = Instant::now();
        let cutoff = self.min_interval;
        self.lock()
            .retain(|_, last| now.duration_since(*last) < cutoff);
    }

    /// Test-only observability: number of distinct IPs currently
    /// tracked. Production code has no reason to read this.
    #[cfg(test)]
    pub(crate) fn tracked_ips(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::thread::sleep;

    #[test]
    fn first_call_from_ip_is_allowed() {
        let lim = RegisterRateLimiter::new(Duration::from_secs(30));
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(lim.check(ip), RateLimitDecision::Allowed);
    }

    #[test]
    fn immediate_second_call_is_throttled() {
        let lim = RegisterRateLimiter::new(Duration::from_secs(30));
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(lim.check(ip), RateLimitDecision::Allowed);
        match lim.check(ip) {
            RateLimitDecision::Throttled { retry_after } => {
                // The retry hint must be positive and bounded by the
                // configured interval — anything else means the
                // subtraction in `check` is wrong (e.g. underflow
                // from a clock skew or a typo in the duration math).
                assert!(retry_after > Duration::ZERO);
                assert!(retry_after <= Duration::from_secs(30));
            }
            other => panic!("expected Throttled, got {other:?}"),
        }
    }

    #[test]
    fn different_ips_are_independent() {
        // Regression guard: an attacker on 10.0.0.4 must not be able
        // to exhaust the limit for 10.0.0.3. A previous revision kept
        // a single global last_seen instant — this test would have
        // caught that and prevented it reaching the wire.
        let lim = RegisterRateLimiter::new(Duration::from_secs(30));
        let ip_a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
        let ip_b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4));
        assert_eq!(lim.check(ip_a), RateLimitDecision::Allowed);
        assert_eq!(lim.check(ip_b), RateLimitDecision::Allowed);
    }

    #[test]
    fn call_after_interval_elapses_is_allowed_again() {
        // Use a tiny interval so the test doesn't sleep for real
        // seconds — the logic is the same at any scale.
        let lim = RegisterRateLimiter::new(Duration::from_millis(20));
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
        assert_eq!(lim.check(ip), RateLimitDecision::Allowed);
        sleep(Duration::from_millis(30));
        assert_eq!(lim.check(ip), RateLimitDecision::Allowed);
    }

    #[test]
    fn throttled_retry_does_not_extend_cooldown() {
        // Regression guard: if a hammered IP could *extend* its own
        // cooldown by retrying, a malicious loop would lock itself
        // out forever (and legit users sharing a NAT with it would
        // see weird retry_after jitter). The fix is to only update
        // the stored instant on the Allowed branch.
        let lim = RegisterRateLimiter::new(Duration::from_millis(50));
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 6));
        assert_eq!(lim.check(ip), RateLimitDecision::Allowed);
        sleep(Duration::from_millis(10));
        let RateLimitDecision::Throttled { retry_after: first } = lim.check(ip) else {
            panic!("expected throttled");
        };
        sleep(Duration::from_millis(10));
        let RateLimitDecision::Throttled {
            retry_after: second,
        } = lim.check(ip)
        else {
            panic!("expected throttled");
        };
        // The second retry_after should be strictly smaller — the
        // clock has advanced ~10 ms and the stored instant has NOT
        // been refreshed.
        assert!(
            second < first,
            "expected retry window to shrink, got {first:?} -> {second:?}"
        );
    }

    #[test]
    fn purge_drops_expired_entries() {
        let lim = RegisterRateLimiter::new(Duration::from_millis(20));
        lim.check(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1)));
        lim.check(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 2)));
        assert_eq!(lim.tracked_ips(), 2);
        sleep(Duration::from_millis(40));
        lim.purge_expired();
        assert_eq!(lim.tracked_ips(), 0);
    }
}
