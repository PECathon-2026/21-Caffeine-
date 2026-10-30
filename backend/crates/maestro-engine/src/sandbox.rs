//! Bubblewrap-backed check execution: network off by default, clean
//! environment, `.env*` files masked with `/dev/null`, timeout and output
//! clipping enforced. `Required` mode fails closed when `bwrap` is missing;
//! `Preferred` falls back to host execution with a flag the caller can
//! surface as a warning event.

use maestro_types::config::EnforcementMode;
use maestro_types::{MaestroError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

/// Directory names never traversed when scanning for `.env*` files.
const SKIP_DIRS: [&str; 4] = [".git", ".maestro", "node_modules", "target"];

/// How to run one sandboxed check.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// The only writable bind; also the working directory inside the sandbox.
    pub cwd: PathBuf,
    /// When false (default), the sandbox gets `--unshare-net`.
    pub allow_network: bool,
    /// Env vars copied from the host into the sandbox (PATH is always set).
    pub env_allowlist: Vec<String>,
    /// Wall-clock limit; the child is killed when it elapses.
    pub timeout_ms: i64,
    /// Per-stream clip limit, with a truncation marker appended.
    pub max_output_chars: usize,
    /// `Required` fails closed without bwrap; `Preferred` falls back to host.
    pub mode: EnforcementMode,
    /// Test hook: use this path instead of resolving `bwrap` from `$PATH`.
    /// A nonexistent override simulates a missing binary.
    pub bwrap_path_override: Option<PathBuf>,
}

impl SandboxSpec {
    /// A locked-down default spec for `cwd`: no network, empty allowlist,
    /// 5 minute timeout, 20k char clip, `Required` enforcement.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        SandboxSpec {
            cwd: cwd.into(),
            allow_network: false,
            env_allowlist: Vec::new(),
            timeout_ms: 300_000,
            max_output_chars: 20_000,
            mode: EnforcementMode::Required,
            bwrap_path_override: None,
        }
    }
}

/// Outcome of one check run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    /// Process exit code; `-1` when killed by signal or timed out.
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// True when either stream was clipped (or output was lost to a timeout).
    pub truncated: bool,
    pub duration_ms: i64,
    /// True when the check ran on the host because bwrap was unavailable
    /// under `Preferred` enforcement.
    pub fallback: bool,
}

impl CheckResult {
    /// Convenience: exit code zero.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Run `argv` (no shell) under bubblewrap according to `spec`.
///
/// Fails with [`MaestroError::Sandbox`] when `argv` is empty, when bwrap is
/// missing under `Required` mode, or when the process cannot be spawned.
/// A timeout is reported as a `CheckResult` with `exit_code == -1`.
pub async fn run_check(spec: &SandboxSpec, argv: &[String]) -> Result<CheckResult> {
    let Some(program) = argv.first() else {
        return Err(MaestroError::Sandbox("empty check command".into()));
    };

    match resolve_bwrap(spec) {
        Some(bwrap) => {
            let args = bwrap_args(spec)?;
            let mut cmd = Command::new(bwrap);
            cmd.args(&args).args(argv);
            exec(spec, cmd, false).await
        }
        None => match spec.mode {
            EnforcementMode::Required => Err(MaestroError::Sandbox(
                "bwrap not found on PATH and sandbox mode is 'required'".into(),
            )),
            EnforcementMode::Preferred => {
                tracing::warn!("bwrap unavailable; running check on host (sandbox=preferred)");
                let mut cmd = Command::new(program);
                cmd.args(&argv[1..]);
                cmd.env_clear();
                cmd.env("PATH", "/usr/bin:/bin");
                for key in &spec.env_allowlist {
                    if let Some(val) = std::env::var_os(key) {
                        cmd.env(key, val);
                    }
                }
                exec(spec, cmd, true).await
            }
        },
    }
}

/// Resolve the bwrap binary: the override when set (missing override = no
/// bwrap, for fail-closed tests), otherwise a which-style walk of `$PATH`.
pub fn resolve_bwrap(spec: &SandboxSpec) -> Option<PathBuf> {
    if let Some(overridden) = &spec.bwrap_path_override {
        return is_executable(overridden).then(|| overridden.clone());
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("bwrap");
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// Build the bwrap argument vector (everything before the check argv).
pub(crate) fn bwrap_args(spec: &SandboxSpec) -> Result<Vec<String>> {
    let cwd = spec
        .cwd
        .canonicalize()
        .map_err(|e| MaestroError::Sandbox(format!("bad sandbox cwd: {e}")))?;
    let cwd_str = cwd.to_string_lossy().into_owned();

    let mut args: Vec<String> = vec![
        "--die-with-parent".into(),
        "--new-session".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
    ];
    if !spec.allow_network {
        args.push("--unshare-net".into());
    }
    for dir in ["/usr", "/bin", "/lib", "/lib64"] {
        if Path::new(dir).exists() {
            args.extend(["--ro-bind".into(), dir.into(), dir.into()]);
        }
    }
    args.extend(["--tmpfs".into(), "/tmp".into()]);
    args.extend(["--dir".into(), "/tmp/maestro-home".into()]);
    args.extend([
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
    ]);
    args.extend(["--bind".into(), cwd_str.clone(), cwd_str.clone()]);
    // A task worktree deliberately excludes installed dependencies. Expose the
    // closest ancestor node_modules read-only at its original resolution path.
    let deps = cwd.ancestors().map(|p| p.join("node_modules")).find(|p| {
        p.is_dir() && std::fs::read_dir(p).is_ok_and(|mut entries| entries.next().is_some())
    });
    if let Some(deps) = deps {
        let source = deps
            .canonicalize()
            .map_err(|e| MaestroError::Sandbox(e.to_string()))?;
        args.extend([
            "--ro-bind".into(),
            source.to_string_lossy().into_owned(),
            source.to_string_lossy().into_owned(),
        ]);
        // Turbopack resolves only inside its lockfile workspace root. Give
        // worktrees a local, read-only dependency mount as well as the
        // original path needed by absolute imports and package-manager links.
        let local_deps = cwd.join("node_modules");
        if local_deps != deps && !local_deps.is_symlink() {
            args.extend([
                "--ro-bind".into(),
                source.to_string_lossy().into_owned(),
                local_deps.to_string_lossy().into_owned(),
            ]);
        }
        if source != deps && !deps.starts_with(&cwd) {
            args.extend([
                "--symlink".into(),
                source.to_string_lossy().into_owned(),
                deps.to_string_lossy().into_owned(),
            ]);
        }
    }
    for path in [
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/ssl/certs",
    ] {
        if Path::new(path).exists() {
            args.extend(["--ro-bind".into(), path.into(), path.into()]);
        }
    }
    args.extend(["--chdir".into(), cwd_str]);
    args.push("--clearenv".into());
    args.extend(["--setenv".into(), "PATH".into(), "/usr/bin:/bin".into()]);
    args.extend(["--setenv".into(), "HOME".into(), "/tmp/maestro-home".into()]);
    args.extend([
        "--setenv".into(),
        "XDG_CACHE_HOME".into(),
        "/tmp/maestro-home/.cache".into(),
    ]);
    args.extend([
        "--setenv".into(),
        "NEXT_TELEMETRY_DISABLED".into(),
        "1".into(),
    ]);
    for key in &spec.env_allowlist {
        if let Some(val) = std::env::var_os(key) {
            args.extend([
                "--setenv".into(),
                key.clone(),
                val.to_string_lossy().into_owned(),
            ]);
        }
    }
    // `--dev-bind` (not `--ro-bind`): unprivileged user namespaces force
    // `nodev` on plain binds, which would make the masked file unreadable
    // (EACCES) instead of empty. Reads yield nothing, writes are discarded.
    for masked in env_files_under(&cwd) {
        args.extend([
            "--dev-bind".into(),
            "/dev/null".into(),
            masked.to_string_lossy().into_owned(),
        ]);
    }
    Ok(args)
}

/// Every `.env` / `.env.*` file (except `.env.example`) under `root`,
/// skipping `.git`, `.maestro`, `node_modules`, and `target` directories.
fn env_files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else if crate::policy::is_secret_file(&name) {
                found.push(path);
            }
        }
    }
    found
}

async fn exec(spec: &SandboxSpec, mut cmd: Command, fallback: bool) -> Result<CheckResult> {
    cmd.current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let start = Instant::now();
    let child = cmd
        .spawn()
        .map_err(|e| MaestroError::Sandbox(format!("failed to spawn check: {e}")))?;
    let timeout = std::time::Duration::from_millis(spec.timeout_ms.max(1) as u64);

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let (stdout, out_clip) = clip_chars(
                &String::from_utf8_lossy(&output.stdout),
                spec.max_output_chars,
            );
            let (stderr, err_clip) = clip_chars(
                &String::from_utf8_lossy(&output.stderr),
                spec.max_output_chars,
            );
            Ok(CheckResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout,
                stderr,
                truncated: out_clip || err_clip,
                duration_ms: start.elapsed().as_millis() as i64,
                fallback,
            })
        }
        Ok(Err(e)) => Err(MaestroError::Sandbox(format!("check wait failed: {e}"))),
        // Dropping the wait future kills the child (kill_on_drop).
        Err(_) => Ok(CheckResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: format!(
                "check timed out after {} ms and was killed",
                spec.timeout_ms
            ),
            truncated: true,
            duration_ms: start.elapsed().as_millis() as i64,
            fallback,
        }),
    }
}

/// Clip to `max` characters (char-boundary safe) with a truncation marker.
fn clip_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    let clipped: String = text.chars().take(max).collect();
    (format!("{clipped}\n…[output truncated]"), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(dir: &Path) -> SandboxSpec {
        SandboxSpec::new(dir)
    }

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn bwrap_present() -> bool {
        let available = resolve_bwrap(&SandboxSpec::new("/")).is_some();
        if !available {
            eprintln!("skipping: bwrap not installed");
        }
        available
    }

    #[tokio::test]
    async fn fails_closed_when_required_and_bwrap_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = spec(dir.path());
        spec.bwrap_path_override = Some(PathBuf::from("/nonexistent/bwrap"));
        spec.mode = EnforcementMode::Required;
        match run_check(&spec, &argv(&["true"])).await {
            Err(MaestroError::Sandbox(_)) => {}
            other => panic!("expected Sandbox error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn preferred_falls_back_to_host() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = spec(dir.path());
        spec.bwrap_path_override = Some(PathBuf::from("/nonexistent/bwrap"));
        spec.mode = EnforcementMode::Preferred;
        let result = run_check(&spec, &argv(&["echo", "hello"])).await.unwrap();
        assert!(result.fallback);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn output_is_clipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = spec(dir.path());
        spec.bwrap_path_override = Some(PathBuf::from("/nonexistent/bwrap"));
        spec.mode = EnforcementMode::Preferred;
        spec.max_output_chars = 5;
        let result = run_check(&spec, &argv(&["echo", "0123456789"]))
            .await
            .unwrap();
        assert!(result.truncated);
        assert!(result.stdout.starts_with("01234"));
        assert!(result.stdout.contains("[output truncated]"));
    }

    #[tokio::test]
    async fn empty_argv_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            run_check(&spec(dir.path()), &[]).await,
            Err(MaestroError::Sandbox(_))
        ));
    }

    #[tokio::test]
    async fn env_files_are_masked_inside_sandbox() {
        if !bwrap_present() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=topsecret\n").unwrap();
        std::fs::write(dir.path().join(".env.example"), "EXAMPLE=ok\n").unwrap();
        let spec = spec(dir.path());

        let result = run_check(&spec, &argv(&["cat", ".env"])).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "", "masked .env must read as empty");
        assert!(!result.stdout.contains("topsecret"));

        let result = run_check(&spec, &argv(&["cat", ".env.example"]))
            .await
            .unwrap();
        assert!(result.stdout.contains("EXAMPLE=ok"));
    }

    #[tokio::test]
    async fn nested_env_files_are_masked() {
        if !bwrap_present() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/config")).unwrap();
        std::fs::write(dir.path().join("app/config/.env.production"), "K=v\n").unwrap();
        let spec = spec(dir.path());
        let result = run_check(&spec, &argv(&["cat", "app/config/.env.production"]))
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
        assert_eq!(result.stdout, "");
    }

    #[tokio::test]
    async fn timeout_kills_the_child() {
        if !bwrap_present() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut spec = spec(dir.path());
        spec.timeout_ms = 300;
        let start = Instant::now();
        let result = run_check(&spec, &argv(&["sleep", "10"])).await.unwrap();
        assert_eq!(result.exit_code, -1);
        assert!(result.stderr.contains("timed out"));
        assert!(
            start.elapsed().as_secs() < 5,
            "child was not killed promptly"
        );
    }

    #[tokio::test]
    async fn node_gets_a_private_home_and_ancestor_dependencies() {
        if !bwrap_present() || !Path::new("/usr/bin/node").exists() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("node_modules/demo")).unwrap();
        std::fs::write(
            root.join("node_modules/demo/index.js"),
            "module.exports = 'dependency-found';",
        )
        .unwrap();
        let workspace = root.join(".maestro/worktrees/test");
        std::fs::create_dir_all(&workspace).unwrap();
        let result=run_check(&spec(&workspace),&argv(&["node","-e","console.log(require('os').homedir()); console.log(require('demo')); if (!require.resolve('demo').startsWith(process.cwd() + '/node_modules/')) throw new Error('Dependency escaped workspace root'); require('fs').writeFileSync(process.env.HOME+'/cache','ok')"])).await.unwrap();
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert!(result.stdout.contains("/tmp/maestro-home"));
        assert!(result.stdout.contains("dependency-found"));
        assert!(!root.join("cache").exists());
        // Bubblewrap leaves an empty mountpoint in the writable worktree.
        // A second invocation must still locate the actual ancestor install.
        let again = run_check(
            &spec(&workspace),
            &argv(&["node", "-e", "console.log(require('demo'))"]),
        )
        .await
        .unwrap();
        assert_eq!(again.exit_code, 0, "{}", again.stderr);
        assert!(again.stdout.contains("dependency-found"));
    }

    #[tokio::test]
    async fn basic_exec_inside_sandbox() {
        if !bwrap_present() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hi there").unwrap();
        let result = run_check(&spec(dir.path()), &argv(&["cat", "hello.txt"]))
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(!result.fallback);
        assert_eq!(result.stdout, "hi there");
    }

    #[test]
    fn clip_is_char_boundary_safe() {
        let (clipped, truncated) = clip_chars("héllo wörld", 3);
        assert!(truncated);
        assert!(clipped.starts_with("hél"));
        let (same, truncated) = clip_chars("short", 100);
        assert!(!truncated);
        assert_eq!(same, "short");
    }
}
