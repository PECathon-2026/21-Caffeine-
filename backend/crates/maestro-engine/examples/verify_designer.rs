//! Replay a Designer task against a real repository with live configured models.
//! Stores evidence separately and never schedules implementation or integration.
//! Usage: cargo run -p maestro-engine --example verify_designer -- ROOT TASK_JSON EVIDENCE_DIR
use maestro_engine::Orchestrator;
use maestro_providers::ProviderRegistry;
use maestro_store::Store;
use maestro_types::{
    config::IntegrationStrategy,
    domain::{MissionStatus, Role},
    plan::PlanTask,
};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("Usage: verify_designer ROOT TASK_JSON EVIDENCE_DIR".into());
    }
    let root = PathBuf::from(&args[1]);
    let input: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&args[2])?)?;
    let task: PlanTask = serde_json::from_value(input["task"].clone())?;
    if task.role != Role::Designer || !task.dependencies.is_empty() {
        return Err("Only an independent Designer task may be replayed".into());
    }
    let mut config = maestro_engine::load_config(&root)?;
    maestro_engine::inject_env(&config, &maestro_engine::load_credentials(&root)?);
    // No coder or patch exists; direct completion avoids creating integration refs.
    config.integration.strategy = IntegrationStrategy::Direct;
    let store = Store::open(&PathBuf::from(&args[3]))?;
    let registry = Arc::new(ProviderRegistry::new(&config));
    let orch = Orchestrator::new(
        store.clone(),
        config,
        registry,
        root,
        "designer-verification".into(),
    );
    let mission = orch
        .create_mission(input["objective"].as_str().ok_or("Missing objective")?)
        .await?;
    store.insert_tasks(&mission.id, &[task])?;
    store.set_mission_status(&mission.id, MissionStatus::Planned, None)?;
    store.refresh_ready(&mission.id)?;
    let start = std::time::Instant::now();
    let status = orch
        .run_mission(&mission.id, &CancellationToken::new())
        .await?;
    println!(
        "{}",
        serde_json::json!({"missionId":mission.id,"status":status.as_str(),"elapsedSeconds":start.elapsed().as_secs(),"tasks":store.list_tasks(&mission.id)?})
    );
    if status != MissionStatus::Completed {
        return Err("Designer did not complete; inspect saved evidence".into());
    }
    Ok(())
}
