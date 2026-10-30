//! Shared command context: project root, output mode, and constructors for
//! the two execution paths (daemon client vs in-process engine).

use crate::output::Style;
use maestro_engine::Orchestrator;
use maestro_providers::ProviderRegistry;
use maestro_store::Store;
use maestro_tui::DaemonClient;
use maestro_types::config::MaestroConfig;
use maestro_types::{new_id, MaestroError, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long `ensure_daemon` waits for an auto-spawned daemon to come up.
const DAEMON_START_WINDOW: Duration = Duration::from_secs(10);
const DAEMON_POLL: Duration = Duration::from_millis(200);
/// Probe budget for mutating commands, where missing a slow-but-alive daemon
/// would fork state (two orchestrators on one workspace).
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
/// Probe budget for read-only commands: a stale daemon.json pointing at a
/// filtered address must not stall `status`/`missions`/… — they have a local
/// fallback anyway.
const HEALTH_TIMEOUT_FAST: Duration = Duration::from_millis(500);

pub struct Ctx {
    pub root: PathBuf,
    pub json: bool,
    pub style: Style,
}

impl Ctx {
    pub fn new(cwd: &Path, json: bool, no_color: bool) -> Result<Ctx> {
        let absolute = if cwd.is_absolute() {
            cwd.to_path_buf()
        } else {
            std::env::current_dir()?.join(cwd)
        };
        let root = absolute
            .canonicalize()
            .map_err(|e| MaestroError::NotFound(format!("{}: {e}", absolute.display())))?;
        let color = {
            use std::io::IsTerminal;
            !json
                && !no_color
                && std::env::var_os("NO_COLOR").is_none()
                && std::io::stdout().is_terminal()
        };
        Ok(Ctx {
            root,
            json,
            style: Style { color },
        })
    }

    /// Load and validate `maestro.toml` (with an actionable NotFound message).
    pub fn config(&self) -> Result<MaestroConfig> {
        maestro_engine::load_config(&self.root).map_err(|e| match e {
            MaestroError::NotFound(_) => MaestroError::NotFound(format!(
                "no maestro.toml in {}; run `maestro init` first",
                self.root.display()
            )),
            other => other,
        })
    }

    /// Open the local store (requires an initialized project so a stray
    /// `--cwd` never grows a .maestro directory by accident).
    pub fn store(&self) -> Result<Store> {
        self.config()?;
        Store::open(&maestro_engine::state_dir(&self.root)?)
    }

    /// Build the full in-process engine: config + credentials + store +
    /// provider registry + orchestrator under a `cli_<8hex>` generation.
    pub fn orchestrator(&self) -> Result<(Orchestrator, Store, MaestroConfig)> {
        let config = self.config()?;
        let creds = maestro_engine::load_credentials(&self.root)?;
        maestro_engine::inject_env(&config, &creds);
        let store = Store::open(&maestro_engine::state_dir(&self.root)?)?;
        let registry = Arc::new(ProviderRegistry::new(&config));
        let generation = format!("cli_{}", &new_id()[..8]);
        let orch = Orchestrator::new(
            store.clone(),
            config.clone(),
            registry,
            self.root.clone(),
            generation,
        );
        Ok((orch, store, config))
    }
}

/// Probe for a live daemon: read `.maestro/daemon.json` and hit /health.
/// Full [`HEALTH_TIMEOUT`] budget — use for mutating commands.
pub async fn probe_daemon(root: &Path) -> Option<DaemonClient> {
    probe_daemon_with(root, HEALTH_TIMEOUT).await
}

/// Short-budget probe for read-only commands (they fall back to the local
/// store, so a false negative only degrades freshness, never correctness).
pub async fn probe_daemon_fast(root: &Path) -> Option<DaemonClient> {
    probe_daemon_with(root, HEALTH_TIMEOUT_FAST).await
}

async fn probe_daemon_with(root: &Path, budget: Duration) -> Option<DaemonClient> {
    let client = DaemonClient::from_root(root).ok()?;
    match tokio::time::timeout(budget, client.health()).await {
        Ok(Ok(h)) if h.ok => Some(client),
        _ => None,
    }
}

/// Spawn `maestro serve --cwd <root>` fully detached (null stdio, its own
/// process group) so missions survive the launching terminal.
pub fn spawn_detached_daemon(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::{Command, Stdio};
    let state_dir =
        maestro_engine::state_dir(root).map_err(|e| std::io::Error::other(e.to_string()))?;
    // Keep boot failures available to the launching UI. Replaced on each
    // launch so an old error is never mistaken for the current attempt.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(state_dir.join("daemon.stderr.log"))?;
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("serve")
        .arg("--cwd")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ())
}

/// Return a live daemon client, auto-spawning a detached daemon and polling
/// for up to ~10s when none is running.
pub async fn ensure_daemon(root: &Path) -> Result<DaemonClient> {
    if let Some(client) = probe_daemon(root).await {
        return Ok(client);
    }
    spawn_detached_daemon(root)
        .map_err(|e| MaestroError::Io(format!("failed to spawn daemon: {e}")))?;
    let deadline = std::time::Instant::now() + DAEMON_START_WINDOW;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(DAEMON_POLL).await;
        if let Some(client) = probe_daemon(root).await {
            return Ok(client);
        }
    }
    Err(MaestroError::Io(maestro_tui::daemon_startup_error(root)))
}

/// Error used by commands that strictly require a daemon.
pub fn no_daemon() -> MaestroError {
    MaestroError::NotFound("no daemon running (start one with `maestro serve`)".into())
}

/// Read a secret from stdin. On a TTY the prompt goes to stderr and terminal
/// echo is disabled via `stty -echo` (restored afterwards, even on error);
/// piped input is read as-is. No extra dependencies.
pub fn read_secret(prompt: &str) -> Result<String> {
    use std::io::{BufRead, IsTerminal, Write};
    let stdin = std::io::stdin();
    let tty = stdin.is_terminal();
    if tty {
        let mut err = std::io::stderr();
        let _ = write!(err, "{prompt}");
        let _ = err.flush();
    }
    let echo_off = tty && stty(&["-echo"]);
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if echo_off {
        stty(&["echo"]);
        eprintln!();
    }
    read?;
    let secret = line.trim_end_matches(['\n', '\r']).to_string();
    if secret.is_empty() {
        return Err(MaestroError::Other("empty secret; nothing stored".into()));
    }
    Ok(secret)
}

#[cfg(unix)]
fn stty(args: &[&str]) -> bool {
    std::process::Command::new("stty")
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn stty(_args: &[&str]) -> bool {
    false
}
