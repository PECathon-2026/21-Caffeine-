//! DTOs for the daemon HTTP API. Wire format is camelCase JSON.

use crate::domain::{
    Approval, ChatMessage, Mission, MissionStats, MissionStatus, ProviderHealth, Task,
};
use crate::plan::Plan;
use crate::provider::ModelDiscovery;
use crate::Millis;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub version: String,
    pub generation: String,
    pub pid: i64,
    pub started_at: Millis,
    /// Highest event sequence number in the log (the SSE cursor head).
    #[serde(default)]
    pub last_seq: i64,
    /// Identity of the daemon's binary, captured at boot (see
    /// `maestro_types::build_id`). Empty from pre-handshake daemons.
    #[serde(default)]
    pub build_id: String,
    /// Roles whose primary model runs on a mock provider — canned replies,
    /// no API calls. Clients surface this loudly; silence would let a mock
    /// masquerade as a real control plane.
    #[serde(default)]
    pub mock_roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitMissionRequest {
    pub objective: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitMissionResponse {
    pub mission_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionSummary {
    pub mission: Mission,
    pub stats: MissionStats,
    pub task_counts: TaskCounts,
    pub pending_approvals: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCounts {
    pub total: i64,
    pub completed: i64,
    pub running: i64,
    pub failed: i64,
    pub blocked: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionDetail {
    pub mission: Mission,
    pub tasks: Vec<Task>,
    pub stats: MissionStats,
    #[serde(default)]
    pub plan: Option<Plan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerRequest {
    pub text: String,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryRequest {
    pub task_id: String,
    /// "provider/model" override for the retried attempts.
    #[serde(default)]
    pub model_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDecisionRequest {
    pub approve: bool,
    /// Replacement plan (revalidated server-side) when editing before approval.
    #[serde(default)]
    pub edited_plan: Option<Plan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecisionRequest {
    pub approve: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResponse {
    pub unified: String,
    #[serde(default)]
    pub patch_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateSnapshot {
    pub missions: Vec<MissionSummary>,
    pub approvals: Vec<Approval>,
    pub workers: Vec<WorkerInfo>,
    pub provider_health: Vec<ProviderHealth>,
    pub generation: String,
    /// Highest event sequence number in the log (the SSE cursor head).
    #[serde(default)]
    pub last_seq: i64,
    /// Identity of the daemon's binary, captured at boot (see
    /// `maestro_types::build_id`). Empty from pre-handshake daemons.
    #[serde(default)]
    pub build_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerInfo {
    pub mission_id: String,
    pub status: MissionStatus,
    pub started_at: Millis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendRequest {
    pub text: String,
    #[serde(default)]
    pub images: Vec<String>,
}

/// POST /api/chat/sessions body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateChatSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendResponse {
    pub messages: Vec<ChatMessage>,
    /// Set when the message launched a mission.
    #[serde(default)]
    pub mission_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsResponse {
    pub discovery: ModelDiscovery,
    /// Which model each role currently runs (config-resolved).
    #[serde(default)]
    pub roles: Vec<RoleModelInfo>,
}

/// One role's resolved model assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleModelInfo {
    pub role: String,
    pub provider: String,
    pub model: String,
    /// Fallback models as "provider/model" strings, in order.
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

/// Control-plane operation latency metrics (see `maestro bench` / GET /api/metrics).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpMetrics {
    pub op: String,
    pub count: i64,
    pub p50_us: i64,
    pub p99_us: i64,
    pub max_us: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsResponse {
    pub ops: Vec<OpMetrics>,
    pub uptime_ms: i64,
    pub rss_bytes: i64,
    /// Live circuit-breaker snapshots per provider.
    #[serde(default)]
    pub provider_health: Vec<ProviderHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub error: String,
}

/// `.maestro/daemon.json` — daemon discovery info for clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    pub url: String,
    pub pid: i64,
    pub generation: String,
    pub started_at: Millis,
    #[serde(default)]
    pub token: Option<String>,
}
