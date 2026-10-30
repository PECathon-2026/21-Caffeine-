//! Axum router and handlers for the daemon HTTP API (SPEC §Daemon HTTP API).
//! All bodies are JSON; errors serialize as `{"error": msg}` with the status
//! mapped from [`MaestroError`] (NotFound → 404, Conflict → 409, storage/io
//! → 500, everything else → 400).

use crate::sse::events_handler;
use crate::workers::Workers;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use maestro_engine::{looks_like_implementation_request, Orchestrator};
use maestro_providers::ProviderRegistry;
use maestro_store::Store;
use maestro_types::api::{
    ApprovalDecisionRequest, ChatSendRequest, ChatSendResponse, CreateChatSessionRequest,
    DiffResponse, HealthResponse, MetricsResponse, MissionDetail, MissionSummary, ModelsResponse,
    PlanDecisionRequest, RetryRequest, StateSnapshot, SteerRequest, SubmitMissionRequest,
    SubmitMissionResponse, TaskCounts, WorkerInfo,
};
use maestro_types::domain::{Approval, ChatMessage, ChatRole, Mission, MissionStatus, TaskStatus};
use maestro_types::event::kind;
use maestro_types::{now_ms, MaestroError, Millis};
use serde::Deserialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Decider identity recorded on API-driven approvals and directives.
const API_ACTOR: &str = "api";

/// Reason the store writes on tasks blocked by dependency propagation
/// (`refresh_ready`). Resume reopens exactly these — never approval-parked
/// or agent-declared blocks.
const DEP_BLOCKED_REASON: &str = "dependency failed or blocked";

/// Terminal missions included in capped snapshots (`/api/state`,
/// `GET /api/missions`) alongside every non-terminal mission.
const SNAPSHOT_TERMINAL_CAP: usize = 20;

/// Upper bound on cached mission diffs — they can be megabytes each, and the
/// daemon is long-lived, so the cache must not grow with mission count. The
/// least recently used entry is evicted beyond this.
const DIFF_CACHE_CAP: usize = 8;

/// Shared state for every handler.
pub(crate) struct AppState {
    pub(crate) orch: Orchestrator,
    pub(crate) store: Store,
    pub(crate) registry: Arc<ProviderRegistry>,
    pub(crate) workers: Workers,
    pub(crate) generation: String,
    /// Binary identity captured at boot (stale-daemon handshake).
    pub(crate) build_id: String,
    /// Roles assigned to mock providers (computed at boot, for /health).
    pub(crate) mock_roles: Vec<String>,
    pub(crate) token: Option<String>,
    pub(crate) started_at: Millis,
    /// Cancelled by POST /api/shutdown or a signal; ends serving and SSE.
    pub(crate) shutdown: CancellationToken,
    /// GET /diff responses as `(mission_id, integration_head, response)` in
    /// LRU order (back = most recent), capped at [`DIFF_CACHE_CAP`] — avoids
    /// a git spawn per poll without retaining every mission's diff forever.
    pub(crate) diff_cache: Mutex<VecDeque<(String, String, DiffResponse)>>,
    #[allow(dead_code)]
    pub(crate) root: PathBuf,
}

/// JSON error envelope carrying the mapped HTTP status.
pub(crate) struct ApiFailure(pub(crate) MaestroError);

impl From<MaestroError> for ApiFailure {
    fn from(e: MaestroError) -> Self {
        ApiFailure(e)
    }
}

impl From<serde_json::Error> for ApiFailure {
    fn from(e: serde_json::Error) -> Self {
        ApiFailure(MaestroError::from(e))
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            MaestroError::NotFound(_) => StatusCode::NOT_FOUND,
            MaestroError::Conflict(_) => StatusCode::CONFLICT,
            MaestroError::Storage(_) | MaestroError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        let body = Json(serde_json::json!({ "error": self.0.to_string() }));
        (status, body).into_response()
    }
}

type ApiResult<T> = std::result::Result<T, ApiFailure>;

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "missing or invalid bearer token" })),
    )
        .into_response()
}

/// Bearer-token check: enforced whenever a token is configured (even on
/// loopback). Binding off-loopback without a token is refused at boot.
async fn auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if let Some(expected) = &state.token {
        let presented = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if !presented.is_some_and(|p| constant_time_eq(p.as_bytes(), expected.as_bytes())) {
            return unauthorized();
        }
    }
    next.run(req).await
}

/// Length-leaking-only equality: comparison time depends on the candidate's
/// length, never on how many leading bytes match the secret.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Build the full router. `/health` stays outside the auth layer so liveness
/// probes work without credentials.
pub(crate) fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/api/state", get(get_state))
        .route("/api/missions", get(list_missions).post(submit_mission))
        .route("/api/missions/{id}", get(mission_detail))
        .route("/api/missions/{id}/pause", post(pause_mission))
        .route("/api/missions/{id}/resume", post(resume_mission))
        .route("/api/missions/{id}/cancel", post(cancel_mission))
        .route("/api/missions/{id}/retry", post(retry_task))
        .route("/api/missions/{id}/steer", post(steer_mission))
        .route("/api/missions/{id}/plan", get(get_plan))
        .route("/api/missions/{id}/plan/approve", post(decide_plan))
        .route("/api/missions/{id}/diff", get(mission_diff))
        .route("/api/approvals", get(list_approvals))
        .route("/api/approvals/{id}", post(decide_approval))
        .route("/api/events", get(events_handler))
        .route("/api/models", get(list_models))
        .route("/api/previews", get(list_previews))
        .route("/api/previews/stop", post(stop_previews))
        .route("/api/images/{id}", get(get_image))
        .route(
            "/api/images",
            post(upload_image).layer(axum::extract::DefaultBodyLimit::max(17 * 1024 * 1024)),
        )
        .route("/api/history", get(code_history).post(save_version))
        .route("/api/history/undo", get(preview_undo).post(undo_version))
        .route("/api/history/{id}/diff", get(version_diff))
        .route(
            "/api/history/{id}/restore",
            get(preview_restore).post(restore_version),
        )
        .route(
            "/api/chat/sessions",
            get(list_chat_sessions).post(create_chat_session),
        )
        .route(
            "/api/chat/{session_id}/messages",
            get(chat_history).post(chat_send),
        )
        .route("/api/metrics", get(get_metrics))
        .route("/api/shutdown", post(shutdown))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .route("/health", get(health))
        .merge(api)
        .with_state(state)
}

// ---- helpers ----------------------------------------------------------------

fn mission_summary(state: &AppState, mission: Mission) -> ApiResult<MissionSummary> {
    let stats = state.store.mission_stats(&mission.id)?;
    let tasks = state.store.list_tasks(&mission.id)?;
    let mut counts = TaskCounts {
        total: tasks.len() as i64,
        ..TaskCounts::default()
    };
    for t in &tasks {
        match t.status {
            TaskStatus::Completed => counts.completed += 1,
            TaskStatus::Running => counts.running += 1,
            TaskStatus::Failed => counts.failed += 1,
            TaskStatus::Blocked => counts.blocked += 1,
            TaskStatus::Proposed | TaskStatus::Ready => {}
        }
    }
    let pending = state.store.pending_approvals(Some(&mission.id))?.len() as i64;
    Ok(MissionSummary {
        mission,
        stats,
        task_counts: counts,
        pending_approvals: pending,
    })
}

/// Set the mission status and emit the corresponding audit event.
fn set_status(
    state: &AppState,
    mission_id: &str,
    status: MissionStatus,
    reason: Option<&str>,
) -> ApiResult<()> {
    state.store.set_mission_status(mission_id, status, reason)?;
    state.store.append_event(
        mission_id,
        None,
        kind::MISSION_STATUS,
        serde_json::json!({ "status": status.as_str(), "reason": reason }),
    )?;
    Ok(())
}

/// Missions included in a capped snapshot: every non-terminal mission plus
/// the [`SNAPSHOT_TERMINAL_CAP`] most recent terminal ones (`list_missions`
/// returns newest first). `all=true` bypasses the cap.
fn snapshot_missions(state: &AppState, all: bool) -> ApiResult<Vec<Mission>> {
    let missions = state.store.list_missions()?;
    if all {
        return Ok(missions);
    }
    let mut terminal = 0usize;
    Ok(missions
        .into_iter()
        .filter(|m| {
            if m.status.is_terminal() {
                terminal += 1;
                terminal <= SNAPSHOT_TERMINAL_CAP
            } else {
                true
            }
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    /// `?all=true` bypasses the terminal-mission cap.
    #[serde(default)]
    all: bool,
}

// ---- handlers ---------------------------------------------------------------

async fn health(State(state): State<Arc<AppState>>) -> ApiResult<Json<HealthResponse>> {
    Ok(Json(HealthResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION").to_string(),
        generation: state.generation.clone(),
        pid: std::process::id() as i64,
        started_at: state.started_at,
        last_seq: state.store.last_event_seq()?,
        build_id: state.build_id.clone(),
        mock_roles: state.mock_roles.clone(),
    }))
}

async fn get_state(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SnapshotQuery>,
) -> ApiResult<Json<StateSnapshot>> {
    let mut missions = Vec::new();
    for mission in snapshot_missions(&state, q.all)? {
        missions.push(mission_summary(&state, mission)?);
    }
    let mut workers = Vec::new();
    for (mission_id, started_at) in state.workers.snapshot() {
        let status = state.store.get_mission(&mission_id)?.status;
        workers.push(WorkerInfo {
            mission_id,
            status,
            started_at,
        });
    }
    Ok(Json(StateSnapshot {
        missions,
        approvals: state.store.pending_approvals(None)?,
        workers,
        provider_health: state.registry.health_snapshots(),
        build_id: state.build_id.clone(),
        generation: state.generation.clone(),
        last_seq: state.store.last_event_seq()?,
    }))
}

async fn list_missions(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SnapshotQuery>,
) -> ApiResult<Json<Vec<MissionSummary>>> {
    let mut out = Vec::new();
    for mission in snapshot_missions(&state, q.all)? {
        out.push(mission_summary(&state, mission)?);
    }
    Ok(Json(out))
}

async fn submit_mission(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubmitMissionRequest>,
) -> ApiResult<Json<SubmitMissionResponse>> {
    if req.objective.trim().is_empty() {
        return Err(MaestroError::Config("objective must not be empty".into()).into());
    }
    let mission = state.orch.create_mission(&req.objective).await?;
    if let Some(session_id) = &req.session_id {
        state
            .store
            .set_active_mission(session_id, Some(&mission.id))?;
    }
    state.workers.ensure(&state.orch, &mission.id);
    Ok(Json(SubmitMissionResponse {
        mission_id: mission.id,
    }))
}

async fn mission_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<MissionDetail>> {
    let mission = state.store.get_mission(&id)?;
    Ok(Json(MissionDetail {
        tasks: state.store.list_tasks(&id)?,
        stats: state.store.mission_stats(&id)?,
        plan: state.store.get_mission_plan(&id)?,
        mission,
    }))
}

async fn code_history(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<maestro_types::history::History>> {
    Ok(Json(state.orch.code_history().await?))
}

#[derive(Deserialize)]
struct SaveVersion {
    label: String,
}

async fn save_version(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SaveVersion>,
) -> ApiResult<Json<maestro_types::history::Version>> {
    if req.label.trim().is_empty() || req.label.len() > 500 {
        return Err(MaestroError::Config("version label must be 1–500 characters".into()).into());
    }
    Ok(Json(state.orch.save_code_version(&req.label).await?))
}

async fn version_diff(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        serde_json::json!({"unified":state.orch.version_diff(&id).await?}),
    ))
}

async fn preview_restore(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<maestro_types::history::RestorePreview>> {
    Ok(Json(state.orch.preview_restore(&id).await?))
}

async fn preview_undo(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<maestro_types::history::RestorePreview>> {
    let v = state.store.history_undo_target()?;
    Ok(Json(state.orch.preview_restore(&v.id).await?))
}

async fn apply_restore(
    state: &AppState,
    id: &str,
    req: maestro_types::history::RestoreRequest,
    undo: bool,
) -> ApiResult<Json<maestro_types::history::RestoreResult>> {
    let token = req.token.ok_or_else(|| {
        MaestroError::Config("preview the restore first and send its token".into())
    })?;
    let result = state.orch.restore_version(id, &token, undo).await?;
    for m in &result.retired_missions {
        if let Some(token) = state.workers.cancel_token(m) {
            token.cancel();
        }
    }
    Ok(Json(result))
}

async fn restore_version(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<maestro_types::history::RestoreRequest>,
) -> ApiResult<Json<maestro_types::history::RestoreResult>> {
    apply_restore(&state, &id, req, false).await
}

async fn undo_version(
    State(state): State<Arc<AppState>>,
    Json(req): Json<maestro_types::history::RestoreRequest>,
) -> ApiResult<Json<maestro_types::history::RestoreResult>> {
    let v = state.store.history_undo_target()?;
    apply_restore(&state, &v.id, req, true).await
}

async fn pause_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mission = state.store.get_mission(&id)?;
    if !mission.status.is_active() {
        return Err(MaestroError::Conflict(format!(
            "mission is {}; only active missions can be paused",
            mission.status.as_str()
        ))
        .into());
    }
    // Status first, then cancel: the worker observes `paused` at its safe
    // boundary and exits without overwriting it.
    set_status(&state, &id, MissionStatus::Paused, Some("paused via api"))?;
    if let Some(token) = state.workers.cancel_token(&id) {
        token.cancel();
    }
    Ok(Json(serde_json::json!({ "status": "paused" })))
}

async fn resume_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.store.assert_history_mission_current(&id)?;
    let mission = state.store.get_mission(&id)?;
    match mission.status {
        MissionStatus::Blocked => {
            // Reopen recoverable tasks BEFORE the worker runs, or it
            // immediately re-blocks on the same dependency propagation.
            // Only dependency-propagated blocks are reopened; approval-parked
            // ("Waiting for approval …") and agent-declared blocks stay put.
            for task in state.store.list_tasks(&id)? {
                let reason = task.status_reason.as_deref().unwrap_or("");
                // Dependency-propagated and budget-exhaustion blocks are
                // recoverable on resume (the operator raises the cap and
                // resumes); approval parks and agent-declared blocks stay.
                if task.status == TaskStatus::Blocked
                    && (reason == DEP_BLOCKED_REASON || reason.starts_with("budget exhausted"))
                {
                    state.store.set_task_status(
                        &task.id,
                        TaskStatus::Proposed,
                        Some("reopened on resume"),
                        None,
                    )?;
                }
            }
            state.store.refresh_ready(&id)?;
            set_status(&state, &id, MissionStatus::Planned, Some("resumed via api"))?;
        }
        MissionStatus::Paused => {
            set_status(&state, &id, MissionStatus::Planned, Some("resumed via api"))?;
        }
        MissionStatus::Planning | MissionStatus::Planned | MissionStatus::Running => {}
        // AwaitingApproval / AwaitingPlanApproval / terminal: 409. Recovery
        // for a stuck final approval flows through the approval decision
        // handler (reject/finalize-failure both move the mission to blocked).
        other => {
            return Err(MaestroError::Conflict(format!(
                "mission is {}; it cannot be resumed",
                other.as_str()
            ))
            .into());
        }
    }
    state.workers.ensure(&state.orch, &id);
    state.orch.poke();
    let status = state.store.get_mission(&id)?.status;
    Ok(Json(serde_json::json!({ "status": status.as_str() })))
}

async fn cancel_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mission = state.store.get_mission(&id)?;
    if mission.status.is_terminal() {
        return Err(MaestroError::Conflict(format!(
            "mission is already {}",
            mission.status.as_str()
        ))
        .into());
    }
    set_status(
        &state,
        &id,
        MissionStatus::Cancelled,
        Some("cancelled via api"),
    )?;
    match state.workers.cancel_token(&id) {
        // The worker observes the terminal status and reclaims the
        // mission's worktrees when it exits.
        Some(token) => token.cancel(),
        // No worker will ever run this mission again: reclaim inline.
        None => state.orch.cleanup_mission_workspaces(&id, true).await,
    }
    Ok(Json(serde_json::json!({ "status": "cancelled" })))
}

async fn retry_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<RetryRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let task = state.store.get_task(&req.task_id)?;
    if task.mission_id != id {
        return Err(MaestroError::NotFound(format!("task {} in mission {id}", req.task_id)).into());
    }
    let task = state
        .store
        .retry_task(&req.task_id, req.model_override.clone())?;
    state.store.append_event(
        &id,
        Some(&task.id),
        kind::TASK_RETRY,
        serde_json::json!({ "manual": true, "modelOverride": req.model_override }),
    )?;
    state.workers.ensure(&state.orch, &id);
    state.orch.poke();
    Ok(Json(serde_json::to_value(task)?))
}

async fn steer_mission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SteerRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.text.trim().is_empty() {
        return Err(MaestroError::Config("steer text must not be empty".into()).into());
    }
    state.store.get_mission(&id)?; // 404 before writing anything
    let directive = state
        .store
        .add_directive(&id, req.task_id.as_deref(), &req.text, API_ACTOR)?;
    state.store.append_event(
        &id,
        req.task_id.as_deref(),
        kind::DIRECTIVE_ADDED,
        serde_json::json!({ "directiveId": directive.id, "text": directive.text }),
    )?;
    state.orch.poke();
    Ok(Json(serde_json::json!({ "directiveId": directive.id })))
}

async fn get_plan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<maestro_types::plan::Plan>> {
    let plan = state
        .store
        .get_mission_plan(&id)?
        .ok_or_else(|| MaestroError::NotFound(format!("mission {id} has no plan yet")))?;
    Ok(Json(plan))
}

async fn decide_plan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<PlanDecisionRequest>,
) -> ApiResult<Json<Mission>> {
    let mission = state
        .orch
        .decide_plan(&id, req.approve, req.edited_plan)
        .await?;
    if req.approve {
        state.workers.ensure(&state.orch, &id);
    }
    Ok(Json(mission))
}

async fn mission_diff(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<DiffResponse>> {
    // Integrated diffs are immutable for a given integration head: serve
    // them from the cache instead of spawning git per request. The mutex is
    // never held across an await.
    let head = state.store.get_mission(&id)?.integration_head;
    if let Some(head) = &head {
        let mut cache = state
            .diff_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pos) = cache.iter().position(|(mid, h, _)| mid == &id && h == head) {
            // Refresh the hit's LRU position so hot missions stay cached.
            if let Some(entry) = cache.remove(pos) {
                let resp = entry.2.clone();
                cache.push_back(entry);
                return Ok(Json(resp));
            }
        }
    }
    let (unified, patch_hash) = state.orch.mission_diff(&id).await?;
    let resp = DiffResponse {
        unified,
        patch_hash,
    };
    if let Some(head) = head {
        let mut cache = state
            .diff_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // One entry per mission (a stale head is replaced, never
        // accumulated); evict the least recently used beyond the cap.
        cache.retain(|(mid, _, _)| mid != &id);
        if cache.len() >= DIFF_CACHE_CAP {
            cache.pop_front();
        }
        cache.push_back((id, head, resp.clone()));
    }
    Ok(Json(resp))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalsQuery {
    #[serde(default)]
    mission_id: Option<String>,
}

async fn list_approvals(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ApprovalsQuery>,
) -> ApiResult<Json<Vec<Approval>>> {
    Ok(Json(
        state.store.pending_approvals(q.mission_id.as_deref())?,
    ))
}

async fn decide_approval(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ApprovalDecisionRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let approval = state
        .store
        .decide_approval(&id, req.approve, API_ACTOR, req.note.as_deref())?;
    state.store.append_event(
        &approval.mission_id,
        approval.task_id.as_deref(),
        kind::APPROVAL_DECIDED,
        serde_json::json!({
            "approvalId": approval.id,
            "action": approval.action,
            "approved": req.approve,
        }),
    )?;
    if approval.action == "integrate" {
        return decide_integrate_approval(&state, approval, req.approve).await;
    }
    // A policy `ask` parked task(s) as blocked; EITHER decision releases
    // every task of this mission waiting on this approval (dedupe can park
    // several siblings on one approval id), not just `approval.task_id`.
    // Rejection must release too: nothing else ever unparks an
    // approval-parked task, and the re-run's policy decider surfaces the
    // rejection as PolicyDenied (consuming an attempt) instead of leaving
    // the task — and the mission — blocked forever.
    let reason = if req.approve {
        "approval granted"
    } else {
        "approval rejected"
    };
    let mut released = false;
    for task in state.store.list_tasks(&approval.mission_id)? {
        if task.status != TaskStatus::Blocked {
            continue;
        }
        let own = approval.task_id.as_deref() == Some(task.id.as_str());
        let parked = task
            .status_reason
            .as_deref()
            .is_some_and(|r| r.contains(&approval.id));
        if own || parked {
            state
                .store
                .set_task_status(&task.id, TaskStatus::Ready, Some(reason), None)?;
            released = true;
        }
    }
    if released || approval.task_id.is_some() {
        state.workers.ensure(&state.orch, &approval.mission_id);
        state.orch.poke();
    }
    Ok(Json(serde_json::json!({ "approval": approval })))
}

/// Terminal handling of a decided final-integration approval. Both failure
/// paths park the mission as `blocked` (instead of stranding it in
/// `awaiting_approval` with a consumed approval): a rejected approval blocks
/// with a fixed reason, and a failed finalize (dirty tree, moved HEAD, stale
/// hash) blocks with the error — `resume` then re-enters the worker, which
/// mints a fresh approval through try_complete when needed.
async fn decide_integrate_approval(
    state: &AppState,
    approval: Approval,
    approve: bool,
) -> ApiResult<Json<serde_json::Value>> {
    if !approve {
        let mission = state.store.get_mission(&approval.mission_id)?;
        if mission.status == MissionStatus::AwaitingApproval {
            set_status(
                state,
                &approval.mission_id,
                MissionStatus::Blocked,
                Some("final integration was rejected"),
            )?;
        }
        return Ok(Json(serde_json::json!({ "approval": approval })));
    }
    match state.orch.finalize_mission(&approval.mission_id).await {
        Ok(outcome) => Ok(Json(serde_json::json!({
            "approval": approval,
            "finalized": { "commit": outcome.commit, "branch": outcome.branch },
        }))),
        Err(e) => {
            let reason = e.to_string();
            let mission = state.store.get_mission(&approval.mission_id)?;
            if mission.status == MissionStatus::AwaitingApproval {
                set_status(
                    state,
                    &approval.mission_id,
                    MissionStatus::Blocked,
                    Some(&reason),
                )?;
            }
            Err(match e {
                MaestroError::NotFound(_) | MaestroError::Conflict(_) => e,
                other => MaestroError::Conflict(other.to_string()),
            }
            .into())
        }
    }
}

async fn list_models(State(state): State<Arc<AppState>>) -> ApiResult<Json<ModelsResponse>> {
    let roles = state
        .orch
        .config()
        .roles
        .iter()
        .filter(|(role, _)| !state.orch.config().runtime.guild_roles || role.as_str() != "designer")
        .map(|(role, assignment)| maestro_types::api::RoleModelInfo {
            role: role.clone(),
            provider: assignment.model.provider.clone(),
            model: assignment.model.model.clone(),
            fallbacks: assignment
                .fallbacks
                .iter()
                .map(|f| format!("{}/{}", f.provider, f.model))
                .collect(),
        })
        .collect();
    Ok(Json(ModelsResponse {
        discovery: state.registry.discover_models().await,
        roles,
    }))
}

async fn list_chat_sessions(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<maestro_types::domain::ChatSession>>> {
    Ok(Json(state.store.list_chat_sessions()?))
}

async fn create_chat_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateChatSessionRequest>,
) -> ApiResult<Json<maestro_types::domain::ChatSession>> {
    let title = req.title.as_deref().unwrap_or("New chat");
    Ok(Json(state.store.create_chat_session(title)?))
}

/// Replay a session's messages (last 500, chronological) so clients can
/// resume where they left off.
async fn chat_history(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Vec<ChatMessage>>> {
    Ok(Json(state.store.list_chat_messages(&session_id, 500)?))
}

/// Chat turn routing: steering when the session's mission is active,
/// mission launch for implementation requests, architect Q&A otherwise.
async fn chat_send(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(mut req): Json<ChatSendRequest>,
) -> ApiResult<Json<ChatSendResponse>> {
    if req.text.trim().is_empty() {
        return Err(MaestroError::Config("message text must not be empty".into()).into());
    }
    if req.images.len() > 4 {
        return Err(MaestroError::Config("Attach at most four images per message".into()).into());
    }
    for id in &req.images {
        maestro_engine::media::data_url(&state.root, id)?;
        req.text.push_str(&format!("\n[[image:{id}]]"));
    }
    // First message into a client-minted session id creates the session;
    // its opening words become the title.
    let title: String = req.text.trim().chars().take(48).collect();
    let session = state.store.ensure_chat_session(&session_id, &title)?;
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(2);
    messages.push(state.store.add_chat_message(
        &session_id,
        ChatRole::User,
        &req.text,
        session.active_mission_id.as_deref(),
        None,
        None,
    )?);

    // 1. Active mission → steering directive.
    if let Some(mission_id) = &session.active_mission_id {
        let mission = state.store.get_mission(mission_id)?;
        if mission.status.is_active() {
            let directive = state
                .store
                .add_directive(mission_id, None, &req.text, API_ACTOR)?;
            state.store.append_event(
                mission_id,
                None,
                kind::DIRECTIVE_ADDED,
                serde_json::json!({ "directiveId": directive.id, "text": directive.text }),
            )?;
            state.orch.poke();
            messages.push(state.store.add_chat_message(
                &session_id,
                ChatRole::System,
                &format!(
                    "Steering directive added to the running mission: {}",
                    req.text
                ),
                Some(mission_id),
                None,
                None,
            )?);
            return Ok(Json(ChatSendResponse {
                messages,
                mission_id: Some(mission_id.clone()),
            }));
        }
    }

    // 2. Implementation request → launch a mission. A refused launch (e.g.
    // the workspace is not a git repository) answers in chat instead of
    // surfacing as a bare API failure.
    if looks_like_implementation_request(&req.text) {
        let mission = match state.orch.create_mission(&req.text).await {
            Ok(mission) => mission,
            Err(err) => {
                messages.push(state.store.add_chat_message(
                    &session_id,
                    ChatRole::Assistant,
                    &format!("I can't launch a mission here: {err}"),
                    None,
                    None,
                    None,
                )?);
                return Ok(Json(ChatSendResponse {
                    messages,
                    mission_id: None,
                }));
            }
        };
        state
            .store
            .set_active_mission(&session_id, Some(&mission.id))?;
        state.workers.ensure(&state.orch, &mission.id);
        messages.push(state.store.add_chat_message(
            &session_id,
            ChatRole::Assistant,
            &format!("Mission launched: {}", req.text),
            Some(&mission.id),
            None,
            None,
        )?);
        return Ok(Json(ChatSendResponse {
            messages,
            mission_id: Some(mission.id),
        }));
    }

    // 3. Question → architect answer. A stopped mission stays in history,
    // but must not keep this informational conversation attached to it.
    state.store.set_active_mission(&session_id, None)?;
    let cancel = state.shutdown.child_token();
    let answer = state.orch.answer_question(&req.text, &cancel).await?;
    messages.push(state.store.add_chat_message(
        &session_id,
        ChatRole::Assistant,
        &answer,
        None,
        None,
        None,
    )?);
    Ok(Json(ChatSendResponse {
        messages,
        mission_id: None,
    }))
}

/// Resident set size in bytes (Linux `/proc/self/statm`; 0 elsewhere).
fn rss_bytes() -> i64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    statm
        .split_whitespace()
        .nth(1)
        .and_then(|pages| pages.parse::<i64>().ok())
        .map(|pages| pages * 4096)
        .unwrap_or(0)
}

async fn get_metrics(State(state): State<Arc<AppState>>) -> ApiResult<Json<MetricsResponse>> {
    Ok(Json(MetricsResponse {
        ops: state.store.metrics().snapshot(),
        uptime_ms: now_ms() - state.started_at,
        rss_bytes: rss_bytes(),
        provider_health: state.registry.health_snapshots(),
    }))
}

async fn shutdown(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    tracing::info!("shutdown requested via api");
    maestro_engine::browser::stop_all(&state.root);
    // The response is already in flight; graceful shutdown lets it complete.
    state.shutdown.cancel();
    Json(serde_json::json!({ "ok": true }))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageUpload {
    name: String,
    data_url: String,
}
async fn upload_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageUpload>,
) -> ApiResult<Json<serde_json::Value>> {
    let root = state.root.clone();
    let asset = tokio::task::spawn_blocking(move || {
        maestro_engine::media::upload(&root, &req.name, &req.data_url)
    })
    .await
    .map_err(|e| MaestroError::Other(e.to_string()))??;
    Ok(Json(serde_json::to_value(asset)?))
}

async fn list_previews(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(maestro_engine::browser::list(&state.root))
}
async fn stop_previews(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    maestro_engine::browser::stop_all(&state.root);
    Json(serde_json::json!({"stopped":true}))
}

async fn get_image(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        maestro_engine::media::image_bytes(&state.root, &id)?,
    ))
}
