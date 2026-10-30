//! `maestro run`: drive a mission to a terminal/awaiting state while
//! streaming one progress line per interesting event.
//!
//! With a running daemon (and no `--local`) the mission is submitted there
//! and followed over SSE; otherwise the whole engine runs in-process with a
//! ctrl-c → `CancellationToken` hookup so interrupts stop at safe boundaries.

use super::emit_json;
use crate::context::{probe_daemon, Ctx};
use crate::output::{event_line, fmt_cost, short};
use futures::StreamExt;
use maestro_store::Store;
use maestro_tui::DaemonClient;
use maestro_types::domain::{Approval, MissionStats, MissionStatus, TaskStatus};
use maestro_types::event::kind;
use maestro_types::{MaestroError, Result};
use tokio_util::sync::CancellationToken;

pub async fn run(ctx: &Ctx, objective: &str, local: bool) -> Result<i32> {
    if local {
        // Two orchestrators on one workspace race each other (scope locks,
        // worktrees, merge queue) — refuse --local while a daemon is alive.
        if probe_daemon(&ctx.root).await.is_some() {
            return Err(MaestroError::Conflict(
                "a daemon is running; use `maestro run` (daemon mode) or stop it first".into(),
            ));
        }
        return run_local(ctx, objective).await;
    }
    if let Some(client) = probe_daemon(&ctx.root).await {
        return run_via_daemon(ctx, &client, objective).await;
    }
    run_local(ctx, objective).await
}

async fn run_local(ctx: &Ctx, objective: &str) -> Result<i32> {
    let (orch, store, _config) = ctx.orchestrator()?;
    let mission = orch.create_mission(objective).await?;
    if !ctx.json {
        println!(
            "mission {} {}",
            ctx.style.accent(&mission.id),
            ctx.style.dim("(local)")
        );
    }

    let cancel = CancellationToken::new();
    let ctrlc = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancel.cancel();
            }
        })
    };
    // Subscribe before running so no progress event is missed; suppressed
    // entirely in --json mode (machine output is the final document only).
    let printer = if ctx.json {
        None
    } else {
        let mut rx = store.subscribe();
        let mission_id = mission.id.clone();
        let style = ctx.style;
        Some(tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) if ev.mission_id == mission_id => {
                        if let Some(line) = event_line(&ev, &style) {
                            println!("{line}");
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }))
    };

    let outcome = orch.run_mission(&mission.id, &cancel).await;
    // Give the printer a beat to drain the tail of the bus, then stop it.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    if let Some(p) = printer {
        p.abort();
    }
    ctrlc.abort();
    let status = outcome?;
    finish(ctx, &store, &mission.id, status)
}

async fn run_via_daemon(ctx: &Ctx, client: &DaemonClient, objective: &str) -> Result<i32> {
    // The mission is new: everything it emits has seq > the current head, so
    // following from the pre-submission cursor skips replaying the entire
    // historical log while missing nothing.
    let after = client.health().await.map(|h| h.last_seq).unwrap_or(0);
    let resp = client.submit_mission(objective, None).await?;
    let mission_id = resp.mission_id;
    if !ctx.json {
        println!(
            "mission {} {}",
            ctx.style.accent(&mission_id),
            ctx.style.dim(&format!("(daemon at {})", client.base_url()))
        );
    }
    let mut stream = client.events(Some(mission_id.clone()), after);
    let status = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                if !ctx.json {
                    println!(
                        "\ndetached; the mission keeps running in the daemon \
                         (`maestro events {} --follow`)",
                        short(&mission_id)
                    );
                }
                return Ok(0);
            }
            item = stream.next() => match item {
                Some(Ok(ev)) => {
                    if !ctx.json {
                        if let Some(line) = event_line(&ev, &ctx.style) {
                            println!("{line}");
                        }
                    }
                    if ev.kind == kind::MISSION_STATUS {
                        let status = ev
                            .payload
                            .get("status")
                            .and_then(|v| v.as_str())
                            .and_then(MissionStatus::parse);
                        if let Some(status) = status {
                            if !matches!(
                                status,
                                MissionStatus::Planning
                                    | MissionStatus::Planned
                                    | MissionStatus::Running
                            ) {
                                break status;
                            }
                        }
                    }
                }
                Some(Err(_)) => {
                    eprintln!("{}", ctx.style.dim("event stream disconnected; reconnecting…"));
                }
                None => break MissionStatus::Failed,
            }
        }
    };
    // Final accounting from the daemon's snapshot of the mission.
    let detail = client.mission_detail(&mission_id).await?;
    let pending = client.approvals(Some(&mission_id)).await?;
    print_outcome(
        ctx,
        &mission_id,
        status,
        &detail.stats,
        count_done(&detail.tasks),
        &pending,
    )
}

fn finish(ctx: &Ctx, store: &Store, mission_id: &str, status: MissionStatus) -> Result<i32> {
    let stats = store.mission_stats(mission_id)?;
    let tasks = store.list_tasks(mission_id)?;
    let pending = store.pending_approvals(Some(mission_id))?;
    print_outcome(
        ctx,
        mission_id,
        status,
        &stats,
        (
            tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Completed)
                .count(),
            tasks.len(),
        ),
        &pending,
    )
}

fn count_done(tasks: &[maestro_types::domain::Task]) -> (usize, usize) {
    (
        tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count(),
        tasks.len(),
    )
}

fn print_outcome(
    ctx: &Ctx,
    mission_id: &str,
    status: MissionStatus,
    stats: &MissionStats,
    (done, total): (usize, usize),
    pending: &[Approval],
) -> Result<i32> {
    if ctx.json {
        emit_json(&serde_json::json!({
            "missionId": mission_id,
            "status": status.as_str(),
            "stats": stats,
            "tasks": { "completed": done, "total": total },
            "pendingApprovals": pending.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
        }))?;
    } else {
        println!(
            "\nmission {} {}",
            ctx.style.accent(&short(mission_id)),
            ctx.style.bold(status.as_str())
        );
        println!(
            "  tasks: {done}/{total} completed   tokens: {} in / {} out   cost: {}",
            stats.input_tokens,
            stats.output_tokens,
            fmt_cost(stats.cost_micro_usd)
        );
        for approval in pending {
            println!(
                "  {} {} {}  → maestro approve {}",
                ctx.style.bold("pending:"),
                approval.action,
                approval.resource,
                approval.id
            );
        }
    }
    Ok(match status {
        MissionStatus::Completed
        | MissionStatus::AwaitingApproval
        | MissionStatus::AwaitingPlanApproval
        | MissionStatus::Paused => 0,
        _ => 1,
    })
}
