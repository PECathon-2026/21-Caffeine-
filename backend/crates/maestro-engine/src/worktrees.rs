//! Policy-level worktree management on top of [`crate::git`]: mission
//! branches, per-coder task worktrees, shared read-only workspaces for
//! reviewer/security, and scope re-verification at completion.

use crate::{git, scopes};
use maestro_types::config::EnforcementMode;
use maestro_types::domain::{Mission, Task};
use maestro_types::{MaestroError, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a task's file operations happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskWorkspace {
    /// Workspace directory (a worktree under `.maestro/worktrees/<taskId>`,
    /// or the repo root when `Preferred` enforcement fell back).
    pub path: PathBuf,
    /// Branch checked out in the workspace.
    pub branch: String,
    /// The ref diffs and patch hashes are computed against.
    pub base_ref: String,
}

/// Creates and removes git worktrees according to the configured
/// [`EnforcementMode`]: `Required` fails closed when worktree creation
/// fails; `Preferred` falls back to the repo root with a warning.
pub struct WorktreeManager {
    root: PathBuf,
    mode: EnforcementMode,
}

impl WorktreeManager {
    /// `root` is the primary repository checkout.
    pub fn new(root: impl Into<PathBuf>, mode: EnforcementMode) -> Self {
        WorktreeManager {
            root: root.into(),
            mode,
        }
    }

    /// The repository root this manager operates on.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Ensure `maestro/mission/<id12>` exists (created at the mission's
    /// base head, or current HEAD when unset) and return its name.
    pub async fn ensure_mission_branch(&self, mission: &Mission) -> Result<String> {
        let branch = format!("maestro/mission/{}", short_id(&mission.id, 12));
        if git::rev_parse(&self.root, &format!("refs/heads/{branch}"))
            .await
            .is_ok()
        {
            return Ok(branch);
        }
        let base = match &mission.base_head {
            Some(base) => base.clone(),
            None => git::head(&self.root).await?,
        };
        git::create_branch_at(&self.root, &branch, &base).await?;
        Ok(branch)
    }

    /// Create (or reuse) the isolated worktree for a coder task:
    /// `.maestro/worktrees/<taskId>` on branch
    /// `maestro/<mission8>/<key>-<id6>`, based off the mission integration
    /// head (falling back to the mission branch).
    pub async fn for_coder(&self, mission: &Mission, task: &Task) -> Result<TaskWorkspace> {
        let branch = format!(
            "maestro/{}/{}-{}",
            short_id(&mission.id, 8),
            task.task_key,
            short_id(&task.id, 6)
        );
        let base_ref = match (&mission.integration_head, &mission.mission_branch) {
            (Some(head), _) => head.clone(),
            (None, Some(branch)) => branch.clone(),
            (None, None) => self.ensure_mission_branch(mission).await?,
        };
        // Resolve mutable branch names now; stored fork points must be immutable.
        let base_ref = git::rev_parse(&self.root, &base_ref).await?;
        let path = self.root.join(".maestro").join("worktrees").join(&task.id);

        if path.exists() {
            // Reuse only when the stored fork point still matches: scope
            // checks and patch hashes then diff against the base the branch
            // actually forked from. If the integration head advanced (a
            // dependency's patch merged) — or the fork point was never
            // persisted — recreate the worktree so the new attempt forks
            // from the current base and sees dependency files. Stale partial
            // edits are discarded; attempts always restart from scratch.
            match task.base_ref.clone() {
                Some(stored) if stored == base_ref => {
                    return Ok(TaskWorkspace {
                        path,
                        branch,
                        base_ref: stored,
                    });
                }
                _ => {
                    git::remove_worktree(&self.root, &path, true).await?;
                    if git::rev_parse(&self.root, &format!("refs/heads/{branch}"))
                        .await
                        .is_ok()
                    {
                        git::git(&self.root, &["branch", "-D", &branch]).await?;
                    }
                }
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        match git::add_worktree(&self.root, &path, &branch, &base_ref).await {
            Ok(()) => Ok(TaskWorkspace {
                path,
                branch,
                base_ref,
            }),
            Err(e) => match self.mode {
                EnforcementMode::Required => Err(e),
                EnforcementMode::Preferred => {
                    tracing::warn!(
                        error = %e,
                        "worktree creation failed; falling back to repo root (worktrees=preferred)"
                    );
                    let branch = git::current_branch(&self.root).await?;
                    Ok(TaskWorkspace {
                        path: self.root.clone(),
                        branch,
                        base_ref,
                    })
                }
            },
        }
    }

    /// Reviewer/security ride a dependency coder's worktree read-only.
    /// `dependency_workspaces` maps dependency task keys to their
    /// workspaces; the first dependency with a workspace wins.
    pub fn for_reader(
        &self,
        task: &Task,
        dependency_workspaces: &BTreeMap<String, TaskWorkspace>,
    ) -> Result<TaskWorkspace> {
        for dep_key in &task.dependencies {
            if let Some(workspace) = dependency_workspaces.get(dep_key) {
                return Ok(workspace.clone());
            }
        }
        Err(MaestroError::NotFound(format!(
            "task '{}' has no dependency with a workspace to review",
            task.task_key
        )))
    }

    /// Re-verify at completion that everything the task changed
    /// (`git diff --name-only` against its base) stays inside its declared
    /// write scopes. Violating paths are reported in the error.
    pub async fn assert_task_scopes(
        &self,
        workspace: &TaskWorkspace,
        write_scopes: &[String],
    ) -> Result<()> {
        let compiled = scopes::compile_scopes(write_scopes)?;
        let changed = git::diff_name_only(&workspace.path, &workspace.base_ref).await?;
        let violations: Vec<String> = changed
            .into_iter()
            .filter(|path| !scopes::path_in_scopes(&compiled, path))
            .collect();
        if violations.is_empty() {
            Ok(())
        } else {
            Err(MaestroError::PolicyDenied {
                action: "edit".into(),
                resource: violations.join(", "),
                reason: "changed paths outside declared write scopes".into(),
            })
        }
    }

    /// Remove a task worktree. Refuses when the worktree has uncommitted
    /// changes unless `force` is set. The repo root itself (a `Preferred`
    /// fallback workspace) is never removed.
    pub async fn cleanup(&self, workspace: &TaskWorkspace, force: bool) -> Result<()> {
        if crate::browser::active(&workspace.path) {
            return Ok(());
        }
        if workspace.path == self.root {
            return Ok(());
        }
        if !workspace.path.exists() {
            return Ok(());
        }
        if !force && !git::is_clean(&workspace.path).await? {
            return Err(MaestroError::Conflict(format!(
                "worktree {} has uncommitted changes; refusing to remove without force",
                workspace.path.display()
            )));
        }
        git::remove_worktree(&self.root, &workspace.path, force).await
    }
}

fn short_id(id: &str, len: usize) -> &str {
    let end = id
        .char_indices()
        .nth(len)
        .map(|(i, _)| i)
        .unwrap_or(id.len());
    &id[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_support::init_repo;
    use maestro_types::domain::{MissionStatus, RiskLevel, Role, TaskStatus};
    use maestro_types::{new_id, now_ms};

    fn mission(base_head: Option<String>) -> Mission {
        Mission {
            id: new_id(),
            objective: "test".into(),
            status: MissionStatus::Running,
            plan_summary: None,
            base_head,
            mission_branch: None,
            integration_head: None,
            final_approval_id: None,
            created_at: now_ms(),
            updated_at: now_ms(),
        }
    }

    fn task(key: &str, role: Role, deps: &[&str], scopes: &[&str]) -> Task {
        Task {
            id: new_id(),
            mission_id: new_id(),
            task_key: key.into(),
            role,
            title: key.into(),
            description: "test task".into(),
            status: TaskStatus::Ready,
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            acceptance: vec!["works".into()],
            risk: RiskLevel::Low,
            attempts: 0,
            status_reason: None,
            summary: None,
            lease_owner: None,
            lease_expires_at: None,
            lease_generation: None,
            workspace_path: None,
            branch_name: None,
            base_ref: None,
            patch_hash: None,
            integrated_at: None,
            integration_commit: None,
            parent_task_id: None,
            delegation_depth: 0,
            write_scopes: scopes.iter().map(|s| s.to_string()).collect(),
            model_override: None,
            created_at: now_ms(),
            updated_at: now_ms(),
        }
    }

    #[tokio::test]
    async fn mission_branch_created_once() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let manager = WorktreeManager::new(dir.path(), EnforcementMode::Required);
        let m = mission(None);

        let branch = manager.ensure_mission_branch(&m).await.unwrap();
        assert_eq!(branch, format!("maestro/mission/{}", &m.id[..12]));
        // Idempotent.
        let again = manager.ensure_mission_branch(&m).await.unwrap();
        assert_eq!(branch, again);
    }

    #[tokio::test]
    async fn stale_worktree_is_recreated_when_the_integration_head_advances() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let manager = WorktreeManager::new(dir.path(), EnforcementMode::Required);
        let mut m = mission(None);
        let mut t = task("dependent", Role::Coder, &[], &["src/**"]);

        let ws1 = manager.for_coder(&m, &t).await.unwrap();
        std::fs::write(ws1.path.join("stale.txt"), "partial attempt\n").unwrap();
        t.base_ref = Some(ws1.base_ref.clone());

        // A dependency merged: the integration head moves past the fork
        // point (simulated by a new commit on the mission branch).
        let side = dir.path().join("side");
        git::add_worktree(dir.path(), &side, "tmp/advance", &ws1.base_ref)
            .await
            .unwrap();
        std::fs::write(side.join("merged.txt"), "dependency output\n").unwrap();
        crate::git::test_support::commit_all(&side, "dependency patch").await;
        let new_head = git::head(&side).await.unwrap();
        git::remove_worktree(dir.path(), &side, true).await.unwrap();
        m.integration_head = Some(new_head.clone());

        let ws2 = manager.for_coder(&m, &t).await.unwrap();
        assert_eq!(ws2.path, ws1.path);
        assert_eq!(ws2.base_ref, new_head, "fork point must be the new head");
        assert!(
            ws2.path.join("merged.txt").exists(),
            "the dependency's file must be visible"
        );
        assert!(
            !ws2.path.join("stale.txt").exists(),
            "stale partial edits are discarded"
        );

        // Unchanged base: the worktree is reused, not recreated.
        t.base_ref = Some(new_head.clone());
        std::fs::write(ws2.path.join("wip.txt"), "in progress\n").unwrap();
        let ws3 = manager.for_coder(&m, &t).await.unwrap();
        assert!(
            ws3.path.join("wip.txt").exists(),
            "same-base reuse keeps work"
        );
    }

    #[tokio::test]
    async fn coder_worktree_and_scope_assertion() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let manager = WorktreeManager::new(dir.path(), EnforcementMode::Required);
        let m = mission(None);
        let t = task("add-feature", Role::Coder, &[], &["src/**"]);

        let ws = manager.for_coder(&m, &t).await.unwrap();
        assert!(ws.path.starts_with(dir.path().join(".maestro/worktrees")));
        assert!(ws.branch.contains("add-feature"));
        assert!(ws.path.is_dir());

        // Reuse: second call returns the same workspace.
        let ws2 = manager.for_coder(&m, &t).await.unwrap();
        assert_eq!(ws.path, ws2.path);

        // In-scope change passes.
        std::fs::create_dir_all(ws.path.join("src")).unwrap();
        std::fs::write(ws.path.join("src/new.rs"), "// ok\n").unwrap();
        manager
            .assert_task_scopes(&ws, &t.write_scopes)
            .await
            .unwrap();

        // Out-of-scope change is reported with the violating path.
        std::fs::write(ws.path.join("rogue.txt"), "nope\n").unwrap();
        match manager.assert_task_scopes(&ws, &t.write_scopes).await {
            Err(MaestroError::PolicyDenied { resource, .. }) => {
                assert!(resource.contains("rogue.txt"));
            }
            other => panic!("expected PolicyDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reader_rides_dependency_workspace() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let manager = WorktreeManager::new(dir.path(), EnforcementMode::Required);
        let m = mission(None);
        let coder = task("build-it", Role::Coder, &[], &["src/**"]);
        let reviewer = task("review-it", Role::Reviewer, &["build-it"], &[]);

        let coder_ws = manager.for_coder(&m, &coder).await.unwrap();
        let mut deps = BTreeMap::new();
        deps.insert("build-it".to_string(), coder_ws.clone());

        let reader_ws = manager.for_reader(&reviewer, &deps).unwrap();
        assert_eq!(reader_ws.path, coder_ws.path);

        // Missing dependency workspace is an error.
        let orphan = task("review-what", Role::Security, &["ghost"], &[]);
        assert!(matches!(
            manager.for_reader(&orphan, &deps),
            Err(MaestroError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn unknown_fork_point_recreates_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let manager = WorktreeManager::new(dir.path(), EnforcementMode::Required);
        let m = mission(None);
        let mut t = task("no-base", Role::Coder, &[], &["src/**"]);

        let ws = manager.for_coder(&m, &t).await.unwrap();
        std::fs::write(ws.path.join("stale.txt"), "unknown provenance\n").unwrap();

        // Crash before the fork point was persisted: the old content can't
        // be trusted against any base, so the worktree is recreated.
        t.base_ref = None;
        let ws2 = manager.for_coder(&m, &t).await.unwrap();
        assert_eq!(ws2.path, ws.path);
        assert_eq!(ws2.base_ref, ws.base_ref);
        assert!(!ws2.path.join("stale.txt").exists());
    }

    #[tokio::test]
    async fn cleanup_refuses_dirty_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let manager = WorktreeManager::new(dir.path(), EnforcementMode::Required);
        let m = mission(None);
        let t = task("dirty-task", Role::Coder, &[], &["**"]);

        let ws = manager.for_coder(&m, &t).await.unwrap();
        std::fs::write(ws.path.join("uncommitted.txt"), "x\n").unwrap();

        assert!(matches!(
            manager.cleanup(&ws, false).await,
            Err(MaestroError::Conflict(_))
        ));
        manager.cleanup(&ws, true).await.unwrap();
        assert!(!ws.path.exists());
    }
}
