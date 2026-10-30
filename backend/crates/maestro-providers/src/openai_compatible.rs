//! OpenAI-compatible Chat Completions transport
//! (`POST {base_url}/chat/completions`).
//!
//! Bearer auth is OPTIONAL: keyless local endpoints (ollama, llama.cpp,
//! vLLM…) work without an API key. If `api_key_env` is configured but unset
//! at request time the request proceeds keyless with a warning (hosted
//! endpoints will answer 401, which maps to a non-retryable provider error).
//!
//! `ModelRequest::json == true` sends `response_format: {"type":"json_object"}`.
//! Streaming sends `stream: true` + `stream_options: {"include_usage": true}`
//! and terminates on the `data: [DONE]` sentinel; usage is taken from the
//! final chunk when present.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use maestro_types::config::{ProviderConfig, ProviderKind};
use maestro_types::provider::{MessageRole, ModelRequest, ModelResponse, StreamEvent, Usage};
use maestro_types::{MaestroError, Result};
use tokio_util::sync::CancellationToken;

use crate::provider::{
    drive, map_status_error, map_transport_error, timeout_error, OwnedEventStream, Provider,
};
use crate::sse::SseParser;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI-compatible `/chat/completions` provider.
pub struct OpenAiCompatibleProvider {
    provider_id: String,
    client: reqwest::Client,
    base_url: String,
    api_key_env: Option<String>,
    extra_headers: BTreeMap<String, String>,
    timeout: Duration,
    responses: bool,
    reasoning_effort: Option<String>,
}

impl OpenAiCompatibleProvider {
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
            responses: cfg.kind == ProviderKind::OpenaiResponses,
            reasoning_effort: cfg.reasoning_effort.clone(),
            client,
            base_url: cfg
                .base_url
                .as_deref()
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
            api_key_env: cfg.api_key_env.clone(),
            extra_headers: cfg.headers.clone(),
            timeout: Duration::from_millis(timeout_ms.max(1) as u64),
        })
    }

    /// Bearer token, read lazily at request time. `None` = keyless.
    fn bearer(&self) -> Option<String> {
        let env = self.api_key_env.as_deref()?;
        match std::env::var(env) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => {
                tracing::warn!(
                    provider = %self.provider_id,
                    env,
                    "api_key_env configured but unset; proceeding keyless"
                );
                None
            }
        }
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let mut rb = self.client.post(format!("{}{path}", self.base_url));
        if let Some(token) = self.bearer() {
            rb = rb.bearer_auth(token);
        }
        for (k, v) in &self.extra_headers {
            rb = rb.header(k, v);
        }
        rb
    }

    fn completion_path(&self) -> &'static str {
        if self.responses {
            "/responses"
        } else {
            "/chat/completions"
        }
    }

    fn body(&self, req: &ModelRequest, stream: bool) -> serde_json::Value {
        let messages: Vec<serde_json::Value> = req
            .messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                };
                if m.images.is_empty() { return serde_json::json!({"role": role, "content": m.content}); }
                let mut content = vec![if self.responses { serde_json::json!({"type":"input_text","text":m.content}) } else { serde_json::json!({"type":"text","text":m.content}) }];
                content.extend(m.images.iter().map(|url| if self.responses { serde_json::json!({"type":"input_image","image_url":url,"detail":"auto"}) } else { serde_json::json!({"type":"image_url","image_url":{"url":url,"detail":"auto"}}) }));
                serde_json::json!({"role": role, "content": content})
            })
            .collect();
        if self.responses {
            let mut body = serde_json::json!({
                "model": req.model, "input": messages, "store": false, "stream": stream,
            });
            if let Some(effort) = &self.reasoning_effort {
                body["reasoning"] = serde_json::json!({"effort": effort});
            }
            if let Some(n) = req.max_tokens {
                body["max_output_tokens"] = n.into();
            }
            if let Some(t) = req.temperature {
                body["temperature"] = t.into();
            }
            if req.json {
                body["text"] = serde_json::json!({"format": {"type": "json_object"}});
            }
            return body;
        }
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": messages,
        });
        if let Some(n) = req.max_tokens {
            body["max_tokens"] = serde_json::json!(n);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        if req.json {
            body["response_format"] = serde_json::json!({"type": "json_object"});
        }
        if stream {
            body["stream"] = serde_json::Value::Bool(true);
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }
        body
    }

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

fn parse_usage(v: &serde_json::Value) -> Option<Usage> {
    let u = v.get("usage")?;
    if u.is_null() {
        return None;
    }
    Some(Usage {
        input_tokens: u["prompt_tokens"].as_i64().unwrap_or(0),
        output_tokens: u["completion_tokens"].as_i64().unwrap_or(0),
    })
}

#[async_trait::async_trait]
impl Provider for OpenAiCompatibleProvider {
    async fn generate(
        &self,
        req: &ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<ModelResponse> {
        let started = std::time::Instant::now();
        let fut = async {
            let resp = self
                .send_checked(
                    self.request(self.completion_path())
                        .json(&self.body(req, false)),
                )
                .await?;
            let v: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| map_transport_error(&self.provider_id, &e))?;
            if self.responses {
                return parse_response(
                    &self.provider_id,
                    &v,
                    &req.model,
                    started.elapsed().as_millis() as i64,
                );
            }
            let text = v["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let usage = parse_usage(&v).unwrap_or_default();
            let model = v["model"].as_str().unwrap_or(&req.model).to_string();
            // finish_reason "length" means the turn is truncated — surface
            // that instead of letting cut-off text fail protocol parsing.
            if v["choices"][0]["finish_reason"] == "length" {
                return Err(MaestroError::Provider {
                    provider: self.provider_id.clone(),
                    message: format!(
                        "response truncated at the token limit ({} output tokens); \
                         the turn was discarded",
                        usage.output_tokens
                    ),
                    retryable: false,
                    retry_after_ms: None,
                });
            }
            if text.trim().is_empty() {
                return Err(response_error(
                    &self.provider_id,
                    "provider returned no assistant text",
                ));
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
        let started = std::time::Instant::now();
        let deadline = tokio::time::Instant::now() + self.timeout;
        let resp = drive(&self.provider_id, self.timeout, cancel, async {
            self.send_checked(
                self.request(self.completion_path())
                    .json(&self.body(req, true)),
            )
            .await
        })
        .await?;

        let provider_id = self.provider_id.clone();
        let timeout = self.timeout;
        let cancel = cancel.clone();
        let fallback_model = req.model.clone();
        let responses = self.responses;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamEvent>>(64);
        // Pump task: owned by the returned stream (aborted on drop), exits on
        // its own when the receiver goes away.
        let handle = tokio::spawn(async move {
            let mut bytes = resp.bytes_stream();
            let mut parser = SseParser::new();
            let mut usage = Usage::default();
            let mut model = fallback_model;
            let done = |usage: Usage, model: String| StreamEvent::Done {
                usage,
                model,
                latency_ms: started.elapsed().as_millis() as i64,
            };
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
                        if responses {
                            let _ = tx
                                .send(Err(response_error(
                                    &provider_id,
                                    "Responses stream ended before response.completed",
                                )))
                                .await;
                            return;
                        }
                        // Lenient EOF: some local servers close without [DONE].
                        tracing::warn!(provider = %provider_id, "stream closed without [DONE]");
                        let _ = tx.send(Ok(done(usage, model))).await;
                        return;
                    }
                    Some(Err(e)) => {
                        let _ = tx.send(Err(map_transport_error(&provider_id, &e))).await;
                        return;
                    }
                    Some(Ok(b)) => b,
                };
                for ev in parser.feed(&data) {
                    if ev.data.trim() == "[DONE]" {
                        let result = if responses {
                            Err(response_error(
                                &provider_id,
                                "Responses stream ended before response.completed",
                            ))
                        } else {
                            Ok(done(usage, model))
                        };
                        let _ = tx.send(result).await;
                        return;
                    }
                    let payload: serde_json::Value = match serde_json::from_str(&ev.data) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    if responses {
                        match payload["type"].as_str() {
                            Some("response.output_text.delta") => {
                                if let Some(t) = payload["delta"].as_str() {
                                    if tx
                                        .send(Ok(StreamEvent::Delta { text: t.into() }))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Some(
                                "response.completed" | "response.incomplete" | "response.failed",
                            ) => {
                                let result = parse_response(
                                    &provider_id,
                                    &payload["response"],
                                    &model,
                                    started.elapsed().as_millis() as i64,
                                )
                                .map(|r| done(r.usage, r.model));
                                let _ = tx.send(result).await;
                                return;
                            }
                            Some("error") => {
                                let _ = tx
                                    .send(Err(response_error(
                                        &provider_id,
                                        &format!("Responses stream error: {}", payload["message"]),
                                    )))
                                    .await;
                                return;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if matches!(
                        payload["choices"][0]["finish_reason"].as_str(),
                        Some("length" | "content_filter")
                    ) {
                        let _ = tx
                            .send(Err(response_error(
                                &provider_id,
                                "completion stream truncated or filtered",
                            )))
                            .await;
                        return;
                    }
                    if let Some(m) = payload["model"].as_str() {
                        model = m.to_string();
                    }
                    if let Some(u) = parse_usage(&payload) {
                        usage = u; // final chunk carries usage under include_usage
                    }
                    if let Some(t) = payload["choices"][0]["delta"]["content"].as_str() {
                        if !t.is_empty()
                            && tx
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
        });
        Ok(OwnedEventStream::new(rx, handle).boxed())
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let fut = async {
            let mut rb = self.client.get(format!("{}/models", self.base_url));
            if let Some(token) = self.bearer() {
                rb = rb.bearer_auth(token);
            }
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

fn response_error(provider: &str, message: &str) -> MaestroError {
    MaestroError::Provider {
        provider: provider.into(),
        message: message.into(),
        retryable: false,
        retry_after_ms: None,
    }
}

/// Only assistant output_text belongs in the agent protocol; reasoning and refusals
/// are not tool instructions. Never accept a partial/failed response as a final turn.
fn parse_response(
    provider: &str,
    v: &serde_json::Value,
    fallback: &str,
    latency_ms: i64,
) -> Result<ModelResponse> {
    if v["status"] != "completed" {
        return Err(response_error(
            provider,
            &format!(
                "Responses request did not complete (status: {}, reason: {}, error: {})",
                v["status"], v["incomplete_details"], v["error"]
            ),
        ));
    }
    let mut text = String::new();
    if let Some(items) = v["output"].as_array() {
        for item in items {
            if item["type"] != "message" || item["role"] != "assistant" {
                continue;
            }
            if let Some(parts) = item["content"].as_array() {
                for part in parts {
                    if part["type"] == "output_text" {
                        if let Some(t) = part["text"].as_str() {
                            text.push_str(t);
                        }
                    }
                }
            }
        }
    }
    if text.trim().is_empty() {
        return Err(response_error(
            provider,
            "Responses returned no assistant text (empty output or refusal)",
        ));
    }
    Ok(ModelResponse {
        text,
        model: v["model"].as_str().unwrap_or(fallback).into(),
        latency_ms,
        usage: Usage {
            input_tokens: v["usage"]["input_tokens"].as_i64().unwrap_or(0),
            output_tokens: v["usage"]["output_tokens"].as_i64().unwrap_or(0),
        },
    })
}

#[cfg(test)]
mod vision_tests {
    use super::*;
    use maestro_types::provider::ModelMessage;
    #[test]
    fn images_are_encoded_for_responses_and_chat_completions() {
        let mut message = ModelMessage::user("What is shown?");
        message.images.push("data:image/jpeg;base64,PIXELS".into());
        let req = ModelRequest {
            model: "fixed".into(),
            messages: vec![message],
            max_tokens: Some(1024),
            temperature: None,
            json: false,
        };
        for kind in [
            ProviderKind::OpenaiResponses,
            ProviderKind::OpenaiCompatible,
        ] {
            let cfg: ProviderConfig =
                serde_json::from_value(serde_json::json!({"kind":kind})).unwrap();
            let p = OpenAiCompatibleProvider::new("test", &cfg, 1000).unwrap();
            let body = p.body(&req, false);
            if kind == ProviderKind::OpenaiResponses {
                assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
                assert_eq!(
                    body["input"][0]["content"][1]["image_url"],
                    req.messages[0].images[0]
                );
            } else {
                assert_eq!(
                    body["messages"][0]["content"][1]["image_url"]["url"],
                    req.messages[0].images[0]
                );
            }
        }
    }
}
