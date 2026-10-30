//! Integration tests for the maestro-store public API.

use maestro_store::{ApprovalRequest, Store};
use maestro_types::domain::{
    ApprovalStatus, ChatRole, Gate, Invocation, InvocationStatus, MergeStatus, MissionStatus,
    RiskLevel, Role, TaskStatus, Verdict,
};
use maestro_types::plan::{Plan, PlanTask};
use maestro_types::{new_id, now_ms};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
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

#[test]
fn migrations_are_idempotent_across_opens() {
    let dir = tempfile::tempdir().unwrap();
    let store1 = Store::open(dir.path()).unwrap();
    let mission = store1.create_mission("build the thing").unwrap();
    // Second open on the same state dir re-runs the migration check.
    let store2 = Store::open(dir.path()).unwrap();
    let missions = store2.list_missions().unwrap();
    assert_eq!(missions.len(), 1);
    assert_eq!(missions[0].id, mission.id);
    assert!(store2.integrity_check().unwrap());
}

#[test]
fn mission_lifecycle_and_setters() {
    let (_d, store) = open();
    let m = store.create_mission("objective").unwrap();
    assert_eq!(m.status, MissionStatus::Planning);
    assert!(store.get_mission("missing").is_err());

    store
        .set_mission_status(&m.id, MissionStatus::Running, Some("plan approved"))
        .unwrap();
    let got = store.get_mission(&m.id).unwrap();
    assert_eq!(got.status, MissionStatus::Running);
    assert!(got.updated_at >= m.updated_at);

    store
        .set_mission_git_base(&m.id, "abc123", "maestro/mission/abc")
        .unwrap();
    store.set_mission_integration_head(&m.id, "def456").unwrap();
    store.set_mission_final_approval(&m.id, "appr-1").unwrap();
    let got = store.get_mission(&m.id).unwrap();
    assert_eq!(got.base_head.as_deref(), Some("abc123"));
    assert_eq!(got.mission_branch.as_deref(), Some("maestro/mission/abc"));
    assert_eq!(got.integration_head.as_deref(), Some("def456"));
    assert_eq!(got.final_approval_id.as_deref(), Some("appr-1"));
}

#[test]
fn plan_json_roundtrip() {
    let (_d, store) = open();
    let m = store.create_mission("obj").unwrap();
    assert!(store.get_mission_plan(&m.id).unwrap().is_none());
    let plan = Plan {
        summary: "two steps".to_string(),
        tasks: vec![
            plan_task("design", Role::Designer, &[], &[]),
            plan_task("build", Role::Coder, &["design"], &["src/**"]),
        ],
    };
    store.set_mission_plan(&m.id, &plan).unwrap();
    let stored = store.get_mission_plan(&m.id).unwrap().unwrap();
    assert_eq!(stored.summary, "two steps");
    assert_eq!(stored.tasks.len(), 2);
    assert_eq!(
        store.get_mission(&m.id).unwrap().plan_summary.as_deref(),
        Some("two steps")
    );
    assert!(store.get_mission_plan("missing").is_err());
}

#[test]
fn dependency_promotion_and_blocking() {
    let (_d, store) = open();
    let m = store.create_mission("deps").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("a", Role::Coder, &[], &["src/a/**"]),
                plan_task("b", Role::Reviewer, &["a"], &[]),
                plan_task("c", Role::Coder, &["b"], &["src/c/**"]),
                plan_task("d", Role::Reviewer, &["c"], &[]),
            ],
        )
        .unwrap();
    assert_eq!(tasks.len(), 4);
    assert!(tasks.iter().all(|t| t.status == TaskStatus::Proposed));

    store.refresh_ready(&m.id).unwrap();
    let by_key = |k: &str| {
        store
            .list_tasks(&m.id)
            .unwrap()
            .into_iter()
            .find(|t| t.task_key == k)
            .unwrap()
    };
    assert_eq!(by_key("a").status, TaskStatus::Ready);
    assert_eq!(by_key("b").status, TaskStatus::Proposed);

    store
        .set_task_status(&by_key("a").id, TaskStatus::Completed, None, Some("ok"))
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(by_key("b").status, TaskStatus::Ready);
    assert_eq!(by_key("c").status, TaskStatus::Proposed);

    // Failure blocks the whole downstream chain in one refresh (fixpoint).
    store
        .set_task_status(&by_key("b").id, TaskStatus::Failed, Some("boom"), None)
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(by_key("c").status, TaskStatus::Blocked);
    assert_eq!(by_key("d").status, TaskStatus::Blocked);
}

#[test]
fn dep_blocked_tasks_unblock_when_the_dependency_recovers() {
    let (_d, store) = open();
    let m = store.create_mission("unblock").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("a", Role::Coder, &[], &["src/**"]),
                plan_task("b", Role::Reviewer, &["a"], &[]),
                plan_task("c", Role::Coder, &["b"], &["docs/**"]),
            ],
        )
        .unwrap();
    let (a, b, c) = (&tasks[0].id, &tasks[1].id, &tasks[2].id);
    store.refresh_ready(&m.id).unwrap();

    // a parks (e.g. waiting for an approval): b and c block.
    store
        .set_task_status(a, TaskStatus::Blocked, Some("Waiting for approval x"), None)
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    let blocked_b = store.get_task(b).unwrap();
    assert_eq!(blocked_b.status, TaskStatus::Blocked);
    assert_eq!(
        blocked_b.status_reason.as_deref(),
        Some("dependency failed or blocked")
    );
    assert_eq!(store.get_task(c).unwrap().status, TaskStatus::Blocked);

    // The approval-parked task itself is never auto-unblocked.
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(store.get_task(a).unwrap().status, TaskStatus::Blocked);

    // a resumes and completes: b becomes ready, c returns to proposed.
    store
        .set_task_status(a, TaskStatus::Completed, None, Some("done"))
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    let b_task = store.get_task(b).unwrap();
    assert_eq!(b_task.status, TaskStatus::Ready);
    assert!(b_task.status_reason.is_none());
    let c_task = store.get_task(c).unwrap();
    assert_eq!(c_task.status, TaskStatus::Proposed);
    assert!(c_task.status_reason.is_none());
}

#[test]
fn lease_cas_race_exactly_one_winner() {
    let (_d, store) = open();
    let m = store.create_mission("race").unwrap();
    let tasks = store
        .insert_tasks(&m.id, &[plan_task("solo", Role::Coder, &[], &["src/**"])])
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    let task_id = tasks[0].id.clone();

    let mut wins = 0usize;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                let task_id = task_id.clone();
                scope.spawn(move || {
                    store
                        .acquire_lease(&task_id, &format!("worker-{i}"), "gen-1", 30_000)
                        .unwrap()
                })
            })
            .collect();
        for h in handles {
            if h.join().unwrap() {
                wins += 1;
            }
        }
    });
    assert_eq!(wins, 1, "exactly one thread must win the lease CAS");
    let task = store.get_task(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert!(task.lease_owner.is_some());
}

#[test]
fn lease_renew_recover_and_generation() {
    let (_d, store) = open();
    let m = store.create_mission("leases").unwrap();
    let tasks = store
        .insert_tasks(&m.id, &[plan_task("t", Role::Coder, &[], &["src/**"])])
        .unwrap();
    let id = &tasks[0].id;
    store.refresh_ready(&m.id).unwrap();

    assert!(store.acquire_lease(id, "w1", "gen-1", 50).unwrap());
    assert!(
        !store.acquire_lease(id, "w2", "gen-1", 50).unwrap(),
        "not ready anymore"
    );
    assert!(store.renew_lease(id, "w1", 50).unwrap());
    assert!(
        !store.renew_lease(id, "intruder", 50).unwrap(),
        "owner mismatch"
    );

    // Expired lease is recovered back to ready with lease cleared.
    let recovered = store.recover_expired_leases(now_ms() + 60_000).unwrap();
    assert_eq!(recovered, 1);
    let t = store.get_task(id).unwrap();
    assert_eq!(t.status, TaskStatus::Ready);
    assert!(t.lease_owner.is_none() && t.lease_expires_at.is_none());

    // Foreign-generation running task is reset on daemon recovery.
    assert!(store.acquire_lease(id, "w1", "gen-1", 60_000).unwrap());
    assert_eq!(store.recover_generation("gen-1").unwrap(), 0);
    assert_eq!(store.recover_generation("gen-2").unwrap(), 1);
    assert_eq!(store.get_task(id).unwrap().status, TaskStatus::Ready);
}

#[test]
fn set_task_status_clears_lease_and_scope_locks() {
    let (_d, store) = open();
    let m = store.create_mission("clear").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("t1", Role::Coder, &[], &["src/**"]),
                plan_task("t2", Role::Coder, &[], &["src/**"]),
            ],
        )
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    let (t1, t2) = (&tasks[0].id, &tasks[1].id);

    assert!(store.acquire_lease(t1, "w1", "g", 60_000).unwrap());
    assert!(store
        .acquire_scope_locks(&m.id, t1, &["src/**".to_string()])
        .unwrap());
    assert!(!store
        .acquire_scope_locks(&m.id, t2, &["src/**".to_string()])
        .unwrap());

    let done = store
        .set_task_status(t1, TaskStatus::Completed, None, Some("shipped"))
        .unwrap();
    assert_eq!(done.status, TaskStatus::Completed);
    assert!(done.lease_owner.is_none());
    assert!(done.lease_generation.is_none());
    assert_eq!(done.summary.as_deref(), Some("shipped"));
    // t1's locks are gone, so t2 can take the overlapping scope now.
    assert!(store
        .acquire_scope_locks(&m.id, t2, &["src/**".to_string()])
        .unwrap());
}

#[test]
fn retry_cascade_and_refusals() {
    let (_d, store) = open();
    let m = store.create_mission("retry").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("a", Role::Coder, &[], &["src/**"]),
                plan_task("b", Role::Reviewer, &["a"], &[]),
                plan_task("c", Role::Coder, &["b"], &["docs/**"]),
            ],
        )
        .unwrap();
    let a = &tasks[0];
    let b = &tasks[1];
    let c = &tasks[2];
    store.refresh_ready(&m.id).unwrap();

    // a fails; b and c become blocked.
    store
        .set_task_status(&a.id, TaskStatus::Failed, Some("check failed"), None)
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(store.get_task(&b.id).unwrap().status, TaskStatus::Blocked);
    assert_eq!(store.get_task(&c.id).unwrap().status, TaskStatus::Blocked);

    // A stale queue entry for a must be purged by the retry.
    let entry = store.enqueue_merge(&m.id, &a.id, "hash-a").unwrap();
    assert_eq!(entry.status, MergeStatus::Pending);

    store.increment_attempts(&a.id).unwrap();
    let retried = store
        .retry_task(&a.id, Some("mock/fixer".to_string()))
        .unwrap();
    assert_eq!(retried.status, TaskStatus::Ready);
    assert_eq!(retried.attempts, 0);
    assert_eq!(retried.model_override.as_deref(), Some("mock/fixer"));
    // Cascade: blocked descendants return to proposed (transitively).
    assert_eq!(store.get_task(&b.id).unwrap().status, TaskStatus::Proposed);
    assert_eq!(store.get_task(&c.id).unwrap().status, TaskStatus::Proposed);
    // Non-merged merge entries for the task are deleted.
    assert!(store.next_pending_merge(&m.id).unwrap().is_none());

    // Refuse: running task.
    assert!(store.acquire_lease(&a.id, "w", "g", 60_000).unwrap());
    assert!(store.retry_task(&a.id, None).is_err());
    // Refuse: dependencies not completed.
    assert!(store.retry_task(&b.id, None).is_err());
    // Refuse: integrated task.
    let done = store
        .set_task_status(&a.id, TaskStatus::Completed, None, None)
        .unwrap();
    assert_eq!(done.status, TaskStatus::Completed);
    store.mark_task_integrated(&a.id, "commit-1").unwrap();
    assert!(store.retry_task(&a.id, None).is_err());
}

#[test]
fn retry_reopens_completed_reviewers_and_clears_patch_identity() {
    let (_d, store) = open();
    let m = store.create_mission("retry-reviewers").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("coder", Role::Coder, &[], &["src/**"]),
                plan_task("review", Role::Reviewer, &["coder"], &[]),
                plan_task("audit", Role::Security, &["coder"], &[]),
                plan_task("other-coder", Role::Coder, &["coder"], &["docs/**"]),
            ],
        )
        .unwrap();
    let (coder, review, audit, other) = (&tasks[0].id, &tasks[1].id, &tasks[2].id, &tasks[3].id);
    store.refresh_ready(&m.id).unwrap();

    // Whole chain completed; the coder carries a patch hash.
    for id in [coder, review, audit, other] {
        store
            .set_task_status(id, TaskStatus::Completed, None, Some("ok"))
            .unwrap();
    }
    store.set_task_patch_hash(coder, "stale-hash").unwrap();

    let retried = store.retry_task(coder, None).unwrap();
    assert_eq!(retried.status, TaskStatus::Ready);
    assert!(retried.patch_hash.is_none(), "patch hash must be cleared");
    assert!(retried.integrated_at.is_none());
    assert!(retried.integration_commit.is_none());
    // Completed reviewer/security descendants reopen (their gates are
    // hash-stale by construction); completed coder descendants do not.
    assert_eq!(store.get_task(review).unwrap().status, TaskStatus::Proposed);
    assert_eq!(store.get_task(audit).unwrap().status, TaskStatus::Proposed);
    assert_eq!(store.get_task(other).unwrap().status, TaskStatus::Completed);
}

#[test]
fn stale_owner_terminal_write_does_not_clobber_new_lease_owner() {
    let (_d, store) = open();
    let m = store.create_mission("cas").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("t", Role::Coder, &[], &["src/**"]),
                plan_task("bystander", Role::Coder, &[], &["src/**"]),
            ],
        )
        .unwrap();
    let id = &tasks[0].id;
    store.refresh_ready(&m.id).unwrap();

    // w1 leases the task, then stalls past its TTL; the lease is recovered
    // and w2 acquires it (the two-writer daemon + `run --local` topology).
    assert!(store.acquire_lease(id, "w1", "gen-1", 50).unwrap());
    assert_eq!(store.recover_expired_leases(now_ms() + 60_000).unwrap(), 1);
    assert!(store.acquire_lease(id, "w2", "gen-2", 60_000).unwrap());
    assert!(store
        .acquire_scope_locks(&m.id, id, &["src/**".to_string()])
        .unwrap());

    // The stale worker resumes and tries to record its attempt outcome:
    // every owner-checked write must refuse with LeaseLost and leave w2's
    // running row, lease, and scope locks untouched.
    let err = store
        .set_task_status_owned(id, "w1", TaskStatus::Completed, None, Some("stale"))
        .unwrap_err();
    assert!(matches!(err, maestro_types::MaestroError::LeaseLost { .. }));
    let err = store
        .set_task_status_owned(id, "w1", TaskStatus::Ready, Some("stale retry"), None)
        .unwrap_err();
    assert!(matches!(err, maestro_types::MaestroError::LeaseLost { .. }));
    let err = store.increment_attempts_owned(id, "w1").unwrap_err();
    assert!(matches!(err, maestro_types::MaestroError::LeaseLost { .. }));

    let t = store.get_task(id).unwrap();
    assert_eq!(
        t.status,
        TaskStatus::Running,
        "w2's running row must survive"
    );
    assert_eq!(
        t.lease_owner.as_deref(),
        Some("w2"),
        "w2's lease must survive"
    );
    assert_eq!(
        t.attempts, 0,
        "the stale worker must not burn w2's retry budget"
    );
    assert!(
        t.summary.is_none(),
        "the stale summary must not be recorded"
    );
    assert!(
        !store
            .acquire_scope_locks(&m.id, &tasks[1].id, &["src/**".to_string()])
            .unwrap(),
        "w2's scope locks must survive the stale writes"
    );

    // The real owner's writes still work and release everything.
    assert_eq!(store.increment_attempts_owned(id, "w2").unwrap(), 1);
    let done = store
        .set_task_status_owned(id, "w2", TaskStatus::Completed, None, Some("shipped"))
        .unwrap();
    assert_eq!(done.status, TaskStatus::Completed);
    assert!(done.lease_owner.is_none());
    assert!(store
        .acquire_scope_locks(&m.id, &tasks[1].id, &["src/**".to_string()])
        .unwrap());
    // A second owned write after release is LeaseLost, not a double-apply.
    assert!(store
        .set_task_status_owned(id, "w2", TaskStatus::Failed, Some("late"), None)
        .is_err());
    assert_eq!(store.get_task(id).unwrap().status, TaskStatus::Completed);
}

#[test]
fn retry_demotes_ready_descendants_back_to_proposed() {
    let (_d, store) = open();
    let m = store.create_mission("retry-ready").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("a", Role::Coder, &[], &["src/**"]),
                plan_task("r", Role::Reviewer, &["a"], &[]),
                plan_task("b", Role::Coder, &["r"], &["docs/**"]),
            ],
        )
        .unwrap();
    let (a, r, b) = (&tasks[0].id, &tasks[1].id, &tasks[2].id);
    store.refresh_ready(&m.id).unwrap();

    // a and r complete; b becomes ready but is never scheduled
    // (e.g. max_concurrent saturated).
    store
        .set_task_status(a, TaskStatus::Completed, None, Some("ok"))
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    store
        .set_task_status(r, TaskStatus::Completed, None, Some("ok"))
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(store.get_task(b).unwrap().status, TaskStatus::Ready);

    // Retrying a must not leave b runnable: its readiness was derived from
    // a chain that is no longer completed.
    store.retry_task(a, None).unwrap();
    assert_eq!(
        store.get_task(b).unwrap().status,
        TaskStatus::Proposed,
        "ready descendants must be demoted so the scheduler cannot run them"
    );
    // refresh_ready re-derives readiness and must NOT promote b back while
    // its dependency chain is unfinished (a is ready, r is proposed).
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(store.get_task(a).unwrap().status, TaskStatus::Ready);
    assert_eq!(store.get_task(r).unwrap().status, TaskStatus::Proposed);
    assert_eq!(store.get_task(b).unwrap().status, TaskStatus::Proposed);
}

#[test]
fn retry_demotes_a_directly_ready_reviewer() {
    let (_d, store) = open();
    let m = store.create_mission("retry-ready-reviewer").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("a", Role::Coder, &[], &["src/**"]),
                plan_task("review", Role::Reviewer, &["a"], &[]),
            ],
        )
        .unwrap();
    let (a, review) = (&tasks[0].id, &tasks[1].id);
    store.refresh_ready(&m.id).unwrap();
    store
        .set_task_status(a, TaskStatus::Completed, None, Some("ok"))
        .unwrap();
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(store.get_task(review).unwrap().status, TaskStatus::Ready);

    store.retry_task(a, None).unwrap();
    assert_eq!(store.get_task(review).unwrap().status, TaskStatus::Proposed);
    store.refresh_ready(&m.id).unwrap();
    assert_eq!(
        store.get_task(review).unwrap().status,
        TaskStatus::Proposed,
        "the reviewer must wait for the retried coder to complete again"
    );
}

#[test]
fn guarded_mission_transitions_never_overwrite_a_cancel() {
    let (_d, store) = open();
    let m = store.create_mission("guarded").unwrap();

    // The normal path: planning → running.
    assert!(store.begin_mission_running(&m.id).unwrap());
    assert_eq!(
        store.get_mission(&m.id).unwrap().status,
        MissionStatus::Running
    );
    // Idempotent while running.
    assert!(store.begin_mission_running(&m.id).unwrap());

    // A cancel that lands first must win: the guarded transition writes
    // nothing and reports the lost race.
    store
        .set_mission_status(&m.id, MissionStatus::Cancelled, Some("user cancel"))
        .unwrap();
    assert!(!store.begin_mission_running(&m.id).unwrap());
    assert_eq!(
        store.get_mission(&m.id).unwrap().status,
        MissionStatus::Cancelled
    );

    // The interrupted-fallback CAS must also refuse: cancelled is terminal
    // and must never be downgraded to a resumable pause.
    assert!(!store
        .set_mission_status_if(
            &m.id,
            MissionStatus::Running,
            MissionStatus::Paused,
            Some("interrupted"),
        )
        .unwrap());
    assert_eq!(
        store.get_mission(&m.id).unwrap().status,
        MissionStatus::Cancelled
    );

    // And it applies when the precondition truly holds.
    store
        .set_mission_status(&m.id, MissionStatus::Running, None)
        .unwrap();
    assert!(store
        .set_mission_status_if(
            &m.id,
            MissionStatus::Running,
            MissionStatus::Paused,
            Some("interrupted"),
        )
        .unwrap());
    assert_eq!(
        store.get_mission(&m.id).unwrap().status,
        MissionStatus::Paused
    );

    // Missing missions are NotFound, not a silent false.
    assert!(store.begin_mission_running("missing").is_err());
    assert!(store
        .set_mission_status_if(
            "missing",
            MissionStatus::Running,
            MissionStatus::Paused,
            None
        )
        .is_err());
}

#[test]
fn pending_approvals_lists_all_missions_and_filters_one() {
    let (_d, store) = open();
    let m1 = store.create_mission("one").unwrap();
    let m2 = store.create_mission("two").unwrap();
    for (m, res) in [(&m1, "a.txt"), (&m2, "b.txt")] {
        store
            .create_approval(ApprovalRequest {
                mission_id: &m.id,
                task_id: None,
                action: "edit",
                resource: res,
                patch_hash: None,
                policy_snapshot: json!({}),
                expires_at: None,
            })
            .unwrap();
    }
    // Both the all-missions and the per-mission statements (split for index
    // use) must agree.
    assert_eq!(store.pending_approvals(None).unwrap().len(), 2);
    let only_m1 = store.pending_approvals(Some(&m1.id)).unwrap();
    assert_eq!(only_m1.len(), 1);
    assert_eq!(only_m1[0].mission_id, m1.id);
    assert_eq!(only_m1[0].resource, "a.txt");
}

#[test]
fn increment_attempts_counts_up() {
    let (_d, store) = open();
    let m = store.create_mission("attempts").unwrap();
    let tasks = store
        .insert_tasks(&m.id, &[plan_task("t", Role::Designer, &[], &[])])
        .unwrap();
    assert_eq!(store.increment_attempts(&tasks[0].id).unwrap(), 1);
    assert_eq!(store.increment_attempts(&tasks[0].id).unwrap(), 2);
    assert!(store.increment_attempts("missing").is_err());
}

#[test]
fn delegation_limits() {
    let (_d, store) = open();
    let m = store.create_mission("delegate").unwrap();
    let roots = store
        .insert_tasks(&m.id, &[plan_task("root", Role::Designer, &[], &[])])
        .unwrap();
    let root = &roots[0];

    // Coder delegation requires scopes.
    let bad = plan_task("child-coder", Role::Coder, &[], &[]);
    assert!(store
        .add_delegated_task(&m.id, &root.id, &bad, 3, 50)
        .is_err());

    let child = store
        .add_delegated_task(
            &m.id,
            &root.id,
            &plan_task("child", Role::Reviewer, &[], &[]),
            2,
            50,
        )
        .unwrap();
    assert_eq!(child.delegation_depth, 1);
    assert_eq!(child.parent_task_id.as_deref(), Some(root.id.as_str()));
    let grandchild = store
        .add_delegated_task(
            &m.id,
            &child.id,
            &plan_task("gc", Role::Reviewer, &[], &[]),
            2,
            50,
        )
        .unwrap();
    assert_eq!(grandchild.delegation_depth, 2);
    // Depth 3 > max 2.
    assert!(store
        .add_delegated_task(
            &m.id,
            &grandchild.id,
            &plan_task("ggc", Role::Reviewer, &[], &[]),
            2,
            50
        )
        .is_err());
    // Mission task cap (already 3 tasks).
    assert!(store
        .add_delegated_task(
            &m.id,
            &root.id,
            &plan_task("cap", Role::Reviewer, &[], &[]),
            5,
            3
        )
        .is_err());
    // Duplicate key conflicts.
    assert!(store
        .add_delegated_task(
            &m.id,
            &root.id,
            &plan_task("child", Role::Reviewer, &[], &[]),
            5,
            50
        )
        .is_err());
}

#[test]
fn scope_lock_overlap_semantics() {
    let (_d, store) = open();
    let m = store.create_mission("scopes").unwrap();

    assert!(store
        .acquire_scope_locks(&m.id, "t1", &["src/api/**".to_string()])
        .unwrap());
    // Overlapping prefix from another task fails.
    assert!(!store
        .acquire_scope_locks(&m.id, "t2", &["src/api/handlers/*.rs".to_string()])
        .unwrap());
    // Empty static prefix conservatively overlaps everything.
    assert!(!store
        .acquire_scope_locks(&m.id, "t2", &["**/mod.rs".to_string()])
        .unwrap());
    // Disjoint prefix succeeds.
    assert!(store
        .acquire_scope_locks(&m.id, "t2", &["docs/**".to_string()])
        .unwrap());
    // Same task can re-acquire / extend its own locks.
    assert!(store
        .acquire_scope_locks(
            &m.id,
            "t1",
            &["src/api/**".to_string(), "src/api/v2/**".to_string()]
        )
        .unwrap());
    // Different mission does not conflict.
    let m2 = store.create_mission("other").unwrap();
    assert!(store
        .acquire_scope_locks(&m2.id, "t9", &["src/api/**".to_string()])
        .unwrap());
    // Release opens the door.
    assert!(store.release_scope_locks("t1").unwrap() >= 2);
    assert!(store
        .acquire_scope_locks(&m.id, "t2", &["src/api/handlers/*.rs".to_string()])
        .unwrap());
}

#[test]
fn approval_dedupe_decide_and_expiry() {
    let (_d, store) = open();
    let m = store.create_mission("approvals").unwrap();
    let req = |expires_at| ApprovalRequest {
        mission_id: &m.id,
        task_id: None,
        action: "integrate",
        resource: "mission",
        patch_hash: Some("h1"),
        policy_snapshot: json!({"rules": []}),
        expires_at,
    };

    let a1 = store.create_approval(req(None)).unwrap();
    let a2 = store.create_approval(req(None)).unwrap();
    assert_eq!(
        a1.id, a2.id,
        "pending approvals dedupe by (mission, action, resource)"
    );
    assert_eq!(store.pending_approvals(Some(&m.id)).unwrap().len(), 1);

    let decided = store
        .decide_approval(&a1.id, true, "anay", Some("lgtm"))
        .unwrap();
    assert_eq!(decided.status, ApprovalStatus::Approved);
    assert_eq!(decided.decided_by.as_deref(), Some("anay"));
    // Deciding twice conflicts.
    assert!(store.decide_approval(&a1.id, false, "anay", None).is_err());
    // After the decision a fresh pending approval can be created.
    let a3 = store.create_approval(req(None)).unwrap();
    assert_ne!(a3.id, a1.id);

    // Lazy expiry.
    let m2 = store.create_mission("exp").unwrap();
    let expired = store
        .create_approval(ApprovalRequest {
            mission_id: &m2.id,
            task_id: None,
            action: "edit",
            resource: "src/main.rs",
            patch_hash: None,
            policy_snapshot: json!({}),
            expires_at: Some(now_ms() - 1),
        })
        .unwrap();
    assert_eq!(
        store.get_approval(&expired.id).unwrap().status,
        ApprovalStatus::Expired
    );
    assert!(store
        .decide_approval(&expired.id, true, "anay", None)
        .is_err());
    assert!(store.pending_approvals(Some(&m2.id)).unwrap().is_empty());
}

#[test]
fn merge_queue_supersede_and_reconcile() {
    let (_d, store) = open();
    let m = store.create_mission("merges").unwrap();
    let tasks = store
        .insert_tasks(
            &m.id,
            &[
                plan_task("t1", Role::Coder, &[], &["a/**"]),
                plan_task("t2", Role::Coder, &[], &["b/**"]),
            ],
        )
        .unwrap();

    let e1 = store.enqueue_merge(&m.id, &tasks[0].id, "hash-1").unwrap();
    assert_eq!(e1.position, 1);
    // Requeueing the same task supersedes the earlier entry.
    let e2 = store.enqueue_merge(&m.id, &tasks[0].id, "hash-2").unwrap();
    assert_eq!(e2.position, 2);
    let e3 = store.enqueue_merge(&m.id, &tasks[1].id, "hash-3").unwrap();
    assert_eq!(e3.position, 3);

    let next = store.next_pending_merge(&m.id).unwrap().unwrap();
    assert_eq!(next.id, e2.id, "superseded entry must not be served");

    // Crash while checking → reconciliation returns it to pending.
    store
        .set_merge_status(&e2.id, MergeStatus::Checking, None, None)
        .unwrap();
    assert_eq!(store.next_pending_merge(&m.id).unwrap().unwrap().id, e3.id);
    assert_eq!(store.reconcile_merge_queue(&m.id).unwrap(), 1);
    assert_eq!(store.next_pending_merge(&m.id).unwrap().unwrap().id, e2.id);

    store
        .set_merge_status(&e2.id, MergeStatus::Merged, Some("clean"), Some("commit-9"))
        .unwrap();
    store
        .mark_task_integrated(&tasks[0].id, "commit-9")
        .unwrap();
    let t = store.get_task(&tasks[0].id).unwrap();
    assert!(t.integrated_at.is_some());
    assert_eq!(t.integration_commit.as_deref(), Some("commit-9"));
    // Merged entries survive a task retry cleanup only if merged — covered in retry test.
}

#[test]
fn gates_upsert_and_directives() {
    let (_d, store) = open();
    let m = store.create_mission("gates").unwrap();
    let gate = Gate {
        id: new_id(),
        mission_id: m.id.clone(),
        task_id: "coder-task".to_string(),
        role: Role::Reviewer,
        source_task_id: "review-task".to_string(),
        verdict: Verdict::ChangesRequested,
        reasons: vec!["needs tests".to_string()],
        patch_hash: Some("h1".to_string()),
        created_at: now_ms(),
    };
    store.record_gate(&gate).unwrap();
    let updated = Gate {
        verdict: Verdict::Approved,
        reasons: vec![],
        patch_hash: Some("h2".to_string()),
        ..gate.clone()
    };
    store.record_gate(&updated).unwrap();
    let gates = store.gates_for_task("coder-task").unwrap();
    assert_eq!(gates.len(), 1, "upsert on (task_id, role)");
    assert_eq!(gates[0].verdict, Verdict::Approved);
    assert_eq!(gates[0].patch_hash.as_deref(), Some("h2"));

    // Directives: global + task-scoped visibility.
    let g = store
        .add_directive(&m.id, None, "prefer small diffs", "anay")
        .unwrap();
    let t = store
        .add_directive(&m.id, Some("task-1"), "focus on tests", "anay")
        .unwrap();
    let all = store.active_directives(&m.id, Some("task-1")).unwrap();
    assert_eq!(all.len(), 2);
    let global_only = store.active_directives(&m.id, Some("task-2")).unwrap();
    assert_eq!(global_only.len(), 1);
    assert_eq!(global_only[0].id, g.id);
    let none_scoped = store.active_directives(&m.id, None).unwrap();
    assert_eq!(none_scoped.len(), 1);
    store.deactivate_directive(&t.id).unwrap();
    assert_eq!(
        store
            .active_directives(&m.id, Some("task-1"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn invocations_stats_and_scorecards() {
    let (_d, store) = open();
    let m = store.create_mission("stats").unwrap();
    let mk = |status: InvocationStatus, latency: i64, cost: i64| Invocation {
        id: new_id(),
        mission_id: m.id.clone(),
        task_id: None,
        role: Role::Coder,
        provider_id: "anthropic".to_string(),
        model: "claude-x".to_string(),
        status,
        input_tokens: 100,
        output_tokens: 50,
        cost_micro_usd: cost,
        latency_ms: latency,
        error: None,
        created_at: now_ms(),
    };
    store
        .record_invocation(&mk(InvocationStatus::Succeeded, 100, 500))
        .unwrap();
    store
        .record_invocation(&mk(InvocationStatus::Succeeded, 300, 700))
        .unwrap();
    store
        .record_invocation(&mk(InvocationStatus::Failed, 900, 0))
        .unwrap();

    let stats = store.mission_stats(&m.id).unwrap();
    assert_eq!(stats.model_calls, 3);
    assert_eq!(stats.input_tokens, 300);
    assert_eq!(stats.output_tokens, 150);
    assert_eq!(stats.cost_micro_usd, 1200);
    assert_eq!(stats.failed_calls, 1);

    let cards = store.scorecards().unwrap();
    assert_eq!(cards.len(), 1);
    let c = &cards[0];
    assert_eq!(c.role, Role::Coder);
    assert_eq!(c.provider_id, "anthropic");
    assert_eq!(c.calls, 3);
    assert!((c.success_rate - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(c.p50_latency_ms, 300);
    assert_eq!(c.total_tokens, 450);
    assert_eq!(c.cost_micro_usd, 1200);

    // Empty-mission stats are zeroed, not an error.
    let empty = store.mission_stats("nope").unwrap();
    assert_eq!(empty.model_calls, 0);
}

#[test]
fn artifacts_hash_and_lookup() {
    let (_d, store) = open();
    let m = store.create_mission("artifacts").unwrap();
    let content = json!({"files": ["src/lib.rs"], "notes": "worktree ready"});
    let a = store
        .add_artifact(&m.id, Some("task-1"), "worktree", content.clone())
        .unwrap();
    assert_eq!(a.content_hash.len(), 64);
    // Same content → same hash (deterministic serialization).
    let b = store
        .add_artifact(&m.id, Some("task-1"), "worktree", content)
        .unwrap();
    assert_eq!(a.content_hash, b.content_hash);
    assert_ne!(a.id, b.id);

    store
        .add_artifact(&m.id, None, "note", json!("mission-level"))
        .unwrap();
    assert_eq!(store.artifacts_for_task("task-1").unwrap().len(), 2);
    assert_eq!(store.artifacts_for_mission(&m.id).unwrap().len(), 3);
    let got = &store.artifacts_for_task("task-1").unwrap()[0];
    assert_eq!(got.content["notes"], "worktree ready");
}

#[test]
fn daemon_registry_and_liveness() {
    let (_d, store) = open();
    let d1 = store
        .register_daemon("gen-1", 100, "http://127.0.0.1:4310")
        .unwrap();
    assert_eq!(d1.status, "alive");
    assert_eq!(store.alive_daemon().unwrap().unwrap().generation, "gen-1");

    // A new generation marks all others dead.
    store
        .register_daemon("gen-2", 200, "http://127.0.0.1:4311")
        .unwrap();
    let alive = store.alive_daemon().unwrap().unwrap();
    assert_eq!(alive.generation, "gen-2");
    assert_eq!(alive.pid, 200);

    store.heartbeat_daemon("gen-2").unwrap();
    assert!(store.heartbeat_daemon("gen-9").is_err());
}

#[test]
fn chat_sessions_and_messages() {
    let (_d, store) = open();
    let s = store.create_chat_session("architect chat").unwrap();
    let m = store.create_mission("from chat").unwrap();
    store.set_active_mission(&s.id, Some(&m.id)).unwrap();
    assert_eq!(
        store
            .get_chat_session(&s.id)
            .unwrap()
            .active_mission_id
            .as_deref(),
        Some(m.id.as_str())
    );

    for i in 0..5 {
        store
            .add_chat_message(&s.id, ChatRole::User, &format!("msg {i}"), None, None, None)
            .unwrap();
    }
    store
        .add_chat_message(
            &s.id,
            ChatRole::Assistant,
            "launching",
            Some(&m.id),
            None,
            None,
        )
        .unwrap();
    let last3 = store.list_chat_messages(&s.id, 3).unwrap();
    assert_eq!(last3.len(), 3);
    assert_eq!(last3[0].content, "msg 3");
    assert_eq!(last3[2].content, "launching");
    assert_eq!(last3[2].mission_id.as_deref(), Some(m.id.as_str()));

    assert_eq!(store.list_chat_sessions().unwrap().len(), 1);
    store.set_active_mission(&s.id, None).unwrap();
    assert!(store
        .get_chat_session(&s.id)
        .unwrap()
        .active_mission_id
        .is_none());
    // Message into an unknown session fails (FK).
    assert!(store
        .add_chat_message("missing", ChatRole::User, "hi", None, None, None)
        .is_err());
}

#[test]
fn provider_health_breaker_opens_and_resets() {
    let (_d, store) = open();
    let fresh = store.get_provider_health("anthropic").unwrap();
    assert_eq!(fresh.provider_id, "anthropic");
    assert_eq!(fresh.success_count + fresh.failure_count, 0);

    let h1 = store
        .record_provider_result("anthropic", false, 3, 10_000)
        .unwrap();
    assert_eq!(h1.consecutive_failures, 1);
    assert!(h1.open_until.is_none());
    let h2 = store
        .record_provider_result("anthropic", false, 3, 10_000)
        .unwrap();
    assert!(h2.open_until.is_none());
    let h3 = store
        .record_provider_result("anthropic", false, 3, 10_000)
        .unwrap();
    assert_eq!(h3.consecutive_failures, 3);
    let open_until = h3.open_until.expect("breaker must open at threshold");
    assert!(open_until > now_ms());

    let h4 = store
        .record_provider_result("anthropic", true, 3, 10_000)
        .unwrap();
    assert_eq!(h4.consecutive_failures, 0);
    assert!(h4.open_until.is_none());
    assert_eq!(h4.success_count, 1);
    assert_eq!(h4.failure_count, 3);
    assert_eq!(store.list_provider_health().unwrap().len(), 1);
}

#[test]
fn last_event_seq_tracks_the_log_head() {
    let (_d, store) = open();
    assert_eq!(store.last_event_seq().unwrap(), 0);
    let m = store.create_mission("seq head").unwrap();
    let first = store.append_event(&m.id, None, "k", json!({})).unwrap();
    let second = store.append_event(&m.id, None, "k", json!({})).unwrap();
    assert!(second.seq > first.seq);
    assert_eq!(store.last_event_seq().unwrap(), second.seq);
}

#[tokio::test]
async fn event_bus_delivers_appended_events() {
    let (_d, store) = open();
    let m = store.create_mission("events").unwrap();
    let mut rx = store.subscribe();
    let appended = store
        .append_event(
            &m.id,
            Some("task-1"),
            "task.status",
            json!({"status": "ready"}),
        )
        .unwrap();
    assert!(appended.seq >= 1);

    let received = rx.recv().await.unwrap();
    assert_eq!(received.seq, appended.seq);
    assert_eq!(received.kind, "task.status");
    assert_eq!(received.task_id.as_deref(), Some("task-1"));
    assert_eq!(received.payload["status"], "ready");

    // Cursor replay with and without mission filter.
    let all = store.events_after(0, None).unwrap();
    assert_eq!(all.len(), 1);
    assert!(store
        .events_after(appended.seq, Some(&m.id))
        .unwrap()
        .is_empty());
    assert!(store.events_after(0, Some("other")).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_appends_broadcast_in_seq_order() {
    let (_d, store) = open();
    let m = store.create_mission("ordering").unwrap();
    let mut rx = store.subscribe();

    // Hammer append_event from 8 threads (below bus capacity so nothing
    // lags); the broadcast order must match the assigned seq order.
    const THREADS: usize = 8;
    const PER_THREAD: usize = 100;
    std::thread::scope(|scope| {
        for t in 0..THREADS {
            let store = store.clone();
            let mission_id = m.id.clone();
            scope.spawn(move || {
                for i in 0..PER_THREAD {
                    store
                        .append_event(&mission_id, None, "tick", json!({ "t": t, "i": i }))
                        .unwrap();
                }
            });
        }
    });

    let mut last = 0i64;
    for _ in 0..(THREADS * PER_THREAD) {
        let ev = rx.recv().await.unwrap();
        assert!(
            ev.seq > last,
            "out-of-order broadcast: seq {} after {}",
            ev.seq,
            last
        );
        last = ev.seq;
    }
}

#[test]
fn events_page_bounds_the_replay() {
    let (_d, store) = open();
    let m = store.create_mission("paging").unwrap();
    for i in 0..25 {
        store
            .append_event(&m.id, None, "tick", json!({ "i": i }))
            .unwrap();
    }

    let mut cursor = 0i64;
    let mut pages = 0usize;
    let mut total = 0usize;
    loop {
        let page = store.events_page(cursor, Some(&m.id), 10).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 10);
        for pair in page.windows(2) {
            assert!(pair[1].seq > pair[0].seq);
        }
        total += page.len();
        cursor = page.last().unwrap().seq;
        pages += 1;
    }
    assert_eq!(pages, 3, "25 events at page size 10 → 3 pages");
    assert_eq!(total, 25);
    // The compatibility wrapper still returns everything.
    assert_eq!(store.events_after(0, Some(&m.id)).unwrap().len(), 25);
}

#[test]
fn approval_dedupe_is_per_task() {
    let (_d, store) = open();
    let m = store.create_mission("per-task-approvals").unwrap();
    let req = |task_id| ApprovalRequest {
        mission_id: &m.id,
        task_id,
        action: "edit",
        resource: "src/shared.rs",
        patch_hash: None,
        policy_snapshot: json!({}),
        expires_at: None,
    };
    let a = store.create_approval(req(Some("task-a"))).unwrap();
    let b = store.create_approval(req(Some("task-b"))).unwrap();
    assert_ne!(a.id, b.id, "different tasks must get distinct approvals");
    assert_eq!(store.pending_approvals(Some(&m.id)).unwrap().len(), 2);
    // Same task still dedupes.
    let a2 = store.create_approval(req(Some("task-a"))).unwrap();
    assert_eq!(a.id, a2.id);
    // NULL task_id dedupes against NULL only.
    let none1 = store.create_approval(req(None)).unwrap();
    let none2 = store.create_approval(req(None)).unwrap();
    assert_eq!(none1.id, none2.id);
    assert_ne!(none1.id, a.id);
}

#[tokio::test]
async fn lagged_receiver_recovers_via_events_after() {
    let (_d, store) = open();
    let m = store.create_mission("lag").unwrap();
    let mut rx = store.subscribe();

    // Overflow the 1024-capacity bus without receiving.
    let mut last_seq = 0;
    for i in 0..1100 {
        last_seq = store
            .append_event(&m.id, None, "tick", json!({ "i": i }))
            .unwrap()
            .seq;
    }
    let lagged = rx.recv().await;
    let skipped = match lagged {
        Err(RecvError::Lagged(n)) => n,
        other => panic!("expected Lagged, got {other:?}"),
    };
    assert!(skipped > 0);

    // The receiver resumes at some later event; the durable log fills the gap.
    let live = rx.recv().await.unwrap();
    let replay = store.events_after(0, Some(&m.id)).unwrap();
    assert_eq!(replay.len(), 1100);
    assert_eq!(replay.last().unwrap().seq, last_seq);
    // Seqs are contiguous and cover everything the bus dropped.
    for pair in replay.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq + 1);
    }
    assert!(replay.iter().any(|e| e.seq == live.seq));
}

#[test]
fn metrics_snapshot_is_sane() {
    let (_d, store) = open();
    let m = store.create_mission("metrics").unwrap();
    for _ in 0..50 {
        store.append_event(&m.id, None, "k", json!({})).unwrap();
    }
    store.list_missions().unwrap();
    let snapshot = store.metrics().snapshot();
    assert!(snapshot
        .iter()
        .any(|o| o.op == "event.append" && o.count == 50));
    assert!(snapshot.iter().any(|o| o.op == "mission.create"));
    for op in &snapshot {
        assert!(op.count > 0);
        assert!(
            op.p50_us <= op.p99_us,
            "{}: p50 {} > p99 {}",
            op.op,
            op.p50_us,
            op.p99_us
        );
        assert!(
            op.p99_us <= op.max_us,
            "{}: p99 {} > max {}",
            op.op,
            op.p99_us,
            op.max_us
        );
    }
}

/// SPEC latency budget smoke test: 1000 event appends plus 1000 lease
/// acquire/release cycles must finish in < 10 s total (<10ms/op with slack).
#[test]
fn latency_smoke_test() {
    let (_d, store) = open();
    let m = store.create_mission("bench").unwrap();
    let tasks = store
        .insert_tasks(&m.id, &[plan_task("hot", Role::Coder, &[], &["src/**"])])
        .unwrap();
    let task_id = &tasks[0].id;
    store.refresh_ready(&m.id).unwrap();

    let start = std::time::Instant::now();
    for i in 0..1000 {
        store
            .append_event(&m.id, None, "bench.tick", json!({ "i": i }))
            .unwrap();
    }
    for _ in 0..1000 {
        assert!(store
            .acquire_lease(task_id, "bench", "gen", 60_000)
            .unwrap());
        store
            .set_task_status(task_id, TaskStatus::Ready, None, None)
            .unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "latency budget breached: {elapsed:?} for 3000 ops"
    );
}

#[test]
fn find_approval_honors_prior_decisions() {
    let (_dir, store) = open();
    let mission = store.create_mission("approvals").unwrap();
    let created = store
        .create_approval(ApprovalRequest {
            mission_id: &mission.id,
            task_id: Some("task-1"),
            action: "edit",
            resource: "src/app.rs",
            patch_hash: None,
            policy_snapshot: json!({}),
            expires_at: None,
        })
        .unwrap();
    let found = store
        .find_approval(&mission.id, Some("task-1"), "edit", "src/app.rs")
        .unwrap()
        .unwrap();
    assert_eq!(found.id, created.id);
    assert_eq!(found.status, ApprovalStatus::Pending);
    // Different task scope does not match.
    assert!(store
        .find_approval(&mission.id, Some("task-2"), "edit", "src/app.rs")
        .unwrap()
        .is_none());
    store
        .decide_approval(&created.id, true, "tester", None)
        .unwrap();
    let decided = store
        .find_approval(&mission.id, Some("task-1"), "edit", "src/app.rs")
        .unwrap()
        .unwrap();
    assert_eq!(decided.status, ApprovalStatus::Approved);
}

#[test]
fn replace_mission_tasks_swaps_plan_before_execution() {
    let (_dir, store) = open();
    let mission = store.create_mission("replan").unwrap();
    store
        .insert_tasks(
            &mission.id,
            &[
                plan_task("design", Role::Designer, &[], &[]),
                plan_task("implement", Role::Coder, &["design"], &["src/**"]),
            ],
        )
        .unwrap();
    let replaced = store
        .replace_mission_tasks(
            &mission.id,
            &[plan_task("implement-v2", Role::Coder, &[], &["docs/**"])],
        )
        .unwrap();
    assert_eq!(replaced.len(), 1);
    let tasks = store.list_tasks(&mission.id).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_key, "implement-v2");
}

#[test]
fn replace_mission_tasks_refuses_after_start() {
    let (_dir, store) = open();
    let mission = store.create_mission("replan-late").unwrap();
    let tasks = store
        .insert_tasks(&mission.id, &[plan_task("solo", Role::Designer, &[], &[])])
        .unwrap();
    store.refresh_ready(&mission.id).unwrap();
    assert!(store
        .acquire_lease(&tasks[0].id, "w1", "gen1", 60_000)
        .unwrap());
    let err = store
        .replace_mission_tasks(&mission.id, &[plan_task("other", Role::Designer, &[], &[])])
        .unwrap_err();
    assert!(err.to_string().contains("already started"));
}
