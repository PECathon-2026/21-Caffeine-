//! End-to-end daemon tests: real HTTP on 127.0.0.1:0, mock providers, and
//! throwaway git repositories.

use maestro_daemon::{start_daemon, DaemonHandle, DaemonOptions};
use maestro_store::Store;
use maestro_types::api::{ChatSendResponse, MissionDetail, SubmitMissionResponse};
use maestro_types::config::{EnforcementMode, MaestroConfig};
use maestro_types::domain::{Approval, ChatRole, MissionStatus, RiskLevel, Role, TaskStatus};
use maestro_types::event::{kind, EventRecord};
use maestro_types::plan::PlanTask;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const POLL: Duration = Duration::from_millis(50);
const DEADLINE: Duration = Duration::from_secs(20);

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &Path) {
    sh(dir, &["init", "-b", "main"]);
    sh(dir, &["config", "user.name", "Test"]);
    sh(dir, &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.join("README.md"), "# test repo\n").unwrap();
    // `maestro init` always ignores the state directory; mirror that here so
    // finalization's clean-tree check sees a clean root.
    std::fs::write(dir.join(".gitignore"), ".maestro/\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "// lib\n").unwrap();
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-m", "initial"]);
}

fn test_config() -> MaestroConfig {
    let mut config = MaestroConfig::default();
    config.safety.sandbox = EnforcementMode::Preferred;
    config.safety.worktrees = EnforcementMode::Required;
    config.integration.final_approval = true;
    config.runtime.lease_ttl_ms = 10_000;
    set_role_model(&mut config, "coder", "coder-writer");
    config
}

fn set_role_model(config: &mut MaestroConfig, role: &str, model: &str) {
    config.roles.get_mut(role).unwrap().model.model = model.to_string();
}

/// Init a repo, write the config, and start a daemon on an ephemeral port.
async fn boot(dir: &Path, config: MaestroConfig, token: Option<&str>) -> DaemonHandle {
    init_repo(dir);
    maestro_engine::save_config(dir, &config).unwrap();
    // maestro.toml is untracked; commit it so finalize sees a clean tree.
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-m", "config"]);
    start_daemon(DaemonOptions {
        root: dir.to_path_buf(),
        bind: None,
        token: token.map(str::to_string),
    })
    .await
    .unwrap()
}

struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    fn new(handle: &DaemonHandle, token: Option<&str>) -> Client {
        Client {
            http: reqwest::Client::new(),
            base: handle.url.clone(),
            token: token.map(str::to_string),
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(t) = &self.token {
            rb = rb.bearer_auth(t);
        }
        rb
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> T {
        let resp = self.req(reqwest::Method::GET, path).send().await.unwrap();
        assert!(resp.status().is_success(), "GET {path}: {}", resp.status());
        resp.json().await.unwrap()
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> T {
        let resp = self
            .req(reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let text = resp.text().await.unwrap();
        assert!(status.is_success(), "POST {path}: {status} {text}");
        serde_json::from_str(&text).unwrap()
    }

    /// POST without asserting success: returns (status, parsed body).
    async fn post_raw(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let resp = self
            .req(reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        let value =
            serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "raw": text }));
        (status, value)
    }

    async fn submit(&self, objective: &str) -> String {
        let resp: SubmitMissionResponse = self
            .post_json(
                "/api/missions",
                serde_json::json!({ "objective": objective }),
            )
            .await;
        resp.mission_id
    }

    async fn mission(&self, id: &str) -> MissionDetail {
        self.get_json(&format!("/api/missions/{id}")).await
    }

    /// Poll the mission until `pred` holds; panics after [`DEADLINE`].
    async fn wait_for(&self, id: &str, pred: impl Fn(&MissionDetail) -> bool) -> MissionDetail {
        let start = std::time::Instant::now();
        loop {
            let detail = self.mission(id).await;
            if pred(&detail) {
                return detail;
            }
            assert!(
                start.elapsed() < DEADLINE,
                "mission {id} never reached the expected state (status {})",
                detail.mission.status.as_str()
            );
            tokio::time::sleep(POLL).await;
        }
    }
}

/// Minimal SSE reader: collect `data:` payloads as [`EventRecord`]s until
/// `enough` holds, bounded by [`DEADLINE`].
async fn read_sse(
    resp: reqwest::Response,
    enough: impl Fn(&[EventRecord]) -> bool,
) -> Vec<EventRecord> {
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut out: Vec<EventRecord> = Vec::new();
    let start = std::time::Instant::now();
    while !enough(&out) {
        assert!(start.elapsed() < DEADLINE, "sse: not enough frames in time");
        let chunk = tokio::time::timeout(DEADLINE, stream.next())
            .await
            .expect("sse chunk timeout")
            .expect("sse stream ended early")
            .expect("sse chunk error");
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find("\n\n") {
            let frame: String = buf.drain(..pos + 2).collect();
            for line in frame.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(rec) = serde_json::from_str::<EventRecord>(data) {
                        out.push(rec);
                    }
                }
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_approve_finalize_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    let mission_id = client.submit("implement the feature").await;
    let detail = client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::AwaitingApproval
        })
        .await;
    assert!(detail
        .tasks
        .iter()
        .all(|t| t.status == TaskStatus::Completed));
    assert!(detail.plan.is_some());

    let approvals: Vec<Approval> = client
        .get_json(&format!("/api/approvals?missionId={mission_id}"))
        .await;
    let integrate = approvals
        .iter()
        .find(|a| a.action == "integrate")
        .expect("an integrate approval is pending");

    let decided: serde_json::Value = client
        .post_json(
            &format!("/api/approvals/{}", integrate.id),
            serde_json::json!({ "approve": true }),
        )
        .await;
    assert!(decided["finalized"]["commit"].is_string());

    let detail = client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;
    assert_eq!(detail.mission.status, MissionStatus::Completed);
    // The coder's file landed on the user branch.
    assert!(dir.path().join("src/generated.txt").exists());

    // The real tool executor captured drafts, and published versions remain
    // available after task-worktree cleanup. Undo reverses working files only.
    let history: maestro_types::history::History = client.get_json("/api/history").await;
    for kind in ["before", "draft", "applied"] {
        assert!(
            history.versions.iter().any(|v| v.kind == kind),
            "missing {kind}"
        );
    }
    let preview: maestro_types::history::RestorePreview =
        client.get_json("/api/history/undo").await;
    assert!(preview.conflicts.is_empty());
    let restored: maestro_types::history::RestoreResult = client
        .post_json(
            "/api/history/undo",
            serde_json::json!({"token":preview.token}),
        )
        .await;
    assert!(!dir.path().join("src/generated.txt").exists());
    let recovery: serde_json::Value = client
        .get_json(&format!("/api/history/{}/diff", restored.recovery_id))
        .await;
    assert!(recovery.get("unified").is_some());

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sse_streams_mission_and_task_events_with_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    let mission_id = client.submit("implement the feature").await;

    // Live stream from cursor 0 while the mission runs.
    let resp = client
        .req(
            reqwest::Method::GET,
            &format!("/api/events?missionId={mission_id}&after=0"),
        )
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let events = read_sse(resp, |seen| {
        seen.iter().any(|e| e.kind == kind::MISSION_STATUS)
            && seen.iter().any(|e| e.kind == kind::TASK_STATUS)
    })
    .await;
    assert!(events.iter().all(|e| e.mission_id == mission_id));
    let mid_seq = events[events.len() / 2].seq;

    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;

    // Last-Event-ID replay: only events after the presented cursor come back.
    let resp = client
        .req(
            reqwest::Method::GET,
            &format!("/api/events?missionId={mission_id}"),
        )
        .header("Last-Event-ID", mid_seq.to_string())
        .send()
        .await
        .unwrap();
    let replayed = read_sse(resp, |seen| !seen.is_empty()).await;
    assert!(replayed.iter().all(|e| e.seq > mid_seq));

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn steer_adds_a_directive_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    set_role_model(&mut config, "designer", "slow-designer");
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    let mission_id = client.submit("implement the feature").await;
    client
        .wait_for(&mission_id, |d| d.mission.status == MissionStatus::Running)
        .await;

    let resp: serde_json::Value = client
        .post_json(
            &format!("/api/missions/{mission_id}/steer"),
            serde_json::json!({ "text": "prefer small diffs" }),
        )
        .await;
    assert!(resp["directiveId"].is_string());

    // Verify durability through a second read-only store handle.
    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let directives = store.active_directives(&mission_id, None).unwrap();
    assert_eq!(directives.len(), 1);
    assert_eq!(directives[0].text, "prefer small diffs");
    let events = store.events_after(0, Some(&mission_id)).unwrap();
    assert!(events.iter().any(|e| e.kind == kind::DIRECTIVE_ADDED));

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_then_resume_completes_the_mission() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    set_role_model(&mut config, "designer", "slow-designer");
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    let mission_id = client.submit("implement the feature").await;
    client
        .wait_for(&mission_id, |d| d.mission.status == MissionStatus::Running)
        .await;

    let paused: serde_json::Value = client
        .post_json(
            &format!("/api/missions/{mission_id}/pause"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(paused["status"], "paused");
    client
        .wait_for(&mission_id, |d| d.mission.status == MissionStatus::Paused)
        .await;

    let resumed: serde_json::Value = client
        .post_json(
            &format!("/api/missions/{mission_id}/resume"),
            serde_json::json!({}),
        )
        .await;
    assert!(resumed["status"].is_string());

    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;
    assert!(dir.path().join("src/generated.txt").exists());

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_reclaims_mission_worktrees_and_branches() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    let mission_id = client.submit("implement the feature").await;
    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::AwaitingApproval
        })
        .await;

    let has_entries = |p: &Path| {
        std::fs::read_dir(p)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    };
    let integration_root = dir.path().join(".maestro/integration");
    let worktrees_root = dir.path().join(".maestro/worktrees");
    assert!(
        has_entries(&integration_root),
        "integration worktree exists while awaiting approval"
    );

    let cancelled: serde_json::Value = client
        .post_json(
            &format!("/api/missions/{mission_id}/cancel"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(cancelled["status"], "cancelled");
    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::Cancelled
        })
        .await;

    // Cleanup runs inline (no live worker) or on worker exit; poll briefly.
    let deadline = std::time::Instant::now() + DEADLINE;
    while has_entries(&integration_root) || has_entries(&worktrees_root) {
        assert!(
            std::time::Instant::now() < deadline,
            "cancelled mission's worktrees were not reclaimed"
        );
        tokio::time::sleep(POLL).await;
    }

    // The abandoned mission branch is deleted too.
    let branches = Command::new("git")
        .current_dir(dir.path())
        .args(["branch", "--list", "maestro/*"])
        .output()
        .expect("git runs");
    let listing = String::from_utf8_lossy(&branches.stdout);
    assert!(
        listing.trim().is_empty(),
        "no maestro branches may survive a cancel, got: {listing}"
    );

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_needs_no_token_but_configured_token_is_enforced() {
    // Tokenless daemon: everything works without credentials.
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let open_client = Client::new(&handle, None);
    let state: serde_json::Value = open_client.get_json("/api/state").await;
    assert!(state["generation"].is_string());
    handle.shutdown().await;

    // Token-configured daemon: 401 without it, even on loopback.
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), Some("sekrit")).await;
    let bare = reqwest::Client::new();
    let resp = bare
        .get(format!("{}/api/state", handle.url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].is_string());

    // Wrong token is also refused.
    let resp = bare
        .get(format!("{}/api/state", handle.url))
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    // The right token passes; /health stays open for liveness probes.
    let authed = Client::new(&handle, Some("sekrit"));
    let state: serde_json::Value = authed.get_json("/api/state").await;
    assert!(state["generation"].is_string());
    let health = bare
        .get(format!("{}/health", handle.url))
        .send()
        .await
        .unwrap();
    assert!(health.status().is_success());

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_lock_is_reclaimed_and_foreign_leases_recovered() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    maestro_engine::save_config(dir.path(), &test_config()).unwrap();
    sh(dir.path(), &["add", "-A"]);
    sh(dir.path(), &["commit", "-m", "config"]);

    // Seed state as a dead daemon would have left it: a paused mission with
    // a task still `running` under a foreign lease generation.
    let state_dir = dir.path().join(".maestro");
    let store = Store::open(&state_dir).unwrap();
    let mission = store.create_mission("recover me").unwrap();
    let tasks = store
        .insert_tasks(
            &mission.id,
            &[PlanTask {
                key: "implement-thing".into(),
                role: Role::Coder,
                title: "Implement".into(),
                description: "Do the thing.".into(),
                dependencies: vec![],
                acceptance: vec!["done".into()],
                risk: RiskLevel::Low,
                write_scopes: vec!["src/**".into()],
            }],
        )
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();
    assert!(store
        .acquire_lease(
            &tasks[0].id,
            "dead-worker",
            "daemon_deadbeefdeadbeef",
            600_000
        )
        .unwrap());
    store
        .set_mission_status(&mission.id, MissionStatus::Paused, Some("daemon died"))
        .unwrap();
    drop(store);

    // Simulate the dead daemon's leftovers: lock dir + daemon.json with a
    // pid that cannot exist.
    std::fs::create_dir(state_dir.join("daemon.lock")).unwrap();
    std::fs::write(
        state_dir.join("daemon.json"),
        r#"{"url":"http://127.0.0.1:1","pid":999999999,"generation":"daemon_deadbeefdeadbeef","startedAt":0}"#,
    )
    .unwrap();

    // Boot succeeds (stale lock reclaimed) and recovery ran.
    let handle = start_daemon(DaemonOptions {
        root: dir.path().to_path_buf(),
        bind: None,
        token: None,
    })
    .await
    .unwrap();
    let client = Client::new(&handle, None);
    let detail = client.mission(&mission.id).await;
    assert_eq!(detail.tasks[0].status, TaskStatus::Ready);
    assert!(detail.tasks[0].lease_generation.is_none());

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_send_materializes_a_client_minted_session() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    // The TUI mints session ids locally and sends without creating first.
    let sid = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
    let resp: ChatSendResponse = client
        .post_json(
            &format!("/api/chat/{sid}/messages"),
            serde_json::json!({ "text": "what does this repo do?" }),
        )
        .await;
    assert!(!resp.messages.is_empty(), "first send must succeed");

    // The session now exists, titled from the opening words.
    let sessions: serde_json::Value = client.get_json("/api/chat/sessions").await;
    let found = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == sid)
        .expect("session materialized");
    assert_eq!(found["title"], "what does this repo do?");

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn models_response_includes_role_assignments() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);
    let resp: serde_json::Value = client.get_json("/api/models").await;
    let roles = resp["roles"].as_array().expect("roles array");
    assert_eq!(roles.len(), 5, "all five roles resolved");
    let coder = roles
        .iter()
        .find(|r| r["role"] == "coder")
        .expect("coder row");
    assert_eq!(coder["provider"], "mock");
    assert!(coder["model"].as_str().is_some());
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_history_replays_a_session_chronologically() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    let sid = "f1e2d3c4b5a6978877665544332211aa";
    for text in ["what is this repo?", "and how are missions run?"] {
        let _: ChatSendResponse = client
            .post_json(
                &format!("/api/chat/{sid}/messages"),
                serde_json::json!({ "text": text }),
            )
            .await;
    }
    let history: Vec<serde_json::Value> =
        client.get_json(&format!("/api/chat/{sid}/messages")).await;
    // user + assistant per turn, chronological.
    assert!(
        history.len() >= 4,
        "expected 2 turns of history: {}",
        history.len()
    );
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[0]["content"], "what is this repo?");
    let roles: Vec<&str> = history
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert!(roles.contains(&"assistant"));
    // Session list includes it, titled from the first words.
    let sessions: serde_json::Value = client.get_json("/api/chat/sessions").await;
    assert!(sessions.as_array().unwrap().iter().any(|s| s["id"] == sid));

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_routes_questions_and_implementation_requests() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    let session: serde_json::Value = client
        .post_json(
            "/api/chat/sessions",
            serde_json::json!({ "title": "hello" }),
        )
        .await;
    let session_id = session["id"].as_str().unwrap().to_string();

    // A question goes to the architect.
    let resp: ChatSendResponse = client
        .post_json(
            &format!("/api/chat/{session_id}/messages"),
            serde_json::json!({ "text": "how is authentication structured?" }),
        )
        .await;
    assert!(resp.mission_id.is_none());
    let last = resp.messages.last().unwrap();
    assert_eq!(last.role, ChatRole::Assistant);
    assert!(!last.content.trim().is_empty());
    assert!(!last.content.trim_start().starts_with('{'));

    // The reported polite inspection request must not create a task graph.
    let inspection: ChatSendResponse = client.post_json(
        &format!("/api/chat/{session_id}/messages"),
        serde_json::json!({"text":"Hey, Can you look into the repo and explain it to me nicelt"}),
    ).await;
    assert!(inspection.mission_id.is_none());
    let followup: ChatSendResponse = client
        .post_json(
            &format!("/api/chat/{session_id}/messages"),
            serde_json::json!({"text":"Now explain me the tech stack of this project"}),
        )
        .await;
    assert!(followup.mission_id.is_none());
    let missions: serde_json::Value = client.get_json("/api/missions").await;
    assert!(missions.as_array().unwrap().is_empty());

    // An implementation request launches a mission.
    let resp: ChatSendResponse = client
        .post_json(
            &format!("/api/chat/{session_id}/messages"),
            serde_json::json!({ "text": "implement the feature" }),
        )
        .await;
    let mission_id = resp.mission_id.expect("mission launched");
    assert!(resp
        .messages
        .iter()
        .any(|m| m.role == ChatRole::Assistant && m.mission_id.as_deref() == Some(&mission_id)));
    client.mission(&mission_id).await; // it exists

    let sessions: Vec<serde_json::Value> = client.get_json("/api/chat/sessions").await;
    assert_eq!(sessions.len(), 1);

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn second_daemon_on_the_same_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;

    let second = start_daemon(DaemonOptions {
        root: dir.path().to_path_buf(),
        bind: None,
        token: None,
    })
    .await;
    assert!(matches!(
        second,
        Err(maestro_types::MaestroError::Conflict(_))
    ));

    // After a clean shutdown the lock is released and a new daemon can start.
    handle.shutdown().await;
    let third = start_daemon(DaemonOptions {
        root: dir.path().to_path_buf(),
        bind: None,
        token: None,
    })
    .await
    .unwrap();
    third.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn health_reports_the_event_cursor_head() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    let health: serde_json::Value = client.get_json("/health").await;
    assert!(health["ok"].as_bool().unwrap());
    assert_eq!(health["lastSeq"].as_i64().unwrap(), 0);
    assert_eq!(health["generation"].as_str().unwrap(), handle.generation);
    // Stale-daemon handshake: same process, same binary, ids must agree.
    assert_eq!(
        health["buildId"].as_str().unwrap(),
        maestro_types::build_id(),
        "in-process daemon must report this binary's id"
    );
    // The test config runs everything on the mock provider — health says so.
    let mock_roles: Vec<String> = serde_json::from_value(health["mockRoles"].clone()).unwrap();
    assert_eq!(
        mock_roles.len(),
        5,
        "all five roles are mock: {mock_roles:?}"
    );

    let mission_id = client.submit("implement the feature").await;
    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;
    let health: serde_json::Value = client.get_json("/health").await;
    assert!(health["lastSeq"].as_i64().unwrap() > 0);

    // The snapshot exposes the same cursor head.
    let state: serde_json::Value = client.get_json("/api/state").await;
    assert_eq!(state["lastSeq"], health["lastSeq"]);

    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Regression tests for the audit findings (final-approval stranding, resume
// of blocked missions, approval release fan-out, snapshot caps, handle drop,
// paged SSE replay).

/// Finding 1a: rejecting the final "integrate" approval must not strand the
/// mission in awaiting_approval — it becomes blocked with an explicit reason.
/// Also pins resume-of-awaiting_approval down as a 409 (recovery flows
/// through the approval decision, never through resume).
#[tokio::test(flavor = "multi_thread")]
async fn rejected_final_integration_blocks_the_mission() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    let mission_id = client.submit("implement the feature").await;
    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::AwaitingApproval
        })
        .await;

    // Resume must not touch a mission that is awaiting the final approval.
    let (status, _) = client
        .post_raw(
            &format!("/api/missions/{mission_id}/resume"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 409, "resume of awaiting_approval must 409");

    let approvals: Vec<Approval> = client
        .get_json(&format!("/api/approvals?missionId={mission_id}"))
        .await;
    let integrate = approvals.iter().find(|a| a.action == "integrate").unwrap();
    let (status, body) = client
        .post_raw(
            &format!("/api/approvals/{}", integrate.id),
            serde_json::json!({ "approve": false }),
        )
        .await;
    assert_eq!(status, 200, "reject itself succeeds: {body}");

    client
        .wait_for(&mission_id, |d| d.mission.status == MissionStatus::Blocked)
        .await;
    // The block reason is carried on the MISSION_STATUS audit event.
    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let events = store.events_after(0, Some(&mission_id)).unwrap();
    assert!(
        events.iter().any(|e| e.kind == kind::MISSION_STATUS
            && e.payload["status"] == serde_json::json!("blocked")
            && e.payload["reason"] == serde_json::json!("final integration was rejected")),
        "blocked event carries the rejection reason"
    );
    drop(store);
    // Nothing landed on the user branch.
    assert!(!dir.path().join("src/generated.txt").exists());

    handle.shutdown().await;
}

/// Finding 1b (end-to-end recovery): approving the final integration with a
/// dirty user tree consumes the approval and fails finalize. The mission must
/// come to rest as blocked (not stranded in awaiting_approval), and after the
/// user cleans the tree, resume must drive it to completed — the still-valid
/// approved approval is re-verified by the worker's integrate path.
#[tokio::test(flavor = "multi_thread")]
async fn failed_finalize_blocks_and_resume_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    let mission_id = client.submit("implement the feature").await;
    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::AwaitingApproval
        })
        .await;
    let approvals: Vec<Approval> = client
        .get_json(&format!("/api/approvals?missionId={mission_id}"))
        .await;
    let integrate = approvals.iter().find(|a| a.action == "integrate").unwrap();

    // Dirty the user tree, then approve: finalize refuses, 409 comes back,
    // and the mission is parked as blocked with the failure as its reason.
    let dirt = dir.path().join("src/uncommitted.txt");
    std::fs::write(&dirt, "wip\n").unwrap();
    let (status, body) = client
        .post_raw(
            &format!("/api/approvals/{}", integrate.id),
            serde_json::json!({ "approve": true }),
        )
        .await;
    assert_eq!(status, 409, "finalize with a dirty tree must 409: {body}");
    assert!(
        body["error"].as_str().unwrap().contains("not clean"),
        "409 carries the reason: {body}"
    );
    client
        .wait_for(&mission_id, |d| d.mission.status == MissionStatus::Blocked)
        .await;
    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let events = store.events_after(0, Some(&mission_id)).unwrap();
    assert!(
        events.iter().any(|e| e.kind == kind::MISSION_STATUS
            && e.payload["status"] == serde_json::json!("blocked")
            && e.payload["reason"]
                .as_str()
                .is_some_and(|r| r.contains("not clean"))),
        "blocked event explains the failed finalize"
    );
    drop(store);

    // Recovery: clean the tree and resume. The worker re-enters integrate,
    // finds the approved hash-fresh approval, and finalizes.
    std::fs::remove_file(&dirt).unwrap();
    let (status, _) = client
        .post_raw(
            &format!("/api/missions/{mission_id}/resume"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 200);
    client
        .wait_for(&mission_id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;
    assert!(dir.path().join("src/generated.txt").exists());

    handle.shutdown().await;
}

/// Finding 2: resuming a blocked mission must reopen dependency-blocked
/// tasks before the worker runs, or the worker instantly re-blocks and the
/// resume is a no-op.
#[tokio::test(flavor = "multi_thread")]
async fn resume_reopens_dependency_blocked_tasks() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    config.integration.final_approval = false;
    maestro_engine::save_config(dir.path(), &config).unwrap();
    sh(dir.path(), &["add", "-A"]);
    sh(dir.path(), &["commit", "-m", "config"]);

    // Craft the stranded state: dep task completed (as after a successful
    // retry), dependent still carrying the dependency-propagation block, and
    // the mission at rest as blocked.
    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let mission = store.create_mission("recover the dependent").unwrap();
    let tasks = store
        .insert_tasks(
            &mission.id,
            &[
                PlanTask {
                    key: "design-a".into(),
                    role: Role::Designer,
                    title: "Design A".into(),
                    description: "First.".into(),
                    dependencies: vec![],
                    acceptance: vec!["done".into()],
                    risk: RiskLevel::Low,
                    write_scopes: vec![],
                },
                PlanTask {
                    key: "design-b".into(),
                    role: Role::Designer,
                    title: "Design B".into(),
                    description: "Second.".into(),
                    dependencies: vec!["design-a".into()],
                    acceptance: vec!["done".into()],
                    risk: RiskLevel::Low,
                    write_scopes: vec![],
                },
            ],
        )
        .unwrap();
    store
        .set_task_status(&tasks[0].id, TaskStatus::Completed, None, Some("done"))
        .unwrap();
    store
        .set_task_status(
            &tasks[1].id,
            TaskStatus::Blocked,
            Some("dependency failed or blocked"),
            None,
        )
        .unwrap();
    store
        .set_mission_status(
            &mission.id,
            MissionStatus::Blocked,
            Some("blocked design-b"),
        )
        .unwrap();
    drop(store);

    let handle = start_daemon(DaemonOptions {
        root: dir.path().to_path_buf(),
        bind: None,
        token: None,
    })
    .await
    .unwrap();
    let client = Client::new(&handle, None);

    // Blocked missions are not auto-resumed by boot recovery.
    let detail = client.mission(&mission.id).await;
    assert_eq!(detail.mission.status, MissionStatus::Blocked);

    let (status, _) = client
        .post_raw(
            &format!("/api/missions/{}/resume", mission.id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 200);

    // The dependent was reopened (never blocked again) and the mission runs
    // to completion instead of instantly re-blocking.
    let detail = client
        .wait_for(&mission.id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;
    let b = detail
        .tasks
        .iter()
        .find(|t| t.task_key == "design-b")
        .unwrap();
    assert_eq!(b.status, TaskStatus::Completed);

    handle.shutdown().await;
}

/// Finding 3: approving an approval must release every task of the mission
/// parked on it ("Waiting for approval <id>"), not only `approval.task_id` —
/// approval dedupe can park several siblings on one row.
#[tokio::test(flavor = "multi_thread")]
async fn approve_releases_all_tasks_parked_on_the_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let mission = store.create_mission("release my siblings").unwrap();
    let tasks = store
        .insert_tasks(
            &mission.id,
            &[
                PlanTask {
                    key: "design-a".into(),
                    role: Role::Designer,
                    title: "A".into(),
                    description: "a".into(),
                    dependencies: vec![],
                    acceptance: vec!["done".into()],
                    risk: RiskLevel::Low,
                    write_scopes: vec![],
                },
                PlanTask {
                    key: "design-b".into(),
                    role: Role::Designer,
                    title: "B".into(),
                    description: "b".into(),
                    dependencies: vec![],
                    acceptance: vec!["done".into()],
                    risk: RiskLevel::Low,
                    write_scopes: vec![],
                },
            ],
        )
        .unwrap();
    let approval = store
        .create_approval(maestro_store::ApprovalRequest {
            mission_id: &mission.id,
            task_id: Some(&tasks[0].id),
            action: "network",
            resource: "https://example.com",
            patch_hash: None,
            policy_snapshot: serde_json::json!({}),
            expires_at: None,
        })
        .unwrap();
    for task in &tasks {
        store
            .set_task_status(
                &task.id,
                TaskStatus::Blocked,
                Some(&format!("Waiting for approval {}", approval.id)),
                None,
            )
            .unwrap();
    }
    store
        .set_mission_status(&mission.id, MissionStatus::Blocked, Some("parked"))
        .unwrap();
    drop(store);

    let (status, _) = client
        .post_raw(
            &format!("/api/approvals/{}", approval.id),
            serde_json::json!({ "approve": true }),
        )
        .await;
    assert_eq!(status, 200);

    // BOTH parked tasks were released (they may already be running/completed
    // by the time we look — the invariant is that neither stays blocked).
    client
        .wait_for(&mission.id, |d| {
            d.tasks.iter().all(|t| t.status != TaskStatus::Blocked)
        })
        .await;
    client
        .wait_for(&mission.id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;

    handle.shutdown().await;
}

/// Finding (wave 4): REJECTING a policy approval must release the parked
/// task(s) just like approving does — the decision is terminal either way,
/// and nothing else ever unparks an approval-parked task. The released task
/// re-runs (its policy decider surfaces the denial) instead of stranding the
/// task, and the mission, as blocked forever.
#[tokio::test(flavor = "multi_thread")]
async fn reject_releases_tasks_parked_on_the_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    // Same crafted state as the approve fan-out test: two tasks parked on
    // one approval, mission at rest as blocked.
    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let mission = store.create_mission("unstrand my tasks").unwrap();
    let tasks = store
        .insert_tasks(
            &mission.id,
            &[
                PlanTask {
                    key: "design-a".into(),
                    role: Role::Designer,
                    title: "A".into(),
                    description: "a".into(),
                    dependencies: vec![],
                    acceptance: vec!["done".into()],
                    risk: RiskLevel::Low,
                    write_scopes: vec![],
                },
                PlanTask {
                    key: "design-b".into(),
                    role: Role::Designer,
                    title: "B".into(),
                    description: "b".into(),
                    dependencies: vec![],
                    acceptance: vec!["done".into()],
                    risk: RiskLevel::Low,
                    write_scopes: vec![],
                },
            ],
        )
        .unwrap();
    let approval = store
        .create_approval(maestro_store::ApprovalRequest {
            mission_id: &mission.id,
            task_id: Some(&tasks[0].id),
            action: "network",
            resource: "https://example.com",
            patch_hash: None,
            policy_snapshot: serde_json::json!({}),
            expires_at: None,
        })
        .unwrap();
    for task in &tasks {
        store
            .set_task_status(
                &task.id,
                TaskStatus::Blocked,
                Some(&format!("Waiting for approval {}", approval.id)),
                None,
            )
            .unwrap();
    }
    store
        .set_mission_status(&mission.id, MissionStatus::Blocked, Some("parked"))
        .unwrap();
    drop(store);

    let (status, _) = client
        .post_raw(
            &format!("/api/approvals/{}", approval.id),
            serde_json::json!({ "approve": false }),
        )
        .await;
    assert_eq!(status, 200);

    // Both parked tasks were released — neither stays blocked — and the
    // mission does not sit blocked forever: the re-run drives it to rest
    // (the designers never touch the rejected resource, so it completes).
    client
        .wait_for(&mission.id, |d| {
            d.tasks.iter().all(|t| t.status != TaskStatus::Blocked)
        })
        .await;
    client
        .wait_for(&mission.id, |d| {
            d.mission.status == MissionStatus::Completed
        })
        .await;

    handle.shutdown().await;
}

/// Finding (wave 4): Last-Event-ID must win over ?after=. A standard
/// EventSource reconnect resends its original URL (with the stale ?after=)
/// plus the header carrying the true resume point; replaying from the query
/// param would re-send the whole backlog on every reconnect.
#[tokio::test(flavor = "multi_thread")]
async fn last_event_id_wins_over_the_after_query_param() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    const TOTAL: usize = 40;
    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let mission = store.create_mission("reconnect resume").unwrap();
    for i in 0..TOTAL {
        store
            .append_event(
                &mission.id,
                None,
                "bench.tick",
                serde_json::json!({ "i": i }),
            )
            .unwrap();
    }
    let seqs: Vec<i64> = store
        .events_after(0, Some(&mission.id))
        .unwrap()
        .iter()
        .filter(|e| e.kind == "bench.tick")
        .map(|e| e.seq)
        .collect();
    drop(store);
    let mid = seqs[TOTAL / 2];
    let expected = seqs.iter().filter(|s| **s > mid).count();

    // Stale ?after=0 in the URL, true cursor in the header.
    let resp = client
        .req(
            reqwest::Method::GET,
            &format!("/api/events?missionId={}&after=0", mission.id),
        )
        .header("Last-Event-ID", mid.to_string())
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let events = read_sse(resp, |seen| {
        seen.iter().filter(|e| e.kind == "bench.tick").count() >= expected
    })
    .await;

    let ticks: Vec<&EventRecord> = events.iter().filter(|e| e.kind == "bench.tick").collect();
    assert_eq!(ticks.len(), expected);
    assert!(
        ticks.iter().all(|e| e.seq > mid),
        "replay must start after Last-Event-ID, not after the stale ?after="
    );

    handle.shutdown().await;
}

/// Finding 5: /api/state and GET /api/missions include every non-terminal
/// mission but only the 20 most recent terminal ones; ?all=true bypasses.
#[tokio::test(flavor = "multi_thread")]
async fn state_snapshot_caps_terminal_missions() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let client = Client::new(&handle, None);

    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    for i in 0..30 {
        let m = store.create_mission(&format!("done {i}")).unwrap();
        store
            .set_mission_status(&m.id, MissionStatus::Completed, None)
            .unwrap();
    }
    let running = store.create_mission("still going").unwrap();
    store
        .set_mission_status(&running.id, MissionStatus::Running, None)
        .unwrap();
    drop(store);

    let state: serde_json::Value = client.get_json("/api/state").await;
    let missions = state["missions"].as_array().unwrap();
    assert_eq!(missions.len(), 21, "1 active + 20 recent terminal");
    assert!(missions
        .iter()
        .any(|m| m["mission"]["id"] == serde_json::json!(running.id)));

    let all: serde_json::Value = client.get_json("/api/state?all=true").await;
    assert_eq!(all["missions"].as_array().unwrap().len(), 31);

    let capped: Vec<serde_json::Value> = client.get_json("/api/missions").await;
    assert_eq!(capped.len(), 21);
    let full: Vec<serde_json::Value> = client.get_json("/api/missions?all=true").await;
    assert_eq!(full.len(), 31);

    handle.shutdown().await;
}

/// Finding 6: dropping a DaemonHandle (instead of calling shutdown) must not
/// leak the daemon — the port stops accepting shortly after the drop.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_handle_shuts_the_daemon_down() {
    let dir = tempfile::tempdir().unwrap();
    let handle = boot(dir.path(), test_config(), None).await;
    let url = handle.url.clone();
    let http = reqwest::Client::new();
    // Alive before the drop.
    assert!(http.get(format!("{url}/health")).send().await.is_ok());

    drop(handle);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match http.get(format!("{url}/health")).send().await {
            Err(_) => break, // connection refused — daemon is gone
            Ok(_) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "daemon still accepting 5s after the handle was dropped"
                );
                tokio::time::sleep(POLL).await;
            }
        }
    }
}

/// Finding 7: SSE replay pages through the log in bounded batches; a large
/// backlog is still delivered completely and in order.
#[tokio::test(flavor = "multi_thread")]
async fn sse_replay_pages_through_large_logs() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.integration.final_approval = false;
    let handle = boot(dir.path(), config, None).await;
    let client = Client::new(&handle, None);

    const TOTAL: usize = 2_500; // > 2 replay pages of 1000
    let store = Store::open(&dir.path().join(".maestro")).unwrap();
    let mission = store.create_mission("event flood").unwrap();
    for i in 0..TOTAL {
        store
            .append_event(
                &mission.id,
                None,
                "bench.tick",
                serde_json::json!({ "i": i }),
            )
            .unwrap();
    }
    drop(store);

    let resp = client
        .req(
            reqwest::Method::GET,
            &format!("/api/events?missionId={}&after=0", mission.id),
        )
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let events = read_sse(resp, |seen| {
        seen.iter().filter(|e| e.kind == "bench.tick").count() >= TOTAL
    })
    .await;

    let ticks: Vec<&EventRecord> = events.iter().filter(|e| e.kind == "bench.tick").collect();
    assert_eq!(ticks.len(), TOTAL);
    for (i, ev) in ticks.iter().enumerate() {
        assert_eq!(ev.payload["i"], serde_json::json!(i), "in-order replay");
        if i > 0 {
            assert!(ev.seq > ticks[i - 1].seq, "seq strictly ascending");
        }
    }

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn history_requires_auth_binds_preview_and_fences_inflight_planning() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    set_role_model(&mut config, "planner", "slow-planner");
    let handle = boot(dir.path(), config, Some("history-test-token")).await;
    let anonymous = Client::new(&handle, None);
    assert_eq!(
        anonymous
            .req(reqwest::Method::GET, "/api/history")
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    let client = Client::new(&handle, Some("history-test-token"));
    let mid = client.submit("change the app").await;
    let preview: maestro_types::history::RestorePreview =
        client.get_json("/api/history/undo").await;
    assert!(preview.affected_missions.contains(&mid));
    assert_eq!(
        client
            .post_raw("/api/history/undo", serde_json::json!({}))
            .await
            .0,
        400
    );
    std::fs::write(dir.path().join("manual.txt"), "Keep manual edits\n").unwrap();
    assert_eq!(
        client
            .post_raw(
                "/api/history/undo",
                serde_json::json!({"token":preview.token})
            )
            .await
            .0,
        409
    );
    let preview: maestro_types::history::RestorePreview =
        client.get_json("/api/history/undo").await;
    let _: maestro_types::history::RestoreResult = client
        .post_json(
            "/api/history/undo",
            serde_json::json!({"token":preview.token}),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let detail: MissionDetail = client.get_json(&format!("/api/missions/{mid}")).await;
    assert_eq!(detail.mission.status, MissionStatus::Cancelled);
    assert_eq!(
        client
            .post_raw(
                &format!("/api/missions/{mid}/resume"),
                serde_json::json!({})
            )
            .await
            .0,
        409
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("manual.txt")).unwrap(),
        "Keep manual edits\n"
    );
    assert!(!dir.path().join("src/generated.txt").exists());
    handle.shutdown().await;
}
