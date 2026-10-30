use crate::domain::{RiskLevel, Role};
use serde::{Deserialize, Serialize};

/// The Planner's JSON output, validated by `maestro-engine::plan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub summary: String,
    pub tasks: Vec<PlanTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTask {
    /// Kebab-case, unique within the plan.
    pub key: String,
    pub role: Role,
    pub title: String,
    pub description: String,
    /// Keys of earlier tasks this depends on.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Acceptance criteria the agent must satisfy.
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default = "default_risk")]
    pub risk: RiskLevel,
    /// Relative globs a coder may write. Required non-empty for coder tasks.
    #[serde(default)]
    pub write_scopes: Vec<String>,
}

fn default_risk() -> RiskLevel {
    RiskLevel::Low
}
