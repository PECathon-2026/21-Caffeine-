//! The `maestro` binary's logic (SPEC §CLI commands), kept in a lib so arg
//! routing and formatting are unit-testable. `main.rs` stays thin: it only
//! answers `--version` on a fast path and calls [`run`].
//!
//! Behavior matrix: every command that mutates state goes through the daemon
//! when one is running; reads and the documented subset (run --local, steer,
//! approve/reject, retry, plan, diff, models, events replay) fall back to
//! the in-process engine/store. pause/resume/cancel/shutdown require a
//! daemon.

#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod args;
pub mod commands;
pub mod context;
pub mod output;

use args::{join_words, Cli, Command};
use clap::Parser;
use context::Ctx;
use maestro_types::Result;

/// Parse argv, run the selected command on a fresh tokio runtime, and return
/// the process exit code. Usage errors exit 2 (clap); command errors print
/// `error: <msg>` to stderr and return 1.
pub fn run() -> i32 {
    let cli = Cli::parse();
    init_tracing(&cli);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start async runtime: {e}");
            return 1;
        }
    };
    match runtime.block_on(dispatch(cli)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// Logs go to stderr, never stdout (stdout is user/JSON output). `serve`
/// defaults to info; everything else stays quiet unless RUST_LOG is set.
fn init_tracing(cli: &Cli) {
    use tracing_subscriber::EnvFilter;
    let default = if matches!(cli.command, Some(Command::Serve { .. })) {
        "info"
    } else {
        "error"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

async fn dispatch(cli: Cli) -> Result<i32> {
    let ctx = Ctx::new(&cli.cwd, cli.json, cli.no_color)?;
    let Some(command) = cli.command else {
        return no_args(ctx).await;
    };
    match command {
        Command::Init { local } => commands::project::init(&ctx, local).await,
        Command::Serve { bind, token } => commands::project::serve(&ctx, bind, token).await,
        Command::Status => commands::missions::status(&ctx).await,
        Command::Submit { objective } => {
            commands::missions::submit(&ctx, &join_words(&objective)).await
        }
        Command::Run { objective, local } => {
            commands::run::run(&ctx, &join_words(&objective), local).await
        }
        Command::Missions => commands::missions::missions(&ctx).await,
        Command::Mission { id } => commands::missions::mission(&ctx, &id).await,
        Command::Tasks { mission_id } => commands::missions::tasks(&ctx, &mission_id).await,
        Command::Events {
            mission_id,
            follow,
            after,
        } => commands::missions::events(&ctx, &mission_id, follow, after).await,
        Command::Approvals => commands::missions::approvals(&ctx).await,
        Command::Approve { approval_id } => {
            commands::missions::decide_approval(&ctx, &approval_id, true).await
        }
        Command::Reject { approval_id } => {
            commands::missions::decide_approval(&ctx, &approval_id, false).await
        }
        Command::Steer { mission_id, text } => {
            commands::missions::steer(&ctx, &mission_id, &join_words(&text)).await
        }
        Command::Pause { mission_id } => {
            commands::missions::lifecycle(&ctx, &mission_id, "pause").await
        }
        Command::Resume { mission_id } => {
            commands::missions::lifecycle(&ctx, &mission_id, "resume").await
        }
        Command::Cancel { mission_id } => {
            commands::missions::lifecycle(&ctx, &mission_id, "cancel").await
        }
        Command::Retry {
            mission_id,
            task_id,
            model,
        } => commands::missions::retry(&ctx, &mission_id, &task_id, model).await,
        Command::Plan {
            mission_id,
            approve,
            reject,
        } => commands::missions::plan(&ctx, &mission_id, approve, reject).await,
        Command::Diff { mission_id } => commands::missions::diff(&ctx, &mission_id).await,
        Command::Models => commands::project::models(&ctx).await,
        Command::History { save, diff } => {
            commands::history::history(&ctx, save.as_deref(), diff.as_deref()).await
        }
        Command::Undo { apply } => commands::history::restore(&ctx, None, apply.as_deref()).await,
        Command::Restore { version, apply } => {
            commands::history::restore(&ctx, Some(&version), apply.as_deref()).await
        }
        Command::Connect {
            provider_id,
            global,
        } => commands::project::connect(&ctx, &provider_id, global).await,
        Command::Doctor => commands::project::doctor(&ctx).await,
        Command::Bench { iterations } => commands::bench::bench(&ctx, iterations).await,
        Command::Shutdown => commands::missions::shutdown(&ctx).await,
    }
}

/// Bare `maestro`: open the TUI when the project is initialized (spawning a
/// detached daemon on demand so chat missions survive terminal exit),
/// otherwise point at `maestro init`.
async fn no_args(ctx: Ctx) -> Result<i32> {
    let has_project_config = maestro_engine::config::config_path(&ctx.root).exists();
    if !has_project_config && maestro_engine::config::user_config_path().is_none() {
        eprintln!(
            "no maestro.toml in {} — run `maestro init` to set up this project",
            ctx.root.display()
        );
        return Ok(1);
    }
    let spawn_root = ctx.root.clone();
    maestro_tui::run_tui(maestro_tui::TuiOptions {
        root: ctx.root,
        spawn_daemon: Some(Box::new(move || {
            context::spawn_detached_daemon(&spawn_root)
        })),
    })
    .await?;
    Ok(0)
}
