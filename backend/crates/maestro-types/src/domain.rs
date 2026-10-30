use crate::Millis;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Planner,
    Designer,
    Coder,
    Reviewer,
    Security,
}

impl Role {
    pub fn guild_name(&self) -> &'static str {
        match self {
            Self::Planner | Self::Designer => "Shogun",
            Self::Coder => "Samurai",
            Self::Reviewer => "Sentinel",
            Self::Security => "Shadow",
        }
    }
    pub const ALL: [Role; 5] = [
        Role::Planner,
        Role::Designer,
        Role::Coder,
        Role::Reviewer,
        Role::Security,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Planner => "planner",
            Role::Designer => "designer",
            Role::Coder => "coder",
            Role::Reviewer => "reviewer",
            Role::Security => "security",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "planner" => Some(Role::Planner),
            "designer" => Some(Role::Designer),
            "coder" => Some(Role::Coder),
            "reviewer" => Some(Role::Reviewer),
            "security" => Some(Role::Security),
            _ => None,
        }
    }

    /// Reviewer and Security are read-only by unbreakable invariant.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Role::Reviewer | Role::Security)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    Planning,
    AwaitingPlanApproval,
    Planned,
    Running,
    AwaitingApproval,
    Completed,
    Failed,
    Blocked,
    Paused,
    Cancelled,
}

impl MissionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MissionStatus::Planning => "planning",
            MissionStatus::AwaitingPlanApproval => "awaiting_plan_approval",
            MissionStatus::Planned => "planned",
            MissionStatus::Running => "running",
            MissionStatus::AwaitingApproval => "awaiting_approval",
            MissionStatus::Completed => "completed",
            MissionStatus::Failed => "failed",
            MissionStatus::Blocked => "blocked",
            MissionStatus::Paused => "paused",
            MissionStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planning" => Self::Planning,
            "awaiting_plan_approval" => Self::AwaitingPlanApproval,
            "planned" => Self::Planned,
            "running" => Self::Running,
            "awaiting_approval" => Self::AwaitingApproval,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "blocked" => Self::Blocked,
            "paused" => Self::Paused,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Planning | Self::Planned | Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Proposed,
    Ready,
    Running,
    Completed,
    Failed,
    Blocked,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Proposed => "proposed",
            TaskStatus::Ready => "ready",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "proposed" => Self::Proposed,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "blocked" => Self::Blocked,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    ChangesRequested,
    Rejected,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Approved => "approved",
            Verdict::ChangesRequested => "changes_requested",
            Verdict::Rejected => "rejected",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "approved" => Self::Approved,
            "changes_requested" => Self::ChangesRequested,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mission {
    pub id: String,
    pub objective: String,
    pub status: MissionStatus,
    #[serde(default)]
    pub plan_summary: Option<String>,
    #[serde(default)]
    pub base_head: Option<String>,
    #[serde(default)]
    pub mission_branch: Option<String>,
    #[serde(default)]
    pub integration_head: Option<String>,
    #[serde(default)]
    pub final_approval_id: Option<String>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub mission_id: String,
    /// Stable kebab-case key unique within the mission (`implement-endpoint`).
    pub task_key: String,
    pub role: Role,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    /// Task keys this task depends on.
    pub dependencies: Vec<String>,
    pub acceptance: Vec<String>,
    pub risk: RiskLevel,
    pub attempts: i64,
    #[serde(default)]
    pub status_reason: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    // lease
    #[serde(default)]
    pub lease_owner: Option<String>,
    #[serde(default)]
    pub lease_expires_at: Option<Millis>,
    #[serde(default)]
    pub lease_generation: Option<String>,
    // worktree
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub base_ref: Option<String>,
    // integration
    #[serde(default)]
    pub patch_hash: Option<String>,
    #[serde(default)]
    pub integrated_at: Option<Millis>,
    #[serde(default)]
    pub integration_commit: Option<String>,
    // delegation
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub delegation_depth: i64,
    /// Declared write scopes (relative globs). Non-empty for coder tasks.
    #[serde(default)]
    pub write_scopes: Vec<String>,
    /// Retry-with-different-model override, e.g. "anthropic/claude-sonnet-5".
    #[serde(default)]
    pub model_override: Option<String>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationStatus {
    Succeeded,
    Failed,
    Cancelled,
}

impl InvocationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            InvocationStatus::Succeeded => "succeeded",
            InvocationStatus::Failed => "failed",
            InvocationStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invocation {
    pub id: String,
    pub mission_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub role: Role,
    pub provider_id: String,
    pub model: String,
    pub status: InvocationStatus,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// USD cost in millionths of a dollar (integer math, no float drift).
    pub cost_micro_usd: i64,
    pub latency_ms: i64,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: Millis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    pub id: String,
    pub mission_id: String,
    /// The coder task whose patch this verdict covers.
    pub task_id: String,
    /// The reviewing role (reviewer or security).
    pub role: Role,
    /// The task that produced the verdict.
    pub source_task_id: String,
    pub verdict: Verdict,
    pub reasons: Vec<String>,
    /// sha256 of the exact diff the verdict covered.
    #[serde(default)]
    pub patch_hash: Option<String>,
    pub created_at: Millis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directive {
    pub id: String,
    pub mission_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub text: String,
    pub created_by: String,
    pub active: bool,
    pub created_at: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl ApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Rejected => "rejected",
            ApprovalStatus::Expired => "expired",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "expired" => Self::Expired,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub mission_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    /// e.g. "edit", "integrate", "network".
    pub action: String,
    /// The resource the action targets (path, "mission", url…).
    pub resource: String,
    /// sha256 binding this approval to an exact diff, when applicable.
    #[serde(default)]
    pub patch_hash: Option<String>,
    /// Snapshot of the policy context / diff / checks at request time (JSON).
    pub policy_snapshot: serde_json::Value,
    pub status: ApprovalStatus,
    #[serde(default)]
    pub expires_at: Option<Millis>,
    #[serde(default)]
    pub decided_at: Option<Millis>,
    #[serde(default)]
    pub decided_by: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    pub created_at: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStatus {
    Pending,
    Checking,
    Merged,
    Conflict,
    Failed,
    Superseded,
}

impl MergeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MergeStatus::Pending => "pending",
            MergeStatus::Checking => "checking",
            MergeStatus::Merged => "merged",
            MergeStatus::Conflict => "conflict",
            MergeStatus::Failed => "failed",
            MergeStatus::Superseded => "superseded",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "checking" => Self::Checking,
            "merged" => Self::Merged,
            "conflict" => Self::Conflict,
            "failed" => Self::Failed,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeEntry {
    pub id: String,
    pub mission_id: String,
    pub task_id: String,
    pub position: i64,
    pub patch_hash: String,
    pub status: MergeStatus,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub merge_commit: Option<String>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub active_mission_id: Option<String>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
    Agent,
    System,
}

impl ChatRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Agent => "agent",
            ChatRole::System => "system",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "agent" => Self::Agent,
            "system" => Self::System,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub session_id: String,
    pub role: ChatRole,
    pub content: String,
    #[serde(default)]
    pub mission_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub approval_id: Option<String>,
    pub created_at: Millis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInstance {
    pub generation: String,
    pub pid: i64,
    pub url: String,
    pub status: String, // "alive" | "dead"
    pub started_at: Millis,
    pub heartbeat_at: Millis,
}

/// Aggregated per-mission usage, derived from invocation rows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionStats {
    pub model_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micro_usd: i64,
    pub failed_calls: i64,
}

impl MissionStats {
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens + self.output_tokens
    }
    pub fn cost_usd(&self) -> f64 {
        self.cost_micro_usd as f64 / 1_000_000.0
    }
}

/// Circuit-breaker + health snapshot for one provider (persisted).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub provider_id: String,
    pub success_count: i64,
    pub failure_count: i64,
    pub consecutive_failures: i64,
    /// When the breaker is open, calls are refused until this instant.
    #[serde(default)]
    pub open_until: Option<Millis>,
    pub updated_at: Millis,
}
