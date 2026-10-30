//! Plan validation: parse the Planner's raw output strictly and enforce
//! every structural rule before any task is materialized.

use crate::scopes;
use maestro_types::config::RuntimeConfig;
use maestro_types::domain::{RiskLevel, Role};
use maestro_types::plan::Plan;
use maestro_types::protocol::extract_json_object;
use maestro_types::{MaestroError, Result};
use std::collections::HashSet;

/// Hard upper bound on tasks per plan (independent of config).
pub const MAX_PLAN_TASKS: usize = 12;

fn invalid(msg: impl Into<String>) -> MaestroError {
    MaestroError::PlanInvalid(msg.into())
}

fn is_kebab(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Parse and validate a raw Planner reply into a [`Plan`].
///
/// Enforced rules:
/// - exactly one JSON object (code fences tolerated), camelCase fields;
/// - 1..=12 tasks, also capped by `config.max_tasks_per_mission`;
/// - unique kebab-case task keys;
/// - no planner-role tasks;
/// - dependencies reference EARLIER task keys only;
/// - every acceptance entry is a non-empty string;
/// - coder tasks declare non-empty, relative, `..`-free write scopes;
/// - every coder task has at least one dependent reviewer task;
/// - every high-risk coder task has a dependent security task.
pub fn validate_plan(raw_json: &str, config: &RuntimeConfig) -> Result<Plan> {
    let json =
        extract_json_object(raw_json).ok_or_else(|| invalid("no JSON object in planner output"))?;
    let plan: Plan =
        serde_json::from_str(json).map_err(|e| invalid(format!("cannot parse plan: {e}")))?;

    if plan.summary.trim().is_empty() {
        return Err(invalid("plan summary must be non-empty"));
    }
    if plan.tasks.is_empty() {
        return Err(invalid("plan must contain at least one task"));
    }
    if plan.tasks.len() > MAX_PLAN_TASKS {
        return Err(invalid(format!(
            "plan has {} tasks; maximum is {MAX_PLAN_TASKS}",
            plan.tasks.len()
        )));
    }
    if plan.tasks.len() > config.max_tasks_per_mission {
        return Err(invalid(format!(
            "plan has {} tasks; mission cap is {}",
            plan.tasks.len(),
            config.max_tasks_per_mission
        )));
    }

    let mut earlier: HashSet<&str> = HashSet::new();
    for task in &plan.tasks {
        if !is_kebab(&task.key) {
            return Err(invalid(format!(
                "task key '{}' is not kebab-case",
                task.key
            )));
        }
        if earlier.contains(task.key.as_str()) {
            return Err(invalid(format!("duplicate task key '{}'", task.key)));
        }
        if config.guild_roles && task.role == Role::Designer {
            return Err(invalid(
                "Shogun owns reconnaissance and design; do not create a separate Designer task",
            ));
        }
        if task.role == Role::Planner {
            return Err(invalid(format!(
                "task '{}' uses the planner role; plans may not contain planner tasks",
                task.key
            )));
        }
        if task.title.trim().is_empty() || task.description.trim().is_empty() {
            return Err(invalid(format!(
                "task '{}' must have a non-empty title and description",
                task.key
            )));
        }
        for dep in &task.dependencies {
            if !earlier.contains(dep.as_str()) {
                return Err(invalid(format!(
                    "task '{}' depends on '{dep}', which is not an earlier task",
                    task.key
                )));
            }
        }
        for item in &task.acceptance {
            if item.trim().is_empty() {
                return Err(invalid(format!(
                    "task '{}' has an empty acceptance entry",
                    task.key
                )));
            }
        }
        if task.role == Role::Coder {
            if task.write_scopes.is_empty() {
                return Err(invalid(format!(
                    "coder task '{}' must declare non-empty writeScopes",
                    task.key
                )));
            }
            for scope in &task.write_scopes {
                scopes::validate_scope(scope)
                    .map_err(|e| invalid(format!("coder task '{}': {e}", task.key)))?;
            }
        }
        earlier.insert(task.key.as_str());
    }

    // Coder ↔ reviewer pairing and high-risk security coverage.
    for task in &plan.tasks {
        if task.role != Role::Coder {
            continue;
        }
        let has_reviewer = plan
            .tasks
            .iter()
            .any(|t| t.role == Role::Reviewer && t.dependencies.iter().any(|d| d == &task.key));
        if !has_reviewer {
            return Err(invalid(format!(
                "coder task '{}' has no dependent reviewer task",
                task.key
            )));
        }
        if task.risk == RiskLevel::High || config.guild_roles {
            let has_security = plan
                .tasks
                .iter()
                .any(|t| t.role == Role::Security && t.dependencies.iter().any(|d| d == &task.key));
            if !has_security {
                return Err(invalid(format!(
                    "high-risk coder task '{}' has no dependent security task",
                    task.key
                )));
            }
        }
    }

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RuntimeConfig {
        RuntimeConfig::default()
    }

    fn valid_plan_json() -> String {
        r#"{
            "summary": "Implement and review the feature.",
            "tasks": [
                {"key": "write-code", "role": "coder", "title": "Write",
                 "description": "Implement it", "acceptance": ["it compiles"],
                 "risk": "low", "writeScopes": ["src/**"]},
                {"key": "review-code", "role": "reviewer", "title": "Review",
                 "description": "Review it", "dependencies": ["write-code"],
                 "acceptance": ["verdict recorded"]}
            ]
        }"#
        .to_string()
    }

    #[test]
    fn guild_requires_shadow_even_for_low_risk_and_has_no_designer() {
        let mut cfg = config();
        cfg.guild_roles = true;
        assert!(validate_plan(&valid_plan_json(), &cfg).is_err());
        let mut plan: serde_json::Value = serde_json::from_str(&valid_plan_json()).unwrap();
        plan["tasks"].as_array_mut().unwrap().push(serde_json::json!({"key":"shadow-review","role":"security","title":"Shadow review","description":"Review correctness and security","dependencies":["write-code","review-code"],"acceptance":["Fresh evidence-bound verdict"]}));
        assert!(validate_plan(&plan.to_string(), &cfg).is_ok());
        plan["tasks"][0]["role"] = serde_json::json!("designer");
        assert!(validate_plan(&plan.to_string(), &cfg).is_err());
    }

    #[test]
    fn valid_plan_passes() {
        let plan = validate_plan(&valid_plan_json(), &config()).unwrap();
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[0].key, "write-code");
    }

    #[test]
    fn fenced_output_is_tolerated() {
        let fenced = format!("```json\n{}\n```", valid_plan_json());
        assert!(validate_plan(&fenced, &config()).is_ok());
    }

    #[test]
    fn prose_is_rejected() {
        assert!(matches!(
            validate_plan("I think we should refactor.", &config()),
            Err(MaestroError::PlanInvalid(_))
        ));
    }

    #[test]
    fn empty_and_oversized_task_lists_rejected() {
        let empty = r#"{"summary": "s", "tasks": []}"#;
        assert!(validate_plan(empty, &config()).is_err());

        let mut tasks = String::new();
        for i in 0..13 {
            if i > 0 {
                tasks.push(',');
            }
            tasks.push_str(&format!(
                r#"{{"key": "task-{i}", "role": "designer", "title": "t", "description": "d"}}"#
            ));
        }
        let too_many = format!(r#"{{"summary": "s", "tasks": [{tasks}]}}"#);
        match validate_plan(&too_many, &config()) {
            Err(MaestroError::PlanInvalid(msg)) => assert!(msg.contains("maximum")),
            other => panic!("expected PlanInvalid, got {other:?}"),
        }
    }

    #[test]
    fn bad_keys_rejected() {
        for bad_key in [
            "Not-Kebab",
            "has_underscore",
            "-leading",
            "trailing-",
            "sp ace",
            "",
        ] {
            let raw = format!(
                r#"{{"summary": "s", "tasks": [
                    {{"key": "{bad_key}", "role": "designer", "title": "t", "description": "d"}}
                ]}}"#
            );
            assert!(
                validate_plan(&raw, &config()).is_err(),
                "key '{bad_key}' accepted"
            );
        }
    }

    #[test]
    fn duplicate_keys_rejected() {
        let raw = r#"{"summary": "s", "tasks": [
            {"key": "same-key", "role": "designer", "title": "t", "description": "d"},
            {"key": "same-key", "role": "designer", "title": "t", "description": "d"}
        ]}"#;
        match validate_plan(raw, &config()) {
            Err(MaestroError::PlanInvalid(msg)) => assert!(msg.contains("duplicate")),
            other => panic!("expected PlanInvalid, got {other:?}"),
        }
    }

    #[test]
    fn planner_role_tasks_rejected() {
        let raw = r#"{"summary": "s", "tasks": [
            {"key": "plan-more", "role": "planner", "title": "t", "description": "d"}
        ]}"#;
        assert!(validate_plan(raw, &config()).is_err());
    }

    #[test]
    fn forward_and_unknown_dependencies_rejected() {
        let forward = r#"{"summary": "s", "tasks": [
            {"key": "first-task", "role": "designer", "title": "t", "description": "d",
             "dependencies": ["second-task"]},
            {"key": "second-task", "role": "designer", "title": "t", "description": "d"}
        ]}"#;
        assert!(validate_plan(forward, &config()).is_err());

        let unknown = r#"{"summary": "s", "tasks": [
            {"key": "only-task", "role": "designer", "title": "t", "description": "d",
             "dependencies": ["ghost-task"]}
        ]}"#;
        assert!(validate_plan(unknown, &config()).is_err());
    }

    #[test]
    fn empty_acceptance_entry_rejected() {
        let raw = r#"{"summary": "s", "tasks": [
            {"key": "a-task", "role": "designer", "title": "t", "description": "d",
             "acceptance": ["ok", "  "]}
        ]}"#;
        match validate_plan(raw, &config()) {
            Err(MaestroError::PlanInvalid(msg)) => assert!(msg.contains("acceptance")),
            other => panic!("expected PlanInvalid, got {other:?}"),
        }
    }

    #[test]
    fn coder_scope_rules_enforced() {
        // Missing scopes.
        let missing = r#"{"summary": "s", "tasks": [
            {"key": "code-it", "role": "coder", "title": "t", "description": "d"},
            {"key": "review-it", "role": "reviewer", "title": "t", "description": "d",
             "dependencies": ["code-it"]}
        ]}"#;
        assert!(validate_plan(missing, &config()).is_err());

        // Absolute scope.
        let absolute = r#"{"summary": "s", "tasks": [
            {"key": "code-it", "role": "coder", "title": "t", "description": "d",
             "writeScopes": ["/etc/**"]},
            {"key": "review-it", "role": "reviewer", "title": "t", "description": "d",
             "dependencies": ["code-it"]}
        ]}"#;
        assert!(validate_plan(absolute, &config()).is_err());

        // Parent traversal.
        let parent = r#"{"summary": "s", "tasks": [
            {"key": "code-it", "role": "coder", "title": "t", "description": "d",
             "writeScopes": ["src/../../x/**"]},
            {"key": "review-it", "role": "reviewer", "title": "t", "description": "d",
             "dependencies": ["code-it"]}
        ]}"#;
        assert!(validate_plan(parent, &config()).is_err());
    }

    #[test]
    fn coder_without_dependent_reviewer_rejected() {
        let raw = r#"{"summary": "s", "tasks": [
            {"key": "code-it", "role": "coder", "title": "t", "description": "d",
             "writeScopes": ["src/**"]}
        ]}"#;
        match validate_plan(raw, &config()) {
            Err(MaestroError::PlanInvalid(msg)) => assert!(msg.contains("reviewer")),
            other => panic!("expected PlanInvalid, got {other:?}"),
        }
    }

    #[test]
    fn high_risk_coder_requires_security_task() {
        let missing_security = r#"{"summary": "s", "tasks": [
            {"key": "risky-code", "role": "coder", "title": "t", "description": "d",
             "risk": "high", "writeScopes": ["src/**"]},
            {"key": "review-it", "role": "reviewer", "title": "t", "description": "d",
             "dependencies": ["risky-code"]}
        ]}"#;
        match validate_plan(missing_security, &config()) {
            Err(MaestroError::PlanInvalid(msg)) => assert!(msg.contains("security")),
            other => panic!("expected PlanInvalid, got {other:?}"),
        }

        let with_security = r#"{"summary": "s", "tasks": [
            {"key": "risky-code", "role": "coder", "title": "t", "description": "d",
             "risk": "high", "writeScopes": ["src/**"]},
            {"key": "review-it", "role": "reviewer", "title": "t", "description": "d",
             "dependencies": ["risky-code"]},
            {"key": "audit-it", "role": "security", "title": "t", "description": "d",
             "dependencies": ["risky-code"]}
        ]}"#;
        assert!(validate_plan(with_security, &config()).is_ok());
    }
}
