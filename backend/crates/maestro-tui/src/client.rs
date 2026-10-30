//! Async HTTP client for the daemon API (SPEC §Daemon HTTP API) plus the SSE
//! event stream with replay + auto-reconnect. Re-exported from the crate root
//! so the CLI can reuse it.

use crate::sse::SseParser;
use futures::{Stream, StreamExt};
use maestro_types::api::{
    ApprovalDecisionRequest, ChatSendRequest, ChatSendResponse, DaemonInfo, DiffResponse,
    HealthResponse, MetricsResponse, MissionDetail, MissionSummary, ModelsResponse,
    PlanDecisionRequest, RetryRequest, StateSnapshot, SteerRequest, SubmitMissionRequest,
    SubmitMissionResponse,
};
use maestro_types::domain::Approval;
use maestro_types::plan::Plan;
use maestro_types::{MaestroError, Result};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

/// Reconnect backoff bounds for the SSE stream.
const SSE_BACKOFF_MIN: Duration = Duration::from_secs(1);
const SSE_BACKOFF_MAX: Duration = Duration::from_secs(15);
/// After this many consecutive failed connections the SSE stream gives up
/// and ends (no idle retry timer while the daemon is permanently gone).
pub const SSE_MAX_FAILURES: u32 = 5;

#[derive(Clone)]
pub struct DaemonClient {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
    /// (min, max) SSE reconnect backoff; overridable in tests.
    sse_backoff: (Duration, Duration),
}

impl std::fmt::Debug for DaemonClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonClient")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

fn http_err(e: reqwest::Error) -> MaestroError {
    MaestroError::Io(format!("daemon http: {e}"))
}

impl DaemonClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> DaemonClient {
        let base = base_url.into().trim_end_matches('/').to_string();
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default();
        DaemonClient {
            http,
            base,
            token,
            sse_backoff: (SSE_BACKOFF_MIN, SSE_BACKOFF_MAX),
        }
    }

    /// Shrink the SSE reconnect backoff so give-up paths are testable.
    #[cfg(test)]
    fn set_sse_backoff(&mut self, min: Duration, max: Duration) {
        self.sse_backoff = (min, max);
    }

    /// Read `<root>/.maestro/daemon.json` and build a client from it.
    pub fn from_root(root: &Path) -> Result<DaemonClient> {
        let path = root.join(".maestro").join("daemon.json");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| MaestroError::NotFound(format!("{}: {e}", path.display())))?;
        let info: DaemonInfo = serde_json::from_str(&raw)
            .map_err(|e| MaestroError::Config(format!("daemon.json: {e}")))?;
        Ok(DaemonClient::new(info.url, info.token))
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = &self.token {
            rb = rb.bearer_auth(token);
        }
        rb
    }

    async fn decode<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        if status.is_success() {
            return resp.json::<T>().await.map_err(http_err);
        }
        let msg = match resp.json::<maestro_types::api::ApiError>().await {
            Ok(e) => e.error,
            Err(_) => format!("http {status}"),
        };
        Err(match status.as_u16() {
            404 => MaestroError::NotFound(msg),
            409 => MaestroError::Conflict(msg),
            401 => MaestroError::Other(format!("unauthorized: {msg}")),
            400 => MaestroError::Other(format!("bad request: {msg}")),
            _ => MaestroError::Other(msg),
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self
            .req(reqwest::Method::GET, path)
            .send()
            .await
            .map_err(http_err)?;
        Self::decode(resp).await
    }

    async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let resp = self
            .req(reqwest::Method::POST, path)
            .json(body)
            .send()
            .await
            .map_err(http_err)?;
        Self::decode(resp).await
    }

    async fn post_empty(&self, path: &str) -> Result<()> {
        let resp = self
            .req(reqwest::Method::POST, path)
            .send()
            .await
            .map_err(http_err)?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let msg = match resp.json::<maestro_types::api::ApiError>().await {
            Ok(e) => e.error,
            Err(_) => format!("http {status}"),
        };
        Err(MaestroError::Other(msg))
    }

    // ---- endpoints ---------------------------------------------------------

    pub async fn health(&self) -> Result<HealthResponse> {
        self.get_json("/health").await
    }

    pub async fn code_history(&self) -> Result<maestro_types::history::History> {
        self.get_json("/api/history").await
    }

    pub async fn save_version(&self, label: &str) -> Result<maestro_types::history::Version> {
        self.post_json("/api/history", &serde_json::json!({"label":label}))
            .await
    }

    pub async fn version_diff(&self, id: &str) -> Result<String> {
        let value: serde_json::Value = self.get_json(&format!("/api/history/{id}/diff")).await?;
        Ok(value["unified"].as_str().unwrap_or("").to_string())
    }

    pub async fn preview_restore(
        &self,
        id: Option<&str>,
    ) -> Result<maestro_types::history::RestorePreview> {
        let path = id
            .map(|id| format!("/api/history/{id}/restore"))
            .unwrap_or_else(|| "/api/history/undo".into());
        self.get_json(&path).await
    }

    pub async fn restore_version(
        &self,
        id: Option<&str>,
        token: &str,
    ) -> Result<maestro_types::history::RestoreResult> {
        let path = id
            .map(|id| format!("/api/history/{id}/restore"))
            .unwrap_or_else(|| "/api/history/undo".into());
        self.post_json(
            &path,
            &maestro_types::history::RestoreRequest {
                token: Some(token.into()),
            },
        )
        .await
    }

    pub async fn state_snapshot(&self) -> Result<StateSnapshot> {
        self.get_json("/api/state").await
    }

    pub async fn list_missions(&self) -> Result<Vec<MissionSummary>> {
        self.get_json("/api/missions").await
    }

    pub async fn mission_detail(&self, id: &str) -> Result<MissionDetail> {
        self.get_json(&format!("/api/missions/{id}")).await
    }

    pub async fn submit_mission(
        &self,
        objective: &str,
        session_id: Option<&str>,
    ) -> Result<SubmitMissionResponse> {
        self.post_json(
            "/api/missions",
            &SubmitMissionRequest {
                objective: objective.to_string(),
                session_id: session_id.map(Into::into),
            },
        )
        .await
    }

    pub async fn pause(&self, id: &str) -> Result<()> {
        self.post_empty(&format!("/api/missions/{id}/pause")).await
    }

    pub async fn resume(&self, id: &str) -> Result<()> {
        self.post_empty(&format!("/api/missions/{id}/resume")).await
    }

    pub async fn cancel(&self, id: &str) -> Result<()> {
        self.post_empty(&format!("/api/missions/{id}/cancel")).await
    }

    pub async fn retry(&self, id: &str, req: &RetryRequest) -> Result<()> {
        let _: serde_json::Value = self
            .post_json(&format!("/api/missions/{id}/retry"), req)
            .await?;
        Ok(())
    }

    pub async fn steer(&self, id: &str, req: &SteerRequest) -> Result<()> {
        let _: serde_json::Value = self
            .post_json(&format!("/api/missions/{id}/steer"), req)
            .await?;
        Ok(())
    }

    pub async fn plan_get(&self, id: &str) -> Result<Plan> {
        self.get_json(&format!("/api/missions/{id}/plan")).await
    }

    pub async fn plan_decide(&self, id: &str, req: &PlanDecisionRequest) -> Result<()> {
        let _: serde_json::Value = self
            .post_json(&format!("/api/missions/{id}/plan/approve"), req)
            .await?;
        Ok(())
    }

    pub async fn diff(&self, id: &str) -> Result<DiffResponse> {
        self.get_json(&format!("/api/missions/{id}/diff")).await
    }

    pub async fn approvals(&self, mission_id: Option<&str>) -> Result<Vec<Approval>> {
        let path = match mission_id {
            Some(m) => format!("/api/approvals?missionId={m}"),
            None => "/api/approvals".to_string(),
        };
        self.get_json(&path).await
    }

    pub async fn approval_decide(&self, id: &str, req: &ApprovalDecisionRequest) -> Result<()> {
        let _: serde_json::Value = self.post_json(&format!("/api/approvals/{id}"), req).await?;
        Ok(())
    }

    pub async fn models(&self) -> Result<ModelsResponse> {
        self.get_json("/api/models").await
    }

    pub async fn previews(&self, stop: bool) -> Result<serde_json::Value> {
        if stop {
            self.post_json("/api/previews/stop", &serde_json::json!({}))
                .await
        } else {
            self.get_json("/api/previews").await
        }
    }
    pub async fn upload_image(&self, path: &str) -> Result<serde_json::Value> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use tokio::io::AsyncReadExt;
        let path = if let Some(tail) = path.strip_prefix("~/") {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(tail)
        } else {
            path.into()
        };
        let mut bytes = Vec::new();
        tokio::fs::File::open(&path)
            .await?
            .take(12 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > 12 * 1024 * 1024 {
            return Err(MaestroError::Config("Image exceeds 12 MiB".into()));
        }
        let mime = match path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => {
                return Err(MaestroError::Config(
                    "Attach a PNG, JPEG or WebP file".into(),
                ))
            }
        };
        self.post_json("/api/images",&serde_json::json!({"name":path.file_name().unwrap_or_default().to_string_lossy(),"dataUrl":format!("data:{mime};base64,{}",STANDARD.encode(bytes))})).await
    }

    pub async fn chat_send(&self, session_id: &str, text: &str) -> Result<ChatSendResponse> {
        self.post_json(
            &format!("/api/chat/{session_id}/messages"),
            &ChatSendRequest {
                text: text.to_string(),
                images: Vec::new(),
            },
        )
        .await
    }

    pub async fn chat_sessions(&self) -> Result<Vec<maestro_types::domain::ChatSession>> {
        self.get_json("/api/chat/sessions").await
    }

    pub async fn chat_history(
        &self,
        session_id: &str,
    ) -> Result<Vec<maestro_types::domain::ChatMessage>> {
        self.get_json(&format!("/api/chat/{session_id}/messages"))
            .await
    }

    pub async fn metrics(&self) -> Result<MetricsResponse> {
        self.get_json("/api/metrics").await
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.post_empty("/api/shutdown").await
    }

    /// Subscribe to `/api/events` as a `Stream`. Replays from `after`
    /// (exclusive cursor) then follows live; auto-reconnects with 1s..15s
    /// backoff sending `Last-Event-ID`. Each disconnect surfaces one
    /// `Err(...)` item so the UI can flip its connectivity dot. After
    /// [`SSE_MAX_FAILURES`] consecutive failed connections the stream gives
    /// up and ends (yields `None`) so no retry timer runs while the daemon
    /// is permanently gone — callers restart the subscription when the
    /// daemon is reachable again. Dropping the stream aborts the underlying
    /// task.
    pub fn events(
        &self,
        mission_id: Option<String>,
        after: i64,
    ) -> impl Stream<Item = Result<maestro_types::event::EventRecord>> + Send + Unpin + use<> {
        let (tx, rx) = mpsc::channel(256);
        let client = self.clone();
        let handle = tokio::spawn(async move {
            client.run_events(mission_id, after, tx).await;
        });
        EventStream { rx, handle }
    }

    async fn run_events(
        &self,
        mission_id: Option<String>,
        mut after: i64,
        tx: mpsc::Sender<Result<maestro_types::event::EventRecord>>,
    ) {
        let (backoff_min, backoff_max) = self.sse_backoff;
        let mut backoff = backoff_min;
        let mut failures: u32 = 0;
        loop {
            let mut url = format!("{}/api/events?after={after}", self.base);
            if let Some(m) = &mission_id {
                url.push_str("&missionId=");
                url.push_str(m);
            }
            let mut rb = self
                .http
                .get(&url)
                .header("accept", "text/event-stream")
                .header("Last-Event-ID", after.to_string());
            if let Some(token) = &self.token {
                rb = rb.bearer_auth(token);
            }
            let resp = rb.send().await;
            match resp {
                Ok(resp) if resp.status().is_success() => {
                    backoff = backoff_min;
                    failures = 0;
                    let mut parser = SseParser::new();
                    let mut body = resp.bytes_stream();
                    while let Some(chunk) = body.next().await {
                        let Ok(bytes) = chunk else { break };
                        let frames = parser.push(&bytes);
                        // `id:` may also arrive on heartbeat frames; track it
                        // for Last-Event-ID even without data.
                        if let Some(id) = parser.last_id().and_then(|s| s.parse::<i64>().ok()) {
                            after = after.max(id);
                        }
                        for frame in frames {
                            match serde_json::from_str::<maestro_types::event::EventRecord>(
                                &frame.data,
                            ) {
                                Ok(rec) => {
                                    after = after.max(rec.seq);
                                    if tx.send(Ok(rec)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(_) => {
                                    // Heartbeats / unknown frames: ignore.
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            failures += 1;
            if tx
                .send(Err(MaestroError::Io("event stream disconnected".into())))
                .await
                .is_err()
            {
                return;
            }
            if failures >= SSE_MAX_FAILURES {
                // Give up: ending the task ends the stream, so the caller
                // idles with zero timers until it resubscribes.
                return;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(backoff_max);
        }
    }
}

/// Owned SSE subscription: dropping it aborts the reconnect task.
struct EventStream {
    rx: mpsc::Receiver<Result<maestro_types::event::EventRecord>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Stream for EventStream {
    type Item = Result<maestro_types::event::EventRecord>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Best-effort initial SSE cursor: read MAX(seq) from the local event log so
/// a fresh TUI doesn't replay the whole audit history over HTTP. The daemon
/// API has no cursor endpoint (see report); the DB is opened read-only and
/// any failure just returns 0 (full replay, still correct).
pub fn initial_cursor(root: &Path) -> i64 {
    let db = root.join(".maestro").join("maestro.db");
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let Ok(conn) = rusqlite::Connection::open_with_flags(&db, flags) else {
        return 0;
    };
    conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_root_reads_daemon_json() {
        let dir = tempfile::tempdir().unwrap();
        let mdir = dir.path().join(".maestro");
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::write(
            mdir.join("daemon.json"),
            r#"{"url":"http://127.0.0.1:7999/","pid":1,"generation":"g","startedAt":0,"token":"sekrit"}"#,
        )
        .unwrap();
        let client = DaemonClient::from_root(dir.path()).unwrap();
        assert_eq!(client.base_url(), "http://127.0.0.1:7999");
        assert!(client.token.is_some());
    }

    #[test]
    fn from_root_missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            DaemonClient::from_root(dir.path()),
            Err(MaestroError::NotFound(_))
        ));
    }

    #[test]
    fn initial_cursor_defaults_to_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(initial_cursor(dir.path()), 0);
    }

    #[tokio::test]
    async fn events_gives_up_after_max_consecutive_failures() {
        // Grab a free port, then close it so connections are refused.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut client = DaemonClient::new(format!("http://127.0.0.1:{port}"), None);
        client.set_sse_backoff(Duration::from_millis(1), Duration::from_millis(1));
        let mut stream = client.events(None, 0);
        let mut errs = 0u32;
        while let Some(item) = stream.next().await {
            assert!(item.is_err(), "closed port must never yield events");
            errs += 1;
            assert!(errs <= SSE_MAX_FAILURES, "must park after giving up");
        }
        // One Err per attempt, then the stream ends (task parked): the
        // caller sees `None` and owns the restart decision.
        assert_eq!(errs, SSE_MAX_FAILURES);
    }
}
