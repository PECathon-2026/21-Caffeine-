//! Provider registry: lazy construction from `MaestroConfig`, per-provider
//! rate limiting + circuit breaking, cost metering, and model discovery.
//!
//! Providers are built on first use (`kind` → transport), cached as
//! `Arc<dyn Provider>`, and wired with a [`RateLimiter`] (from
//! `ProviderConfig::requests_per_minute`) and a [`CircuitBreaker`].
//! Credentials are never read at build time — each transport reads its env
//! var lazily at request time.
//!
//! `generate` returns `(ModelResponse, cost_micro_usd)`. `stream` returns a
//! [`MeteredStream`]: the cost is computed when the `Done` event passes
//! through and is readable via [`MeteredStream::cost_micro_usd`] afterwards
//! (a stream cannot carry a value that only exists at its end, so this is
//! the "similarly" of the contract).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use maestro_types::config::{MaestroConfig, ModelRef, ProviderKind};
use maestro_types::provider::{
    DiscoveredModel, ModelDiscovery, ModelRequest, ModelResponse, StreamEvent,
};
use maestro_types::{now_ms, MaestroError, Result};
use tokio_util::sync::CancellationToken;

use crate::anthropic::AnthropicProvider;
use crate::breaker::CircuitBreaker;
use crate::limiter::{RateLimiter, TokenLimiter};
#[cfg(any(test, feature = "test-support"))]
use crate::mock::MockProvider;
use crate::openai_compatible::OpenAiCompatibleProvider;
use crate::pricing;
use crate::provider::Provider;

struct Entry {
    provider: Arc<dyn Provider>,
    limiter: RateLimiter,
    token_limiter: Arc<TokenLimiter>,
    breaker: CircuitBreaker,
}

/// Builds, caches and meters providers. Cheap to clone behind an `Arc` at
/// the daemon level; internally synchronized.
pub struct ProviderRegistry {
    config: MaestroConfig,
    timeout_ms: i64,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
}

impl ProviderRegistry {
    /// Snapshot the config; providers are built lazily on first use. The
    /// per-request timeout is `config.runtime.request_timeout_ms`.
    pub fn new(config: &MaestroConfig) -> Self {
        Self {
            config: config.clone(),
            timeout_ms: config.runtime.request_timeout_ms,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Restore a provider's circuit-breaker state from a persisted snapshot.
    /// Must be called before the provider's first request to take effect
    /// (it replaces the cached entry's breaker by rebuilding the entry).
    pub fn restore_health(&self, health: &maestro_types::domain::ProviderHealth) -> Result<()> {
        let entry = self.entry(&health.provider_id)?;
        // CircuitBreaker has interior mutability only for counters; swap via
        // rebuilding the entry with a restored breaker.
        let restored = Arc::new(Entry {
            provider: entry.provider.clone(),
            token_limiter: entry.token_limiter.clone(),
            limiter: RateLimiter::new(
                self.config
                    .providers
                    .get(&health.provider_id)
                    .and_then(|c| c.requests_per_minute),
            ),
            breaker: CircuitBreaker::from_health(health),
        });
        self.lock_entries()
            .insert(health.provider_id.clone(), restored);
        Ok(())
    }

    /// Snapshot every instantiated provider's breaker state for persistence.
    pub fn health_snapshots(&self) -> Vec<maestro_types::domain::ProviderHealth> {
        self.lock_entries()
            .values()
            .map(|e| e.breaker.to_health())
            .collect()
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Entry>>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn entry(&self, provider_id: &str) -> Result<Arc<Entry>> {
        if let Some(e) = self.lock_entries().get(provider_id) {
            return Ok(e.clone());
        }
        let cfg = self.config.providers.get(provider_id).ok_or_else(|| {
            MaestroError::NotFound(format!("provider '{provider_id}' is not configured"))
        })?;
        let provider: Arc<dyn Provider> = match cfg.kind {
            ProviderKind::Anthropic => {
                Arc::new(AnthropicProvider::new(provider_id, cfg, self.timeout_ms)?)
            }
            ProviderKind::OpenaiCompatible | ProviderKind::OpenaiResponses => Arc::new(
                OpenAiCompatibleProvider::new(provider_id, cfg, self.timeout_ms)?,
            ),
            #[cfg(any(test, feature = "test-support"))]
            ProviderKind::Mock => Arc::new(MockProvider::new()),
            // A consumer can enable the types crate's fixture feature without
            // enabling this crate's fixture transport. Fail closed in that case.
            #[allow(unreachable_patterns)]
            _ => {
                return Err(MaestroError::Config(
                    "test-only provider transport is not enabled".into(),
                ))
            }
        };
        let entry = Arc::new(Entry {
            provider,
            limiter: RateLimiter::new(cfg.requests_per_minute),
            token_limiter: Arc::new(TokenLimiter::new(cfg.tokens_per_minute)),
            breaker: CircuitBreaker::new(provider_id),
        });
        // Double-checked insert: a racing builder may have won; keep theirs
        // so cached mock state stays consistent.
        let mut entries = self.lock_entries();
        Ok(entries
            .entry(provider_id.to_string())
            .or_insert(entry)
            .clone())
    }

    /// The cached (or newly built) provider for `provider_id`.
    /// Unknown ids yield `MaestroError::NotFound`.
    pub fn get(&self, provider_id: &str) -> Result<Arc<dyn Provider>> {
        Ok(self.entry(provider_id)?.provider.clone())
    }

    /// Admission control for a LangChain-owned API request. Shares the same
    /// deployment limiter and breaker as native planner/chat requests.
    pub async fn acquire_external(
        &self,
        model: &ModelRef,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let cfg = self
            .config
            .providers
            .get(&model.provider)
            .ok_or_else(|| MaestroError::Config("unknown external provider".into()))?;
        if cfg.deployment.as_deref().is_some_and(|p| p != model.model) {
            return Err(MaestroError::Config(
                "external model does not match pinned deployment".into(),
            ));
        }
        let entry = self.entry(&model.provider)?;
        entry
            .token_limiter
            .acquire(&model.provider, req, cancel)
            .await?;
        entry.limiter.acquire(cancel).await?;
        entry.breaker.try_acquire()
    }

    pub fn finish_external(&self, model: &ModelRef, success: Option<bool>) -> Result<()> {
        let entry = self.entry(&model.provider)?;
        match success {
            Some(true) => entry.breaker.record_success(),
            Some(false) => entry.breaker.record_failure(),
            None => entry.breaker.release_probe(),
        }
        Ok(())
    }

    /// Rate-limited, breaker-guarded generate. The request's `model` field is
    /// overwritten with `model.model` so callers cannot desync the two.
    /// Returns the response and its cost in micro-USD (0 for unknown models).
    pub async fn generate(
        &self,
        model: &ModelRef,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<(ModelResponse, i64)> {
        if let Some(pinned) = self
            .config
            .providers
            .get(&model.provider)
            .and_then(|p| p.deployment.as_deref())
        {
            if pinned != model.model {
                return Err(MaestroError::Config(format!(
                    "provider '{}' is pinned to deployment '{pinned}', not '{}'",
                    model.provider, model.model
                )));
            }
        }
        let entry = self.entry(&model.provider)?;
        entry
            .token_limiter
            .acquire(&model.provider, req, cancel)
            .await?;
        entry.limiter.acquire(cancel).await?;
        entry.breaker.try_acquire()?;
        let mut req = req.clone();
        req.model = model.model.clone();
        match entry.provider.generate(&req, cancel).await {
            Ok(resp) => {
                entry.breaker.record_success();
                let (cost, known) = pricing::cost_micro_usd(&model.model, &resp.usage);
                if !known {
                    tracing::debug!(model = %model, "no pricing entry; cost recorded as 0");
                }
                Ok((resp, cost))
            }
            Err(MaestroError::Cancelled) => {
                // Neutral for the counters, but a held half-open probe must be
                // released or the breaker refuses everyone until restart.
                entry.breaker.release_probe();
                Err(MaestroError::Cancelled)
            }
            Err(e) => {
                entry.breaker.record_failure();
                Err(e)
            }
        }
    }

    /// Rate-limited, breaker-guarded stream. Breaker success is recorded when
    /// `Done` flows through; failure when the stream errors (cancellation is
    /// neutral). Cost is computed at `Done` — read it afterwards via
    /// [`MeteredStream::cost_micro_usd`].
    pub async fn stream(
        &self,
        model: &ModelRef,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<MeteredStream> {
        if let Some(pinned) = self
            .config
            .providers
            .get(&model.provider)
            .and_then(|p| p.deployment.as_deref())
        {
            if pinned != model.model {
                return Err(MaestroError::Config(format!(
                    "provider '{}' is pinned to deployment '{pinned}', not '{}'",
                    model.provider, model.model
                )));
            }
        }
        let entry = self.entry(&model.provider)?;
        entry
            .token_limiter
            .acquire(&model.provider, req, cancel)
            .await?;
        entry.limiter.acquire(cancel).await?;
        entry.breaker.try_acquire()?;
        let mut req = req.clone();
        req.model = model.model.clone();
        match entry.provider.stream(&req, cancel).await {
            Ok(inner) => Ok(MeteredStream {
                inner,
                entry,
                model: model.model.clone(),
                cost: None,
                settled: false,
            }),
            Err(MaestroError::Cancelled) => {
                // Same as generate: release a held half-open probe.
                entry.breaker.release_probe();
                Err(MaestroError::Cancelled)
            }
            Err(e) => {
                entry.breaker.record_failure();
                Err(e)
            }
        }
    }

    /// Hit `list_models` on every configured provider concurrently. Failures
    /// are collected per provider; models referenced by role assignments that
    /// discovery did not report are merged in with `discovered: false`.
    pub async fn discover_models(&self) -> ModelDiscovery {
        let futures = self.config.providers.keys().map(|id| {
            let id = id.clone();
            async move {
                let outcome =
                    if let Some(deployment) = self.config.providers[&id].deployment.as_ref() {
                        Ok(vec![deployment.clone()])
                    } else {
                        match self.get(&id) {
                            Ok(provider) => provider.list_models().await,
                            Err(e) => Err(e),
                        }
                    };
                (id, outcome)
            }
        });
        let results = futures::future::join_all(futures).await;

        let mut models: Vec<DiscoveredModel> = Vec::new();
        let mut errors = std::collections::BTreeMap::new();
        for (provider_id, outcome) in results {
            match outcome {
                Ok(ids) => {
                    for model in ids {
                        models.push(DiscoveredModel {
                            provider_id: provider_id.clone(),
                            model,
                            display_name: None,
                            context_window: None,
                            discovered: self.config.providers[&provider_id].deployment.is_none(),
                        });
                    }
                }
                Err(e) => {
                    errors.insert(provider_id, e.to_string());
                }
            }
        }

        // Merge configured-but-undiscovered role models (primary + fallbacks).
        let mut seen: std::collections::BTreeSet<(String, String)> = models
            .iter()
            .map(|m| (m.provider_id.clone(), m.model.clone()))
            .collect();
        for assignment in self.config.roles.values() {
            for m in std::iter::once(&assignment.model).chain(assignment.fallbacks.iter()) {
                let key = (m.provider.clone(), m.model.clone());
                if seen.insert(key) {
                    models.push(DiscoveredModel {
                        provider_id: m.provider.clone(),
                        model: m.model.clone(),
                        display_name: None,
                        context_window: None,
                        discovered: false,
                    });
                }
            }
        }
        models.sort_by(|a, b| (&a.provider_id, &a.model).cmp(&(&b.provider_id, &b.model)));

        ModelDiscovery {
            models,
            errors,
            fetched_at: now_ms(),
        }
    }
}

/// A provider stream wrapped with cost + breaker metering. Yields the exact
/// events of the underlying stream; after `Done` has been observed,
/// [`MeteredStream::cost_micro_usd`] reports the invocation cost.
pub struct MeteredStream {
    inner: BoxStream<'static, Result<StreamEvent>>,
    entry: Arc<Entry>,
    model: String,
    cost: Option<(i64, bool)>,
    settled: bool,
}

impl MeteredStream {
    /// `(cost_micro_usd, pricing_known)` — `None` until `Done` has been
    /// yielded. Unknown models cost `(0, false)`.
    pub fn cost_micro_usd(&self) -> Option<(i64, bool)> {
        self.cost
    }
}

impl Stream for MeteredStream {
    type Item = Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(ev))) => {
                if let StreamEvent::Done { usage, .. } = &ev {
                    this.cost = Some(pricing::cost_micro_usd(&this.model, usage));
                    if !this.settled {
                        this.entry.breaker.record_success();
                        this.settled = true;
                    }
                }
                Poll::Ready(Some(Ok(ev)))
            }
            Poll::Ready(Some(Err(e))) => {
                if !this.settled {
                    if matches!(e, MaestroError::Cancelled) {
                        // Cancellation is neutral, but a held half-open probe
                        // must still be released (see `stream`).
                        this.entry.breaker.release_probe();
                    } else {
                        this.entry.breaker.record_failure();
                    }
                    this.settled = true;
                }
                Poll::Ready(Some(Err(e)))
            }
            other => other,
        }
    }
}

impl Drop for MeteredStream {
    /// A stream dropped mid-flight (e.g. the consumer was cancelled) never
    /// settles via `poll_next`; release any held half-open probe so the
    /// breaker is not wedged open forever.
    fn drop(&mut self) {
        if !self.settled {
            self.entry.breaker.release_probe();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_types::provider::ModelMessage;

    fn mock_config() -> MaestroConfig {
        MaestroConfig::default()
    }

    fn req(model: &str, user: &str) -> ModelRequest {
        ModelRequest {
            model: model.into(),
            messages: vec![ModelMessage::user(user)],
            max_tokens: None,
            temperature: None,
            json: false,
        }
    }

    #[tokio::test]
    async fn pinned_deployment_does_not_advertise_catalog_or_accept_other_models() {
        let mut config = mock_config();
        config.providers.get_mut("mock").unwrap().deployment = Some("planner".into());
        // Keep only one configured role so fallback merging cannot add fixture refs.
        config.roles.retain(|role, _| role == "planner");
        let reg = ProviderRegistry::new(&config);
        let discovery = reg.discover_models().await;
        assert_eq!(discovery.models.len(), 1);
        assert_eq!(discovery.models[0].model, "planner");
        assert!(!discovery.models[0].discovered);
        let model = ModelRef {
            provider: "mock".into(),
            model: "unavailable".into(),
        };
        let err = reg
            .generate(
                &model,
                &req("unavailable", "hello"),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("pinned"));
    }

    #[tokio::test]
    async fn unknown_provider_is_not_found() {
        let reg = ProviderRegistry::new(&mock_config());
        let err = match reg.get("nope") {
            Err(e) => e,
            Ok(_) => panic!("expected NotFound"),
        };
        assert!(matches!(err, MaestroError::NotFound(_)), "got {err:?}");
        let mr = ModelRef {
            provider: "nope".into(),
            model: "x".into(),
        };
        let err = reg
            .generate(&mr, &req("x", "hi"), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, MaestroError::NotFound(_)));
    }

    #[tokio::test]
    async fn generate_via_mock_returns_plan_and_zero_cost() {
        let reg = ProviderRegistry::new(&mock_config());
        let mr = ModelRef {
            provider: "mock".into(),
            model: "planner".into(),
        };
        // req.model is overwritten from the ModelRef
        let (resp, cost) = reg
            .generate(
                &mr,
                &req("something-else", "objective"),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let plan: maestro_types::plan::Plan = serde_json::from_str(&resp.text).unwrap();
        assert_eq!(plan.tasks.len(), 4);
        assert_eq!(cost, 0); // mock models have no pricing entry
    }

    #[tokio::test]
    async fn stream_meters_cost_at_done() {
        let reg = ProviderRegistry::new(&mock_config());
        let mr = ModelRef {
            provider: "mock".into(),
            model: "designer".into(),
        };
        let mut s = reg
            .stream(&mr, &req("designer", "go"), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(s.cost_micro_usd(), None);
        let mut dones = 0;
        while let Some(ev) = s.next().await {
            if matches!(ev.unwrap(), StreamEvent::Done { .. }) {
                dones += 1;
            }
        }
        assert_eq!(dones, 1);
        assert_eq!(s.cost_micro_usd(), Some((0, false)));
    }

    #[tokio::test]
    async fn provider_instances_are_cached() {
        // One instance per provider id: repeated lookups return the same Arc
        // (limiter/breaker state would otherwise reset between calls).
        let reg = ProviderRegistry::new(&mock_config());
        let first = reg.get("mock").unwrap();
        let second = reg.get("mock").unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn breaker_opens_after_consecutive_failures() {
        // An anthropic provider pointed at a closed port fails fast with a
        // retryable transport error; after 5 failures the breaker opens.
        let mut config = mock_config();
        config.providers.insert(
            "dead".into(),
            maestro_types::config::ProviderConfig {
                kind: ProviderKind::Anthropic,
                base_url: Some("http://127.0.0.1:1".into()),
                api_key_env: Some("MAESTRO_TEST_DEAD_KEY".into()),
                headers: Default::default(),
                requests_per_minute: None,
                tokens_per_minute: None,
                deployment: None,
                reasoning_effort: None,
            },
        );
        std::env::set_var("MAESTRO_TEST_DEAD_KEY", "k");
        let reg = ProviderRegistry::new(&config);
        let mr = ModelRef {
            provider: "dead".into(),
            model: "m".into(),
        };
        let cancel = CancellationToken::new();
        for _ in 0..5 {
            let err = reg
                .generate(&mr, &req("m", "hi"), &cancel)
                .await
                .unwrap_err();
            match err {
                MaestroError::Provider { retryable, .. } => assert!(retryable),
                other => panic!("expected provider error, got {other:?}"),
            }
        }
        let err = reg
            .generate(&mr, &req("m", "hi"), &cancel)
            .await
            .unwrap_err();
        match err {
            MaestroError::Provider {
                message,
                retryable,
                retry_after_ms,
                ..
            } => {
                assert!(
                    message.contains("circuit breaker open"),
                    "message: {message}"
                );
                assert!(retryable);
                assert!(retry_after_ms.unwrap() > 0);
            }
            other => panic!("expected breaker-open error, got {other:?}"),
        }
        // health snapshot reflects the open breaker
        let snaps = reg.health_snapshots();
        let dead = snaps.iter().find(|h| h.provider_id == "dead").unwrap();
        assert!(dead.open_until.is_some());
        assert_eq!(dead.consecutive_failures, 5);
    }

    /// Snapshot whose cooldown has already elapsed: the next `try_acquire`
    /// on the restored breaker is admitted as the half-open probe.
    fn half_open_health(provider_id: &str) -> maestro_types::domain::ProviderHealth {
        maestro_types::domain::ProviderHealth {
            provider_id: provider_id.into(),
            success_count: 0,
            failure_count: 5,
            consecutive_failures: 5,
            open_until: Some(now_ms() - 1),
            updated_at: now_ms(),
        }
    }

    #[tokio::test]
    async fn cancelled_generate_probe_does_not_wedge_the_breaker() {
        let reg = ProviderRegistry::new(&mock_config());
        reg.restore_health(&half_open_health("mock")).unwrap();
        let mr = ModelRef {
            provider: "mock".into(),
            model: "slow-designer".into(),
        };
        // The admitted probe is cancelled mid-flight.
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = reg
            .generate(&mr, &req("slow-designer", "hi"), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, MaestroError::Cancelled), "got {err:?}");
        // The next caller must be admitted as a fresh probe, not refused.
        reg.generate(&mr, &req("slow-designer", "hi"), &CancellationToken::new())
            .await
            .expect("breaker must re-admit a probe after a cancelled one");
    }

    #[tokio::test]
    async fn cancelled_stream_acquire_releases_the_probe() {
        let reg = ProviderRegistry::new(&mock_config());
        reg.restore_health(&half_open_health("mock")).unwrap();
        let mr = ModelRef {
            provider: "mock".into(),
            model: "slow-designer".into(),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = reg
            .stream(&mr, &req("slow-designer", "hi"), &cancel)
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, MaestroError::Cancelled), "got {err:?}");
        reg.generate(&mr, &req("slow-designer", "hi"), &CancellationToken::new())
            .await
            .expect("breaker must re-admit a probe after a cancelled stream acquire");
    }

    #[tokio::test]
    async fn dropped_stream_probe_releases_the_breaker() {
        let reg = ProviderRegistry::new(&mock_config());
        reg.restore_health(&half_open_health("mock")).unwrap();
        let mr = ModelRef {
            provider: "mock".into(),
            model: "designer".into(),
        };
        let s = reg
            .stream(&mr, &req("designer", "go"), &CancellationToken::new())
            .await
            .unwrap();
        drop(s); // consumer cancelled: dropped mid-flight without settling
        reg.generate(&mr, &req("designer", "go"), &CancellationToken::new())
            .await
            .expect("breaker must re-admit a probe after a dropped stream");
    }

    #[tokio::test]
    async fn cancelled_item_mid_stream_releases_the_probe() {
        // Exercise the poll_next Cancelled arm directly with a hand-built
        // MeteredStream whose breaker holds the half-open probe.
        let entry = Arc::new(Entry {
            provider: Arc::new(MockProvider::new()),
            limiter: RateLimiter::new(None),
            token_limiter: Arc::new(TokenLimiter::new(None)),
            breaker: CircuitBreaker::with_params("p", 1, 40),
        });
        entry.breaker.record_failure(); // open
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        entry.breaker.try_acquire().unwrap(); // half-open probe admitted
        let mut s = MeteredStream {
            inner: futures::stream::iter(vec![Err(MaestroError::Cancelled)]).boxed(),
            entry: entry.clone(),
            model: "m".into(),
            cost: None,
            settled: false,
        };
        let item = s.next().await;
        assert!(
            matches!(item, Some(Err(MaestroError::Cancelled))),
            "got {item:?}"
        );
        drop(s);
        entry
            .breaker
            .try_acquire()
            .expect("probe must be released on Cancelled item");
        let health = entry.breaker.to_health();
        assert_eq!(
            health.failure_count, 1,
            "cancellation must not count as failure"
        );
        assert_eq!(
            health.success_count, 0,
            "cancellation must not count as success"
        );
    }

    #[tokio::test]
    async fn discover_models_merges_configured_refs() {
        let mut config = mock_config();
        // add a fallback ref that live discovery will not report
        if let Some(assign) = config.roles.get_mut("coder") {
            assign.fallbacks.push(ModelRef {
                provider: "mock".into(),
                model: "not-a-real-mock-model".into(),
            });
        }
        let reg = ProviderRegistry::new(&config);
        let discovery = reg.discover_models().await;
        assert!(
            discovery.errors.is_empty(),
            "errors: {:?}",
            discovery.errors
        );
        let discovered: Vec<_> = discovery.models.iter().filter(|m| m.discovered).collect();
        assert!(discovered.iter().any(|m| m.model == "planner"));
        let merged = discovery
            .models
            .iter()
            .find(|m| m.model == "not-a-real-mock-model")
            .unwrap();
        assert!(!merged.discovered);
        assert_eq!(merged.provider_id, "mock");
    }

    #[tokio::test]
    async fn discover_models_collects_per_provider_errors() {
        let mut config = mock_config();
        config.providers.insert(
            "dead2".into(),
            maestro_types::config::ProviderConfig {
                kind: ProviderKind::OpenaiCompatible,
                base_url: Some("http://127.0.0.1:1".into()),
                api_key_env: None,
                headers: Default::default(),
                requests_per_minute: None,
                tokens_per_minute: None,
                deployment: None,
                reasoning_effort: None,
            },
        );
        let reg = ProviderRegistry::new(&config);
        let discovery = reg.discover_models().await;
        assert!(discovery.errors.contains_key("dead2"));
        // the healthy mock provider still discovered fine
        assert!(discovery
            .models
            .iter()
            .any(|m| m.provider_id == "mock" && m.discovered));
    }
}
