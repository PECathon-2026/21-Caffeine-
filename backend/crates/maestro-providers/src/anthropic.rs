//! Anthropic Messages API transport (`POST {base_url}/v1/messages`).
//!
//! Auth: `x-api-key` read lazily at request time from the env var named by
//! `ProviderConfig::api_key_env` (default `ANTHROPIC_API_KEY`), plus
//! `anthropic-version: 2023-06-01`. Streaming uses hand-rolled SSE parsing
//! over `reqwest::Response::bytes_stream` (see [`crate::sse`]).
//!
//! Note: the Messages API has no strict-JSON response format, so
//! `ModelRequest::json` is ignored here (JSON discipline comes from prompts).

use std::collections::BTreeMap;
use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use maestro_types::config::ProviderConfig;
use maestro_types::provider::{MessageRole, ModelRequest, ModelResponse, StreamEvent, Usage};
use maestro_types::{MaestroError, Result};
use tokio_util::sync::CancellationToken;

use crate::provider::{
    drive, map_status_error, map_transport_error, timeout_error, OwnedEventStream, Provider,
};
use crate::sse::SseParser;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// The Messages API requires `max_tokens`; used when the request omits it.
/// Sized for agent turns that inline whole files (a `write_file` turn is
/// densely escaped JSON, near one token per character): 4096 truncated
/// real-world write turns mid-string.
const DEFAULT_MAX_TOKENS: u32 = 16_384;

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    provider_id: String,
    client: reqwest::Client,
    base_url: String,
    api_key_env: String,
    extra_headers: BTreeMap<String, String>,
    timeout: Duration,
}

impl AnthropicProvider {
    /// Build from config. `timeout_ms` is the per-request timeout, normally
    /// `RuntimeConfig::request_timeout_ms`.
    pub fn new(
        provider_id: impl Into<String>,
        cfg: &ProviderConfig,
        timeout_ms: i64,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| MaestroError::Config(format!("http client: {e}")))?;
        Ok(Self {
            provider_id: provider_id.into(),
            client,
            base_url: cfg
                .base_url
                .as_deref()
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
            api_key_env: cfg
                .api_key_env
                .clone()
                .unwrap_or_else(|| DEFAULT_API_KEY_ENV.to_string()),
            extra_headers: cfg.headers.clone(),
            timeout: Duration::from_millis(timeout_ms.max(1) as u64),
        })
    }

    /// Credentials are read lazily at request time — never cached — so the
    /// daemon's credential injection (env) is picked up without a rebuild.
    fn api_key(&self) -> Result<String> {
        std::env::var(&self.api_key_env).map_err(|_| {
            MaestroError::Config(format!(
                "provider '{}': environment variable {} is not set",
                self.provider_id, self.api_key_env
            ))
        })
    }

    fn request(&self, path: &str, key: &str) -> reqwest::RequestBuilder {
        let mut rb = self
            .client
            .post(format!("{}{path}", self.base_url))
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION);
        for (k, v) in &self.extra_headers {
            rb = rb.header(k, v);
        }
        rb
    }

    fn body(&self, req: &ModelRequest, stream: bool) -> serde_json::Value {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut messages: Vec<serde_json::Value> = Vec::new();
        for m in &req.messages {
            match m.role {
                MessageRole::System => system_parts.push(&m.content),
                MessageRole::User => {
                    let mut content = vec![serde_json::json!({"type":"text","text":m.content})];
                    for url in &m.images {
                        if let Some((header, data)) = url
                            .strip_prefix("data:")
                            .and_then(|s| s.split_once(";base64,"))
                        {
                            content.push(serde_json::json!({"type":"image","source":{"type":"base64","media_type":header,"data":data}}));
                        } else {
                            content.push(serde_json::json!({"type":"image","source":{"type":"url","url":url}}));
                        }
                    }
                    messages.push(serde_json::json!({"role":"user","content":content}))
                }
                MessageRole::Assistant => {
                    messages.push(serde_json::json!({"role": "assistant", "content": m.content}))
                }
            }
        }
        let mut body = serde_json::json!({
            "model": req.model,
            "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "messages": messages,
        });
        if !system_parts.is_empty() {
            body["system"] = serde_json::Value::String(system_parts.join("\n\n"));
        }
        if let Some(t) = req.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        if stream {
            body["stream"] = serde_json::Value::Bool(true);
        }
        if req.json {
            // No response_format on this API; JSON output is prompt-enforced.
            tracing::debug!(provider = %self.provider_id, "json flag ignored by anthropic transport");
        }
        body
    }

    /// Send and fail non-2xx statuses into `MaestroError::Provider`.
    async fn send_checked(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let resp = rb
            .send()
            .await
            .map_err(|e| map_transport_error(&self.provider_id, &e))?;
        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_status_error(&self.provider_id, status, &headers, &body));
        }
        Ok(resp)
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    async fn generate(
        &self,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<ModelResponse> {
        let key = self.api_key()?;
        let started = std::time::Instant::now();
        let fut = async {
            let resp = self
                .send_checked(
                    self.request("/v1/messages", &key)
                        .json(&self.body(req, false)),
                )
                .await?;
            let v: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| map_transport_error(&self.provider_id, &e))?;
            let text = v["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            let usage = Usage {
                input_tokens: v["usage"]["input_tokens"].as_i64().unwrap_or(0),
                output_tokens: v["usage"]["output_tokens"].as_i64().unwrap_or(0),
            };
            let model = v["model"].as_str().unwrap_or(&req.model).to_string();
            // A max_tokens stop means the turn is truncated — surface that
            // instead of letting the cut-off text fail protocol parsing.
            if v["stop_reason"] == "max_tokens" {
                return Err(MaestroError::Provider {
                    provider: self.provider_id.clone(),
                    message: format!(
                        "response truncated at the max_tokens cap ({} output tokens); \
                         the turn was discarded",
                        usage.output_tokens
                    ),
                    retryable: false,
                    retry_after_ms: None,
                });
            }
            Ok(ModelResponse {
                text,
                usage,
                model,
                latency_ms: started.elapsed().as_millis() as i64,
            })
        };
        drive(&self.provider_id, self.timeout, cancel, fut).await
    }

    async fn stream(
        &self,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let key = self.api_key()?;
        let started = std::time::Instant::now();
        let deadline = tokio::time::Instant::now() + self.timeout;
        // Establish the connection (headers) under cancel + timeout.
        let resp = drive(&self.provider_id, self.timeout, cancel, async {
            self.send_checked(
                self.request("/v1/messages", &key)
                    .json(&self.body(req, true)),
            )
            .await
        })
        .await?;

        let provider_id = self.provider_id.clone();
        let timeout = self.timeout;
        let cancel = cancel.clone();
        let fallback_model = req.model.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamEvent>>(64);
        // The pump task is owned by the returned stream (aborted on drop) and
        // exits by itself when the receiver goes away.
        let handle = tokio::spawn(async move {
            let mut bytes = resp.bytes_stream();
            let mut parser = SseParser::new();
            let mut usage = Usage::default();
            let mut model = fallback_model;
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        let _ = tx.send(Err(MaestroError::Cancelled)).await;
                        return;
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        let _ = tx.send(Err(timeout_error(&provider_id, timeout))).await;
                        return;
                    }
                    c = bytes.next() => c,
                };
                let data = match chunk {
                    None => {
                        let _ = tx
                            .send(Err(MaestroError::Provider {
                                provider: provider_id.clone(),
                                message: "stream ended before message_stop".into(),
                                retryable: true,
                                retry_after_ms: None,
                            }))
                            .await;
                        return;
                    }
                    Some(Err(e)) => {
                        let _ = tx.send(Err(map_transport_error(&provider_id, &e))).await;
                        return;
                    }
                    Some(Ok(b)) => b,
                };
                for ev in parser.feed(&data) {
                    let payload: serde_json::Value =
                        serde_json::from_str(&ev.data).unwrap_or(serde_json::Value::Null);
                    match ev.event.as_deref() {
                        Some("message_start") => {
                            if let Some(n) = payload["message"]["usage"]["input_tokens"].as_i64() {
                                usage.input_tokens = n;
                            }
                            if let Some(m) = payload["message"]["model"].as_str() {
                                model = m.to_string();
                            }
                        }
                        Some("content_block_delta") => {
                            if payload["delta"]["type"] == "text_delta" {
                                if let Some(t) = payload["delta"]["text"].as_str() {
                                    if tx
                                        .send(Ok(StreamEvent::Delta {
                                            text: t.to_string(),
                                        }))
                                        .await
                                        .is_err()
                                    {
                                        return; // consumer dropped
                                    }
                                }
                            }
                        }
                        Some("message_delta") => {
                            if let Some(n) = payload["usage"]["output_tokens"].as_i64() {
                                usage.output_tokens = n;
                            }
                            if let Some(n) = payload["usage"]["input_tokens"].as_i64() {
                                usage.input_tokens = n;
                            }
                        }
                        Some("message_stop") => {
                            let _ = tx
                                .send(Ok(StreamEvent::Done {
                                    usage,
                                    model,
                                    latency_ms: started.elapsed().as_millis() as i64,
                                }))
                                .await;
                            return;
                        }
                        Some("error") => {
                            let msg = payload["error"]["message"]
                                .as_str()
                                .unwrap_or("stream error")
                                .to_string();
                            let overloaded = payload["error"]["type"] == "overloaded_error";
                            let _ = tx
                                .send(Err(MaestroError::Provider {
                                    provider: provider_id.clone(),
                                    message: format!("stream error: {msg}"),
                                    retryable: overloaded,
                                    retry_after_ms: None,
                                }))
                                .await;
                            return;
                        }
                        _ => {} // ping, content_block_start/stop…
                    }
                }
            }
        });
        Ok(OwnedEventStream::new(rx, handle).boxed())
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let key = self.api_key()?;
        let fut = async {
            let mut rb = self
                .client
                .get(format!("{}/v1/models?limit=1000", self.base_url))
                .header("x-api-key", &key)
                .header("anthropic-version", ANTHROPIC_VERSION);
            for (k, v) in &self.extra_headers {
                rb = rb.header(k, v);
            }
            let resp = self.send_checked(rb).await?;
            let v: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| map_transport_error(&self.provider_id, &e))?;
            Ok(v["data"]
                .as_array()
                .map(|models| {
                    models
                        .iter()
                        .filter_map(|m| m["id"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default())
        };
        drive(
            &self.provider_id,
            self.timeout,
            &CancellationToken::new(),
            fut,
        )
        .await
    }
}
