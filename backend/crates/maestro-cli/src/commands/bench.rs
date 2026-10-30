//! `maestro bench`: the latency harness behind SPEC's <10ms control-plane
//! budget. Runs a synthetic workload against a scratch store in a temp
//! directory — N event appends, N lease acquire/release CAS cycles, N policy
//! evaluations — and reports p50/p99/max per op from the store's own metrics
//! recorder. Reports only; never asserts.

use super::emit_json;
use crate::context::Ctx;
use crate::output::render_table;
use maestro_engine::{profile_rules, PolicyEngine};
use maestro_store::Store;
use maestro_types::config::PolicyAction;
use maestro_types::domain::{RiskLevel, Role, TaskStatus};
use maestro_types::plan::PlanTask;
use maestro_types::{new_id, MaestroError, Result};
use std::time::Instant;

/// Ops surfaced in the report, in display order.
const REPORT_OPS: [&str; 4] = [
    "event.append",
    "lease.acquire",
    "task.set_status",
    "policy.evaluate",
];

const BUDGET_US: i64 = 10_000; // SPEC §Goals: <10ms per control-plane op.

pub async fn bench(ctx: &Ctx, iterations: usize) -> Result<i32> {
    let scratch = std::env::temp_dir().join(format!("maestro-bench-{}", new_id()));
    let outcome = run_workload(ctx, &scratch, iterations.max(1));
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

fn run_workload(ctx: &Ctx, scratch: &std::path::Path, iterations: usize) -> Result<i32> {
    let store = Store::open(scratch)?;
    let mission = store.create_mission("bench workload")?;
    let spec = PlanTask {
        key: "bench-lease".to_string(),
        role: Role::Coder,
        title: "bench lease target".to_string(),
        description: "scratch task for lease CAS cycles".to_string(),
        dependencies: vec![],
        acceptance: vec![],
        risk: RiskLevel::Low,
        write_scopes: vec!["src/**".to_string()],
    };
    let tasks = store.insert_tasks(&mission.id, &[spec])?;
    store.refresh_ready(&mission.id)?;
    let task_id = tasks
        .first()
        .map(|t| t.id.clone())
        .ok_or_else(|| MaestroError::Other("bench task was not created".into()))?;

    // 1) event appends
    let started = Instant::now();
    for i in 0..iterations {
        store.append_event(
            &mission.id,
            None,
            "bench.tick",
            serde_json::json!({ "i": i }),
        )?;
    }
    let events_ms = started.elapsed().as_millis();

    // 2) lease acquire/release cycles (CAS acquire, status flip back releases
    //    the lease + scope locks)
    let started = Instant::now();
    for _ in 0..iterations {
        if !store.acquire_lease(&task_id, "bench-worker", "bench-gen", 60_000)? {
            return Err(MaestroError::Other("lease CAS unexpectedly lost".into()));
        }
        store.set_task_status(&task_id, TaskStatus::Ready, None, None)?;
    }
    let lease_ms = started.elapsed().as_millis();

    // 3) policy evaluations (manually timed into the same recorder)
    let engine = PolicyEngine::new(Role::Coder, &profile_rules("balanced"))?;
    let started = Instant::now();
    for _ in 0..iterations {
        let t = Instant::now();
        let _ = engine.evaluate(PolicyAction::Edit, "src/main.rs");
        store
            .metrics()
            .record("policy.evaluate", t.elapsed().as_micros() as u64);
    }
    let policy_ms = started.elapsed().as_millis();

    let snapshot = store.metrics().snapshot();
    let mut ops: Vec<maestro_types::api::OpMetrics> = Vec::new();
    for name in REPORT_OPS {
        if let Some(m) = snapshot.iter().find(|m| m.op == name) {
            ops.push(m.clone());
        }
    }

    if ctx.json {
        emit_json(&serde_json::json!({
            "iterations": iterations,
            "budgetUs": BUDGET_US,
            "wallMs": { "events": events_ms as i64, "lease": lease_ms as i64, "policy": policy_ms as i64 },
            "ops": ops,
        }))?;
        return Ok(0);
    }

    println!(
        "bench: {iterations} iterations per op {}",
        ctx.style.dim(&format!(
            "(events {events_ms}ms, lease cycles {lease_ms}ms, policy {policy_ms}ms wall)"
        ))
    );
    println!();
    let rows: Vec<Vec<String>> = ops
        .iter()
        .map(|m| {
            vec![
                m.op.clone(),
                m.count.to_string(),
                m.p50_us.to_string(),
                m.p99_us.to_string(),
                m.max_us.to_string(),
                if m.p99_us <= BUDGET_US {
                    "ok".to_string()
                } else {
                    "OVER".to_string()
                },
            ]
        })
        .collect();
    print!(
        "{}",
        render_table(
            &["OP", "COUNT", "P50(µs)", "P99(µs)", "MAX(µs)", "<10ms"],
            &rows,
            &ctx.style
        )
    );
    println!(
        "\n{}",
        ctx.style
            .dim("budget: 10ms per control-plane op (SPEC §Goals); report only, no assertion")
    );
    Ok(0)
}
