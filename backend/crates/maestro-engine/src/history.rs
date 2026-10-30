//! Git-backed code history. Preview merges happen in Git's object database;
//! applying a restore changes working files, never HEAD or the user's index.
use crate::{git, Orchestrator};
use maestro_types::{
    history::{History, PendingRestore, RestorePreview, RestoreResult, Version},
    new_id, now_ms, MaestroError, Result,
};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio};
use tokio::io::AsyncWriteExt;

async fn tree(root: &Path, commit: &str) -> Result<String> {
    git::git(root, &["rev-parse", &format!("{commit}^{{tree}}")]).await
}

async fn commit_tree(root: &Path, tree: &str, parent: &str) -> Result<String> {
    git::git(
        root,
        &[
            "-c",
            "user.name=Maestro History",
            "-c",
            "user.email=history@localhost",
            "commit-tree",
            tree,
            "-p",
            parent,
            "-m",
            "Maestro restored version",
        ],
    )
    .await
}

fn excluded(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/')
        .any(|p| matches!(p, ".git" | ".maestro" | "node_modules"))
        || ((name == ".env" || name.starts_with(".env."))
            && !matches!(name, ".env.example" | ".env.sample" | ".env.template"))
}

async fn diff(root: &Path, from: &str, to: &str) -> Result<String> {
    Ok(String::from_utf8_lossy(&patch_bytes(root, from, to).await?).into_owned())
}

async fn patch_bytes(root: &Path, from: &str, to: &str) -> Result<Vec<u8>> {
    let out = tokio::process::Command::new("git")
        .args([
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            from,
            to,
            "--",
        ])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !out.status.success() {
        return Err(MaestroError::Git(
            String::from_utf8_lossy(&out.stderr).into(),
        ));
    }
    // Preserve trailing blank lines in Git binary patches and non-UTF8 text.
    Ok(out.stdout)
}

impl Orchestrator {
    pub(crate) async fn capture_version(
        &self,
        path: &Path,
        mission: Option<&str>,
        task: Option<&str>,
        kind: &str,
        label: &str,
        current: bool,
    ) -> Result<Version> {
        let _history = self.inner.history_lock.lock().await;
        let (base, snapshot) = git::snapshot_working_tree(path).await?;
        let history = self.inner.store.history()?;
        let previous = if let Some(task) = task {
            history
                .versions
                .iter()
                .find(|v| v.task_id.as_deref() == Some(task))
        } else {
            history
                .versions
                .iter()
                .find(|v| Some(&v.id) == history.current_id.as_ref())
        };
        if kind == "draft" {
            if let Some(v) = previous {
                if tree(path, &v.snapshot).await? == tree(path, &snapshot).await? {
                    return Ok(v.clone());
                }
            }
        }
        let v = Version {
            id: new_id(),
            parent_id: previous.map(|v| v.id.clone()),
            mission_id: mission.map(Into::into),
            task_id: task.map(Into::into),
            kind: kind.into(),
            label: label.into(),
            snapshot,
            base_snapshot: previous.map(|v| v.snapshot.clone()).unwrap_or(base),
            logical_snapshot: None,
            created_at: now_ms(),
        };
        self.pin_version(&v).await?;
        self.inner.store.insert_version(&v, current)?;
        if let Some(m) = mission {
            self.inner.store.append_event(
                m,
                task,
                "history.saved",
                serde_json::json!({"versionId":v.id,"label":v.label,"kind":v.kind}),
            )?;
        }
        Ok(v)
    }

    async fn pin_version(&self, version: &Version) -> Result<()> {
        git::git(
            &self.inner.root,
            &[
                "update-ref",
                &format!("refs/maestro/versions/{}", version.id),
                &version.snapshot,
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn code_history(&self) -> Result<History> {
        let _workspace = self.inner.workspace_gate.write().await;
        self.ensure_history_recovered().await?;
        if self.inner.store.history()?.versions.is_empty() {
            self.capture_version(
                &self.inner.root,
                None,
                None,
                "manual",
                "Initial project version",
                true,
            )
            .await?;
        }
        self.inner.store.history()
    }

    pub async fn save_code_version(&self, label: &str) -> Result<Version> {
        let _workspace = self.inner.workspace_gate.write().await;
        self.ensure_history_recovered().await?;
        self.capture_version(&self.inner.root, None, None, "manual", label, true)
            .await
    }

    pub async fn version_diff(&self, id: &str) -> Result<String> {
        let v = self.inner.store.history_version(id)?;
        diff(&self.inner.root, &v.base_snapshot, &v.snapshot).await
    }

    /// Reconcile a crash between file application and the database receipt.
    /// Ambiguous partial/external edits are never silently overwritten.
    pub(crate) async fn ensure_history_recovered(&self) -> Result<()> {
        let _history = self.inner.history_lock.lock().await;
        let Some(pending) = self.inner.store.pending_restore()? else {
            return Ok(());
        };
        let (_, actual) = git::snapshot_working_tree(&self.inner.root).await?;
        let actual = tree(&self.inner.root, &actual).await?;
        if actual == tree(&self.inner.root, &pending.after_snapshot).await? {
            self.inner.store.finish_restore(&pending, true)?;
        } else if actual == tree(&self.inner.root, &pending.before_snapshot).await? {
            self.inner.store.finish_restore(&pending, false)?;
        } else {
            return Err(MaestroError::Conflict(format!("interrupted restore has additional file changes; recovery version {} is preserved at refs/maestro/versions/{}. Inspect the working files before recovery", pending.result.recovery_id, pending.result.recovery_id)));
        }
        Ok(())
    }

    async fn preview_inner(&self, id: &str) -> Result<(RestorePreview, String, String)> {
        let store = &self.inner.store;
        let target = store.history_version(id)?;
        let history = store.history()?;
        let current = history.current_id.as_ref().ok_or_else(|| {
            MaestroError::Conflict("save a project version before restoring".into())
        })?;
        let current = store.history_version(current)?;
        let (head, actual) = git::snapshot_working_tree(&self.inner.root).await?;
        let output = tokio::process::Command::new("git")
            .args([
                "merge-tree",
                "--write-tree",
                "--no-messages",
                &format!(
                    "--merge-base={}",
                    current
                        .logical_snapshot
                        .as_ref()
                        .unwrap_or(&current.snapshot)
                ),
                &actual,
                &target.snapshot,
            ])
            .current_dir(&self.inner.root)
            .stdin(Stdio::null())
            .output()
            .await?;
        let merged = String::from_utf8_lossy(&output.stdout);
        let merged_tree = merged.lines().next().unwrap_or("");
        let mut conflicts = Vec::new();
        if !output.status.success() {
            if output.status.code() != Some(1) {
                return Err(MaestroError::Git(
                    String::from_utf8_lossy(&output.stderr).into(),
                ));
            }
            // merge-tree lists stage entries after the tree OID.
            conflicts = merged
                .lines()
                .skip(1)
                .filter_map(|s| s.split_once('\t').map(|(_, p)| p.to_string()))
                .collect();
            conflicts.sort();
            conflicts.dedup();
            if conflicts.is_empty() {
                conflicts.push("Git could not merge this version with current changes".into());
            }
        }
        let resulting = if conflicts.is_empty() {
            commit_tree(&self.inner.root, merged_tree, &actual).await?
        } else {
            target.snapshot.clone()
        };
        let changed = git::git(
            &self.inner.root,
            &[
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                &actual,
                &resulting,
                "--",
            ],
        )
        .await?;
        for path in changed.split('\0').filter(|p| !p.is_empty()) {
            if excluded(path) {
                conflicts.push(format!(
                    "{path}: private/runtime paths are excluded from restore"
                ));
            }
        }
        let missions = store.list_missions()?;
        let affected_missions = missions
            .iter()
            .filter(|m| !m.status.is_terminal())
            .map(|m| m.id.clone())
            .collect::<Vec<_>>();
        let index = git::git(
            &self.inner.root,
            &[
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
            ],
        )
        .await?;
        let fingerprint = serde_json::json!([
            target.id,
            current.id,
            head,
            tree(&self.inner.root, &actual).await?,
            index,
            history.versions.first().map(|v| &v.id),
            missions
                .iter()
                .map(|m| (&m.id, m.status.as_str()))
                .collect::<Vec<_>>()
        ]);
        let token = hex::encode(Sha256::digest(fingerprint.to_string().as_bytes()));
        let unified = diff(&self.inner.root, &actual, &resulting).await?;
        Ok((
            RestorePreview {
                target,
                unified,
                conflicts,
                token,
                affected_missions,
            },
            actual,
            resulting,
        ))
    }

    pub async fn preview_restore(&self, id: &str) -> Result<RestorePreview> {
        let _workspace = self.inner.workspace_gate.write().await;
        self.ensure_history_recovered().await?;
        Ok(self.preview_inner(id).await?.0)
    }

    pub async fn restore_version(
        &self,
        id: &str,
        token: &str,
        undo: bool,
    ) -> Result<RestoreResult> {
        let _workspace = self.inner.workspace_gate.write().await;
        self.ensure_history_recovered().await?;
        if undo
            && self.inner.store.history_undo_target()?.id
                != self.inner.store.history_version(id)?.id
        {
            return Err(MaestroError::Conflict(
                "latest request changed; preview undo again".into(),
            ));
        }
        let (preview, before, after) = self.preview_inner(id).await?;
        if preview.token != token {
            return Err(MaestroError::Conflict(
                "project or history changed since preview; preview again".into(),
            ));
        }
        if !preview.conflicts.is_empty() {
            return Err(MaestroError::Conflict(format!(
                "restore conflicts: {}",
                preview.conflicts.join(", ")
            )));
        }
        crate::browser::stop_all(&self.inner.root);
        let recovery = self
            .capture_version(
                &self.inner.root,
                None,
                None,
                "recovery",
                &format!("Before restoring {}", preview.target.label),
                false,
            )
            .await?;
        let version = Version {
            id: new_id(),
            parent_id: self.inner.store.history()?.current_id,
            mission_id: None,
            task_id: None,
            kind: "restored".into(),
            label: format!(
                "{}: {}",
                if undo { "Undo" } else { "Restored" },
                preview.target.label
            ),
            snapshot: after.clone(),
            base_snapshot: before.clone(),
            logical_snapshot: Some(preview.target.snapshot.clone()),
            created_at: now_ms(),
        };
        self.pin_version(&version).await?;
        let result = RestoreResult {
            version,
            recovery_id: recovery.id,
            retired_missions: preview.affected_missions,
        };
        let patch = patch_bytes(&self.inner.root, &before, &after).await?;
        let pending = PendingRestore {
            result: result.clone(),
            before_snapshot: before,
            after_snapshot: after,
            undo_mission: if undo {
                preview.target.mission_id
            } else {
                None
            },
        };
        // Default git apply checks every hunk before writing any file, and
        // refuses symlink traversal/untracked collisions. It does not stage.
        if !patch.is_empty() {
            self.apply_history_patch(&patch, true).await?;
        }
        self.inner.store.prepare_restore(&pending)?;
        if !patch.is_empty() {
            if let Err(error) = self.apply_history_patch(&patch, false).await {
                self.ensure_history_recovered().await?;
                return Err(error);
            }
        }
        self.inner.store.finish_restore(&pending, true)?;
        for m in &result.retired_missions {
            self.inner.store.append_event(
                m,
                None,
                "mission.status",
                serde_json::json!({"status":"cancelled","reason":"superseded by code restore"}),
            )?;
            self.inner.store.append_event(
                m,
                None,
                "history.restored",
                serde_json::json!({"versionId":result.version.id,"recoveryId":result.recovery_id}),
            )?;
        }
        self.poke();
        Ok(result)
    }

    async fn apply_history_patch(&self, patch: &[u8], check: bool) -> Result<()> {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["apply", "--binary", "--whitespace=nowarn"]);
        if check {
            cmd.arg("--check");
        }
        let mut child = cmd
            .arg("-")
            .current_dir(&self.inner.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| MaestroError::Other("missing git stdin".into()))?
            .write_all(patch)
            .await?;
        let out = child.wait_with_output().await?;
        if !out.status.success() {
            return Err(MaestroError::Conflict(
                String::from_utf8_lossy(&out.stderr).into(),
            ));
        }
        Ok(())
    }
}
