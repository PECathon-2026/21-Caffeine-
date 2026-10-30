//! clap argument definitions for the `maestro` binary (SPEC §CLI commands).
//!
//! `--version` is answered on a fast path in `main.rs` before clap is ever
//! touched; the flag is still declared here so `--help` documents it and so
//! it works in any argv position.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "maestro",
    version,
    about = "Local multi-model engineering control plane",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Project root (defaults to the current directory).
    #[arg(long, global = true, default_value = ".", value_name = "PATH")]
    pub cwd: PathBuf,

    /// Machine-readable JSON output (no ANSI).
    #[arg(long, global = true)]
    pub json: bool,

    /// Disable ANSI styling even on a TTY.
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Set up this project: ignore .maestro/, and write a maestro.toml
    /// unless a user-level config (~/.config/maestro) already covers it.
    Init {
        /// Write a project maestro.toml even when a user-level config exists
        /// (the project file overrides it wholly).
        #[arg(long)]
        local: bool,
    },
    /// Run the control daemon in the foreground.
    Serve {
        /// Listen address, e.g. 127.0.0.1:7700 (default: loopback, ephemeral port).
        #[arg(long, value_name = "ADDR")]
        bind: Option<String>,
        /// Bearer token required off-loopback.
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
    },
    /// Daemon liveness and a mission overview.
    Status,
    /// Submit a mission to the daemon (auto-spawns one when needed).
    Submit {
        #[arg(required = true, value_name = "OBJECTIVE")]
        objective: Vec<String>,
    },
    /// Run a mission and stream progress (daemon when running, else in-process).
    Run {
        #[arg(required = true, value_name = "OBJECTIVE")]
        objective: Vec<String>,
        /// Force in-process execution even when a daemon is running.
        #[arg(long)]
        local: bool,
    },
    /// List missions.
    Missions,
    /// Show one mission (tasks, stats, plan summary).
    Mission {
        #[arg(value_name = "MISSION_ID")]
        id: String,
    },
    /// List a mission's tasks.
    Tasks {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
    },
    /// Replay (and optionally follow) a mission's event log.
    Events {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
        /// Follow live events over the daemon SSE stream.
        #[arg(long)]
        follow: bool,
        /// Replay events with seq greater than this cursor.
        #[arg(long, default_value_t = 0, value_name = "SEQ")]
        after: i64,
    },
    /// List pending approvals.
    Approvals,
    /// Approve a pending approval (finalizes integrate approvals).
    Approve {
        #[arg(value_name = "APPROVAL_ID")]
        approval_id: String,
    },
    /// Reject a pending approval.
    Reject {
        #[arg(value_name = "APPROVAL_ID")]
        approval_id: String,
    },
    /// Add a steering directive to a running mission.
    Steer {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
        #[arg(required = true, value_name = "TEXT")]
        text: Vec<String>,
    },
    /// Pause a running mission (requires a daemon).
    Pause {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
    },
    /// Resume a paused mission (requires a daemon).
    Resume {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
    },
    /// Cancel a mission (requires a daemon).
    Cancel {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
    },
    /// Retry a failed/blocked task, optionally on a different model.
    Retry {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
        #[arg(value_name = "TASK_ID")]
        task_id: String,
        /// "provider/model" override for the retried attempts.
        #[arg(long, value_name = "REF")]
        model: Option<String>,
    },
    /// Show a mission's plan, or decide a pending plan approval.
    Plan {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
        #[arg(long)]
        approve: bool,
        #[arg(long, conflicts_with = "approve")]
        reject: bool,
    },
    /// Print the mission's unified diff.
    Diff {
        #[arg(value_name = "MISSION_ID")]
        mission_id: String,
    },
    /// List configured and discovered models per provider.
    Models,
    /// Saved project and draft versions, or save/inspect a version.
    History {
        #[arg(long, conflicts_with = "diff")]
        save: Option<String>,
        #[arg(long)]
        diff: Option<String>,
    },
    /// Preview undoing the latest request. Apply with the returned preview token.
    Undo {
        #[arg(long, value_name = "PREVIEW_TOKEN")]
        apply: Option<String>,
    },
    /// Preview restoring a saved version; preserves non-conflicting later edits.
    Restore {
        version: String,
        #[arg(long, value_name = "PREVIEW_TOKEN")]
        apply: Option<String>,
    },
    /// Store an API key for a provider (read from stdin, echo off).
    Connect {
        #[arg(value_name = "PROVIDER_ID")]
        provider_id: String,
        /// Store in the user-level store (~/.config/maestro), shared by
        /// every project that doesn't override the provider.
        #[arg(long)]
        global: bool,
    },
    /// Environment checks: git, bwrap, config, db integrity, daemon, credentials.
    Doctor,
    /// Control-plane latency harness (event append, lease CAS, policy eval).
    Bench {
        /// Iterations per operation.
        #[arg(long, default_value_t = 1000, value_name = "N")]
        iterations: usize,
    },
    /// Ask a running daemon to shut down.
    Shutdown,
}

impl Command {
    /// Stable command name, used for tracing and tests.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Init { .. } => "init",
            Command::Serve { .. } => "serve",
            Command::Status => "status",
            Command::Submit { .. } => "submit",
            Command::Run { .. } => "run",
            Command::Missions => "missions",
            Command::Mission { .. } => "mission",
            Command::Tasks { .. } => "tasks",
            Command::Events { .. } => "events",
            Command::Approvals => "approvals",
            Command::Approve { .. } => "approve",
            Command::Reject { .. } => "reject",
            Command::Steer { .. } => "steer",
            Command::Pause { .. } => "pause",
            Command::Resume { .. } => "resume",
            Command::Cancel { .. } => "cancel",
            Command::Retry { .. } => "retry",
            Command::Plan { .. } => "plan",
            Command::Diff { .. } => "diff",
            Command::Models => "models",
            Command::History { .. } => "history",
            Command::Undo { .. } => "undo",
            Command::Restore { .. } => "restore",
            Command::Connect { .. } => "connect",
            Command::Doctor => "doctor",
            Command::Bench { .. } => "bench",
            Command::Shutdown => "shutdown",
        }
    }
}

/// Join a multi-word positional back into one objective string.
pub fn join_words(words: &[String]) -> String {
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(argv: &[&str]) -> Cli {
        Cli::try_parse_from(argv).expect("parses")
    }

    #[test]
    fn no_subcommand_is_allowed() {
        let cli = parse(&["maestro"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.cwd, std::path::PathBuf::from("."));
        assert!(!cli.json);
    }

    #[test]
    fn global_flags_work_after_subcommand() {
        let cli = parse(&["maestro", "status", "--json", "--cwd", "/tmp/x"]);
        assert!(cli.json);
        assert_eq!(cli.cwd, std::path::PathBuf::from("/tmp/x"));
        assert!(matches!(cli.command, Some(Command::Status)));
    }

    #[test]
    fn run_collects_objective_words_and_local_flag() {
        let cli = parse(&["maestro", "run", "--local", "add", "a", "login", "page"]);
        match cli.command {
            Some(Command::Run { objective, local }) => {
                assert!(local);
                assert_eq!(join_words(&objective), "add a login page");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn run_requires_an_objective() {
        assert!(Cli::try_parse_from(["maestro", "run"]).is_err());
        assert!(Cli::try_parse_from(["maestro", "submit"]).is_err());
    }

    #[test]
    fn retry_takes_model_override() {
        let cli = parse(&[
            "maestro",
            "retry",
            "m1",
            "t1",
            "--model",
            "anthropic/claude",
        ]);
        match cli.command {
            Some(Command::Retry {
                mission_id,
                task_id,
                model,
            }) => {
                assert_eq!(mission_id, "m1");
                assert_eq!(task_id, "t1");
                assert_eq!(model.as_deref(), Some("anthropic/claude"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn plan_approve_and_reject_conflict() {
        assert!(Cli::try_parse_from(["maestro", "plan", "m1", "--approve", "--reject"]).is_err());
        let cli = parse(&["maestro", "plan", "m1", "--approve"]);
        match cli.command {
            Some(Command::Plan {
                approve, reject, ..
            }) => {
                assert!(approve);
                assert!(!reject);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn events_flags_route() {
        let cli = parse(&["maestro", "events", "m1", "--follow", "--after", "42"]);
        match cli.command {
            Some(Command::Events {
                mission_id,
                follow,
                after,
            }) => {
                assert_eq!(mission_id, "m1");
                assert!(follow);
                assert_eq!(after, 42);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn bench_default_iterations() {
        let cli = parse(&["maestro", "bench"]);
        match cli.command {
            Some(Command::Bench { iterations }) => assert_eq!(iterations, 1000),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn every_command_routes_by_name() {
        for (argv, name) in [
            (vec!["maestro", "init"], "init"),
            (vec!["maestro", "serve"], "serve"),
            (vec!["maestro", "doctor"], "doctor"),
            (vec!["maestro", "models"], "models"),
            (vec!["maestro", "missions"], "missions"),
            (vec!["maestro", "approvals"], "approvals"),
            (vec!["maestro", "shutdown"], "shutdown"),
            (vec!["maestro", "steer", "m", "go", "faster"], "steer"),
            (vec!["maestro", "pause", "m"], "pause"),
            (vec!["maestro", "resume", "m"], "resume"),
            (vec!["maestro", "cancel", "m"], "cancel"),
            (vec!["maestro", "diff", "m"], "diff"),
            (vec!["maestro", "connect", "anthropic"], "connect"),
            (vec!["maestro", "approve", "a"], "approve"),
            (vec!["maestro", "reject", "a"], "reject"),
            (vec!["maestro", "mission", "m"], "mission"),
            (vec!["maestro", "tasks", "m"], "tasks"),
        ] {
            let cli = parse(&argv);
            assert_eq!(cli.command.expect("has command").name(), name);
        }
    }
}
