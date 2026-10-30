//! Per-provider token-bucket rate limiter (requests per minute).
//!
//! `None` (or `Some(0)`) requests-per-minute means unlimited. The bucket
//! starts full with capacity == rpm and refills continuously at rpm/60 per
//! second. `acquire` awaits capacity using tokio time (so it cooperates with
//! paused test time) and is cancel-safe: cancelling the token returns
//! [`MaestroError::Cancelled`] without consuming a slot.

use std::sync::Mutex;
use std::time::Duration;

use maestro_types::{MaestroError, Result};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Bucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: tokio::time::Instant,
}

impl Bucket {
    fn refill(&mut self, now: tokio::time::Instant) {
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last_refill = now;
    }
}

/// Token bucket keyed to one provider.
#[derive(Debug)]
pub struct RateLimiter {
    bucket: Option<Mutex<Bucket>>,
}

impl RateLimiter {
    /// `requests_per_minute` from `ProviderConfig`; `None` or `Some(0)` =
    /// unlimited (0 would otherwise block forever, so it is treated as unset
    /// with a warning).
    pub fn new(requests_per_minute: Option<u32>) -> Self {
        let bucket = match requests_per_minute {
            None => None,
            Some(0) => {
                tracing::warn!("requests_per_minute = 0 treated as unlimited");
                None
            }
            Some(rpm) => Some(Mutex::new(Bucket {
                capacity: rpm as f64,
                tokens: rpm as f64,
                refill_per_sec: rpm as f64 / 60.0,
                last_refill: tokio::time::Instant::now(),
            })),
        };
        Self { bucket }
    }

    /// Wait for one request slot. Cancel-safe: no slot is consumed on
    /// cancellation, and the mutex is never held across an await.
    pub async fn acquire(&self, cancel: &CancellationToken) -> Result<()> {
        let Some(bucket) = &self.bucket else {
            return Ok(());
        };
        loop {
            let wait = {
                let mut b = bucket
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                b.refill(tokio::time::Instant::now());
                if b.tokens >= 1.0 {
                    b.tokens -= 1.0;
                    return Ok(());
                }
                Duration::from_secs_f64((1.0 - b.tokens) / b.refill_per_sec)
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(MaestroError::Cancelled),
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn burst_then_wait_with_paused_time() {
        let limiter = RateLimiter::new(Some(2));
        let cancel = CancellationToken::new();
        let t0 = tokio::time::Instant::now();
        // full bucket: two immediate slots
        limiter.acquire(&cancel).await.unwrap();
        limiter.acquire(&cancel).await.unwrap();
        assert_eq!(tokio::time::Instant::now(), t0);
        // third must wait ~30s (2 rpm → one token every 30s)
        limiter.acquire(&cancel).await.unwrap();
        let waited = tokio::time::Instant::now() - t0;
        assert!(
            waited >= Duration::from_secs(29) && waited <= Duration::from_secs(31),
            "waited {waited:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unlimited_never_waits() {
        let limiter = RateLimiter::new(None);
        let cancel = CancellationToken::new();
        let t0 = tokio::time::Instant::now();
        for _ in 0..1000 {
            limiter.acquire(&cancel).await.unwrap();
        }
        assert_eq!(tokio::time::Instant::now(), t0);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_while_waiting_returns_cancelled() {
        let limiter = std::sync::Arc::new(RateLimiter::new(Some(1)));
        let cancel = CancellationToken::new();
        limiter.acquire(&cancel).await.unwrap(); // drain the bucket
        let l2 = limiter.clone();
        let c2 = cancel.clone();
        let waiter = tokio::spawn(async move { l2.acquire(&c2).await });
        tokio::task::yield_now().await;
        cancel.cancel();
        let out = waiter.await.unwrap();
        assert!(matches!(out, Err(MaestroError::Cancelled)));
    }
}

/// Reserve estimated input plus the maximum output for a rolling 60-second
/// window. Reservations are deliberately not refunded: Azure charges admission
/// estimates, and cancelled requests may already have reached the deployment.
#[derive(Debug)]
pub struct TokenLimiter {
    limit: Option<u32>,
    reservations: Mutex<std::collections::VecDeque<(tokio::time::Instant, u64)>>,
}

impl TokenLimiter {
    pub fn new(limit: Option<u32>) -> Self {
        Self {
            limit: limit.filter(|n| *n > 0),
            reservations: Mutex::new(Default::default()),
        }
    }

    pub async fn acquire(
        &self,
        provider: &str,
        req: &maestro_types::provider::ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let Some(limit) = self.limit else {
            return Ok(());
        };
        // UTF-8 byte estimate is conservative for ordinary code and prose, but
        // not an exact tokenizer. Provider 429 + Retry-After remains authoritative.
        let input: u64 = req
            .messages
            .iter()
            .map(|m| m.content.len().div_ceil(3) as u64 + 8 + m.images.len() as u64 * 8192)
            .sum();
        let output = req.max_tokens.ok_or_else(|| {
            MaestroError::Config(format!(
                "provider '{provider}' with tokensPerMinute needs an explicit maxOutputTokens"
            ))
        })?;
        self.reserve(provider, input + u64::from(output), limit, cancel)
            .await
    }

    async fn reserve(
        &self,
        provider: &str,
        amount: u64,
        limit: u32,
        cancel: &CancellationToken,
    ) -> Result<()> {
        if amount > u64::from(limit) {
            return Err(MaestroError::Provider { provider: provider.into(), message: format!("estimated request budget {amount} exceeds deployment tokensPerMinute {limit}; reduce context/output or use a higher-quota fallback"), retryable: false, retry_after_ms: None });
        }
        loop {
            if cancel.is_cancelled() {
                return Err(MaestroError::Cancelled);
            }
            let until = {
                let now = tokio::time::Instant::now();
                let mut slots = self.reservations.lock().unwrap_or_else(|p| p.into_inner());
                while slots
                    .front()
                    .is_some_and(|(at, _)| now.duration_since(*at) >= Duration::from_secs(60))
                {
                    slots.pop_front();
                }
                let used: u64 = slots.iter().map(|(_, n)| n).sum();
                if used + amount <= u64::from(limit) {
                    slots.push_back((now, amount));
                    return Ok(());
                }
                slots.front().unwrap().0 + Duration::from_secs(60)
            };
            tracing::info!(provider, "waiting for deployment token quota");
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(MaestroError::Cancelled),
                _ = tokio::time::sleep_until(until) => {}
            }
        }
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn rolling_window_prevents_double_spending_and_expires() {
        let l = TokenLimiter::new(Some(100));
        let c = CancellationToken::new();
        let start = tokio::time::Instant::now();
        l.reserve("p", 60, 100, &c).await.unwrap();
        l.reserve("p", 60, 100, &c).await.unwrap();
        assert!(start.elapsed() >= Duration::from_secs(60));
        assert!(l.reserve("p", 101, 100, &c).await.is_err());
    }
    #[tokio::test(start_paused = true)]
    async fn cancelled_wait_does_not_consume_tokens() {
        let l = std::sync::Arc::new(TokenLimiter::new(Some(100)));
        let c = CancellationToken::new();
        l.reserve("p", 100, 100, &c).await.unwrap();
        let l2 = l.clone();
        let c2 = c.clone();
        let task = tokio::spawn(async move { l2.reserve("p", 1, 100, &c2).await });
        tokio::task::yield_now().await;
        c.cancel();
        assert!(matches!(task.await.unwrap(), Err(MaestroError::Cancelled)));
        assert_eq!(l.reservations.lock().unwrap().len(), 1);
    }
}
