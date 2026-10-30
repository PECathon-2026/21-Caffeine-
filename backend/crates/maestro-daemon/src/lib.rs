//! HTTP + SSE control server for a Maestro workspace: mission workers,
//! daemon generations, the workspace lock, and crash recovery.
//! See SPEC.md §Daemon HTTP API for the route contract; DTOs live in
//! `maestro_types::api` and the reference consumer is the TUI's
//! `DaemonClient`.
//!
//! # Lifecycle
//!
//! [`start_daemon`] boots everything and returns a [`DaemonHandle`] bound to
//! an ephemeral (or configured) port; [`run_daemon`] does the same but blocks
//! until SIGTERM/ctrl-c or `POST /api/shutdown`. Boot order: config +
//! credentials → store → workspace lock → bind → `daemon.json` → generation
//! registration → heartbeat ticker → generation recovery → serve. Shutdown
//! order: stop accepting, drain connections (SSE streams end on the shutdown
//! token), cancel and await workers (5s grace, then abort), stop the
//! heartbeat, release the lock and remove `daemon.json`.

#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod http;
mod lock;
mod sse;
mod workers;

use http::AppState;
use lock::WorkspaceLock;
use maestro_engine::Orchestrator;
use maestro_providers::ProviderRegistry;
use maestro_store::Store;
use maestro_types::api::DaemonInfo;
use maestro_types::config::MaestroConfig;
use maestro_types::event::kind;
use maestro_types::{new_id, now_ms, MaestroError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use workers::Workers;

/// Heartbeat period for the daemon-instance row.
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(5);
/// How long workers get to stop gracefully before being aborted.
const WORKER_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Default bind address: loopback, ephemeral port.
const DEFAULT_BIND: &str = "127.0.0.1:0";

/// Options for [`run_daemon`] / [`start_daemon`].
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// Project root (the git repository holding `maestro.toml`).
    pub root: PathBuf,
    /// Socket address to bind; defaults to `"127.0.0.1:0"`. Binding
    /// off-loopback requires `token`.
    pub bind: Option<String>,
    /// Bearer token. When set it is required on every `/api` request, even
    /// on loopback.
    pub token: Option<String>,
}

/// A running daemon started by [`start_daemon`]: the bound URL plus a
/// shutdown handle. Used by tests and by embedders that don't want the
/// blocking [`run_daemon`].
pub struct DaemonHandle {
    /// Base URL, e.g. `http://127.0.0.1:43521`.
    pub url: String,
    /// This instance's generation id (`daemon_` + 16 hex).
    pub generation: String,
    shutdown: CancellationToken,
    serve_task: Option<tokio::task::JoinHandle<()>>,
}

impl DaemonHandle {
    /// Trigger a graceful shutdown and wait for teardown to finish.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.serve_task.take() {
            let _ = task.await;
        }
    }
}

/// Dropping the handle (tests, embedders) must not leak the daemon
/// in-process: cancel the shutdown token so serving, workers, the heartbeat
/// and the workspace lock unwind in the background. Idempotent with an
/// explicit [`DaemonHandle::shutdown`], which additionally awaits teardown.
impl Drop for DaemonHandle {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Run the daemon until SIGTERM/ctrl-c or `POST /api/shutdown`.
pub async fn run_daemon(opts: DaemonOptions) -> Result<()> {
    let handle = start_daemon(opts).await?;
    let token = handle.shutdown.clone();
    tokio::select! {
        _ = token.cancelled() => {}
        _ = wait_for_signal() => {
            tracing::info!("shutdown signal received");
            token.cancel();
        }
    }
    handle.shutdown().await;
    Ok(())
}

/// Boot the daemon and start serving in a background task.
pub async fn start_daemon(opts: DaemonOptions) -> Result<DaemonHandle> {
    let root = opts.root.clone();

    // Config + credentials. A missing maestro.toml falls back to the default
    // GPT config; requests require a valid API key.
    let config = match maestro_engine::load_config(&root) {
        Ok(config) => config,
        Err(MaestroError::NotFound(_)) => {
            tracing::info!("no maestro.toml; using the default configuration");
            MaestroConfig::default()
        }
        Err(e) => return Err(e),
    };
    if let Ok(creds) = maestro_engine::load_credentials(&root) {
        maestro_engine::inject_env(&config, &creds);
    }

    let state_dir = maestro_engine::state_dir(&root)?;
    let store = Store::open(&state_dir)?;

    // Workspace exclusivity before anything observable happens.
    let ws_lock = WorkspaceLock::acquire(&state_dir)?;

    // Bind. Off-loopback requires a bearer token (SPEC §Daemon HTTP API).
    let bind = opts.bind.as_deref().unwrap_or(DEFAULT_BIND);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| MaestroError::Io(format!("bind {bind}: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| MaestroError::Io(e.to_string()))?;
    if !addr.ip().is_loopback() && opts.token.is_none() {
        return Err(MaestroError::Config(
            "binding off-loopback requires a bearer token".into(),
        ));
    }
    let url = format!("http://{addr}");

    let generation = format!("daemon_{}", &new_id()[..16]);
    let pid = std::process::id() as i64;
    let started_at = now_ms();

    lock::write_info(
        &state_dir,
        &DaemonInfo {
            url: url.clone(),
            pid,
            generation: generation.clone(),
            started_at,
            token: opts.token.clone(),
        },
    )?;
    store.register_daemon(&generation, pid, &url)?;

    let registry = Arc::new(ProviderRegistry::new(&config));
    // Restore persisted circuit-breaker state (best effort).
    if let Ok(saved) = store.list_provider_health() {
        for health in &saved {
            let _ = registry.restore_health(health);
        }
    }
    let mock_role_list = mock_roles(&config);
    let orch = Orchestrator::new(
        store.clone(),
        config,
        registry.clone(),
        root.clone(),
        generation.clone(),
    );
    let workers = Workers::new();
    let shutdown = CancellationToken::new();

    // Heartbeat ticker, owned by the serve task's teardown.
    let heartbeat = OwnedTask(tokio::spawn({
        let store = store.clone();
        let generation = generation.clone();
        async move {
            let mut tick = tokio::time::interval(HEARTBEAT_PERIOD);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                if let Err(e) = store.heartbeat_daemon(&generation) {
                    tracing::warn!(error = %e, "daemon heartbeat failed");
                }
            }
        }
    }));

    recover(&store, &orch, &workers, &generation)?;

    let state = Arc::new(AppState {
        orch,
        store,
        registry,
        workers: workers.clone(),
        generation: generation.clone(),
        build_id: maestro_types::build_id(),
        mock_roles: mock_role_list,
        token: opts.token,
        started_at,
        shutdown: shutdown.clone(),
        diff_cache: std::sync::Mutex::new(std::collections::VecDeque::new()),
        root,
    });
    let app = http::router(state);

    let serve_task = tokio::spawn(serve_and_teardown(
        listener,
        app,
        shutdown.clone(),
        workers,
        heartbeat,
        ws_lock,
    ));

    tracing::info!(%url, %generation, "daemon started");
    Ok(DaemonHandle {
        url,
        generation,
        shutdown,
        serve_task: Some(serve_task),
    })
}

/// Post-crash recovery: reclaim foreign-generation leases, reconcile merge
/// queues, and restart workers for missions that were mid-flight.
fn recover(store: &Store, orch: &Orchestrator, workers: &Workers, generation: &str) -> Result<()> {
    let reclaimed = store.recover_generation(generation)?;
    if reclaimed > 0 {
        tracing::info!(
            reclaimed,
            "recovered stale task leases from previous generations"
        );
    }
    for mission in store.list_missions()? {
        if !mission.status.is_active() {
            continue;
        }
        let reset = store.reconcile_merge_queue(&mission.id)?;
        store.append_event(
            &mission.id,
            None,
            kind::DAEMON_RECOVERED,
            serde_json::json!({
                "generation": generation,
                "status": mission.status.as_str(),
                "mergeEntriesReset": reset,
            }),
        )?;
        workers.ensure(orch, &mission.id);
        tracing::info!(mission_id = %mission.id, status = mission.status.as_str(), "resumed mission after restart");
    }
    Ok(())
}

/// Roles whose primary model resolves to a mock provider.
fn mock_roles(config: &MaestroConfig) -> Vec<String> {
    config
        .roles
        .iter()
        .filter(|(_, assign)| {
            config
                .providers
                .get(&assign.model.provider)
                .is_none_or(|p| p.kind.is_mock())
        })
        .map(|(role, _)| role.clone())
        .collect()
}

/// Serve until the shutdown token fires, then tear everything down in order.
async fn serve_and_teardown(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    shutdown: CancellationToken,
    workers: Workers,
    heartbeat: OwnedTask,
    ws_lock: WorkspaceLock,
) {
    let graceful = shutdown.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move { graceful.cancelled().await })
        .await;
    if let Err(e) = result {
        tracing::error!(error = %e, "http server error");
        shutdown.cancel(); // make sure SSE feeders and waiters unwind
    }

    // Workers: cancel, await with grace, abort stragglers.
    workers.shutdown_all(WORKER_SHUTDOWN_GRACE).await;

    // Provider-health persistence: the store only exposes incremental
    // `record_provider_result`, which cannot absorb a whole snapshot without
    // corrupting counters — breaker state is instead restored best-effort on
    // the next boot from the rows written during normal operation.
    drop(heartbeat); // stops the ticker; liveness expires via the window
    ws_lock.release();
    tracing::info!("daemon stopped");
}

/// Resolve on SIGTERM or ctrl-c (SIGINT).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(stream) => { stream.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// A background task owned by a scope; aborted on drop so it never leaks.
struct OwnedTask(tokio::task::JoinHandle<()>);

impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
