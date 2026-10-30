//! Per-provider circuit breaker.
//!
//! Closed → (N consecutive failures) → Open for a cooldown → Half-open
//! (exactly one probe request allowed) → Closed on probe success, re-Open on
//! probe failure. State is in-memory but syncs to/from
//! [`maestro_types::domain::ProviderHealth`] so the daemon can persist it
//! across restarts (`from_health` / `to_health`).

use std::sync::Mutex;

use maestro_types::domain::ProviderHealth;
use maestro_types::{now_ms, MaestroError, Millis};

/// Default consecutive-failure threshold before the breaker opens.
pub const DEFAULT_THRESHOLD: u32 = 5;
/// Default cooldown while open, in milliseconds.
pub const DEFAULT_COOLDOWN_MS: i64 = 30_000;

#[derive(Debug, Default)]
struct Inner {
    success_count: i64,
    failure_count: i64,
    consecutive_failures: i64,
    /// While set and in the future, calls are refused.
    open_until: Option<Millis>,
    /// True while the single half-open probe is in flight.
    probing: bool,
    updated_at: Millis,
}

/// Consecutive-failure circuit breaker with half-open single probe.
#[derive(Debug)]
pub struct CircuitBreaker {
    provider_id: String,
    threshold: u32,
    cooldown_ms: i64,
    inner: Mutex<Inner>,
}

impl CircuitBreaker {
    /// Breaker with default threshold (5) and cooldown (30 s).
    pub fn new(provider_id: impl Into<String>) -> Self {
        Self::with_params(provider_id, DEFAULT_THRESHOLD, DEFAULT_COOLDOWN_MS)
    }

    /// Breaker with explicit threshold and cooldown.
    pub fn with_params(provider_id: impl Into<String>, threshold: u32, cooldown_ms: i64) -> Self {
        Self {
            provider_id: provider_id.into(),
            threshold: threshold.max(1),
            cooldown_ms: cooldown_ms.max(1),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Restore a breaker from a persisted health snapshot (daemon restart).
    pub fn from_health(health: &ProviderHealth) -> Self {
        let breaker = Self::new(health.provider_id.clone());
        {
            let mut inner = breaker.lock();
            inner.success_count = health.success_count;
            inner.failure_count = health.failure_count;
            inner.consecutive_failures = health.consecutive_failures;
            inner.open_until = health.open_until;
            inner.updated_at = health.updated_at;
        }
        breaker
    }

    /// Snapshot the current state for persistence.
    pub fn to_health(&self) -> ProviderHealth {
        let inner = self.lock();
        ProviderHealth {
            provider_id: self.provider_id.clone(),
            success_count: inner.success_count,
            failure_count: inner.failure_count,
            consecutive_failures: inner.consecutive_failures,
            open_until: inner.open_until,
            updated_at: if inner.updated_at == 0 {
                now_ms()
            } else {
                inner.updated_at
            },
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Ask permission to make a request. When the breaker is open this
    /// returns a retryable `MaestroError::Provider` whose message contains
    /// `"circuit breaker open"` and whose `retry_after_ms` is the remaining
    /// cooldown. After the cooldown, exactly one caller is admitted as the
    /// half-open probe; concurrent callers keep getting refused until the
    /// probe resolves via `record_success` / `record_failure`.
    pub fn try_acquire(&self) -> maestro_types::Result<()> {
        let now = now_ms();
        let mut inner = self.lock();
        let Some(open_until) = inner.open_until else {
            return Ok(());
        };
        if now < open_until {
            return Err(self.open_error(open_until - now, inner.consecutive_failures));
        }
        // cooldown elapsed → half-open
        if inner.probing {
            // a probe is already in flight; refuse others briefly
            return Err(self.open_error(self.cooldown_ms.min(1_000), inner.consecutive_failures));
        }
        inner.probing = true;
        Ok(())
    }

    fn open_error(&self, retry_after_ms: i64, consecutive: i64) -> MaestroError {
        MaestroError::Provider {
            provider: self.provider_id.clone(),
            message: format!(
                "circuit breaker open for provider '{}' ({consecutive} consecutive failures)",
                self.provider_id,
            ),
            retryable: true,
            retry_after_ms: Some(retry_after_ms.max(0)),
        }
    }

    /// Settle a cancelled call without judging the provider: clears the
    /// half-open probe flag (if this caller held it) so the next caller can
    /// probe. Counters and `open_until` are untouched — cancellation says
    /// nothing about provider health, but leaving `probing` set would refuse
    /// every future request until restart.
    pub fn release_probe(&self) {
        let mut inner = self.lock();
        if inner.probing {
            inner.probing = false;
            inner.updated_at = now_ms();
        }
    }

    /// Record a successful call: closes the breaker and resets the streak.
    pub fn record_success(&self) {
        let mut inner = self.lock();
        inner.success_count += 1;
        inner.consecutive_failures = 0;
        inner.open_until = None;
        inner.probing = false;
        inner.updated_at = now_ms();
    }

    /// Record a failed call. Opens the breaker when the consecutive-failure
    /// threshold is reached, or immediately when a half-open probe fails.
    pub fn record_failure(&self) {
        let mut inner = self.lock();
        inner.failure_count += 1;
        inner.consecutive_failures += 1;
        let probe_failed = inner.probing;
        inner.probing = false;
        if probe_failed || inner.consecutive_failures >= self.threshold as i64 {
            inner.open_until = Some(now_ms() + self.cooldown_ms);
            tracing::warn!(
                provider = %self.provider_id,
                consecutive = inner.consecutive_failures,
                cooldown_ms = self.cooldown_ms,
                "circuit breaker opened"
            );
        }
        inner.updated_at = now_ms();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_after(err: &MaestroError) -> Option<i64> {
        match err {
            MaestroError::Provider { retry_after_ms, .. } => *retry_after_ms,
            other => panic!("expected provider error, got {other:?}"),
        }
    }

    #[test]
    fn opens_after_threshold_consecutive_failures() {
        let b = CircuitBreaker::with_params("p", 3, 30_000);
        for _ in 0..2 {
            b.try_acquire().unwrap();
            b.record_failure();
        }
        b.try_acquire().unwrap(); // still closed at 2 failures
        b.record_failure(); // third → open
        let err = b.try_acquire().unwrap_err();
        assert!(err.to_string().contains("circuit breaker open"));
        let remaining = retry_after(&err).unwrap();
        assert!(remaining > 0 && remaining <= 30_000);
    }

    #[test]
    fn success_resets_the_streak() {
        let b = CircuitBreaker::with_params("p", 3, 30_000);
        b.record_failure();
        b.record_failure();
        b.record_success();
        b.record_failure();
        b.record_failure();
        b.try_acquire().unwrap(); // streak was reset, still closed
    }

    #[tokio::test]
    async fn open_to_half_open_to_closed() {
        let b = CircuitBreaker::with_params("p", 2, 40);
        b.record_failure();
        b.record_failure();
        assert!(b.try_acquire().is_err()); // open
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        // half-open: exactly one probe admitted
        b.try_acquire().unwrap();
        assert!(
            b.try_acquire().is_err(),
            "second concurrent probe must be refused"
        );
        b.record_success();
        b.try_acquire().unwrap(); // closed again
        assert_eq!(b.to_health().open_until, None);
    }

    #[tokio::test]
    async fn failed_probe_reopens() {
        let b = CircuitBreaker::with_params("p", 1, 40);
        b.record_failure(); // open
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        b.try_acquire().unwrap(); // probe
        b.record_failure(); // probe failed → reopen for a fresh cooldown
        let err = b.try_acquire().unwrap_err();
        assert!(retry_after(&err).unwrap() > 0);
    }

    #[tokio::test]
    async fn cancelled_probe_releases_the_half_open_slot() {
        let b = CircuitBreaker::with_params("p", 1, 40);
        b.record_failure(); // open
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        b.try_acquire().unwrap(); // half-open probe admitted
        b.release_probe(); // probe cancelled: settle without counting it
                           // next caller must be admitted as a fresh probe, not refused forever
        b.try_acquire().unwrap();
        let health = b.to_health();
        assert_eq!(
            health.failure_count, 1,
            "cancellation must not count as failure"
        );
        assert_eq!(
            health.success_count, 0,
            "cancellation must not count as success"
        );
    }

    #[test]
    fn release_probe_without_probe_is_a_no_op() {
        let b = CircuitBreaker::with_params("p", 2, 30_000);
        b.record_failure();
        b.release_probe(); // closed, no probe held
        let health = b.to_health();
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.open_until, None);
        b.try_acquire().unwrap(); // still closed
    }

    #[test]
    fn health_round_trip() {
        let b = CircuitBreaker::with_params("prov", 2, 30_000);
        b.record_success();
        b.record_failure();
        b.record_failure(); // open
        let health = b.to_health();
        assert_eq!(health.provider_id, "prov");
        assert_eq!(health.success_count, 1);
        assert_eq!(health.failure_count, 2);
        assert_eq!(health.consecutive_failures, 2);
        assert!(health.open_until.is_some());

        let restored = CircuitBreaker::from_health(&health);
        assert!(
            restored.try_acquire().is_err(),
            "restored breaker stays open"
        );
        let again = restored.to_health();
        assert_eq!(again.failure_count, 2);
        assert_eq!(again.open_until, health.open_until);
    }
}
