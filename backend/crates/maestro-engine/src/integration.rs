//! Mission completion: gate verification, the serial checked merge queue,
//! conflict-resolver delegation, final approval, and finalization.

use crate::orchestrator::{clip, FinalizeOutcome, Orchestrator};
use crate::{git, sandbox};
use maestro_store::ApprovalRequest;
use maestro_types::domain::{
    ApprovalStatus, MergeStatus, Mission, MissionStatus, RiskLevel, Role, Task, TaskStatus, Verdict,
};
use maestro_types::event::kind;
use maestro_types::plan::PlanTask;
use maestro_types::{MaestroError, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// What the scheduler should do after a completion attempt.
pub(crate) enum Completion {
    /// The mission reached a terminal or awaiting state.
    Done(MissionStatus),
    /// New tasks were created (conflict resolvers); keep scheduling.
    Continue,
}

impl Orchestrator {
    /// Called when no task is ready or running: verify gates, drive the merge
    /// queue, and either finish the mission or spawn resolver tasks.
    pub(crate) async fn try_complete(&self, mission_id: &str) -> Result<Completion> {
        let store = &self.inner.store;
        let tasks = store.list_tasks(mission_id)?;

        if !tasks.iter().all(|t| t.status == TaskStatus::Completed) {
            let reason = tasks
                .iter()
                .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Blocked))
                .map(|t| {
                    format!(
                        "{} {}: {}",
                        t.status.as_str(),
                        t.task_key,
                        t.status_reason
                            .as_deref()
                            .or(t.summary.as_deref())
                            .unwrap_or("no reason recorded")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let reason = if reason.is_empty() {
                "tasks did not all complete".to_string()
            } else {
                reason
            };
            // Before blocking, merge any fully gated patches so a retried
            // dependent forks from a base that already includes them (its
            // worktree is recreated when the base moved).
            if self.inner.config.integration.strategy
                == maestro_types::config::IntegrationStrategy::MissionBranch
                && tasks.iter().any(|t| {
                    t.role == Role::Coder && t.integrated_at.is_none() && t.patch_hash.is_some()
                })
            {
                if let Some(done) = self.merge_queue_pass(mission_id, &tasks, true).await? {
                    return Ok(done);
                }
            }
            self.set_mission_status_pub(mission_id, MissionStatus::Blocked, Some(&reason))?;
            return Ok(Completion::Done(MissionStatus::Blocked));
        }

        if let Err(reason) = self.gates_satisfied(&tasks)? {
            self.set_mission_status_pub(mission_id, MissionStatus::Blocked, Some(&reason))?;
            return Ok(Completion::Done(MissionStatus::Blocked));
        }

        match self.inner.config.integration.strategy {
            maestro_types::config::IntegrationStrategy::Direct => {
                self.set_mission_status_pub(mission_id, MissionStatus::Completed, None)?;
                Ok(Completion::Done(MissionStatus::Completed))
            }
            maestro_types::config::IntegrationStrategy::MissionBranch => {
                self.integrate(mission_id, &tasks).await
            }
        }
    }

    pub(crate) fn set_mission_status_pub(
        &self,
        mission_id: &str,
        status: MissionStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        self.inner
            .store
            .set_mission_status(mission_id, status, reason)?;
        self.inner.store.append_event(
            mission_id,
            None,
            kind::MISSION_STATUS,
            serde_json::json!({ "status": status.as_str(), "reason": reason }),
        )?;
        Ok(())
    }

    /// Why this coder's patch may not merge: `Some(reason)` when it lacks an
    /// approved, hash-fresh reviewer gate (or security gate when high-risk),
    /// `None` when fully gated.
    fn task_gate_staleness(&self, coder: &Task) -> Result<Option<String>> {
        let gates = self.inner.store.gates_for_task(&coder.id)?;
        let fresh = |role: Role| {
            gates.iter().any(|g| {
                g.role == role && g.verdict == Verdict::Approved && g.patch_hash == coder.patch_hash
            })
        };
        if !fresh(Role::Reviewer) {
            return Ok(Some(format!(
                "coder task {} lacks a fresh approved reviewer gate",
                coder.task_key
            )));
        }
        if (coder.risk == RiskLevel::High || self.inner.config.runtime.guild_roles)
            && !fresh(Role::Security)
        {
            return Ok(Some(format!(
                "high-risk coder task {} lacks a fresh approved security gate",
                coder.task_key
            )));
        }
        Ok(None)
    }

    /// Every coder task with a patch needs an approved, hash-fresh reviewer
    /// gate; high-risk coders additionally need a security gate.
    fn gates_satisfied(&self, tasks: &[Task]) -> Result<std::result::Result<(), String>> {
        for coder in tasks
            .iter()
            .filter(|t| t.role == Role::Coder && t.patch_hash.is_some())
        {
            if let Some(reason) = self.task_gate_staleness(coder)? {
                return Ok(Err(reason));
            }
        }
        Ok(Ok(()))
    }

    /// The integration worktree: a persistent checkout of the mission branch
    /// under `.maestro/integration/<mission8>`.
    async fn integration_worktree(&self, mission: &Mission, branch: &str) -> Result<PathBuf> {
        let short: String = mission.id.chars().take(8).collect();
        let path = self
            .inner
            .root
            .join(".maestro")
            .join("integration")
            .join(short);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // The branch already exists — plain `worktree add <path> <branch>`.
            let path_str = path.to_string_lossy();
            git::git(
                &self.inner.root,
                &["worktree", "add", path_str.as_ref(), branch],
            )
            .await?;
        } else {
            // A crash (SIGKILL) mid-merge leaves MERGE_HEAD behind, which
            // makes the checkout below fail. Detect and recover: abort the
            // half-merge and reset to the last known-good integration state.
            if git::git(&path, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
                .await
                .is_ok()
            {
                tracing::warn!(
                    path = %path.display(),
                    "recovering integration worktree from an interrupted merge"
                );
                let _ = git::git(&path, &["merge", "--abort"]).await;
                let target = mission.integration_head.as_deref().unwrap_or(branch);
                git::reset_hard(&path, target).await?;
            }
            git::git(&path, &["checkout", branch]).await?;
        }
        Ok(path)
    }

    /// Enqueue and merge gated coder patches. `eager` runs mid-mission (some
    /// tasks blocked/failed): only fully gated patches are enqueued and a
    /// stale gate skips the entry instead of blocking. Returns
    /// `Some(completion)` when the pass decided the mission's next state
    /// (blocked, or resolver tasks spawned), `None` when the queue drained.
    pub(crate) async fn merge_queue_pass(
        &self,
        mission_id: &str,
        tasks: &[Task],
        eager: bool,
    ) -> Result<Option<Completion>> {
        let _workspace = self.inner.workspace_gate.read().await;
        self.inner
            .store
            .assert_history_mission_current(mission_id)?;
        let store = &self.inner.store;
        let mission = store.get_mission(mission_id)?;
        let branch = self.inner.worktrees.ensure_mission_branch(&mission).await?;
        // Planning normally records the git base; fill it in for missions
        // that reached integration without one (finalize requires it).
        if mission.mission_branch.is_none() {
            let base = match mission.base_head.clone() {
                Some(base) => base,
                None => git::head(&self.inner.root).await?,
            };
            store.set_mission_git_base(mission_id, &base, &branch)?;
        }
        let int_ws = self.integration_worktree(&mission, &branch).await?;

        store.reconcile_merge_queue(mission_id)?;

        // Enqueue every reviewed, un-integrated coder patch (enqueue
        // supersedes prior non-merged entries, so this is idempotent).
        for task in tasks.iter().filter(|t| {
            t.role == Role::Coder && t.integrated_at.is_none() && t.patch_hash.is_some()
        }) {
            if eager && self.task_gate_staleness(task)?.is_some() {
                continue;
            }
            let hash = task.patch_hash.as_deref().unwrap_or_default();
            let entry = store.enqueue_merge(mission_id, &task.id, hash)?;
            store.append_event(
                mission_id,
                Some(&task.id),
                kind::MERGE_ENQUEUED,
                serde_json::json!({ "position": entry.position, "patchHash": hash }),
            )?;
        }

        while let Some(entry) = store.next_pending_merge(mission_id)? {
            let task = store.get_task(&entry.task_id)?;
            if task.integrated_at.is_some()
                || task.patch_hash.as_deref() != Some(entry.patch_hash.as_str())
            {
                store.set_merge_status(
                    &entry.id,
                    MergeStatus::Superseded,
                    Some("patch changed or already integrated"),
                    None,
                )?;
                continue;
            }
            // Re-verify the gate at merge time: an entry enqueued by an
            // earlier pass may have gone stale since.
            if let Some(reason) = self.task_gate_staleness(&task)? {
                store.set_merge_status(&entry.id, MergeStatus::Superseded, Some(&reason), None)?;
                if eager {
                    continue;
                }
                self.set_mission_status_pub(mission_id, MissionStatus::Blocked, Some(&reason))?;
                return Ok(Some(Completion::Done(MissionStatus::Blocked)));
            }
            let Some(task_branch) = task.branch_name.clone() else {
                store.set_merge_status(
                    &entry.id,
                    MergeStatus::Failed,
                    Some("task has no branch"),
                    None,
                )?;
                self.set_mission_status_pub(
                    mission_id,
                    MissionStatus::Blocked,
                    Some(&format!("task {} has no branch to merge", task.task_key)),
                )?;
                return Ok(Some(Completion::Done(MissionStatus::Blocked)));
            };

            store.set_merge_status(&entry.id, MergeStatus::Checking, None, None)?;
            store.append_event(
                mission_id,
                Some(&task.id),
                kind::MERGE_STARTED,
                serde_json::json!({ "branch": task_branch }),
            )?;
            let previous_head = git::head(&int_ws).await?;

            match git::merge_no_ff(&int_ws, &task_branch).await? {
                git::MergeOutcome::Merged(commit) => {
                    if let Err(detail) = self.run_integration_checks(mission_id, &int_ws).await? {
                        git::reset_hard(&int_ws, &previous_head).await?;
                        store.set_merge_status(
                            &entry.id,
                            MergeStatus::Failed,
                            Some(&detail),
                            None,
                        )?;
                        store.append_event(
                            mission_id,
                            Some(&task.id),
                            kind::MERGE_ROLLED_BACK,
                            serde_json::json!({ "reason": detail }),
                        )?;
                        self.set_mission_status_pub(
                            mission_id,
                            MissionStatus::Blocked,
                            Some(&format!(
                                "integration checks failed after merging {}: {}",
                                task.task_key, detail
                            )),
                        )?;
                        return Ok(Some(Completion::Done(MissionStatus::Blocked)));
                    }
                    store.set_mission_integration_head(mission_id, &commit)?;
                    store.mark_task_integrated(&task.id, &commit)?;
                    store.set_merge_status(&entry.id, MergeStatus::Merged, None, Some(&commit))?;
                    store.append_event(
                        mission_id,
                        Some(&task.id),
                        kind::MERGE_COMPLETED,
                        serde_json::json!({ "commit": commit }),
                    )?;
                    // The patch is merged: its worktree and task branch are
                    // disposable. Best-effort — never fail the mission on
                    // cleanup.
                    self.cleanup_task_workspace(&task).await;
                }
                git::MergeOutcome::Conflict(files) => {
                    store.set_merge_status(
                        &entry.id,
                        MergeStatus::Conflict,
                        Some(&files.join(", ")),
                        None,
                    )?;
                    store.append_event(
                        mission_id,
                        Some(&task.id),
                        kind::MERGE_CONFLICT,
                        serde_json::json!({ "files": files }),
                    )?;
                    let resolver_key = resolver_key_for(&task.task_key);
                    let already_resolved = store
                        .list_tasks(mission_id)?
                        .iter()
                        .any(|t| t.task_key == resolver_key);
                    if already_resolved {
                        self.set_mission_status_pub(
                            mission_id,
                            MissionStatus::Blocked,
                            Some(&format!(
                                "repeated merge conflict integrating {}",
                                task.task_key
                            )),
                        )?;
                        return Ok(Some(Completion::Done(MissionStatus::Blocked)));
                    }
                    self.create_conflict_tasks(&mission, &task, &files).await?;
                    // The conflicted patch now lands via its resolver.
                    store.mark_task_integrated(&task.id, &format!("resolved-by:{resolver_key}"))?;
                    self.set_mission_status_pub(mission_id, MissionStatus::Running, None)?;
                    self.poke();
                    return Ok(Some(Completion::Continue));
                }
            }
        }
        Ok(None)
    }

    async fn integrate(&self, mission_id: &str, tasks: &[Task]) -> Result<Completion> {
        let store = &self.inner.store;
        if let Some(done) = self.merge_queue_pass(mission_id, tasks, false).await? {
            return Ok(done);
        }

        // Queue drained — final approval or auto-finalize.
        let mission = store.get_mission(mission_id)?;
        let Some(head) = mission.integration_head.clone() else {
            // Nothing was ever integrated (no coder produced a patch): the
            // mission ends vacuously, so its checkouts and branch are trash.
            self.cleanup_mission_workspaces(mission_id, true).await;
            self.set_mission_status_pub(mission_id, MissionStatus::Completed, None)?;
            return Ok(Completion::Done(MissionStatus::Completed));
        };
        let base = mission.base_head.clone().unwrap_or_default();

        if !self.inner.config.integration.final_approval {
            // A finalize failure here (dirty tree, moved HEAD, …) is
            // recoverable: block the mission like the approval path does,
            // instead of propagating and letting the worker mark it failed —
            // which would tear down the mission branch holding merged work.
            match self.finalize_mission(mission_id).await {
                Ok(outcome) => {
                    tracing::info!(commit = %outcome.commit, "mission auto-finalized");
                    return Ok(Completion::Done(MissionStatus::Completed));
                }
                Err(e) => {
                    let reason = e.to_string();
                    tracing::warn!(error = %reason, "auto-finalize failed; blocking mission");
                    self.set_mission_status_pub(mission_id, MissionStatus::Blocked, Some(&reason))?;
                    return Ok(Completion::Done(MissionStatus::Blocked));
                }
            }
        }

        let diff = git::unified_diff(&self.inner.root, &base, &head).await?;
        let hash = hex::encode(Sha256::digest(diff.as_bytes()));

        if let Some(approval_id) = &mission.final_approval_id {
            let approval = store.get_approval(approval_id)?;
            match approval.status {
                ApprovalStatus::Approved if approval.patch_hash.as_deref() == Some(&hash) => {
                    self.finalize_mission(mission_id).await?;
                    return Ok(Completion::Done(MissionStatus::Completed));
                }
                ApprovalStatus::Pending if approval.patch_hash.as_deref() == Some(&hash) => {
                    self.set_mission_status_pub(mission_id, MissionStatus::AwaitingApproval, None)?;
                    return Ok(Completion::Done(MissionStatus::AwaitingApproval));
                }
                ApprovalStatus::Rejected => {
                    self.set_mission_status_pub(
                        mission_id,
                        MissionStatus::Blocked,
                        Some("final integration was rejected"),
                    )?;
                    return Ok(Completion::Done(MissionStatus::Blocked));
                }
                _ => {} // expired or stale-hash: request a fresh approval
            }
        }

        let checks: Vec<&str> = self
            .inner
            .config
            .checks
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let risks: Vec<String> = tasks
            .iter()
            .filter(|t| t.risk == RiskLevel::High)
            .map(|t| t.task_key.clone())
            .collect();
        let approval = store.create_approval(ApprovalRequest {
            mission_id,
            task_id: None,
            action: "integrate",
            resource: "mission",
            patch_hash: Some(&hash),
            policy_snapshot: serde_json::json!({
                "diff": clip(&diff, 100_000),
                "checks": checks,
                "highRiskTasks": risks,
                "integrationHead": head,
                "baseHead": base,
            }),
            expires_at: None,
        })?;
        store.set_mission_final_approval(mission_id, &approval.id)?;
        store.append_event(
            mission_id,
            None,
            kind::APPROVAL_REQUESTED,
            serde_json::json!({
                "approvalId": approval.id,
                "action": "integrate",
                "patchHash": hash,
            }),
        )?;
        self.set_mission_status_pub(mission_id, MissionStatus::AwaitingApproval, None)?;
        Ok(Completion::Done(MissionStatus::AwaitingApproval))
    }

    /// Best-effort removal of a task's worktree and branch once its patch is
    /// merged (or the mission is finalized). Failures are logged, never
    /// propagated: cleanup must not fail a mission.
    async fn cleanup_task_workspace(&self, task: &Task) {
        if let Some(ws_path) = task.workspace_path.as_deref() {
            let workspace = crate::worktrees::TaskWorkspace {
                path: PathBuf::from(ws_path),
                branch: task.branch_name.clone().unwrap_or_default(),
                base_ref: task.base_ref.clone().unwrap_or_default(),
            };
            if let Err(e) = self.inner.worktrees.cleanup(&workspace, true).await {
                tracing::warn!(
                    task = %task.task_key,
                    path = %ws_path,
                    error = %e,
                    "task worktree cleanup failed"
                );
            }
        }
        if let Some(branch) = task.branch_name.as_deref() {
            self.delete_branch_best_effort(branch).await;
        }
    }

    /// Delete a local branch if it exists; log (don't fail) on error.
    async fn delete_branch_best_effort(&self, branch: &str) {
        if git::rev_parse(&self.inner.root, &format!("refs/heads/{branch}"))
            .await
            .is_err()
        {
            return;
        }
        if let Err(e) = git::git(&self.inner.root, &["branch", "-D", branch]).await {
            tracing::warn!(branch, error = %e, "task branch cleanup failed");
        }
    }

    /// Best-effort teardown once a mission is done with its checkouts:
    /// remove the integration worktree and prune every remaining coder
    /// worktree/branch of this mission. Never fails the mission.
    ///
    /// `delete_mission_branch` is set when the branch holds nothing worth
    /// keeping: explicit cancels and vacuous completions. Failed missions
    /// and finalization keep it (`false`) — merged work stays reachable.
    pub async fn cleanup_mission_workspaces(&self, mission_id: &str, delete_mission_branch: bool) {
        let short: String = mission_id.chars().take(8).collect();
        let int_path = self
            .inner
            .root
            .join(".maestro")
            .join("integration")
            .join(short);
        if int_path.exists() {
            if let Err(e) = git::remove_worktree(&self.inner.root, &int_path, true).await {
                tracing::warn!(
                    path = %int_path.display(),
                    error = %e,
                    "integration worktree cleanup failed"
                );
            }
        }
        match self.inner.store.list_tasks(mission_id) {
            Ok(tasks) => {
                for task in tasks.iter().filter(|t| t.role == Role::Coder) {
                    self.cleanup_task_workspace(task).await;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not list tasks for worktree cleanup");
            }
        }
        let source_path = self
            .inner
            .root
            .join(".maestro/worktrees")
            .join(format!("source-{mission_id}"));
        if source_path.exists() {
            if let Err(e) = git::remove_worktree(&self.inner.root, &source_path, true).await {
                tracing::warn!(error = %e, "source snapshot worktree cleanup failed");
            }
        }
        if delete_mission_branch {
            match self.inner.store.get_mission(mission_id) {
                Ok(mission) => {
                    if let Some(branch) = mission.mission_branch.as_deref() {
                        self.delete_branch_best_effort(branch).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not load mission for branch cleanup");
                }
            }
        }
    }

    /// Run every configured project check inside the integration worktree.
    /// Returns `Err(detail)` (inner) on the first failure.
    async fn run_integration_checks(
        &self,
        mission_id: &str,
        int_ws: &std::path::Path,
    ) -> Result<std::result::Result<(), String>> {
        let safety = &self.inner.config.safety;
        for check in &self.inner.config.checks {
            self.inner.store.append_event(
                mission_id,
                None,
                kind::CHECK_STARTED,
                serde_json::json!({ "name": check.name }),
            )?;
            let mut spec = sandbox::SandboxSpec::new(int_ws.to_path_buf());
            spec.mode = safety.sandbox;
            spec.allow_network = safety.sandbox_network;
            spec.timeout_ms = safety.check_timeout_ms;
            spec.max_output_chars = safety.max_check_output_chars;
            let result = sandbox::run_check(&spec, &check.command).await?;
            self.inner.store.append_event(
                mission_id,
                None,
                kind::CHECK_FINISHED,
                serde_json::json!({
                    "name": check.name,
                    "exitCode": result.exit_code,
                    "ok": result.success(),
                }),
            )?;
            if !result.success() {
                return Ok(Err(format!(
                    "check `{}` failed (exit {}): {}",
                    check.name,
                    result.exit_code,
                    clip(&result.stderr, 2_000)
                )));
            }
        }
        Ok(Ok(()))
    }

    /// Delegate a scoped resolver coder plus a dependent reviewer for a
    /// conflicted merge.
    async fn create_conflict_tasks(
        &self,
        mission: &Mission,
        conflicted: &Task,
        files: &[String],
    ) -> Result<()> {
        let store = &self.inner.store;
        let runtime = &self.inner.config.runtime;
        let resolver_key = resolver_key_for(&conflicted.task_key);
        let mut scopes = files.to_vec();
        scopes.extend(conflicted.write_scopes.iter().cloned());

        let coder = PlanTask {
            key: resolver_key.clone(),
            role: Role::Coder,
            title: format!("Resolve merge conflict from {}", conflicted.task_key),
            description: format!(
                "Task `{}` ({}) could not be merged into the mission integration branch \
                 `{}` because of conflicts in: {}. Re-apply the intent of that task on top \
                 of the current integration state. The original task branch is `{}`; its \
                 summary was: {}",
                conflicted.task_key,
                conflicted.title,
                mission.mission_branch.as_deref().unwrap_or("(unset)"),
                files.join(", "),
                conflicted.branch_name.as_deref().unwrap_or("(unknown)"),
                conflicted.summary.as_deref().unwrap_or("(none)"),
            ),
            dependencies: vec![],
            acceptance: vec!["the conflicted change is re-applied cleanly".into()],
            risk: conflicted.risk,
            write_scopes: scopes,
        };
        let coder_task = store.add_delegated_task(
            &mission.id,
            &conflicted.id,
            &coder,
            runtime.max_delegation_depth as i64,
            runtime.max_tasks_per_mission as i64,
        )?;

        let reviewer = PlanTask {
            key: format!("{resolver_key}-review"),
            role: Role::Reviewer,
            title: format!("Review the conflict resolution for {}", conflicted.task_key),
            description: "Review the resolver's diff for correctness and record a verdict."
                .to_string(),
            dependencies: vec![resolver_key.clone()],
            acceptance: vec!["verdict recorded".into()],
            risk: RiskLevel::Low,
            write_scopes: vec![],
        };
        store.add_delegated_task(
            &mission.id,
            &coder_task.id,
            &reviewer,
            runtime.max_delegation_depth as i64,
            runtime.max_tasks_per_mission as i64,
        )?;
        store.refresh_ready(&mission.id)?;
        Ok(())
    }

    /// Verify the (required) approval against a freshly recomputed hash and
    /// fast-forward the user's branch onto the mission integration branch.
    pub async fn finalize_mission(&self, mission_id: &str) -> Result<FinalizeOutcome> {
        let _workspace = self.inner.workspace_gate.read().await;
        self.inner
            .store
            .assert_history_mission_current(mission_id)?;
        self.ensure_history_recovered().await?;
        let store = &self.inner.store;
        let mission = store.get_mission(mission_id)?;
        let branch = mission
            .mission_branch
            .clone()
            .ok_or_else(|| MaestroError::Conflict("mission has no integration branch".into()))?;
        let Some(head) = mission.integration_head.clone() else {
            // Nothing was integrated (no coder patches) — the mission is
            // complete as-is.
            self.set_mission_status_pub(mission_id, MissionStatus::Completed, None)?;
            let commit = git::head(&self.inner.root).await?;
            return Ok(FinalizeOutcome { commit, branch });
        };
        let base = mission
            .base_head
            .clone()
            .ok_or_else(|| MaestroError::Conflict("mission has no recorded base".into()))?;

        if self.inner.config.integration.final_approval {
            let approval_id = mission.final_approval_id.clone().ok_or_else(|| {
                MaestroError::Conflict("no final integration approval was requested".into())
            })?;
            let approval = store.get_approval(&approval_id)?;
            if approval.status != ApprovalStatus::Approved {
                return Err(MaestroError::Conflict(format!(
                    "final integration approval is {}",
                    approval.status.as_str()
                )));
            }
            let diff = git::unified_diff(&self.inner.root, &base, &head).await?;
            let hash = hex::encode(Sha256::digest(diff.as_bytes()));
            if approval.patch_hash.as_deref() != Some(hash.as_str()) {
                return Err(MaestroError::Conflict(
                    "stale approval: the integration diff changed after approval".into(),
                ));
            }
        }

        if !git::is_clean(&self.inner.root).await? {
            return Err(MaestroError::Conflict(
                "working tree is not clean; commit or stash before finalizing".into(),
            ));
        }
        let current = git::head(&self.inner.root).await?;
        if current != base {
            return Err(MaestroError::Conflict(format!(
                "HEAD moved since the mission started ({} → {}); rebase the mission first",
                &base[..base.len().min(8)],
                &current[..current.len().min(8)],
            )));
        }

        let commit = git::merge_ff_only(&self.inner.root, &branch).await?;
        self.capture_version(
            &self.inner.root,
            Some(mission_id),
            None,
            "applied",
            &mission.objective,
            true,
        )
        .await?;
        store.append_event(
            mission_id,
            None,
            kind::INTEGRATION_FINALIZED,
            serde_json::json!({ "commit": commit, "branch": branch }),
        )?;
        // The work is on the user branch now; drop the mission's worktrees.
        self.cleanup_mission_workspaces(mission_id, false).await;
        self.set_mission_status_pub(mission_id, MissionStatus::Completed, None)?;
        Ok(FinalizeOutcome { commit, branch })
    }
}

fn resolver_key_for(task_key: &str) -> String {
    let trimmed: String = task_key.chars().take(40).collect();
    format!("resolve-{trimmed}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_support::{commit_all, init_repo};
    use crate::Orchestrator;
    use maestro_providers::ProviderRegistry;
    use maestro_store::Store;
    use maestro_types::config::MaestroConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn integration_worktree_recovers_from_an_interrupted_merge() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let store = Store::open(&dir.path().join(".maestro")).unwrap();
        let config = MaestroConfig::default();
        let registry = Arc::new(ProviderRegistry::new(&config));
        let orch = Orchestrator::new(
            store.clone(),
            config,
            registry,
            dir.path().to_path_buf(),
            "gen-test".to_string(),
        );
        let mission = orch.create_mission("recover").await.unwrap();
        let branch = orch
            .inner
            .worktrees
            .ensure_mission_branch(&mission)
            .await
            .unwrap();
        let int_ws = orch.integration_worktree(&mission, &branch).await.unwrap();

        // Build a conflict: the mission branch and main both edit README.
        std::fs::write(int_ws.join("README.md"), "integration version\n").unwrap();
        commit_all(&int_ws, "integration edit").await;
        let good_head = git::head(&int_ws).await.unwrap();
        std::fs::write(dir.path().join("README.md"), "main version\n").unwrap();
        commit_all(dir.path(), "main edit").await;

        // Simulate a SIGKILL mid-merge: the merge fails and is NOT aborted,
        // leaving MERGE_HEAD behind.
        assert!(git::git(&int_ws, &["merge", "main"]).await.is_err());
        assert!(
            git::git(&int_ws, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
                .await
                .is_ok()
        );
        // Plain checkout would fail in this state (the pre-fix behavior).
        assert!(git::git(&int_ws, &["checkout", &branch]).await.is_err());

        // Re-entering the integration path recovers instead of failing.
        let recovered = orch.integration_worktree(&mission, &branch).await.unwrap();
        assert_eq!(recovered, int_ws);
        assert!(
            git::git(&int_ws, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
                .await
                .is_err(),
            "the half-merge must be aborted"
        );
        assert!(git::is_clean(&int_ws).await.unwrap());
        // Still on the mission branch, at the last good pre-merge commit.
        assert_eq!(git::current_branch(&int_ws).await.unwrap(), branch);
        assert_eq!(git::head(&int_ws).await.unwrap(), good_head);
    }

    #[tokio::test]
    async fn cancelled_mission_workspaces_are_reclaimed() {
        use maestro_types::domain::MissionStatus;

        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let store = Store::open(&dir.path().join(".maestro")).unwrap();
        let config = MaestroConfig::default();
        let registry = Arc::new(ProviderRegistry::new(&config));
        let orch = Orchestrator::new(
            store.clone(),
            config,
            registry,
            dir.path().to_path_buf(),
            "gen-test".to_string(),
        );

        let mission = orch.create_mission("doomed").await.unwrap();
        let branch = orch
            .inner
            .worktrees
            .ensure_mission_branch(&mission)
            .await
            .unwrap();
        let base = git::head(dir.path()).await.unwrap();
        store
            .set_mission_git_base(&mission.id, &base, &branch)
            .unwrap();
        let int_ws = orch.integration_worktree(&mission, &branch).await.unwrap();

        // A coder task with a real worktree, recorded like the orchestrator would.
        let tasks = store
            .insert_tasks(
                &mission.id,
                &[PlanTask {
                    key: "add-x".into(),
                    role: Role::Coder,
                    title: "add x".into(),
                    description: "add x".into(),
                    dependencies: vec![],
                    acceptance: vec![],
                    risk: maestro_types::domain::RiskLevel::Low,
                    write_scopes: vec!["src/**".into()],
                }],
            )
            .unwrap();
        let mission = store.get_mission(&mission.id).unwrap();
        let ws = orch
            .inner
            .worktrees
            .for_coder(&mission, &tasks[0])
            .await
            .unwrap();
        store
            .set_task_worktree(
                &tasks[0].id,
                &ws.path.to_string_lossy(),
                &ws.branch,
                &ws.base_ref,
            )
            .unwrap();
        assert!(ws.path.is_dir());
        assert!(int_ws.is_dir());

        store
            .set_mission_status(&mission.id, MissionStatus::Cancelled, Some("test"))
            .unwrap();
        orch.cleanup_mission_workspaces(&mission.id, true).await;

        assert!(!ws.path.exists(), "coder worktree must be removed");
        assert!(!int_ws.exists(), "integration worktree must be removed");
        assert!(
            git::rev_parse(dir.path(), &format!("refs/heads/{}", ws.branch))
                .await
                .is_err(),
            "task branch must be deleted"
        );
        assert!(
            git::rev_parse(dir.path(), &format!("refs/heads/{branch}"))
                .await
                .is_err(),
            "abandoned mission branch must be deleted"
        );
    }
}
