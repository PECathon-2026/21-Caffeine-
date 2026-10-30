use maestro_engine::{git, Orchestrator};
use maestro_providers::ProviderRegistry;
use maestro_store::Store;
use maestro_types::{
    config::MaestroConfig,
    domain::MissionStatus,
    history::{PendingRestore, RestoreResult},
};
use std::{path::Path, process::Command, sync::Arc};

fn sh(root: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}
fn setup() -> (tempfile::TempDir, Orchestrator, Store) {
    let d = tempfile::tempdir().unwrap();
    sh(d.path(), &["init", "-b", "main"]);
    sh(d.path(), &["config", "user.name", "Test"]);
    sh(d.path(), &["config", "user.email", "test@local"]);
    std::fs::write(
        d.path().join(".gitignore"),
        ".maestro/\n.env\nnode_modules/\n",
    )
    .unwrap();
    std::fs::write(
        d.path().join("app.txt"),
        (0..30).map(|i| format!("line {i}\n")).collect::<String>(),
    )
    .unwrap();
    std::fs::write(d.path().join("staged.txt"), "original\n").unwrap();
    sh(d.path(), &["add", "."]);
    sh(d.path(), &["commit", "-qm", "Initial"]);
    let store = Store::open(&d.path().join(".maestro")).unwrap();
    let config = MaestroConfig::default();
    let registry = Arc::new(ProviderRegistry::new(&config));
    let orch = Orchestrator::new(
        store.clone(),
        config,
        registry,
        d.path().into(),
        "test".into(),
    );
    (d, orch, store)
}
fn replace(root: &Path, old: &str, new: &str) {
    let p = root.join("app.txt");
    let s = std::fs::read_to_string(&p).unwrap();
    std::fs::write(p, s.replace(old, new)).unwrap();
}

#[tokio::test]
async fn undo_preserves_manual_edits_index_head_and_can_restore_forward() {
    let (d, o, s) = setup();
    let root = d.path();
    let m = o.create_mission("Improve app").await.unwrap();
    replace(root, "line 0\n", "AI change\n");
    let forward = o.save_code_version("AI result").await.unwrap();
    replace(root, "line 29\n", "manual change\n");
    std::fs::write(root.join("staged.txt"), "staged version\n").unwrap();
    sh(root, &["add", "staged.txt"]);
    std::fs::write(root.join("staged.txt"), "working version\n").unwrap();
    std::fs::write(root.join("manual.txt"), "keep me\n").unwrap();
    std::fs::write(root.join(".env"), "PRIVATE=keep\n").unwrap();
    let head = sh(root, &["rev-parse", "HEAD"]);
    let index = sh(root, &["diff", "--cached", "--binary"]);
    let target = s.history_undo_target().unwrap();
    let p = o.preview_restore(&target.id).await.unwrap();
    assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
    assert!(p.unified.contains("-AI change"));
    assert!(std::fs::read_to_string(root.join("app.txt"))
        .unwrap()
        .starts_with("AI change"));
    let result = o.restore_version(&target.id, &p.token, true).await.unwrap();
    let actual = std::fs::read_to_string(root.join("app.txt")).unwrap();
    assert!(actual.starts_with("line 0\n"));
    assert!(actual.ends_with("manual change\n"));
    assert_eq!(sh(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(sh(root, &["diff", "--cached", "--binary"]), index);
    assert_eq!(
        std::fs::read_to_string(root.join("staged.txt")).unwrap(),
        "working version\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("manual.txt")).unwrap(),
        "keep me\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".env")).unwrap(),
        "PRIVATE=keep\n"
    );
    assert_eq!(
        s.get_mission(&m.id).unwrap().status,
        MissionStatus::Cancelled
    );
    assert!(s.assert_history_mission_current(&m.id).is_err());
    assert!(s.history_undo_target().is_err());
    assert!(sh(
        root,
        &[
            "show",
            &format!("refs/maestro/versions/{}:app.txt", result.recovery_id)
        ]
    )
    .starts_with("AI change"));
    let p = o.preview_restore(&forward.id).await.unwrap();
    assert!(p.conflicts.is_empty());
    o.restore_version(&forward.id, &p.token, false)
        .await
        .unwrap();
    let actual = std::fs::read_to_string(root.join("app.txt")).unwrap();
    assert!(actual.starts_with("AI change"));
    assert!(actual.ends_with("manual change\n"));
}

#[tokio::test]
async fn conflicts_and_stale_previews_never_change_files_or_stop_missions() {
    let (d, o, s) = setup();
    let m = o.create_mission("Change line").await.unwrap();
    let target = s.history_undo_target().unwrap();
    replace(d.path(), "line 0\n", "AI\n");
    o.save_code_version("AI result").await.unwrap();
    let p = o.preview_restore(&target.id).await.unwrap();
    replace(d.path(), "AI\n", "manual conflicting value\n");
    assert!(o
        .restore_version(&target.id, &p.token, true)
        .await
        .unwrap_err()
        .to_string()
        .contains("changed since preview"));
    let conflict = o.preview_restore(&target.id).await.unwrap();
    assert!(!conflict.conflicts.is_empty());
    assert!(o
        .restore_version(&target.id, &conflict.token, true)
        .await
        .unwrap_err()
        .to_string()
        .contains("restore conflicts"));
    assert!(std::fs::read_to_string(d.path().join("app.txt"))
        .unwrap()
        .starts_with("manual conflicting"));
    assert_eq!(
        s.get_mission(&m.id).unwrap().status,
        MissionStatus::Planning
    );
    assert!(s.pending_restore().unwrap().is_none());
}

#[tokio::test]
async fn versions_restore_new_deleted_binary_and_unicode_paths_after_gc() {
    let (d, o, _) = setup();
    let root = d.path();
    std::fs::write(root.join("旧 file.bin"), [0, 255, 1, 0, 2]).unwrap();
    let old = o.save_code_version("Before replacement").await.unwrap();
    std::fs::remove_file(root.join("旧 file.bin")).unwrap();
    std::fs::write(root.join("new.bin"), [0, 3, 254]).unwrap();
    o.save_code_version("After replacement").await.unwrap();
    sh(root, &["gc", "--prune=now"]);
    let p = o.preview_restore(&old.id).await.unwrap();
    assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
    o.restore_version(&old.id, &p.token, false).await.unwrap();
    assert_eq!(
        std::fs::read(root.join("旧 file.bin")).unwrap(),
        [0, 255, 1, 0, 2]
    );
    assert!(!root.join("new.bin").exists());
}

#[tokio::test]
async fn crash_journal_reconciles_applied_and_unapplied_restores() {
    for applied in [false, true] {
        let (d, o, s) = setup();
        let before = o.save_code_version("before").await.unwrap();
        replace(d.path(), "line 0\n", "after\n");
        let after = o.save_code_version("after").await.unwrap();
        if !applied {
            replace(d.path(), "after\n", "line 0\n");
        }
        let pending = PendingRestore {
            result: RestoreResult {
                version: maestro_types::history::Version {
                    id: maestro_types::new_id(),
                    kind: "restored".into(),
                    ..after.clone()
                },
                recovery_id: before.id.clone(),
                retired_missions: vec![],
            },
            before_snapshot: before.snapshot.clone(),
            after_snapshot: after.snapshot.clone(),
            undo_mission: None,
        };
        s.prepare_restore(&pending).unwrap();
        let history = o.code_history().await.unwrap();
        assert!(s.pending_restore().unwrap().is_none());
        assert_eq!(
            history
                .versions
                .iter()
                .any(|v| v.id == pending.result.version.id),
            applied
        );
    }
}

#[tokio::test]
async fn successive_request_undos_keep_manual_overlay() {
    let (d, o, s) = setup();
    let root = d.path();
    o.create_mission("first").await.unwrap();
    replace(root, "line 0\n", "first\n");
    o.save_code_version("first result").await.unwrap();
    o.create_mission("second").await.unwrap();
    replace(root, "line 10\n", "second\n");
    o.save_code_version("second result").await.unwrap();
    replace(root, "line 29\n", "manual\n");
    for _ in 0..2 {
        let v = s.history_undo_target().unwrap();
        let p = o.preview_restore(&v.id).await.unwrap();
        assert!(p.conflicts.is_empty());
        o.restore_version(&v.id, &p.token, true).await.unwrap();
    }
    let contents = std::fs::read_to_string(root.join("app.txt")).unwrap();
    assert!(contents.starts_with("line 0\n"));
    assert!(contents.contains("line 10\n"));
    assert!(contents.ends_with("manual\n"));
}

#[tokio::test]
async fn saved_draft_survives_discarded_worktree_and_can_be_restored() {
    let (d, o, s) = setup();
    let root = d.path();
    let initial = o.save_code_version("Initial").await.unwrap();
    let wt = d.path().join(".maestro/draft");
    git::add_worktree(root, &wt, "draft-test", "HEAD")
        .await
        .unwrap();
    std::fs::write(wt.join("app.txt"), "draft result\n").unwrap();
    let (_, snapshot) = git::snapshot_working_tree(&wt).await.unwrap();
    let v = maestro_types::history::Version {
        id: maestro_types::new_id(),
        kind: "draft".into(),
        label: "Draft edit".into(),
        snapshot,
        base_snapshot: initial.snapshot.clone(),
        ..initial
    };
    git::git(
        root,
        &[
            "update-ref",
            &format!("refs/maestro/versions/{}", v.id),
            &v.snapshot,
        ],
    )
    .await
    .unwrap();
    s.insert_version(&v, false).unwrap();
    git::remove_worktree(root, &wt, true).await.unwrap();
    sh(root, &["branch", "-D", "draft-test"]);
    sh(root, &["gc", "--prune=now"]);
    let p = o.preview_restore(&v.id).await.unwrap();
    assert!(p.conflicts.is_empty());
    o.restore_version(&v.id, &p.token, false).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("app.txt")).unwrap(),
        "draft result\n"
    );
}

#[tokio::test]
async fn ignored_collision_cannot_be_overwritten_by_restore() {
    let (d, o, s) = setup();
    let root = d.path();
    std::fs::write(root.join("local.txt"), "saved value\n").unwrap();
    let old = o.save_code_version("saved file").await.unwrap();
    std::fs::remove_file(root.join("local.txt")).unwrap();
    std::fs::write(root.join(".gitignore"), ".maestro/\nlocal.txt\n").unwrap();
    o.save_code_version("removed file").await.unwrap();
    std::fs::write(root.join("local.txt"), "new ignored personal value\n").unwrap();
    let p = o.preview_restore(&old.id).await.unwrap();
    assert!(o.restore_version(&old.id, &p.token, false).await.is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("local.txt")).unwrap(),
        "new ignored personal value\n"
    );
    assert!(s.pending_restore().unwrap().is_none());
}

#[tokio::test]
async fn ambiguous_crash_keeps_files_and_recovery_journal_for_inspection() {
    let (d, o, s) = setup();
    let before = o.save_code_version("before").await.unwrap();
    replace(d.path(), "line 0\n", "after\n");
    let after = o.save_code_version("after").await.unwrap();
    let pending = PendingRestore {
        result: RestoreResult {
            version: after.clone(),
            recovery_id: before.id.clone(),
            retired_missions: vec![],
        },
        before_snapshot: before.snapshot.clone(),
        after_snapshot: after.snapshot.clone(),
        undo_mission: None,
    };
    s.prepare_restore(&pending).unwrap();
    replace(d.path(), "after\n", "partially changed manually\n");
    let e = o.code_history().await.unwrap_err().to_string();
    assert!(e.contains(&before.id));
    assert!(s.pending_restore().unwrap().is_some());
    assert!(std::fs::read_to_string(d.path().join("app.txt"))
        .unwrap()
        .starts_with("partially changed manually"));
}

#[tokio::test]
async fn restores_non_utf8_text_and_dangling_symlinks_without_following_them() {
    let (d, o, _) = setup();
    let root = d.path();
    std::fs::write(root.join("bytes.txt"), [255, 254, 65, 10]).unwrap();
    std::os::unix::fs::symlink("missing-target", root.join("dangling")).unwrap();
    let old = o.save_code_version("raw bytes and symlink").await.unwrap();
    std::fs::write(root.join("bytes.txt"), [253, 65, 10]).unwrap();
    std::fs::remove_file(root.join("dangling")).unwrap();
    o.save_code_version("updated bytes").await.unwrap();
    let p = o.preview_restore(&old.id).await.unwrap();
    assert!(p.conflicts.is_empty());
    o.restore_version(&old.id, &p.token, false).await.unwrap();
    assert_eq!(
        std::fs::read(root.join("bytes.txt")).unwrap(),
        [255, 254, 65, 10]
    );
    assert_eq!(
        std::fs::read_link(root.join("dangling"))
            .unwrap()
            .to_string_lossy(),
        "missing-target"
    );
    assert!(!root.join("missing-target").exists());
}

#[test]
fn backup_only_requests_use_chat_but_compound_edits_keep_the_mission() {
    use maestro_engine::looks_like_implementation_request as mission;
    for text in [
        "save this as backup",
        "Can you save a checkpoint named working version?",
        "please back up the current project",
        "make a backup",
        "save a version before I change it",
    ] {
        assert!(!mission(text), "should use chat: {text}");
    }
    for text in [
        "save this as backup then redesign the homepage",
        "back up the project before refactoring the app",
        "save a checkpoint and fix the tests",
        "build a backup tool",
        "create a backup feature",
        "save this file to src/app.ts",
    ] {
        assert!(mission(text), "should use a mission: {text}");
    }
}

#[tokio::test]
async fn architect_backup_tool_saves_current_files_and_returns_real_version_id() {
    let (d, o, s) = setup();
    std::fs::write(
        d.path().join("uncommitted.txt"),
        "backup this current content\n",
    )
    .unwrap();
    let index = sh(d.path(), &["diff", "--cached", "--binary"]);
    let answer = o
        .answer_question(
            "ARCHITECT-BACKUP-PROBE save this as backup",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    let h = s.history().unwrap();
    let v = &h.versions[0];
    assert_eq!(v.label, "Before experiment");
    assert!(answer.contains(&v.id), "{answer}");
    assert_eq!(
        sh(
            d.path(),
            &["show", &format!("{}:uncommitted.txt", v.snapshot)]
        ),
        "backup this current content\n"
    );
    assert_eq!(sh(d.path(), &["diff", "--cached", "--binary"]), index);
    assert!(s.list_missions().unwrap().is_empty());
}

#[tokio::test]
async fn working_agent_backup_tool_records_workspace_before_editing() {
    let (d, _, s) = setup();
    let mut config = MaestroConfig::default();
    config.safety.sandbox = maestro_types::config::EnforcementMode::Preferred;
    config.roles.get_mut("coder").unwrap().model.model = "coder-writer-backup".into();
    let registry = Arc::new(ProviderRegistry::new(&config));
    let o = Orchestrator::new(
        s.clone(),
        config,
        registry,
        d.path().into(),
        "backup-test".into(),
    );
    let m = o
        .create_mission("save a backup then implement the feature")
        .await
        .unwrap();
    o.run_mission(&m.id, &tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let history = s.history().unwrap();
    let saved = history
        .versions
        .iter()
        .find(|v| v.kind == "backup")
        .unwrap();
    assert_eq!(saved.label, "Before coder edits");
    assert!(saved.task_id.is_some());
    assert!(
        !sh(d.path(), &["ls-tree", "-r", "--name-only", &saved.snapshot])
            .contains("src/generated.txt")
    );
    assert!(history.versions.iter().any(|v| v.kind == "draft"
        && sh(d.path(), &["ls-tree", "-r", "--name-only", &v.snapshot])
            .contains("src/generated.txt")));
    assert_ne!(
        history.current_id.as_deref(),
        Some(saved.id.as_str()),
        "task backup must not change project baseline"
    );
}
