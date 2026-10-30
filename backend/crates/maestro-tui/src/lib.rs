//! Chat-first ratatui console for Maestro. See SPEC.md §TUI and
//! docs/UI-BRIEF.md.
//!
//! Public surface: [`run_tui`] + [`TuiOptions`] for the CLI, and
//! [`DaemonClient`] (plain HTTP/SSE client for the daemon API) which the CLI
//! reuses for its non-interactive commands.

#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod app;
pub mod client;
mod commands;
mod keymap;
mod local;
#[cfg(test)]
mod render_tests;
mod sse;
mod state;
mod term;
mod theme;
mod ui;

pub use app::TuiOptions;
pub use client::DaemonClient;
pub use local::daemon_startup_error;

use maestro_types::{MaestroError, Result};

/// Run the interactive console. Requires a TTY on stdout; restores the
/// terminal on every exit path (including panics). When no daemon responds
/// and `opts.spawn_daemon` is set, it is invoked once and the TUI re-probes
/// with capped backoff for ~10s while showing "starting daemon…".
pub async fn run_tui(opts: TuiOptions) -> Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return Err(MaestroError::Other(
            "maestro TUI requires a TTY; use --json commands".into(),
        ));
    }
    app::run(opts).await
}
