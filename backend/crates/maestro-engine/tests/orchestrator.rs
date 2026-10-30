//! End-to-end orchestrator tests: full mock-provider missions against real
//! throwaway git repositories.

use maestro_engine::Orchestrator;
use maestro_providers::ProviderRegistry;
use maestro_store::Store;
use maestro_types::config::{CheckConfig, EnforcementMode, IntegrationStrategy, MaestroConfig};
use maestro_types::domain::RiskLevel;
use maestro_types::domain::{MissionStatus, Role, TaskStatus, Verdict};
use maestro_types::event::kind;
use maestro_types::plan::PlanTask;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
    // Sandbox may lack bwrap in CI; prefer host fallback for checks.
    config.safety.sandbox = EnforcementMode::Preferred;
    config.safety.worktrees = EnforcementMode::Required;
    config.integration.final_approval = false;
    config.runtime.lease_ttl_ms = 10_000;
    config
}

fn orchestrator(root: &Path, config: MaestroConfig) -> (Orchestrator, Store) {
    let store = Store::open(&root.join(".maestro")).unwrap();
    let registry = Arc::new(ProviderRegistry::new(&config));
    let orch = Orchestrator::new(
        store.clone(),
        config,
        registry,
        root.to_path_buf(),
        "test-gen".to_string(),
    );
    (orch, store)
}

fn set_role_model(config: &mut MaestroConfig, role: &str, model: &str) {
    config.roles.get_mut(role).unwrap().model.model = model.to_string();
}

fn plan_task(key: &str, role: Role, deps: &[&str], scopes: &[&str]) -> PlanTask {
    PlanTask {
        key: key.to_string(),
        role,
        title: format!("task {key}"),
        description: format!("description of {key}"),
        dependencies: deps.iter().map(|s| s.to_string()).collect(),
        acceptance: vec!["done".to_string()],
        risk: RiskLevel::Low,
        write_scopes: scopes.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dependent_coder_forks_from_a_base_that_includes_merged_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer");
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("chained coders").await.unwrap();

    let tasks = store
        .insert_tasks(
            &mission.id,
            &[
                plan_task("coder-a", Role::Coder, &[], &["src/**"]),
                plan_task("review-a", Role::Reviewer, &["coder-a"], &[]),
                plan_task("coder-b", Role::Coder, &["review-a"], &["src/**"]),
                plan_task("review-b", Role::Reviewer, &["coder-b"], &[]),
            ],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();

    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Completed);

    // coder-a's patch merged BEFORE coder-b was ever leased: eager
    // integration ran between the waves.
    let events = store.events_after(0, Some(&mission.id)).unwrap();
    let a_id = &tasks[0].id;
    let b_id = &tasks[2].id;
    let a_merged = events
        .iter()
        .position(|e| e.kind == "merge.completed" && e.task_id.as_deref() == Some(a_id))
        .expect("coder-a merge event");
    let b_leased = events
        .iter()
        .position(|e| e.kind == "task.leased" && e.task_id.as_deref() == Some(b_id))
        .expect("coder-b lease event");
    assert!(
        a_merged < b_leased,
        "coder-a must integrate (idx {a_merged}) before coder-b starts (idx {b_leased})"
    );

    // And coder-b's recorded fork point is exactly coder-a's merge commit.
    let after = store.list_tasks(&mission.id).unwrap();
    let a = after.iter().find(|t| t.task_key == "coder-a").unwrap();
    let b = after.iter().find(|t| t.task_key == "coder-b").unwrap();
    assert_eq!(
        b.base_ref, a.integration_commit,
        "coder-b must fork from coder-a's integration commit"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delegated_coder_gets_an_auto_reviewer_and_the_mission_completes() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-delegator");
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("delegate work").await.unwrap();

    store
        .insert_tasks(
            &mission.id,
            &[plan_task("kick-off", Role::Coder, &[], &["src/**"])],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();

    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    let tasks = store.list_tasks(&mission.id).unwrap();
    let last_reason = store
        .events_after(0, Some(&mission.id))
        .unwrap()
        .iter()
        .rev()
        .find(|e| e.kind == "mission.status")
        .map(|e| e.payload.to_string())
        .unwrap_or_default();
    assert_eq!(
        status,
        MissionStatus::Completed,
        "last mission.status: {last_reason}; tasks: {:?}",
        tasks
            .iter()
            .map(|t| format!(
                "{}={} ({})",
                t.task_key,
                t.status.as_str(),
                t.status_reason.as_deref().unwrap_or("-")
            ))
            .collect::<Vec<_>>()
    );

    let delegated = tasks
        .iter()
        .find(|t| t.task_key == "delegated-fix")
        .expect("delegated coder task exists");
    assert_eq!(delegated.role, Role::Coder);
    let auto_review = tasks
        .iter()
        .find(|t| t.task_key == "review-delegated-fix")
        .expect("auto reviewer for the delegated coder exists");
    assert_eq!(auto_review.role, Role::Reviewer);
    assert_eq!(auto_review.dependencies, vec!["delegated-fix".to_string()]);
    // The delegated patch merged under an approved gate.
    let gates = store.gates_for_task(&delegated.id).unwrap();
    assert!(gates.iter().any(|g| g.verdict == Verdict::Approved));
    assert!(delegated.integrated_at.is_some());
    assert!(dir.path().join("src/generated.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn dirty_tree_blocks_auto_finalize_and_keeps_the_mission_branch() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config(); // final_approval = false → auto-finalize
    set_role_model(&mut config, "coder", "coder-writer");
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();

    // Untracked dirt at the root: auto-finalize must refuse and BLOCK the
    // mission (recoverable), not propagate an error that fails it — and the
    // mission branch holding the merged work must survive.
    std::fs::write(dir.path().join("scratch.txt"), "wip\n").unwrap();
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Blocked);
    let m = store.get_mission(&mission.id).unwrap();
    assert_eq!(m.status, MissionStatus::Blocked);
    let branch = m.mission_branch.clone().expect("mission branch recorded");
    sh(
        dir.path(),
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("refs/heads/{branch}"),
        ],
    );

    // The user removes the dirt and re-drives: the mission finalizes.
    std::fs::remove_file(dir.path().join("scratch.txt")).unwrap();
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Completed);
    assert!(dir.path().join("src/generated.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn full_mission_completes_and_finalizes() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer");
    config.integration.final_approval = true;

    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();

    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::AwaitingApproval);

    // All four canonical tasks completed; the coder patch was integrated.
    let tasks = store.list_tasks(&mission.id).unwrap();
    assert_eq!(tasks.len(), 4);
    assert!(tasks.iter().all(|t| t.status == TaskStatus::Completed));
    let coder = tasks.iter().find(|t| t.role == Role::Coder).unwrap();
    assert!(coder.patch_hash.is_some());
    assert!(coder.integrated_at.is_some());

    // Usage accounting recorded real tokens.
    let stats = store.mission_stats(&mission.id).unwrap();
    assert!(stats.model_calls > 0);
    assert!(stats.total_tokens() > 0);

    // Tool events use the TUI contract keys ("arguments"/"output").
    let events = store.events_after(0, Some(&mission.id)).unwrap();
    let tool_call = events.iter().find(|e| e.kind == kind::TOOL_CALL).unwrap();
    assert!(tool_call.payload.get("arguments").is_some());
    assert!(tool_call.payload.get("args").is_none());
    let tool_result = events.iter().find(|e| e.kind == kind::TOOL_RESULT).unwrap();
    assert!(tool_result.payload.get("output").is_some());
    assert!(tool_result.payload.get("result").is_none());

    // The merged coder's worktree and task branch were cleaned up already.
    let coder_branch = coder.branch_name.clone().unwrap();
    let coder_ws = coder.workspace_path.clone().unwrap();
    assert!(
        !Path::new(&coder_ws).exists(),
        "merged coder worktree must be removed"
    );
    let branch_check = Command::new("git")
        .current_dir(dir.path())
        .args([
            "rev-parse",
            "-q",
            "--verify",
            &format!("refs/heads/{coder_branch}"),
        ])
        .output()
        .unwrap();
    assert!(
        !branch_check.status.success(),
        "merged task branch must be deleted"
    );

    // Approve and finalize: the file lands on the user branch.
    let pending = store.pending_approvals(Some(&mission.id)).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].action, "integrate");
    store
        .decide_approval(&pending[0].id, true, "tester", None)
        .unwrap();
    let outcome = orch.finalize_mission(&mission.id).await.unwrap();
    assert!(!outcome.commit.is_empty());
    assert!(dir.path().join("src/generated.txt").exists());
    let mission = store.get_mission(&mission.id).unwrap();
    assert_eq!(mission.status, MissionStatus::Completed);

    // Finalization prunes the mission's worktrees and integration checkout.
    let worktrees_dir = dir.path().join(".maestro/worktrees");
    let leftovers = std::fs::read_dir(&worktrees_dir)
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(
        leftovers, 0,
        ".maestro/worktrees must be empty after finalize"
    );
    let mission8: String = mission.id.chars().take(8).collect();
    assert!(
        !dir.path()
            .join(".maestro/integration")
            .join(mission8)
            .exists(),
        "integration worktree must be removed after finalize"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plan_approval_gates_execution() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer");
    config.runtime.plan_approval = true;

    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();

    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::AwaitingPlanApproval);
    assert!(store
        .list_tasks(&mission.id)
        .unwrap()
        .iter()
        .all(|t| t.status != TaskStatus::Running));

    orch.decide_plan(&mission.id, true, None).await.unwrap();
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Completed);
    assert!(dir.path().join("src/generated.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_plan_cancels_the_mission() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    config.runtime.plan_approval = true;
    let (orch, _store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();
    orch.plan_mission(&mission.id, &cancel).await.unwrap();
    let mission = orch.decide_plan(&mission.id, false, None).await.unwrap();
    assert_eq!(mission.status, MissionStatus::Cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn edited_plan_replaces_tasks() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    config.runtime.plan_approval = true;
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();
    let mut plan = orch.plan_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(plan.tasks.len(), 4);

    // Drop the security audit and keep a 3-task plan.
    plan.tasks.retain(|t| t.key != "security-audit");
    plan.tasks
        .iter_mut()
        .find(|t| t.key == "implement-feature")
        .unwrap()
        .risk = RiskLevel::Low;
    orch.decide_plan(&mission.id, true, Some(plan))
        .await
        .unwrap();
    let tasks = store.list_tasks(&mission.id).unwrap();
    assert_eq!(tasks.len(), 3);
    assert!(!tasks.iter().any(|t| t.task_key == "security-audit"));
}

#[tokio::test(flavor = "multi_thread")]
async fn failing_integration_check_rolls_back_and_blocks() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer");
    config.checks = vec![CheckConfig {
        name: "always-fails".into(),
        command: vec!["false".into()],
    }];

    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Blocked);

    let events = store.events_after(0, Some(&mission.id)).unwrap();
    assert!(events.iter().any(|e| e.kind == kind::MERGE_ROLLED_BACK));
    // The integration head never advanced.
    let mission = store.get_mission(&mission.id).unwrap();
    assert!(mission.integration_head.is_none());
    // The user branch never saw the file.
    assert!(!dir.path().join("src/generated.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn scope_violation_fails_the_coder_task() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer");
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("scoped work").await.unwrap();

    // Hand-crafted plan: the coder may only write docs/**, but the mock
    // coder-writer writes src/generated.txt.
    store
        .insert_tasks(
            &mission.id,
            &[PlanTask {
                key: "implement-docs".into(),
                role: Role::Coder,
                title: "Write docs".into(),
                description: "Write documentation only.".into(),
                dependencies: vec![],
                acceptance: vec!["docs written".into()],
                risk: RiskLevel::Low,
                write_scopes: vec!["docs/**".into()],
            }],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();

    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Blocked);
    let tasks = store.list_tasks(&mission.id).unwrap();
    assert_eq!(tasks[0].status, TaskStatus::Failed);
    // The write was denied at the tool layer (declared-scope enforcement),
    // so the out-of-scope file never touched the worktree.
    let events = store.events_after(0, Some(&mission.id)).unwrap();
    assert!(events.iter().any(|e| e.kind == kind::TOOL_DENIED));
    if let Some(path) = &tasks[0].workspace_path {
        assert!(!Path::new(path).join("src/generated.txt").exists());
    }
    assert!(tasks[0].patch_hash.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn token_budget_exhaustion_blocks_the_mission() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    config.runtime.max_total_tokens = 1;
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Blocked);
    let events = store.events_after(0, Some(&mission.id)).unwrap();
    assert!(events.iter().any(|e| e.kind == kind::BUDGET_EXHAUSTED));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_interrupts_at_a_safe_boundary() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    // The designer runs first in the canonical plan and takes 250ms.
    set_role_model(&mut config, "designer", "slow-designer");
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();

    let canceller = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            cancel.cancel();
        })
    };
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    canceller.await.unwrap();
    assert_eq!(status, MissionStatus::Paused);

    // The interrupted designer task went back to ready — no work was lost.
    let tasks = store.list_tasks(&mission.id).unwrap();
    let designer = tasks.iter().find(|t| t.role == Role::Designer).unwrap();
    assert!(matches!(
        designer.status,
        TaskStatus::Ready | TaskStatus::Proposed
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn reviewer_covering_two_coders_records_a_gate_per_coder() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    // Direct strategy: completion is decided purely by gates, no git merges.
    config.integration.strategy = IntegrationStrategy::Direct;
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch
        .create_mission("two coders, one reviewer")
        .await
        .unwrap();

    let tasks = store
        .insert_tasks(
            &mission.id,
            &[
                plan_task("coder-a", Role::Coder, &[], &["src/a/**"]),
                plan_task("coder-b", Role::Coder, &[], &["src/b/**"]),
                plan_task("review-all", Role::Reviewer, &["coder-a", "coder-b"], &[]),
            ],
        )
        .unwrap();
    // The coders "already ran": completed with distinct patches, sharing a
    // stub workspace the reviewer can ride.
    let stub_ws = dir.path().join("stub-ws");
    std::fs::create_dir_all(&stub_ws).unwrap();
    for (task, hash) in [(&tasks[0], "hash-a"), (&tasks[1], "hash-b")] {
        store
            .set_task_worktree(
                &task.id,
                &stub_ws.to_string_lossy(),
                &format!("branch-{}", task.task_key),
                "base-ref",
            )
            .unwrap();
        store.set_task_patch_hash(&task.id, hash).unwrap();
        store
            .set_task_status(&task.id, TaskStatus::Completed, None, Some("done"))
            .unwrap();
    }
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();

    // Only the reviewer actually runs; it must gate BOTH coders.
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Completed);
    for (task, hash) in [(&tasks[0], "hash-a"), (&tasks[1], "hash-b")] {
        let gates = store.gates_for_task(&task.id).unwrap();
        assert_eq!(gates.len(), 1, "coder {} must have a gate", task.task_key);
        assert_eq!(gates[0].role, Role::Reviewer);
        assert_eq!(gates[0].verdict, Verdict::Approved);
        assert_eq!(
            gates[0].patch_hash.as_deref(),
            Some(hash),
            "gate must bind that coder's own patch hash"
        );
        assert_eq!(gates[0].task_id, task.id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn gate_binds_the_hash_reviewed_not_the_hash_at_final_time() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    // Direct strategy: completion is decided purely by gates, no git merges.
    config.integration.strategy = IntegrationStrategy::Direct;
    // Slow reviewer (250ms per model call) so a coder retry can land while
    // the review is in flight.
    set_role_model(&mut config, "reviewer", "slow-reviewer");
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("review the coder").await.unwrap();

    let tasks = store
        .insert_tasks(
            &mission.id,
            &[
                plan_task("coder-a", Role::Coder, &[], &["src/**"]),
                plan_task("review-a", Role::Reviewer, &["coder-a"], &[]),
            ],
        )
        .unwrap();
    let coder = &tasks[0];
    // The coder "already ran": completed with patch P1 in a stub workspace
    // the reviewer can ride.
    let stub_ws = dir.path().join("stub-ws");
    std::fs::create_dir_all(&stub_ws).unwrap();
    store
        .set_task_worktree(
            &coder.id,
            &stub_ws.to_string_lossy(),
            "branch-a",
            "base-ref",
        )
        .unwrap();
    store
        .set_task_patch_hash(&coder.id, "hash-reviewed")
        .unwrap();
    store
        .set_task_status(&coder.id, TaskStatus::Completed, None, Some("done"))
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();

    // Run the mission in the background and wait for the reviewer to start.
    let runner = {
        let orch = orch.clone();
        let mission_id = mission.id.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { orch.run_mission(&mission_id, &cancel).await })
    };
    let reviewer_id = tasks[1].id.clone();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = store.get_task(&reviewer_id).unwrap().status;
        if status == TaskStatus::Running {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reviewer never started (status {status:?})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // Mid-review (the mock reviewer's model call takes 250ms), the coder is
    // retried and re-completes with a NEW patch P2 the reviewer never saw.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    store
        .set_task_patch_hash(&coder.id, "hash-never-reviewed")
        .unwrap();

    let status = runner.await.unwrap().unwrap();

    // The gate must bind the hash the reviewer actually reviewed (P1) — a
    // gate stamped with the coder's CURRENT hash would silently approve
    // unreviewed code.
    let gates = store.gates_for_task(&coder.id).unwrap();
    assert_eq!(gates.len(), 1);
    assert_eq!(
        gates[0].patch_hash.as_deref(),
        Some("hash-reviewed"),
        "the gate must bind the patch hash captured when the review started"
    );
    // With a stale gate the mission must NOT complete: gates_satisfied sees
    // hash-reviewed != hash-never-reviewed and blocks for a re-review.
    assert_ne!(
        status,
        MissionStatus::Completed,
        "unreviewed code must never satisfy the gate freshness check"
    );
    assert_eq!(status, MissionStatus::Blocked);
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_park_preserves_the_retry_budget() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer");
    // Strict profile: edits Ask → the coder parks on an approval.
    config.safety.profile = "strict".into();
    let (orch, store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("guarded write").await.unwrap();
    store
        .insert_tasks(
            &mission.id,
            &[plan_task("write-it", Role::Coder, &[], &["src/**"])],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();

    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Blocked);

    let task = &store.list_tasks(&mission.id).unwrap()[0];
    assert_eq!(task.status, TaskStatus::Blocked);
    assert!(task
        .status_reason
        .as_deref()
        .unwrap()
        .starts_with("Waiting for approval"));
    // Regression: leasing used to burn an attempt even though no model
    // failure happened — approve-then-one-bad-turn became permanent failure.
    assert_eq!(
        task.attempts, 0,
        "approval park must not consume the retry budget"
    );
    let pending = store.pending_approvals(Some(&mission.id)).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].action, "edit");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_mission_does_not_resume_a_paused_mission() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let (orch, store) = orchestrator(dir.path(), test_config());
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("paused work").await.unwrap();
    store
        .insert_tasks(&mission.id, &[plan_task("d", Role::Designer, &[], &[])])
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Paused, Some("user paused"))
        .unwrap();

    // A retry/approval ensure on a deliberately paused mission must not
    // silently resume it.
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Paused);
    assert_eq!(
        store.get_mission(&mission.id).unwrap().status,
        MissionStatus::Paused
    );
    assert!(store
        .list_tasks(&mission.id)
        .unwrap()
        .iter()
        .all(|t| t.status == TaskStatus::Proposed));
}

#[tokio::test(flavor = "multi_thread")]
async fn architect_answers_questions() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let (orch, _store) = orchestrator(dir.path(), test_config());
    let cancel = CancellationToken::new();
    let answer = orch
        .answer_question("how is authentication structured?", &cancel)
        .await
        .unwrap();
    assert!(!answer.is_empty());
    assert!(!answer.trim_start().starts_with('{'));
}

#[tokio::test(flavor = "multi_thread")]
async fn mission_diff_reflects_the_integration_branch() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer");
    let (orch, _store) = orchestrator(dir.path(), config);
    let cancel = CancellationToken::new();
    let mission = orch.create_mission("implement the feature").await.unwrap();
    let status = orch.run_mission(&mission.id, &cancel).await.unwrap();
    assert_eq!(status, MissionStatus::Completed);
    let (diff, hash) = orch.mission_diff(&mission.id).await.unwrap();
    assert!(diff.contains("generated.txt"));
    assert!(hash.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_receives_actual_check_and_patch_receipts_with_the_final_report() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "coder", "coder-writer-checked");
    set_role_model(&mut config, "reviewer", "reviewer-receipts");
    config.checks.push(CheckConfig {
        name: "generated file exists".into(),
        command: vec![
            "/usr/bin/test".into(),
            "-f".into(),
            "src/generated.txt".into(),
        ],
    });
    let (orch, store) = orchestrator(dir.path(), config);
    let mission = orch
        .create_mission("verify evidence handoff")
        .await
        .unwrap();
    store
        .insert_tasks(
            &mission.id,
            &[
                plan_task("code", Role::Coder, &[], &["src/**"]),
                plan_task("review", Role::Reviewer, &["code"], &[]),
            ],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();
    assert_eq!(
        orch.run_mission(&mission.id, &CancellationToken::new())
            .await
            .unwrap(),
        MissionStatus::Completed
    );
    let artifacts = store.artifacts_for_mission(&mission.id).unwrap();
    let check = artifacts.iter().find(|a| a.kind == "check_result").unwrap();
    assert_eq!(check.content["result"]["exitCode"], 0);
    assert_eq!(check.content["command"][0], "/usr/bin/test");
    let patch = artifacts.iter().find(|a| a.kind == "worktree").unwrap();
    assert_eq!(
        patch.content["changedPaths"],
        serde_json::json!(["src/generated.txt"])
    );
    assert_eq!(patch.content["scopeVerified"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn standalone_inspection_review_uses_repository_without_a_coder_dependency() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let (orch, store) = orchestrator(dir.path(), test_config());
    std::fs::write(dir.path().join("uncommitted-note.txt"), "keep my work").unwrap();
    let mission = orch
        .create_mission("inspect and explain this repository")
        .await
        .unwrap();
    store
        .insert_tasks(
            &mission.id,
            &[
                plan_task("inspect", Role::Reviewer, &[], &[]),
                plan_task("explain", Role::Reviewer, &["inspect"], &[]),
            ],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();
    assert_eq!(
        orch.run_mission(&mission.id, &CancellationToken::new())
            .await
            .unwrap(),
        MissionStatus::Completed
    );
    for task in store.list_tasks(&mission.id).unwrap() {
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.workspace_path.as_deref(), dir.path().to_str());
    }
    assert!(store
        .artifacts_for_mission(&mission.id)
        .unwrap()
        .iter()
        .all(|a| a.kind != "worktree"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("uncommitted-note.txt")).unwrap(),
        "keep my work"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn scope_clarification_recovers_false_blocks_but_preserves_real_block_reasons() {
    for (model, expected) in [
        ("designer-scope-block", MissionStatus::Completed),
        ("designer-genuine-block", MissionStatus::Blocked),
    ] {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let mut config = test_config();
        set_role_model(&mut config, "designer", model);
        let (orch, store) = orchestrator(dir.path(), config);
        let mission = orch
            .create_mission("Design, implement, and launch the app")
            .await
            .unwrap();
        store
            .insert_tasks(
                &mission.id,
                &[plan_task("design", Role::Designer, &[], &[])],
            )
            .unwrap();
        store
            .set_mission_status(&mission.id, MissionStatus::Planned, None)
            .unwrap();
        store.refresh_ready(&mission.id).unwrap();
        assert_eq!(
            orch.run_mission(&mission.id, &CancellationToken::new())
                .await
                .unwrap(),
            expected
        );
        let events = store.events_page(0, Some(&mission.id), 1000).unwrap();
        assert_eq!(
            events.iter().filter(|e| e.kind == kind::AGENT_TURN).count(),
            2,
            "clarification must be bounded"
        );
        if expected == MissionStatus::Blocked {
            let task = &store.list_tasks(&mission.id).unwrap()[0];
            assert_eq!(
                task.status_reason.as_deref(),
                Some("Required design input is unavailable")
            );
            let event = events
                .iter()
                .find(|e| e.kind == kind::TASK_STATUS && e.payload["status"] == "blocked")
                .unwrap();
            assert_eq!(
                event.payload["reason"],
                "Required design input is unavailable"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_turn_is_repaired_without_repeating_inspection() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let mut config = test_config();
    set_role_model(&mut config, "designer", "designer-protocol-recover");
    let (orch, store) = orchestrator(dir.path(), config);
    let mission = orch.create_mission("inspect repository").await.unwrap();
    store
        .insert_tasks(
            &mission.id,
            &[plan_task("design", Role::Designer, &[], &[])],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();
    assert_eq!(
        orch.run_mission(&mission.id, &CancellationToken::new())
            .await
            .unwrap(),
        MissionStatus::Completed
    );
    let events = store.events_page(0, Some(&mission.id), 1000).unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == kind::TOOL_CALL).count(),
        1
    );
    assert_eq!(
        events.iter().filter(|e| e.kind == kind::AGENT_TURN).count(),
        3
    );
    assert!(!events.iter().any(|e| e.kind == kind::TASK_RETRY));
}

#[tokio::test]
async fn private_snapshot_includes_uncommitted_app_without_changing_user_index() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("src/lib.rs"), "staged version\n").unwrap();
    sh(dir.path(), &["add", "src/lib.rs"]);
    std::fs::write(dir.path().join("src/lib.rs"), "working version\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
    std::fs::write(dir.path().join("src/components/App.tsx"), "current app\n").unwrap();
    std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
    std::fs::write(dir.path().join(".maestro/private.json"), "runtime data").unwrap();
    std::fs::write(dir.path().join(".env.production"), "secret-value").unwrap();
    std::fs::write(dir.path().join(".env.example"), "EXAMPLE=placeholder").unwrap();
    let before_head = maestro_engine::git::head(dir.path()).await.unwrap();
    let before_index = maestro_engine::git::git(dir.path(), &["diff", "--cached", "--binary"])
        .await
        .unwrap();
    let (base, snapshot) = maestro_engine::git::snapshot_working_tree(dir.path())
        .await
        .unwrap();
    assert_eq!(base, before_head);
    assert_ne!(snapshot, base);
    assert_eq!(
        maestro_engine::git::head(dir.path()).await.unwrap(),
        before_head
    );
    assert_eq!(
        maestro_engine::git::git(dir.path(), &["diff", "--cached", "--binary"])
            .await
            .unwrap(),
        before_index
    );
    assert_eq!(
        maestro_engine::git::git(dir.path(), &["show", &format!("{snapshot}:src/lib.rs")])
            .await
            .unwrap(),
        "working version"
    );
    assert_eq!(
        maestro_engine::git::git(
            dir.path(),
            &["show", &format!("{snapshot}:src/components/App.tsx")]
        )
        .await
        .unwrap(),
        "current app"
    );
    let names = maestro_engine::git::git(dir.path(), &["ls-tree", "-r", "--name-only", &snapshot])
        .await
        .unwrap();
    assert!(names.contains(".env.example"));
    assert!(!names.contains(".env.production"));
    assert!(!names.contains(".maestro"));
}

#[tokio::test(flavor = "multi_thread")]
async fn langgraph_bridge_shares_dirty_snapshot_enforces_scopes_and_replays_tool_receipts() {
    use maestro_types::config::{AgentRuntime, ProviderKind};
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
    std::fs::write(
        dir.path().join("src/components/app.txt"),
        "uncommitted app\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
    let script = dir.path().join(".maestro/ipc-fixture.py");
    std::fs::write(&script, r#"
import json,sys
init=json.loads(sys.stdin.readline())
def rpc(op,**fields):
 print(json.dumps(dict(op=op,**fields)),flush=True)
 return json.loads(sys.stdin.readline())['result']
role=init['role']
if role in ['designer','coder']:
 result=rpc('tool',name='read_file',callId='read-app',arguments={'path':'src/components/app.txt'})
 assert result['content']=='uncommitted app\n',result
if role=='coder':
 denied=rpc('tool',name='write_file',callId='escape',arguments={'path':'../escape.txt','content':'unsafe'})
 assert 'error' in denied,denied
 args={'path':'src/components/app.txt','old_text':'uncommitted app','new_text':'reviewed redesign'}
 first=rpc('tool',name='replace_text',callId='edit-once',arguments=args)
 assert 'error' not in first,first
 second=rpc('tool',name='replace_text',callId='edit-once',arguments=args)
 assert first==second,(first,second)
report=dict(status='completed',summary='Task complete',evidence=['actual tool results'],risks=[],verdict='approved' if role=='reviewer' else None,reasons=[])
rpc('result',report=report)
"#).unwrap();
    let mut config = test_config();
    config.integration.final_approval = true;
    config.runtime.agent_runtime = AgentRuntime::Langgraph;
    config.runtime.langgraph_command = vec![
        "python3".into(),
        "-u".into(),
        script.to_string_lossy().into(),
    ];
    config.providers.get_mut("mock").unwrap().kind = ProviderKind::OpenaiResponses;
    config.providers.get_mut("mock").unwrap().base_url = Some("http://127.0.0.1:1".into());
    let (orch, store) = orchestrator(dir.path(), config);
    let mission = orch
        .create_mission("redesign the existing uncommitted app")
        .await
        .unwrap();
    store
        .insert_tasks(
            &mission.id,
            &[
                plan_task("design", Role::Designer, &[], &[]),
                plan_task("code", Role::Coder, &["design"], &["src/components/**"]),
                plan_task("review", Role::Reviewer, &["code"], &[]),
            ],
        )
        .unwrap();
    store
        .set_mission_status(&mission.id, MissionStatus::Planned, None)
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();
    assert_eq!(
        orch.run_mission(&mission.id, &CancellationToken::new())
            .await
            .unwrap(),
        MissionStatus::AwaitingApproval
    );
    let tasks = store.list_tasks(&mission.id).unwrap();
    assert!(tasks.iter().all(|t| t.status == TaskStatus::Completed));
    let events = store.events_page(0, Some(&mission.id), 1000).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == kind::TOOL_CALL && e.payload["callId"] == "edit-once")
            .count(),
        1
    );
    assert!(!dir.path().join(".maestro/worktrees/escape.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/components/app.txt")).unwrap(),
        "uncommitted app\n"
    );
    let integrated = store
        .get_mission(&mission.id)
        .unwrap()
        .integration_head
        .unwrap();
    assert_eq!(
        maestro_engine::git::git(
            dir.path(),
            &["show", &format!("{integrated}:src/components/app.txt")]
        )
        .await
        .unwrap(),
        "reviewed redesign"
    );
}
