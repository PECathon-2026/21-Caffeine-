//! The [`Provider`] trait plus shared HTTP-error mapping, cancellation and
//! timeout helpers used by every transport.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::stream::BoxStream;
use futures::Stream;
use maestro_types::provider::{ModelRequest, ModelResponse, StreamEvent};
use maestro_types::{MaestroError, Result};
use tokio_util::sync::CancellationToken;

/// A model backend. Implementations must be cheap to share (`Arc<dyn Provider>`).
///
/// Timeout policy: the per-request timeout is supplied to each transport's
/// constructor (the registry passes `RuntimeConfig::request_timeout_ms`).
/// Cancellation policy: cancelling the token aborts the in-flight HTTP
/// request promptly and yields [`MaestroError::Cancelled`].
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// One-shot completion. Respects the configured timeout and the token.
    async fn generate(
        &self,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<ModelResponse>;

    /// Streaming: yields `StreamEvent::Delta` items then exactly one
    /// `StreamEvent::Done` on success. The configured timeout acts as a
    /// deadline over the whole stream.
    async fn stream(
        &self,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>>;

    /// Live model discovery (e.g. `GET /v1/models`).
    async fn list_models(&self) -> Result<Vec<String>>;
}

/// Parse `Retry-After` (delta-seconds or RFC 7231 http-date). Header lookup
/// is case-insensitive, so this also covers Anthropic's `retry-after`.
pub(crate) fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let raw = raw.trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some((secs as i64).saturating_mul(1000));
    }
    let when = httpdate::parse_http_date(raw).ok()?;
    match when.duration_since(std::time::SystemTime::now()) {
        Ok(d) => Some(d.as_millis() as i64),
        Err(_) => Some(0), // date already passed: retry immediately
    }
}

/// Map a non-success HTTP status to `MaestroError::Provider`.
/// 408, 429 and 5xx (plus network failures elsewhere) are retryable.
pub(crate) fn map_status_error(
    provider: &str,
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> MaestroError {
    let retryable = status.as_u16() == 408 || status.as_u16() == 429 || status.is_server_error();
    let snippet: String = body.chars().take(400).collect();
    MaestroError::Provider {
        provider: provider.to_string(),
        message: format!("http {status}: {snippet}"),
        retryable,
        // Azure/Foundry gateways often omit Retry-After and instead embed
        // "Please wait N seconds" in the 429 body — honor either.
        retry_after_ms: retry_after_ms(headers).or_else(|| retry_after_from_body(body)),
    }
}

/// Extract a "wait N seconds" hint from an error body (Azure OpenAI /
/// Foundry style: "Please wait 14 seconds before retrying."). Capped at
/// 120s so a garbled number cannot stall a worker.
pub(crate) fn retry_after_from_body(body: &str) -> Option<i64> {
    let lower = body.to_ascii_lowercase();
    let idx = lower.find("wait ")?;
    let rest = &lower[idx + 5..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let secs: i64 = digits.parse().ok()?;
    if !rest[digits.len()..].trim_start().starts_with("second") {
        return None;
    }
    Some(secs.clamp(1, 120).saturating_mul(1000))
}

/// Map a reqwest transport error (connect, TLS, reset, decode…) — always
/// retryable, since the request may never have reached the server.
pub(crate) fn map_transport_error(provider: &str, err: &reqwest::Error) -> MaestroError {
    MaestroError::Provider {
        provider: provider.to_string(),
        message: format!("transport: {err}"),
        retryable: true,
        retry_after_ms: None,
    }
}

/// The error produced when the configured request timeout elapses.
pub(crate) fn timeout_error(provider: &str, timeout: Duration) -> MaestroError {
    MaestroError::Provider {
        provider: provider.to_string(),
        message: format!("request timed out after {} ms", timeout.as_millis()),
        retryable: true,
        retry_after_ms: None,
    }
}

/// Drive `fut` racing against cancellation and the configured timeout.
/// Dropping `fut` (on either losing branch) aborts the in-flight HTTP request.
pub(crate) async fn drive<T, F>(
    provider: &str,
    timeout: Duration,
    cancel: &CancellationToken,
    fut: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(MaestroError::Cancelled),
        _ = tokio::time::sleep(timeout) => Err(timeout_error(provider, timeout)),
        out = fut => out,
    }
}

/// A stream backed by a pump task; aborts the task when dropped so no
/// detached work outlives the consumer (SPEC "no leaks" rule).
pub(crate) struct OwnedEventStream {
    rx: tokio_stream::wrappers::ReceiverStream<Result<StreamEvent>>,
    handle: tokio::task::JoinHandle<()>,
}

impl OwnedEventStream {
    pub(crate) fn new(
        rx: tokio::sync::mpsc::Receiver<Result<StreamEvent>>,
        handle: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            rx: tokio_stream::wrappers::ReceiverStream::new(rx),
            handle,
        }
    }
}

impl Stream for OwnedEventStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl Drop for OwnedEventStream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[cfg(test)]
mod body_hint_tests {
    use super::retry_after_from_body;

    #[test]
    fn azure_wait_hint_is_parsed_and_capped() {
        let azure = r#"{"error":{"code":"RateLimitReached","message":"Rate limit of 5000 per 60s exceeded for UserByModelByMinuteOutputTokens. Please wait 14 seconds before retrying."}}"#;
        assert_eq!(retry_after_from_body(azure), Some(14_000));
        assert_eq!(
            retry_after_from_body("please wait 999999 seconds"),
            Some(120_000),
            "hint is capped"
        );
        assert_eq!(retry_after_from_body("wait for it"), None);
        assert_eq!(retry_after_from_body("wait 5 minutes"), None);
        assert_eq!(retry_after_from_body("{\"error\":\"overloaded\"}"), None);
    }
}
