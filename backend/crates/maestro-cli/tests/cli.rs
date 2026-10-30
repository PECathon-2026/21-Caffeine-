//! Integration tests for the `maestro` binary, spawning the built executable
//! (`CARGO_BIN_EXE_maestro`) against throwaway git repos. No daemon is
//! started anywhere here: everything exercises the local/in-process paths.

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_maestro")
}

fn maestro(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        // Hermetic: the developer's real ~/.config/maestro must never leak
        // into these tests (user-level config is the project-file fallback).
        .env("MAESTRO_CONFIG_DIR", root.join(".maestro-test-userdir"))
        .arg("--cwd")
        .arg(root)
        .args(args)
        .output()
        .expect("maestro binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_ok(out: &Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        stdout(out),
        stderr(out)
    );
}

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &Path) {
    sh(dir, &["init", "-b", "main"]);
    sh(dir, &["config", "user.name", "Test"]);
    sh(dir, &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.join("README.md"), "# test repo\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "// lib\n").unwrap();
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-m", "initial"]);
}

/// `maestro init`, then patch the default TOML for keyless mock E2E runs:
/// mock coder-writer model, sandbox preferred (bwrap may be absent in CI),
/// and the requested final-approval mode.
fn setup_project(dir: &Path, final_approval: bool) {
    init_repo(dir);
    let out = maestro(dir, &["init"]);
    assert_ok(&out, "init");
    let path = dir.join("maestro.toml");
    let mut toml = std::fs::read_to_string(&path).unwrap();
    assert!(
        toml.contains("model = \"coder\""),
        "default config shape changed:\n{toml}"
    );
    toml = toml.replace("model = \"coder\"", "model = \"coder-writer\"");
    toml = toml.replace("sandbox = \"required\"", "sandbox = \"preferred\"");
    if !final_approval {
        toml = toml.replace("finalApproval = true", "finalApproval = false");
    }
    assert!(
        toml.contains("sandbox = \"preferred\""),
        "sandbox patch failed:\n{toml}"
    );
    std::fs::write(&path, toml).unwrap();
    // Commit the config (as a user would) so integration's clean-tree check
    // sees a clean root.
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-m", "maestro init"]);
}

// ---------------------------------------------------------------------------

#[test]
fn version_fast_path_prints_semver() {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    let text = text.trim();
    let version = text.strip_prefix("maestro ").unwrap_or_else(|| {
        panic!("unexpected --version output: {text:?}");
    });
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "not a semver triple: {version:?}");
    for p in parts {
        p.parse::<u64>()
            .unwrap_or_else(|_| panic!("non-numeric semver part in {version:?}"));
    }
}

#[test]
fn version_startup_stays_fast() {
    // Generous CI headroom; the observed number on a dev machine is ~1-2ms.
    for _ in 0..5 {
        let started = Instant::now();
        let out = Command::new(bin()).arg("--version").output().unwrap();
        let elapsed = started.elapsed();
        assert!(out.status.success());
        assert!(
            elapsed < Duration::from_millis(500),
            "--version took {elapsed:?} (budget 500ms)"
        );
    }
}

#[test]
fn init_writes_config_and_gitignore_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let out = maestro(dir.path(), &["init"]);
    assert_ok(&out, "init");
    assert!(dir.path().join("maestro.toml").exists());
    let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(gitignore.lines().any(|l| l.trim() == ".maestro/"));

    // Second init: still ok, no duplicate ignore entry, config untouched.
    let before = std::fs::read_to_string(dir.path().join("maestro.toml")).unwrap();
    let out = maestro(dir.path(), &["init"]);
    assert_ok(&out, "second init");
    let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(
        gitignore
            .lines()
            .filter(|l| l.trim() == ".maestro/")
            .count(),
        1
    );
    let after = std::fs::read_to_string(dir.path().join("maestro.toml")).unwrap();
    assert_eq!(before, after);

    // JSON variant parses.
    let out = maestro(dir.path(), &["init", "--json"]);
    assert_ok(&out, "init --json");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["created"], serde_json::json!(false));
}

#[test]
fn run_local_completes_a_mock_mission() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    let out = maestro(
        dir.path(),
        &["run", "--local", "implement", "the", "feature"],
    );
    assert_ok(&out, "run --local");
    let text = stdout(&out);
    assert!(
        text.contains("completed"),
        "expected 'completed' in:\n{text}"
    );
    // The mock coder writes src/generated.txt and it merges to the user branch
    // (final approval disabled in this config).
    assert!(dir.path().join("src/generated.txt").exists());
}

#[test]
fn run_local_json_emits_machine_readable_outcome() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    let out = maestro(
        dir.path(),
        &["run", "--local", "--json", "implement the feature"],
    );
    assert_ok(&out, "run --local --json");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", stdout(&out)));
    assert_eq!(doc["status"], serde_json::json!("completed"));
    assert!(doc["missionId"]
        .as_str()
        .map(|s| s.len() == 32)
        .unwrap_or(false));
    assert!(doc["stats"]["modelCalls"].as_i64().unwrap_or(0) > 0);
}

#[test]
fn status_json_works_without_a_daemon() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    let out = maestro(dir.path(), &["status", "--json"]);
    assert_ok(&out, "status --json");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["daemon"]["running"], serde_json::json!(false));
    assert!(doc["missions"].is_array());
}

#[test]
fn doctor_warns_but_passes_with_preferred_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    let out = maestro(dir.path(), &["doctor", "--json"]);
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out))
        .unwrap_or_else(|e| panic!("doctor output not JSON ({e}):\n{}", stdout(&out)));
    // bwrap may be missing here: with sandbox=preferred that must be a warn,
    // never a failure.
    let checks = doc["checks"].as_array().unwrap();
    let sandbox = checks.iter().find(|c| c["name"] == "sandbox").unwrap();
    assert_ne!(sandbox["status"], serde_json::json!("fail"));
    assert_ok(&out, "doctor");
    assert_eq!(doc["ok"], serde_json::json!(true));

    // Human variant also exits 0.
    let out = maestro(dir.path(), &["doctor"]);
    assert_ok(&out, "doctor (human)");
}

#[test]
fn approval_flow_plan_diff_and_finalize_locally() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), true); // finalApproval = true

    let out = maestro(
        dir.path(),
        &["run", "--local", "--json", "implement the feature"],
    );
    assert_ok(&out, "run --local --json (final approval)");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["status"], serde_json::json!("awaiting_approval"));
    let mission_id = doc["missionId"].as_str().unwrap().to_string();
    // The integrate approval is not yet decided, so the file must not be on
    // the user branch yet.
    assert!(!dir.path().join("src/generated.txt").exists());

    // approvals --json lists the pending integrate approval.
    let out = maestro(dir.path(), &["approvals", "--json"]);
    assert_ok(&out, "approvals --json");
    let approvals: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let list = approvals.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["action"], serde_json::json!("integrate"));
    let approval_id = list[0]["id"].as_str().unwrap().to_string();

    // plan --json returns the stored plan.
    let out = maestro(dir.path(), &["plan", &mission_id, "--json"]);
    assert_ok(&out, "plan --json");
    let plan: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(plan["tasks"]
        .as_array()
        .map(|t| !t.is_empty())
        .unwrap_or(false));

    // diff --json carries the integration diff for the generated file.
    let out = maestro(dir.path(), &["diff", &mission_id, "--json"]);
    assert_ok(&out, "diff --json");
    let diff: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(diff["unified"].as_str().unwrap().contains("generated.txt"));

    // mission/tasks/events smoke over the local store.
    let out = maestro(dir.path(), &["mission", &mission_id, "--json"]);
    assert_ok(&out, "mission --json");
    let out = maestro(dir.path(), &["tasks", &mission_id, "--json"]);
    assert_ok(&out, "tasks --json");
    let tasks: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(tasks.as_array().unwrap().len() >= 4);
    let out = maestro(dir.path(), &["events", &mission_id, "--json"]);
    assert_ok(&out, "events --json");
    let events: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(!events.as_array().unwrap().is_empty());

    // approve → local fallback finalizes: the file lands on the user branch.
    let out = maestro(dir.path(), &["approve", &approval_id, "--json"]);
    assert_ok(&out, "approve");
    let decided: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(decided["status"], serde_json::json!("approved"));
    assert!(decided["finalized"]["commit"].is_string());
    assert!(dir.path().join("src/generated.txt").exists());

    let out = maestro(dir.path(), &["mission", &mission_id, "--json"]);
    assert_ok(&out, "mission after finalize");
    let detail: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(detail["mission"]["status"], serde_json::json!("completed"));
}

#[test]
fn pause_without_daemon_errors_clearly() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    let out = maestro(dir.path(), &["pause", "some-mission"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no daemon running"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn missing_config_gives_actionable_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = maestro(dir.path(), &["status"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("maestro init"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn usage_error_exits_two() {
    let out = Command::new(bin())
        .arg("definitely-not-a-command")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn bench_reports_all_ops() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    let out = maestro(dir.path(), &["bench", "--iterations", "50", "--json"]);
    assert_ok(&out, "bench");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["iterations"], serde_json::json!(50));
    let ops = doc["ops"].as_array().unwrap();
    let names: Vec<&str> = ops.iter().filter_map(|o| o["op"].as_str()).collect();
    for expected in [
        "event.append",
        "lease.acquire",
        "task.set_status",
        "policy.evaluate",
    ] {
        assert!(
            names.contains(&expected),
            "missing op {expected} in {names:?}"
        );
    }
    for op in ops {
        assert!(op["p50Us"].as_i64().is_some());
        assert!(op["p99Us"].as_i64().unwrap() >= op["p50Us"].as_i64().unwrap());
    }
}

// ---------------------------------------------------------------------------
// Regression tests for the audit findings (probe budget for read-only
// commands, run --local vs a live daemon).

/// Point `.maestro/daemon.json` at `url` as a (possibly stale) daemon record.
fn write_daemon_json(dir: &Path, url: &str) {
    let state = dir.join(".maestro");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("daemon.json"),
        format!(r#"{{"url":"{url}","pid":1,"generation":"daemon_fake","startedAt":0}}"#),
    )
    .unwrap();
}

/// Minimal always-healthy fake daemon: answers every request with a canned
/// /health JSON body, one request per connection.
fn spawn_fake_health_server() -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf); // drain the request line + headers
            let body = r#"{"ok":true,"version":"0.0.0","generation":"daemon_fake","pid":1,"startedAt":0,"lastSeq":0}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    url
}

/// Finding: `run --local` while a daemon is alive would race two
/// orchestrators on one workspace — it must refuse instead.
#[test]
fn run_local_refuses_when_a_daemon_is_alive() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    let url = spawn_fake_health_server();
    write_daemon_json(dir.path(), &url);

    let out = maestro(dir.path(), &["run", "--local", "implement the feature"]);
    assert_eq!(out.status.code(), Some(1), "stdout:\n{}", stdout(&out));
    assert!(
        stderr(&out).contains("a daemon is running"),
        "stderr: {}",
        stderr(&out)
    );
    // Nothing ran locally.
    assert!(!dir.path().join("src/generated.txt").exists());
}

/// Finding: a stale daemon.json pointing at a filtered/unresponsive address
/// must not stall read-only commands for the full 3s mutating budget — they
/// probe with a 500ms budget and fall back to the local store.
#[test]
fn stale_daemon_json_does_not_stall_read_commands() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path(), false);
    // Listening but never responding: connects succeed, /health hangs.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    write_daemon_json(dir.path(), &url);

    let started = Instant::now();
    let out = maestro(dir.path(), &["status", "--json"]);
    let elapsed = started.elapsed();
    assert_ok(&out, "status with a stale daemon.json");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["daemon"]["running"], serde_json::json!(false));
    assert!(
        elapsed < Duration::from_millis(2500),
        "read-only probe must give up in ~500ms, took {elapsed:?}"
    );
    drop(listener);
}
