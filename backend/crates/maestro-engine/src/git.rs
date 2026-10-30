//! Thin async wrappers over the `git` CLI (spawned directly, never through a
//! shell). Every failure surfaces as [`MaestroError::Git`] carrying stderr.

use maestro_types::{MaestroError, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Stdio;

/// Result of [`merge_no_ff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Merge commit created; carries the new HEAD sha.
    Merged(String),
    /// Merge conflicted (and was aborted); carries the conflicted paths.
    Conflict(Vec<String>),
}

/// Run git in `dir` and return trimmed-right stdout; non-zero exit becomes
/// [`MaestroError::Git`] with stderr attached.
pub async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| MaestroError::Git(format!("failed to spawn git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(MaestroError::Git(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

/// Current HEAD commit sha.
pub async fn head(repo: &Path) -> Result<String> {
    git(repo, &["rev-parse", "HEAD"]).await
}

/// Current branch name (`HEAD` when detached).
pub async fn current_branch(repo: &Path) -> Result<String> {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await
}

/// True when `git status --porcelain` reports nothing.
pub async fn is_clean(repo: &Path) -> Result<bool> {
    // Maestro's own state dir never counts as dirt: in repos that did not
    // gitignore it (no `maestro init`), an untracked `.maestro/` would
    // otherwise make finalize permanently impossible.
    let status = git(repo, &["status", "--porcelain"]).await?;
    Ok(status.lines().filter(|l| !l.is_empty()).all(|l| {
        l.get(3..)
            .unwrap_or("")
            .trim_start_matches('"')
            .starts_with(".maestro/")
    }))
}

/// Create `branch` pointing at `at` (no checkout).
pub async fn create_branch_at(repo: &Path, branch: &str, at: &str) -> Result<()> {
    git(repo, &["branch", branch, at]).await?;
    Ok(())
}

/// `git worktree add -b <branch> <path> <base>`.
pub async fn add_worktree(repo: &Path, path: &Path, branch: &str, base: &str) -> Result<()> {
    let path_str = path.to_string_lossy();
    git(
        repo,
        &["worktree", "add", "-b", branch, path_str.as_ref(), base],
    )
    .await?;
    Ok(())
}

/// `git worktree remove [--force] <path>`.
pub async fn remove_worktree(repo: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path_str.as_ref());
    git(repo, &args).await?;
    Ok(())
}

/// `git diff --binary <base>` with untracked files intent-added first, so new
/// files count in the diff (and in [`patch_hash`]).
pub async fn diff_binary(worktree: &Path, base_ref: &str) -> Result<String> {
    git(worktree, &["add", "--intent-to-add", "-A"]).await?;
    git(worktree, &["diff", "--binary", base_ref]).await
}

/// sha256 (hex) of the binary diff against `base_ref` — the patch identity
/// gates and approvals bind to.
pub async fn patch_hash(worktree: &Path, base_ref: &str) -> Result<String> {
    let diff = diff_binary(worktree, base_ref).await?;
    let mut hasher = Sha256::new();
    hasher.update(diff.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Changed paths versus `base_ref` (untracked files intent-added first).
/// `--no-renames` is load-bearing: rename detection reports only the NEW
/// path of a rename, which would hide an out-of-scope deletion from
/// scope verification when it pairs with an in-scope addition.
pub async fn diff_name_only(worktree: &Path, base_ref: &str) -> Result<Vec<String>> {
    git(worktree, &["add", "--intent-to-add", "-A"]).await?;
    let out = git(worktree, &["diff", "--name-only", "--no-renames", base_ref]).await?;
    Ok(out
        .lines()
        .map(str::to_string)
        .filter(|l| !l.is_empty())
        .collect())
}

/// Unified diff between two refs.
pub async fn unified_diff(repo: &Path, from: &str, to: &str) -> Result<String> {
    git(repo, &["diff", from, to]).await
}

/// `git merge --no-ff` of `branch` into the current checkout. On conflict the
/// merge is aborted and the conflicted paths are reported.
pub async fn merge_no_ff(worktree: &Path, branch: &str) -> Result<MergeOutcome> {
    match git(worktree, &["merge", "--no-ff", "--no-edit", branch]).await {
        Ok(_) => Ok(MergeOutcome::Merged(head(worktree).await?)),
        Err(merge_err) => {
            let conflicted = git(worktree, &["diff", "--name-only", "--diff-filter=U"])
                .await
                .unwrap_or_default();
            let files: Vec<String> = conflicted
                .lines()
                .map(str::to_string)
                .filter(|l| !l.is_empty())
                .collect();
            if files.is_empty() {
                return Err(merge_err);
            }
            // Best effort abort; the conflict is the primary signal.
            let _ = git(worktree, &["merge", "--abort"]).await;
            Ok(MergeOutcome::Conflict(files))
        }
    }
}

/// `git merge --ff-only <branch>`; returns the new HEAD sha.
pub async fn merge_ff_only(repo: &Path, branch: &str) -> Result<String> {
    git(repo, &["merge", "--ff-only", branch]).await?;
    head(repo).await
}

/// `git reset --hard <refname>`.
pub async fn reset_hard(worktree: &Path, refname: &str) -> Result<()> {
    git(worktree, &["reset", "--hard", refname]).await?;
    Ok(())
}

/// Resolve any ref to a commit sha.
pub async fn rev_parse(repo: &Path, refname: &str) -> Result<String> {
    git(repo, &["rev-parse", "--verify", refname]).await
}

/// One bounded string describing the repository for prompts: tracked files
/// (capped at `max_files`) plus porcelain status (capped at 50 lines).
pub async fn repo_summary(root: &Path, max_files: usize) -> Result<String> {
    let files = git(root, &["ls-files"]).await?;
    let all: Vec<&str> = files.lines().filter(|l| !l.is_empty()).collect();
    let shown = all.len().min(max_files);

    let mut summary = format!("Tracked files ({} shown of {}):\n", shown, all.len());
    for file in &all[..shown] {
        summary.push_str(file);
        summary.push('\n');
    }
    if all.len() > shown {
        summary.push_str("…[file list truncated]\n");
    }

    let status = git(root, &["status", "--porcelain"]).await?;
    if status.is_empty() {
        summary.push_str("Working tree: clean\n");
    } else {
        summary.push_str("Working tree status:\n");
        let mut lines = status.lines();
        for line in lines.by_ref().take(50) {
            summary.push_str(line);
            summary.push('\n');
        }
        if lines.next().is_some() {
            summary.push_str("…[status truncated]\n");
        }
    }
    Ok(summary)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Init a repo with identity configured and one initial commit.
    pub async fn init_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]).await.unwrap();
        git(dir, &["config", "user.email", "test@maestro.local"])
            .await
            .unwrap();
        git(dir, &["config", "user.name", "Maestro Test"])
            .await
            .unwrap();
        git(dir, &["config", "commit.gpgsign", "false"])
            .await
            .unwrap();
        std::fs::write(dir.join("README.md"), "# test repo\n").unwrap();
        git(dir, &["add", "-A"]).await.unwrap();
        git(dir, &["commit", "-q", "-m", "initial"]).await.unwrap();
    }

    pub async fn commit_all(dir: &Path, message: &str) {
        git(dir, &["add", "-A"]).await.unwrap();
        git(dir, &["commit", "-q", "-m", message]).await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[tokio::test]
    async fn head_branch_and_clean() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let sha = head(dir.path()).await.unwrap();
        assert_eq!(sha.len(), 40);
        assert_eq!(current_branch(dir.path()).await.unwrap(), "main");
        assert!(is_clean(dir.path()).await.unwrap());
        std::fs::write(dir.path().join("dirty.txt"), "x").unwrap();
        assert!(!is_clean(dir.path()).await.unwrap());
    }

    #[tokio::test]
    async fn maestro_state_dir_never_counts_as_dirty() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        // Untracked state dir (repo did not gitignore it): still clean.
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        std::fs::write(dir.path().join(".maestro/maestro.db"), "x").unwrap();
        assert!(is_clean(dir.path()).await.unwrap());
        // Real dirt alongside it is still reported.
        std::fs::write(dir.path().join("wip.txt"), "x").unwrap();
        assert!(!is_clean(dir.path()).await.unwrap());
    }

    #[tokio::test]
    async fn worktree_patch_hash_and_name_only() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let base = head(dir.path()).await.unwrap();

        let wt = dir.path().join("wt");
        add_worktree(dir.path(), &wt, "feature/x", &base)
            .await
            .unwrap();

        let empty_hash = patch_hash(&wt, &base).await.unwrap();
        // New untracked file counts via intent-to-add.
        std::fs::write(wt.join("new_file.rs"), "fn main() {}\n").unwrap();
        let changed_hash = patch_hash(&wt, &base).await.unwrap();
        assert_ne!(empty_hash, changed_hash, "hash must change with the diff");

        let names = diff_name_only(&wt, &base).await.unwrap();
        assert_eq!(names, vec!["new_file.rs".to_string()]);

        // Content change changes the hash again.
        std::fs::write(wt.join("new_file.rs"), "fn main() { let _x = 1; }\n").unwrap();
        let third_hash = patch_hash(&wt, &base).await.unwrap();
        assert_ne!(changed_hash, third_hash);

        remove_worktree(dir.path(), &wt, true).await.unwrap();
        assert!(!wt.exists());
    }

    #[tokio::test]
    async fn renames_report_both_paths() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/keep.md"), "unchanged content\n").unwrap();
        commit_all(dir.path(), "add docs").await;
        let base = head(dir.path()).await.unwrap();

        // Identical-content move: rename detection would collapse this to the
        // new path only, hiding the out-of-scope deletion from scope checks.
        git(dir.path(), &["mv", "docs/keep.md", "moved.md"])
            .await
            .unwrap();
        let names = diff_name_only(dir.path(), &base).await.unwrap();
        assert!(
            names.contains(&"docs/keep.md".to_string()),
            "deleted rename source must be reported: {names:?}"
        );
        assert!(names.contains(&"moved.md".to_string()));
    }

    #[tokio::test]
    async fn merge_no_ff_success_and_conflict() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        // Branch that edits README one way.
        git(dir.path(), &["checkout", "-q", "-b", "side"])
            .await
            .unwrap();
        std::fs::write(dir.path().join("README.md"), "side version\n").unwrap();
        commit_all(dir.path(), "side edit").await;
        git(dir.path(), &["checkout", "-q", "main"]).await.unwrap();

        // Clean merge first.
        match merge_no_ff(dir.path(), "side").await.unwrap() {
            MergeOutcome::Merged(commit) => assert_eq!(commit.len(), 40),
            other => panic!("expected Merged, got {other:?}"),
        }

        // Now build a real conflict.
        git(dir.path(), &["checkout", "-q", "-b", "left"])
            .await
            .unwrap();
        std::fs::write(dir.path().join("README.md"), "left version\n").unwrap();
        commit_all(dir.path(), "left edit").await;
        git(dir.path(), &["checkout", "-q", "main"]).await.unwrap();
        std::fs::write(dir.path().join("README.md"), "main version\n").unwrap();
        commit_all(dir.path(), "main edit").await;

        match merge_no_ff(dir.path(), "left").await.unwrap() {
            MergeOutcome::Conflict(files) => {
                assert_eq!(files, vec!["README.md".to_string()]);
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // Aborted: tree is clean again.
        assert!(is_clean(dir.path()).await.unwrap());
    }

    #[tokio::test]
    async fn merge_ff_only_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let base = head(dir.path()).await.unwrap();

        git(dir.path(), &["checkout", "-q", "-b", "ahead"])
            .await
            .unwrap();
        std::fs::write(dir.path().join("extra.txt"), "x\n").unwrap();
        commit_all(dir.path(), "ahead").await;
        let ahead = head(dir.path()).await.unwrap();
        git(dir.path(), &["checkout", "-q", "main"]).await.unwrap();

        let merged = merge_ff_only(dir.path(), "ahead").await.unwrap();
        assert_eq!(merged, ahead);

        reset_hard(dir.path(), &base).await.unwrap();
        assert_eq!(head(dir.path()).await.unwrap(), base);
    }

    #[tokio::test]
    async fn failures_carry_stderr() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        match rev_parse(dir.path(), "no-such-ref").await {
            Err(MaestroError::Git(msg)) => assert!(!msg.is_empty()),
            other => panic!("expected Git error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn repo_summary_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        commit_all(dir.path(), "files").await;
        let summary = repo_summary(dir.path(), 3).await.unwrap();
        assert!(summary.contains("3 shown of 6"));
        assert!(summary.contains("…[file list truncated]"));
        assert!(summary.contains("Working tree: clean"));
    }
}

/// Capture current files using a private index. Never stages, stashes, commits
/// on, or checks out the user's branch. Ignored files and runtime state stay out.
pub async fn snapshot_working_tree(repo: &Path) -> Result<(String, String)> {
    let base = head(repo).await?;
    let dir = repo.join(".maestro");
    std::fs::create_dir_all(&dir)?;
    let index = dir.join(format!("snapshot-index-{}", maestro_types::new_id()));
    async fn indexed(repo: &Path, index: &Path, args: &[&str]) -> Result<String> {
        let result = tokio::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_INDEX_FILE", index)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("GIT_AUTHOR_NAME", "Maestro Snapshot")
            .env("GIT_AUTHOR_EMAIL", "snapshot@localhost")
            .env("GIT_COMMITTER_NAME", "Maestro Snapshot")
            .env("GIT_COMMITTER_EMAIL", "snapshot@localhost")
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| MaestroError::Git(e.to_string()))?;
        if !result.status.success() {
            return Err(MaestroError::Git(
                String::from_utf8_lossy(&result.stderr).trim().into(),
            ));
        }
        Ok(String::from_utf8_lossy(&result.stdout).trim().into())
    }
    let result = async {
        indexed(repo, &index, &["read-tree", &base]).await?;
        let files = git(
            repo,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )
        .await?;
        let tracked = git(repo, &["ls-tree", "-r", "--name-only", "-z", &base]).await?;
        let paths: Vec<_> = files
            .split('\0')
            .filter(|p| !p.is_empty())
            .filter(|p| {
                let name = p.rsplit('/').next().unwrap_or(p);
                !p.split('/')
                    .any(|part| matches!(part, ".maestro" | ".git" | "node_modules"))
                    && (!(name == ".env" || name.starts_with(".env."))
                        || matches!(name, ".env.example" | ".env.sample" | ".env.template"))
                    && (std::fs::symlink_metadata(repo.join(p)).is_ok()
                        || tracked.split('\0').any(|old| old == *p))
            })
            .collect();
        if !paths.is_empty() {
            let path_list = index.with_extension("paths");
            std::fs::write(&path_list, format!("{}\0", paths.join("\0")))?;
            let staged = indexed(
                repo,
                &index,
                &[
                    "add",
                    "-A",
                    "--pathspec-from-file",
                    &path_list.to_string_lossy(),
                    "--pathspec-file-nul",
                ],
            )
            .await;
            let _ = std::fs::remove_file(path_list);
            staged?;
        }
        let tree = indexed(repo, &index, &["write-tree"]).await?;
        let base_tree = git(repo, &["rev-parse", &format!("{base}^{{tree}}")]).await?;
        let snapshot = if tree == base_tree {
            base.clone()
        } else {
            indexed(
                repo,
                &index,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &base,
                    "-m",
                    "Maestro private working-tree snapshot",
                ],
            )
            .await?
        };
        Ok((base, snapshot))
    }
    .await;
    let _ = std::fs::remove_file(&index);
    result
}
