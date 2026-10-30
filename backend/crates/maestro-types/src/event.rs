use crate::Millis;
use serde::{Deserialize, Serialize};

/// Append-only audit record. `seq` is the SQLite AUTOINCREMENT id and the SSE cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub seq: i64,
    pub mission_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: Millis,
}

/// Well-known event kinds. Payloads are free-form JSON but stable per kind.
pub mod kind {
    pub const MISSION_CREATED: &str = "mission.created";
    pub const MISSION_STATUS: &str = "mission.status";
    pub const PLAN_PROPOSED: &str = "plan.proposed";
    pub const PLAN_APPROVED: &str = "plan.approved";
    pub const PLAN_REJECTED: &str = "plan.rejected";
    pub const TASK_STATUS: &str = "task.status";
    pub const TASK_LEASED: &str = "task.leased";
    pub const TASK_HEARTBEAT: &str = "task.heartbeat";
    pub const TASK_RETRY: &str = "task.retry";
    pub const AGENT_TURN: &str = "agent.turn";
    pub const AGENT_STREAM: &str = "agent.stream"; // streamed text delta {taskId, text}
    pub const TOOL_CALL: &str = "tool.call";
    pub const TOOL_RESULT: &str = "tool.result";
    pub const TOOL_DENIED: &str = "tool.denied";
    pub const LOOP_DETECTED: &str = "loop.detected";
    pub const GATE_RECORDED: &str = "gate.recorded";
    pub const DIRECTIVE_ADDED: &str = "directive.added";
    pub const APPROVAL_REQUESTED: &str = "approval.requested";
    pub const APPROVAL_DECIDED: &str = "approval.decided";
    pub const PROVIDER_RETRY: &str = "provider.retry";
    pub const PROVIDER_FALLBACK: &str = "provider.fallback";
    pub const PROVIDER_BREAKER: &str = "provider.breaker";
    pub const BUDGET_EXHAUSTED: &str = "budget.exhausted";
    pub const WORKTREE_CREATED: &str = "worktree.created";
    pub const SCOPE_VIOLATION: &str = "scope.violation";
    pub const MERGE_ENQUEUED: &str = "merge.enqueued";
    pub const MERGE_STARTED: &str = "merge.started";
    pub const MERGE_COMPLETED: &str = "merge.completed";
    pub const MERGE_CONFLICT: &str = "merge.conflict";
    pub const MERGE_ROLLED_BACK: &str = "merge.rolled_back";
    pub const CHECK_STARTED: &str = "check.started";
    pub const CHECK_FINISHED: &str = "check.finished";
    pub const INTEGRATION_FINALIZED: &str = "integration.finalized";
    pub const DAEMON_RECOVERED: &str = "daemon.recovered";
    pub const SANDBOX_FALLBACK: &str = "sandbox.fallback";
}
