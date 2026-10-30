//! Model-provider layer for Maestro.
//!
//! This crate implements the [`Provider`] trait over HTTP transports —
//! Anthropic ([`anthropic::AnthropicProvider`]), OpenAI-compatible
//! ([`openai_compatible::OpenAiCompatibleProvider`]) including Responses; deterministic fixtures are compiled only for tests or
//! the explicit `test-support` feature — plus the supporting machinery
//! the engine codes against:
//!
//! * [`registry::ProviderRegistry`] — lazy provider construction from
//!   [`maestro_types::config::MaestroConfig`], per-provider rate limiting and
//!   circuit breaking, cost metering, and concurrent model discovery.
//! * [`pricing`] — static model-prefix pricing table producing integer
//!   micro-USD costs.
//! * [`limiter::RateLimiter`] — token-bucket requests/minute limiter.
//! * [`breaker::CircuitBreaker`] — consecutive-failure breaker with half-open
//!   probing, convertible to/from the persistable
//!   [`maestro_types::domain::ProviderHealth`] snapshot.
//!
//! ## Timeouts
//!
//! Every transport takes a request timeout in its constructor
//! (`timeout_ms`); the registry passes
//! [`maestro_types::config::RuntimeConfig::request_timeout_ms`] down. For
//! `generate` the timeout bounds the whole call; for `stream` it acts as a
//! deadline for the entire stream (connect + all chunks).
//!
//! ## Cancellation
//!
//! Every request accepts a `tokio_util::sync::CancellationToken`. Cancelling
//! it aborts the in-flight HTTP request promptly (the underlying `reqwest`
//! future/response is dropped) and surfaces
//! [`maestro_types::MaestroError::Cancelled`].
//!
//! ## Error mapping
//!
//! HTTP failures map to [`maestro_types::MaestroError::Provider`]:
//! 408/429/5xx and transport/network errors are `retryable`; `Retry-After`
//! (delta-seconds or http-date, which also covers Anthropic's lower-case
//! `retry-after`) is parsed into `retry_after_ms`.

pub mod anthropic;
pub mod breaker;
pub mod limiter;
#[cfg(any(test, feature = "test-support"))]
pub mod mock;
pub mod openai_compatible;
pub mod pricing;
pub mod provider;
pub mod registry;
pub(crate) mod sse;

pub use breaker::CircuitBreaker;
pub use limiter::RateLimiter;
pub use provider::Provider;
pub use registry::{MeteredStream, ProviderRegistry};
