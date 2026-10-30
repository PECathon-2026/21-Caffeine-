//! Durable code versions. Agent execution checkpoints remain separate.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub id: String,
    pub parent_id: Option<String>,
    pub mission_id: Option<String>,
    pub task_id: Option<String>,
    /// before, draft, backup, applied, recovery, restored, manual
    pub kind: String,
    pub label: String,
    pub snapshot: String,
    pub base_snapshot: String,
    /// Logical baseline after restore, excluding subsequently preserved manual edits.
    #[serde(default)]
    pub logical_snapshot: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub versions: Vec<Version>,
    pub current_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreview {
    pub target: Version,
    pub unified: String,
    pub conflicts: Vec<String>,
    /// Binds apply to the exact project tree, index, HEAD and history viewed.
    pub token: String,
    pub affected_missions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreRequest {
    /// None previews; an exact preview token applies the operation.
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub version: Version,
    pub recovery_id: String,
    pub retired_missions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRestore {
    pub result: RestoreResult,
    pub before_snapshot: String,
    pub after_snapshot: String,
    pub undo_mission: Option<String>,
}
