//! Transport tests against a local fake server: a raw `tokio::net::TcpListener`
//! serving hand-written HTTP/1.1 responses (no extra dev-dependencies).

use std::time::Duration;

use futures::StreamExt;
use maestro_providers::anthropic::AnthropicProvider;
use maestro_providers::openai_compatible::OpenAiCompatibleProvider;
use maestro_providers::Provider;
use maestro_types::config::{ProviderConfig, ProviderKind};
use maestro_types::provider::{ModelMessage, ModelRequest, StreamEvent};
use maestro_types::MaestroError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Serve exactly one connection: read the full request (headers +
/// content-length body), send `response` verbatim, and hand the raw request
/// back through the oneshot.
async fn serve_once(response: String) -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let request = read_request(&mut sock).await;
        sock.write_all(response.as_bytes()).await.unwrap();
        sock.flush().await.unwrap();
        let _ = tx.send(request);
        // give the client a moment to drain before close
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    (format!("http://{addr}"), rx)
}

/// Serve one connection that never answers (for cancellation/timeout tests).
async fn serve_stall() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut sock).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    format!("http://{addr}")
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    // read until end of headers
    let header_end = loop {
        let n = sock.read(&mut chunk).await.unwrap();
        if n == 0 {
            return String::from_utf8_lossy(&buf).into_owned();
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
    };
    // honor content-length so the client finishes writing
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let content_length: usize = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = sock.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn http_response(status: &str, content_type: &str, extra_headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n{extra_headers}connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn cfg(kind: ProviderKind, base_url: &str, api_key_env: Option<&str>) -> ProviderConfig {
    ProviderConfig {
        kind,
        base_url: Some(base_url.to_string()),
        api_key_env: api_key_env.map(str::to_string),
        headers: Default::default(),
        requests_per_minute: None,
        tokens_per_minute: None,
        deployment: None,
        reasoning_effort: None,
    }
}

fn request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.into(),
        messages: vec![
            ModelMessage::system("be brief"),
            ModelMessage::user("hello"),
        ],
        max_tokens: Some(64),
        temperature: Some(0.2),
        json: false,
    }
}

fn set_key(env: &str) {
    std::env::set_var(env, "test-key-123");
}

// ---------------------------------------------------------------- anthropic

#[tokio::test]
async fn anthropic_generate_joins_text_blocks_and_reads_usage() {
    set_key("MAESTRO_T_A1");
    let body = serde_json::json!({
        "model": "claude-sonnet-5",
        "content": [
            {"type": "text", "text": "Hello "},
            {"type": "tool_use", "id": "x", "name": "n", "input": {}},
            {"type": "text", "text": "world"}
        ],
        "usage": {"input_tokens": 12, "output_tokens": 7}
    })
    .to_string();
    let (base, rx) = serve_once(http_response("200 OK", "application/json", "", &body)).await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A1")),
        5_000,
    )
    .unwrap();
    let resp = p
        .generate(&request("claude-sonnet-5"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.text, "Hello world");
    assert_eq!(resp.usage.input_tokens, 12);
    assert_eq!(resp.usage.output_tokens, 7);
    assert_eq!(resp.model, "claude-sonnet-5");

    let raw = rx.await.unwrap();
    assert!(raw.starts_with("POST /v1/messages"));
    assert!(raw.contains("x-api-key: test-key-123"));
    assert!(raw.contains("anthropic-version: 2023-06-01"));
    // system messages are lifted out of the messages array
    assert!(raw.contains("\"system\":\"be brief\""));
    assert!(raw.contains("\"max_tokens\":64"));
}

#[tokio::test]
async fn anthropic_max_tokens_truncation_is_a_clear_error_not_garbled_text() {
    set_key("MAESTRO_T_A9");
    let body = serde_json::json!({
        "model": "claude-sonnet-5",
        "content": [{"type": "text", "text": "{\"kind\":\"tool\",\"tool\":\"write_file\",\"argu"}],
        "stop_reason": "max_tokens",
        "usage": {"input_tokens": 12, "output_tokens": 4096}
    })
    .to_string();
    let (base, _rx) = serve_once(http_response("200 OK", "application/json", "", &body)).await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A9")),
        5_000,
    )
    .unwrap();
    let err = p
        .generate(&request("m"), &CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        MaestroError::Provider {
            retryable, message, ..
        } => {
            assert!(!retryable);
            assert!(message.contains("truncated"), "message: {message}");
        }
        other => panic!("expected provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn openai_length_finish_is_a_clear_error_not_garbled_text() {
    set_key("MAESTRO_T_O9");
    let body = serde_json::json!({
        "model": "m",
        "choices": [{"message": {"content": "{\"kind\":\"tool\",\"tool\":\"wri"}, "finish_reason": "length"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2048}
    })
    .to_string();
    let (base, _rx) = serve_once(http_response("200 OK", "application/json", "", &body)).await;
    let p = OpenAiCompatibleProvider::new(
        "o",
        &cfg(ProviderKind::OpenaiCompatible, &base, Some("MAESTRO_T_O9")),
        5_000,
    )
    .unwrap();
    let err = p
        .generate(&request("m"), &CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        MaestroError::Provider {
            retryable, message, ..
        } => {
            assert!(!retryable);
            assert!(message.contains("truncated"), "message: {message}");
        }
        other => panic!("expected provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn anthropic_stream_yields_deltas_then_done_with_usage() {
    set_key("MAESTRO_T_A2");
    let sse = concat!(
        "event: message_start\n",
        "data: {\"message\":{\"model\":\"claude-sonnet-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
        "event: message_delta\n",
        "data: {\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {}\n\n",
    );
    let (base, _rx) = serve_once(http_response("200 OK", "text/event-stream", "", sse)).await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A2")),
        5_000,
    )
    .unwrap();
    let mut s = p
        .stream(&request("claude-sonnet-5"), &CancellationToken::new())
        .await
        .unwrap();

    let mut text = String::new();
    let mut dones = 0;
    while let Some(ev) = s.next().await {
        match ev.unwrap() {
            StreamEvent::Delta { text: t } => text.push_str(&t),
            StreamEvent::Done { usage, model, .. } => {
                dones += 1;
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
                assert_eq!(model, "claude-sonnet-5");
            }
        }
    }
    assert_eq!(text, "Hello");
    assert_eq!(dones, 1);
}

#[tokio::test]
async fn http_429_is_retryable_with_retry_after_seconds() {
    set_key("MAESTRO_T_A3");
    let (base, _rx) = serve_once(http_response(
        "429 Too Many Requests",
        "application/json",
        "retry-after: 5\r\n",
        "{\"error\":{\"message\":\"rate limited\"}}",
    ))
    .await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A3")),
        5_000,
    )
    .unwrap();
    let err = p
        .generate(&request("m"), &CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        MaestroError::Provider {
            retryable,
            retry_after_ms,
            message,
            ..
        } => {
            assert!(retryable);
            assert_eq!(retry_after_ms, Some(5_000));
            assert!(message.contains("429"));
        }
        other => panic!("expected provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn retry_after_http_date_is_parsed() {
    set_key("MAESTRO_T_A4");
    let when = std::time::SystemTime::now() + Duration::from_secs(7);
    let (base, _rx) = serve_once(http_response(
        "503 Service Unavailable",
        "application/json",
        &format!("Retry-After: {}\r\n", httpdate::fmt_http_date(when)),
        "overloaded",
    ))
    .await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A4")),
        5_000,
    )
    .unwrap();
    let err = p
        .generate(&request("m"), &CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        MaestroError::Provider {
            retryable,
            retry_after_ms,
            ..
        } => {
            assert!(retryable);
            let ms = retry_after_ms.unwrap();
            assert!(ms > 0 && ms <= 8_000, "retry_after_ms = {ms}");
        }
        other => panic!("expected provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn http_400_is_not_retryable() {
    set_key("MAESTRO_T_A5");
    let (base, _rx) = serve_once(http_response(
        "400 Bad Request",
        "application/json",
        "",
        "{\"error\":{\"message\":\"bad\"}}",
    ))
    .await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A5")),
        5_000,
    )
    .unwrap();
    let err = p
        .generate(&request("m"), &CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        MaestroError::Provider { retryable, .. } => assert!(!retryable),
        other => panic!("expected provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_api_key_is_a_config_error() {
    let p = AnthropicProvider::new(
        "a",
        &cfg(
            ProviderKind::Anthropic,
            "http://127.0.0.1:9",
            Some("MAESTRO_T_UNSET_VAR"),
        ),
        1_000,
    )
    .unwrap();
    let err = p
        .generate(&request("m"), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, MaestroError::Config(_)), "got {err:?}");
}

#[tokio::test]
async fn cancellation_aborts_in_flight_request_promptly() {
    set_key("MAESTRO_T_A6");
    let base = serve_stall().await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A6")),
        60_000,
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.cancel();
    });
    let started = std::time::Instant::now();
    let err = p.generate(&request("m"), &cancel).await.unwrap_err();
    assert!(matches!(err, MaestroError::Cancelled));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancel was not prompt"
    );
}

#[tokio::test]
async fn timeout_maps_to_retryable_provider_error() {
    set_key("MAESTRO_T_A7");
    let base = serve_stall().await;
    let p = AnthropicProvider::new(
        "a",
        &cfg(ProviderKind::Anthropic, &base, Some("MAESTRO_T_A7")),
        100,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let err = p
        .generate(&request("m"), &CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        MaestroError::Provider {
            retryable, message, ..
        } => {
            assert!(retryable);
            assert!(message.contains("timed out"), "message: {message}");
        }
        other => panic!("expected provider error, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
}

// ------------------------------------------------------- openai-compatible

#[tokio::test]
async fn openai_generate_parses_choice_and_usage_and_sends_json_format() {
    set_key("MAESTRO_T_O1");
    let body = serde_json::json!({
        "model": "gpt-4o",
        "choices": [{"message": {"role": "assistant", "content": "hi there"}}],
        "usage": {"prompt_tokens": 20, "completion_tokens": 4}
    })
    .to_string();
    let (base, rx) = serve_once(http_response("200 OK", "application/json", "", &body)).await;
    let p = OpenAiCompatibleProvider::new(
        "o",
        &cfg(ProviderKind::OpenaiCompatible, &base, Some("MAESTRO_T_O1")),
        5_000,
    )
    .unwrap();
    let mut req = request("gpt-4o");
    req.json = true;
    let resp = p.generate(&req, &CancellationToken::new()).await.unwrap();
    assert_eq!(resp.text, "hi there");
    assert_eq!(resp.usage.input_tokens, 20);
    assert_eq!(resp.usage.output_tokens, 4);

    let raw = rx.await.unwrap();
    assert!(raw.starts_with("POST /chat/completions"));
    assert!(raw.contains("authorization: Bearer test-key-123"));
    assert!(raw.contains("\"response_format\":{\"type\":\"json_object\"}"));
    // system message stays inline for openai-compatible
    assert!(raw.contains("\"role\":\"system\""));
}

#[tokio::test]
async fn openai_keyless_endpoint_sends_no_auth_header() {
    let body = serde_json::json!({
        "choices": [{"message": {"content": "local"}}],
    })
    .to_string();
    let (base, rx) = serve_once(http_response("200 OK", "application/json", "", &body)).await;
    let p = OpenAiCompatibleProvider::new(
        "o",
        &cfg(ProviderKind::OpenaiCompatible, &base, None),
        5_000,
    )
    .unwrap();
    let resp = p
        .generate(&request("local-model"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.text, "local");
    assert_eq!(resp.usage.input_tokens, 0); // usage absent → zeroed

    let raw = rx.await.unwrap().to_ascii_lowercase();
    assert!(
        !raw.contains("authorization:"),
        "unexpected auth header:\n{raw}"
    );
}

#[tokio::test]
async fn openai_stream_deltas_done_terminator_and_final_usage() {
    let sse = concat!(
        "data: {\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n",
    );
    let (base, rx) = serve_once(http_response("200 OK", "text/event-stream", "", sse)).await;
    let p = OpenAiCompatibleProvider::new(
        "o",
        &cfg(ProviderKind::OpenaiCompatible, &base, None),
        5_000,
    )
    .unwrap();
    let mut s = p
        .stream(&request("gpt-4o"), &CancellationToken::new())
        .await
        .unwrap();

    let mut text = String::new();
    let mut dones = 0;
    while let Some(ev) = s.next().await {
        match ev.unwrap() {
            StreamEvent::Delta { text: t } => text.push_str(&t),
            StreamEvent::Done { usage, model, .. } => {
                dones += 1;
                assert_eq!(usage.input_tokens, 9);
                assert_eq!(usage.output_tokens, 2);
                assert_eq!(model, "gpt-4o");
            }
        }
    }
    assert_eq!(text, "Hello");
    assert_eq!(dones, 1);

    let raw = rx.await.unwrap();
    assert!(raw.contains("\"stream\":true"));
    assert!(raw.contains("\"stream_options\":{\"include_usage\":true}"));
}

#[tokio::test]
async fn openai_list_models_parses_ids() {
    let body = serde_json::json!({
        "data": [{"id": "llama-3.3-70b"}, {"id": "qwen-2.5"}]
    })
    .to_string();
    let (base, _rx) = serve_once(http_response("200 OK", "application/json", "", &body)).await;
    let p = OpenAiCompatibleProvider::new(
        "o",
        &cfg(ProviderKind::OpenaiCompatible, &base, None),
        5_000,
    )
    .unwrap();
    let models = p.list_models().await.unwrap();
    assert_eq!(models, vec!["llama-3.3-70b", "qwen-2.5"]);
}

#[tokio::test]
async fn stream_cancellation_mid_body_yields_cancelled() {
    // Server sends one delta then stalls with the connection open.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut sock).await;
        let head =
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
        let first = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n";
        sock.write_all(head.as_bytes()).await.unwrap();
        sock.write_all(first.as_bytes()).await.unwrap();
        sock.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let base = format!("http://{addr}");
    let p = OpenAiCompatibleProvider::new(
        "o",
        &cfg(ProviderKind::OpenaiCompatible, &base, None),
        60_000,
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let mut s = p.stream(&request("m"), &cancel).await.unwrap();

    // first delta arrives
    let first = s.next().await.unwrap().unwrap();
    assert!(matches!(first, StreamEvent::Delta { .. }));
    // then cancel: the stream must surface Cancelled promptly
    cancel.cancel();
    let started = std::time::Instant::now();
    let next = s.next().await.unwrap();
    assert!(matches!(next, Err(MaestroError::Cancelled)), "got {next:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

// ---------------------------------------------------------------- Responses

fn responses_body() -> serde_json::Value {
    serde_json::json!({"status":"completed", "model":"gpt-test", "output":[
        {"type":"reasoning", "summary":[{"text":"internal"}]},
        {"type":"message", "role":"assistant", "content":[
            {"type":"output_text", "text":"Hello "}, {"type":"output_text", "text":"world"}
        ]}
    ], "usage":{"input_tokens":23,"output_tokens":17}})
}

#[tokio::test]
async fn responses_sends_input_json_format_and_reads_only_assistant_output() {
    set_key("MAESTRO_T_RESPONSES");
    let body = responses_body().to_string();
    let (base, rx) = serve_once(http_response("200 OK", "application/json", "", &body)).await;
    let mut config = cfg(
        ProviderKind::OpenaiResponses,
        &base,
        Some("MAESTRO_T_RESPONSES"),
    );
    config.reasoning_effort = Some("low".into());
    let p = OpenAiCompatibleProvider::new("gpt", &config, 5000).unwrap();
    let mut req = request("gpt-test");
    req.json = true;
    let out = p.generate(&req, &CancellationToken::new()).await.unwrap();
    assert_eq!(out.text, "Hello world");
    assert_eq!(out.usage.input_tokens, 23);
    assert_eq!(out.usage.output_tokens, 17);
    let raw = rx.await.unwrap();
    assert!(raw.starts_with("POST /responses "));
    assert!(raw
        .to_lowercase()
        .contains("authorization: bearer test-key-123"));
    let sent: serde_json::Value =
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(sent["input"][0]["role"], "system");
    assert_eq!(sent["reasoning"]["effort"], "low");
    assert_eq!(sent["max_output_tokens"], 64);
    assert_eq!(sent["store"], false);
    assert_eq!(sent["text"]["format"]["type"], "json_object");
    assert!(sent.get("max_tokens").is_none());
    assert!(sent.get("messages").is_none());
}

#[tokio::test]
async fn responses_rejects_partial_failed_and_empty_outputs() {
    for body in [
        serde_json::json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}),
        serde_json::json!({"status":"failed","error":{"message":"failed"}}),
        serde_json::json!({"status":"completed","output":[]}),
    ] {
        let (base, _) = serve_once(http_response(
            "200 OK",
            "application/json",
            "",
            &body.to_string(),
        ))
        .await;
        let p = OpenAiCompatibleProvider::new(
            "gpt",
            &cfg(ProviderKind::OpenaiResponses, &base, None),
            5000,
        )
        .unwrap();
        assert!(p
            .generate(&request("gpt-test"), &CancellationToken::new())
            .await
            .is_err());
    }
}

#[tokio::test]
async fn responses_stream_requires_completed_event_and_meters_reasoning_tokens() {
    let body=format!("data: {{\"type\":\"response.output_text.delta\",\"delta\":\"Hello world\"}}\n\ndata: {}\n\n",serde_json::json!({"type":"response.completed","response":responses_body()}));
    let (base, rx) = serve_once(http_response("200 OK", "text/event-stream", "", &body)).await;
    let p = OpenAiCompatibleProvider::new(
        "gpt",
        &cfg(ProviderKind::OpenaiResponses, &base, None),
        5000,
    )
    .unwrap();
    let mut stream = p
        .stream(&request("gpt-test"), &CancellationToken::new())
        .await
        .unwrap();
    let mut text = String::new();
    let mut done = 0;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            StreamEvent::Delta { text: t } => text.push_str(&t),
            StreamEvent::Done { usage, .. } => {
                done += 1;
                assert_eq!(usage.output_tokens, 17);
            }
        }
    }
    assert_eq!(text, "Hello world");
    assert_eq!(done, 1);
    let raw = rx.await.unwrap();
    assert!(raw.contains("\"stream\":true"));
    let (base, _) = serve_once(http_response(
        "200 OK",
        "text/event-stream",
        "",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
    ))
    .await;
    let p = OpenAiCompatibleProvider::new(
        "gpt",
        &cfg(ProviderKind::OpenaiResponses, &base, None),
        5000,
    )
    .unwrap();
    let results: Vec<_> = p
        .stream(&request("gpt-test"), &CancellationToken::new())
        .await
        .unwrap()
        .collect()
        .await;
    assert!(results.last().unwrap().is_err());
}

#[tokio::test]
async fn responses_timeout_and_cancellation_are_bounded() {
    let base = serve_stall().await;
    let p =
        OpenAiCompatibleProvider::new("gpt", &cfg(ProviderKind::OpenaiResponses, &base, None), 100)
            .unwrap();
    let err = p
        .generate(&request("gpt-test"), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        p.generate(&request("gpt-test"), &cancel).await,
        Err(MaestroError::Cancelled)
    ));
}
