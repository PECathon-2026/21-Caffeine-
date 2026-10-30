//! Mission-facing commands: status, missions, mission, tasks, events,
//! approvals, steer, pause/resume/cancel, retry, plan, diff, submit,
//! shutdown. Every state mutation goes through the daemon when one is
//! running; reads and the documented subset fall back to the local store.

use super::emit_json;
use crate::context::{ensure_daemon, no_daemon, probe_daemon, probe_daemon_fast, Ctx};
use crate::output::{ago, clip, fmt_cost, raw_event_line, render_table, short, tint_diff};
use futures::StreamExt;
use maestro_store::Store;
use maestro_types::api::{
    ApprovalDecisionRequest, MissionDetail, MissionSummary, PlanDecisionRequest, RetryRequest,
    SteerRequest, TaskCounts,
};
use maestro_types::domain::{Approval, Task, TaskStatus};
use maestro_types::event::kind;
use maestro_types::plan::Plan;
use maestro_types::{MaestroError, Result};

// ---- shared local readers ---------------------------------------------------

/// Terminal missions included in local overviews, mirroring the daemon's
/// `/api/state` cap: every non-terminal mission plus this many of the most
/// recent terminal ones (three store reads per mission otherwise make the
/// overview scale with all missions ever).
const LOCAL_TERMINAL_CAP: usize = 20;

fn local_summaries(store: &Store) -> Result<Vec<MissionSummary>> {
    let mut out = Vec::new();
    let mut terminal = 0usize;
    // list_missions returns newest first.
    for mission in store.list_missions()? {
        if mission.status.is_terminal() {
            terminal += 1;
            if terminal > LOCAL_TERMINAL_CAP {
                continue;
            }
        }
        let stats = store.mission_stats(&mission.id)?;
        let tasks = store.list_tasks(&mission.id)?;
        let task_counts = count_tasks(&tasks);
        let pending_approvals = store.pending_approvals(Some(&mission.id))?.len() as i64;
        out.push(MissionSummary {
            mission,
            stats,
            task_counts,
            pending_approvals,
        });
    }
    Ok(out)
}

fn count_tasks(tasks: &[Task]) -> TaskCounts {
    let mut counts = TaskCounts {
        total: tasks.len() as i64,
        ..TaskCounts::default()
    };
    for t in tasks {
        match t.status {
            TaskStatus::Completed => counts.completed += 1,
            TaskStatus::Running => counts.running += 1,
            TaskStatus::Failed => counts.failed += 1,
            TaskStatus::Blocked => counts.blocked += 1,
            _ => {}
        }
    }
    counts
}

fn local_detail(store: &Store, id: &str) -> Result<MissionDetail> {
    let mission = store.get_mission(id)?;
    let tasks = store.list_tasks(id)?;
    let stats = store.mission_stats(id)?;
    let plan = store.get_mission_plan(id)?;
    Ok(MissionDetail {
        mission,
        tasks,
        stats,
        plan,
    })
}

fn missions_table(ctx: &Ctx, summaries: &[MissionSummary]) -> String {
    let rows: Vec<Vec<String>> = summaries
        .iter()
        .map(|s| {
            vec![
                short(&s.mission.id),
                s.mission.status.as_str().to_string(),
                format!("{}/{}", s.task_counts.completed, s.task_counts.total),
                s.stats.total_tokens().to_string(),
                fmt_cost(s.stats.cost_micro_usd),
                s.pending_approvals.to_string(),
                clip(&s.mission.objective, 48),
            ]
        })
        .collect();
    render_table(
        &[
            "MISSION",
            "STATUS",
            "TASKS",
            "TOKENS",
            "COST",
            "PENDING",
            "OBJECTIVE",
        ],
        &rows,
        &ctx.style,
    )
}

fn tasks_table(ctx: &Ctx, tasks: &[Task]) -> String {
    let rows: Vec<Vec<String>> = tasks
        .iter()
        .map(|t| {
            vec![
                short(&t.id),
                t.task_key.clone(),
                t.role.as_str().to_string(),
                t.status.as_str().to_string(),
                t.attempts.to_string(),
                t.risk.as_str().to_string(),
                clip(
                    t.summary
                        .as_deref()
                        .or(t.status_reason.as_deref())
                        .unwrap_or(&t.title),
                    56,
                ),
            ]
        })
        .collect();
    render_table(
        &["TASK", "KEY", "ROLE", "STATUS", "ATT", "RISK", "SUMMARY"],
        &rows,
        &ctx.style,
    )
}

fn approvals_table(ctx: &Ctx, approvals: &[Approval]) -> String {
    let rows: Vec<Vec<String>> = approvals
        .iter()
        .map(|a| {
            vec![
                a.id.clone(),
                a.action.clone(),
                clip(&a.resource, 40),
                a.task_id
                    .as_deref()
                    .map(short)
                    .unwrap_or_else(|| "-".into()),
                a.status.as_str().to_string(),
                ago(a.created_at),
            ]
        })
        .collect();
    render_table(
        &["APPROVAL", "ACTION", "RESOURCE", "TASK", "STATUS", "AGE"],
        &rows,
        &ctx.style,
    )
}

fn plan_text(ctx: &Ctx, plan: &Plan) -> String {
    let mut out = format!("{} {}\n\n", ctx.style.bold("plan:"), plan.summary);
    let rows: Vec<Vec<String>> = plan
        .tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            vec![
                (i + 1).to_string(),
                t.key.clone(),
                t.role.as_str().to_string(),
                t.risk.as_str().to_string(),
                if t.dependencies.is_empty() {
                    "-".to_string()
                } else {
                    t.dependencies.join(",")
                },
                if t.write_scopes.is_empty() {
                    "-".to_string()
                } else {
                    t.write_scopes.join(",")
                },
                clip(&t.title, 48),
            ]
        })
        .collect();
    out.push_str(&render_table(
        &["#", "KEY", "ROLE", "RISK", "DEPS", "SCOPES", "TITLE"],
        &rows,
        &ctx.style,
    ));
    out
}

// ---- commands ---------------------------------------------------------------

pub async fn status(ctx: &Ctx) -> Result<i32> {
    let daemon = probe_daemon_fast(&ctx.root).await;
    match daemon {
        Some(client) => {
            let health = client.health().await?;
            let snap = client.state_snapshot().await?;
            if ctx.json {
                emit_json(&serde_json::json!({
                    "daemon": {
                        "running": true,
                        "url": client.base_url(),
                        "generation": health.generation,
                        "pid": health.pid,
                        "version": health.version,
                    },
                    "missions": snap.missions,
                    "approvals": snap.approvals,
                }))?;
            } else {
                println!(
                    "daemon   {} {}",
                    ctx.style.accent("running"),
                    ctx.style.dim(&format!(
                        "at {} (gen {}, pid {})",
                        client.base_url(),
                        health.generation,
                        health.pid
                    ))
                );
                print_status_body(ctx, &snap.missions, snap.approvals.len());
            }
        }
        None => {
            let store = ctx.store()?;
            let summaries = local_summaries(&store)?;
            let approvals = store.pending_approvals(None)?;
            if ctx.json {
                emit_json(&serde_json::json!({
                    "daemon": { "running": false },
                    "missions": summaries,
                    "approvals": approvals,
                }))?;
            } else {
                println!("daemon   {}", ctx.style.dim("not running"));
                print_status_body(ctx, &summaries, approvals.len());
            }
        }
    }
    Ok(0)
}

fn print_status_body(ctx: &Ctx, summaries: &[MissionSummary], pending: usize) {
    if summaries.is_empty() {
        println!(
            "{}",
            ctx.style.dim("no missions yet (maestro run <objective>)")
        );
    } else {
        println!();
        print!("{}", missions_table(ctx, summaries));
    }
    if pending > 0 {
        println!(
            "\n{} pending approval(s) — `maestro approvals`",
            ctx.style.bold(&pending.to_string())
        );
    }
}

pub async fn missions(ctx: &Ctx) -> Result<i32> {
    let summaries = match probe_daemon_fast(&ctx.root).await {
        Some(client) => client.list_missions().await?,
        None => local_summaries(&ctx.store()?)?,
    };
    if ctx.json {
        emit_json(&summaries)?;
    } else if summaries.is_empty() {
        println!("{}", ctx.style.dim("no missions"));
    } else {
        print!("{}", missions_table(ctx, &summaries));
    }
    Ok(0)
}

pub async fn mission(ctx: &Ctx, id: &str) -> Result<i32> {
    let detail = match probe_daemon_fast(&ctx.root).await {
        Some(client) => client.mission_detail(id).await?,
        None => local_detail(&ctx.store()?, id)?,
    };
    if ctx.json {
        emit_json(&detail)?;
        return Ok(0);
    }
    let m = &detail.mission;
    println!(
        "mission {} {}",
        ctx.style.accent(&short(&m.id)),
        ctx.style.bold(m.status.as_str())
    );
    println!("  objective: {}", m.objective);
    if let Some(branch) = &m.mission_branch {
        println!(
            "  branch:    {} {}",
            branch,
            ctx.style.dim(&format!(
                "(base {})",
                m.base_head
                    .as_deref()
                    .map(|h| &h[..h.len().min(8)])
                    .unwrap_or("?")
            ))
        );
    }
    println!(
        "  usage:     {} calls, {} in / {} out tokens, {}",
        detail.stats.model_calls,
        detail.stats.input_tokens,
        detail.stats.output_tokens,
        fmt_cost(detail.stats.cost_micro_usd)
    );
    if let Some(plan) = &detail.plan {
        println!("  plan:      {}", clip(&plan.summary, 100));
    }
    if !detail.tasks.is_empty() {
        println!();
        print!("{}", tasks_table(ctx, &detail.tasks));
    }
    Ok(0)
}

pub async fn tasks(ctx: &Ctx, mission_id: &str) -> Result<i32> {
    let tasks = match probe_daemon_fast(&ctx.root).await {
        Some(client) => client.mission_detail(mission_id).await?.tasks,
        None => {
            let store = ctx.store()?;
            store.get_mission(mission_id)?; // NotFound over an empty table
            store.list_tasks(mission_id)?
        }
    };
    if ctx.json {
        emit_json(&tasks)?;
    } else if tasks.is_empty() {
        println!("{}", ctx.style.dim("no tasks (mission not planned yet)"));
    } else {
        print!("{}", tasks_table(ctx, &tasks));
    }
    Ok(0)
}

pub async fn events(ctx: &Ctx, mission_id: &str, follow: bool, after: i64) -> Result<i32> {
    if follow {
        let client = probe_daemon(&ctx.root).await.ok_or_else(no_daemon)?;
        let mut stream = client.events(Some(mission_id.to_string()), after);
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                item = stream.next() => match item {
                    Some(Ok(ev)) => {
                        if ctx.json {
                            println!("{}", serde_json::to_string(&ev)?);
                        } else {
                            println!("{}", raw_event_line(&ev, &ctx.style));
                        }
                    }
                    Some(Err(_)) => eprintln!("{}", ctx.style.dim("event stream disconnected; reconnecting…")),
                    None => break,
                }
            }
        }
        return Ok(0);
    }
    let store = ctx.store()?;
    store.get_mission(mission_id)?;
    let events = store.events_after(after, Some(mission_id))?;
    if ctx.json {
        emit_json(&events)?;
    } else {
        for ev in &events {
            println!("{}", raw_event_line(ev, &ctx.style));
        }
    }
    Ok(0)
}

pub async fn approvals(ctx: &Ctx) -> Result<i32> {
    let approvals = match probe_daemon_fast(&ctx.root).await {
        Some(client) => client.approvals(None).await?,
        None => ctx.store()?.pending_approvals(None)?,
    };
    if ctx.json {
        emit_json(&approvals)?;
    } else if approvals.is_empty() {
        println!("{}", ctx.style.dim("no pending approvals"));
    } else {
        print!("{}", approvals_table(ctx, &approvals));
    }
    Ok(0)
}

pub async fn decide_approval(ctx: &Ctx, approval_id: &str, approve: bool) -> Result<i32> {
    let verb = if approve { "approved" } else { "rejected" };
    if let Some(client) = probe_daemon(&ctx.root).await {
        client
            .approval_decide(
                approval_id,
                &ApprovalDecisionRequest {
                    approve,
                    note: None,
                },
            )
            .await?;
        if ctx.json {
            emit_json(&serde_json::json!({
                "approvalId": approval_id,
                "status": verb,
                "via": "daemon",
            }))?;
        } else {
            println!("approval {} {}", short(approval_id), ctx.style.bold(verb));
        }
        return Ok(0);
    }

    // Local fallback: decide in the store; approving an integrate approval
    // also finalizes the mission (the daemon does the same server-side).
    let store = ctx.store()?;
    let approval = store.decide_approval(approval_id, approve, "cli", None)?;
    store.append_event(
        &approval.mission_id,
        approval.task_id.as_deref(),
        kind::APPROVAL_DECIDED,
        serde_json::json!({
            "approvalId": approval.id,
            "status": approval.status.as_str(),
            "decidedBy": "cli",
        }),
    )?;
    let mut finalize = None;
    if approve && approval.action == "integrate" {
        let (orch, _store, _config) = ctx.orchestrator()?;
        let outcome = orch.finalize_mission(&approval.mission_id).await?;
        finalize = Some(outcome);
    }
    if ctx.json {
        emit_json(&serde_json::json!({
            "approvalId": approval.id,
            "status": approval.status.as_str(),
            "via": "local",
            "finalized": finalize.as_ref().map(|o| serde_json::json!({
                "commit": o.commit,
                "branch": o.branch,
            })),
        }))?;
    } else {
        println!("approval {} {}", short(&approval.id), ctx.style.bold(verb));
        match &finalize {
            Some(o) => println!(
                "mission finalized: {} fast-forwarded to {}",
                o.branch,
                ctx.style.accent(&o.commit[..o.commit.len().min(12)])
            ),
            None if approve && approval.task_id.is_some() => println!(
                "{}",
                ctx.style
                    .dim("the parked task resumes the next time the mission runs")
            ),
            None => {}
        }
    }
    Ok(0)
}

pub async fn steer(ctx: &Ctx, mission_id: &str, text: &str) -> Result<i32> {
    if let Some(client) = probe_daemon(&ctx.root).await {
        client
            .steer(
                mission_id,
                &SteerRequest {
                    text: text.to_string(),
                    task_id: None,
                },
            )
            .await?;
    } else {
        let store = ctx.store()?;
        store.get_mission(mission_id)?;
        let directive = store.add_directive(mission_id, None, text, "cli")?;
        store.append_event(
            mission_id,
            None,
            kind::DIRECTIVE_ADDED,
            serde_json::json!({ "directiveId": directive.id, "text": text }),
        )?;
    }
    if ctx.json {
        emit_json(&serde_json::json!({ "missionId": mission_id, "steered": true }))?;
    } else {
        println!("directive recorded for {}", short(mission_id));
    }
    Ok(0)
}

/// pause / resume / cancel — daemon only (there is no local scheduler to act).
pub async fn lifecycle(ctx: &Ctx, mission_id: &str, op: &str) -> Result<i32> {
    let client = probe_daemon(&ctx.root).await.ok_or_else(no_daemon)?;
    match op {
        "pause" => client.pause(mission_id).await?,
        "resume" => client.resume(mission_id).await?,
        "cancel" => client.cancel(mission_id).await?,
        other => return Err(MaestroError::Other(format!("unknown lifecycle op {other}"))),
    }
    if ctx.json {
        emit_json(&serde_json::json!({ "missionId": mission_id, "op": op, "ok": true }))?;
    } else {
        println!("{op} requested for {}", short(mission_id));
    }
    Ok(0)
}

pub async fn retry(
    ctx: &Ctx,
    mission_id: &str,
    task_id: &str,
    model: Option<String>,
) -> Result<i32> {
    if let Some(client) = probe_daemon(&ctx.root).await {
        client
            .retry(
                mission_id,
                &RetryRequest {
                    task_id: task_id.to_string(),
                    model_override: model.clone(),
                },
            )
            .await?;
    } else {
        let store = ctx.store()?;
        let task = store.get_task(task_id)?;
        if task.mission_id != mission_id {
            return Err(MaestroError::Conflict(format!(
                "task {task_id} does not belong to mission {mission_id}"
            )));
        }
        let task = store.retry_task(task_id, model.clone())?;
        store.append_event(
            mission_id,
            Some(task_id),
            kind::TASK_RETRY,
            serde_json::json!({
                "manual": true,
                "modelOverride": model,
                "taskKey": task.task_key,
            }),
        )?;
    }
    if ctx.json {
        emit_json(&serde_json::json!({
            "missionId": mission_id,
            "taskId": task_id,
            "modelOverride": model,
            "ok": true,
        }))?;
    } else {
        println!(
            "task {} back to ready{}",
            short(task_id),
            model
                .as_deref()
                .map(|m| format!(" on {m}"))
                .unwrap_or_default()
        );
    }
    Ok(0)
}

pub async fn plan(ctx: &Ctx, mission_id: &str, approve: bool, reject: bool) -> Result<i32> {
    // Viewing is read-only (fast probe); deciding mutates (full budget).
    let daemon = if approve || reject {
        probe_daemon(&ctx.root).await
    } else {
        probe_daemon_fast(&ctx.root).await
    };
    if approve || reject {
        match &daemon {
            Some(client) => {
                client
                    .plan_decide(
                        mission_id,
                        &PlanDecisionRequest {
                            approve,
                            edited_plan: None,
                        },
                    )
                    .await?;
            }
            None => {
                let (orch, _store, _config) = ctx.orchestrator()?;
                orch.decide_plan(mission_id, approve, None).await?;
            }
        }
        let verb = if approve { "approved" } else { "rejected" };
        if ctx.json {
            emit_json(&serde_json::json!({ "missionId": mission_id, "plan": verb }))?;
        } else {
            println!("plan {}", ctx.style.bold(verb));
        }
        return Ok(0);
    }
    let plan = match &daemon {
        Some(client) => client.plan_get(mission_id).await?,
        None => ctx.store()?.get_mission_plan(mission_id)?.ok_or_else(|| {
            MaestroError::NotFound(format!("mission {mission_id} has no plan yet"))
        })?,
    };
    if ctx.json {
        emit_json(&plan)?;
    } else {
        print!("{}", plan_text(ctx, &plan));
    }
    Ok(0)
}

pub async fn diff(ctx: &Ctx, mission_id: &str) -> Result<i32> {
    let (unified, patch_hash) = match probe_daemon_fast(&ctx.root).await {
        Some(client) => {
            let resp = client.diff(mission_id).await?;
            (resp.unified, resp.patch_hash)
        }
        None => {
            let (orch, _store, _config) = ctx.orchestrator()?;
            orch.mission_diff(mission_id).await?
        }
    };
    if ctx.json {
        emit_json(&serde_json::json!({ "unified": unified, "patchHash": patch_hash }))?;
    } else if unified.trim().is_empty() {
        println!("{}", ctx.style.dim("no changes"));
    } else {
        print!("{}", tint_diff(&unified, &ctx.style));
    }
    Ok(0)
}

pub async fn submit(ctx: &Ctx, objective: &str) -> Result<i32> {
    ctx.config()?; // fail before spawning a daemon in an uninitialized dir
    let client = ensure_daemon(&ctx.root).await?;
    let resp = client.submit_mission(objective, None).await?;
    if ctx.json {
        emit_json(&serde_json::json!({ "missionId": resp.mission_id }))?;
    } else {
        println!("mission {}", ctx.style.accent(&resp.mission_id));
        println!(
            "{}",
            ctx.style.dim(&format!(
                "follow with `maestro events {} --follow`",
                short(&resp.mission_id)
            ))
        );
    }
    Ok(0)
}

pub async fn shutdown(ctx: &Ctx) -> Result<i32> {
    let client = probe_daemon(&ctx.root).await.ok_or_else(no_daemon)?;
    client.shutdown().await?;
    if ctx.json {
        emit_json(&serde_json::json!({ "shutdown": true }))?;
    } else {
        println!("daemon shutting down");
    }
    Ok(0)
}
