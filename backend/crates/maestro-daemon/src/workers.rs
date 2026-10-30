//! Mission workers: one owned `tokio` task per driven mission, keyed by
//! mission id. Every `JoinHandle` is held in the map and cancelled + awaited
//! (with a bounded grace period) on shutdown — no detached tasks.

use maestro_engine::Orchestrator;
use maestro_store::Store;
use maestro_types::domain::{MissionStatus, TaskStatus};
use maestro_types::event::kind;
use maestro_types::{new_id, now_ms, MaestroError, Millis};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

struct WorkerEntry {
    /// Distinguishes this spawn from any later respawn for the same mission.
    worker_id: String,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
    started_at: Millis,
}

/// Registry of live mission workers. Cheap to clone; all clones share one map.
#[derive(Clone)]
pub(crate) struct Workers {
    map: Arc<Mutex<HashMap<String, WorkerEntry>>>,
}

impl Workers {
    pub(crate) fn new() -> Workers {
        Workers {
            map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, WorkerEntry>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `(mission_id, started_at)` for every live worker.
    pub(crate) fn snapshot(&self) -> Vec<(String, Millis)> {
        self.lock()
            .iter()
            .filter(|(_, w)| !w.handle.is_finished())
            .map(|(id, w)| (id.clone(), w.started_at))
            .collect()
    }

    /// The cancellation token of a live worker for this mission, if any.
    pub(crate) fn cancel_token(&self, mission_id: &str) -> Option<CancellationToken> {
        self.lock()
            .get(mission_id)
            .filter(|w| !w.handle.is_finished())
            .map(|w| w.cancel.clone())
    }

    /// Spawn a worker driving `run_mission` unless a live one already exists.
    /// Returns true when a new worker was spawned.
    pub(crate) fn ensure(&self, orch: &Orchestrator, mission_id: &str) -> bool {
        let mut map = self.lock();
        if let Some(existing) = map.get(mission_id) {
            if !existing.handle.is_finished() {
                return false;
            }
        }
        let cancel = CancellationToken::new();
        let worker_id = new_id();
        let handle = tokio::spawn(run_worker(
            orch.clone(),
            self.clone(),
            mission_id.to_string(),
            worker_id.clone(),
            cancel.clone(),
        ));
        map.insert(
            mission_id.to_string(),
            WorkerEntry {
                worker_id,
                cancel,
                handle,
                started_at: now_ms(),
            },
        );
        true
    }

    /// Spawn a worker iff the mission still needs driving (see
    /// [`mission_needs_driving`]). Closes the ensure/deregister race: an API
    /// call may observe an exiting worker as live (`ensure` returns false)
    /// after `run_mission` returned but before `remove_if` ran, so its poke
    /// lands nowhere. Called once per worker exit; the fresh worker repeats
    /// the check on its own exit, keeping the recursion bounded.
    pub(crate) fn respawn_if_needed(&self, orch: &Orchestrator, mission_id: &str) -> bool {
        mission_needs_driving(orch.store(), mission_id) && self.ensure(orch, mission_id)
    }

    /// Remove the entry for `mission_id` iff it still belongs to `worker_id`
    /// (a respawn may have replaced it).
    fn remove_if(&self, mission_id: &str, worker_id: &str) {
        let mut map = self.lock();
        if map
            .get(mission_id)
            .is_some_and(|w| w.worker_id == worker_id)
        {
            map.remove(mission_id);
        }
    }

    /// Cancel every worker, await each for up to `grace` total, then abort
    /// and await whatever remains. Nothing is left running.
    pub(crate) async fn shutdown_all(&self, grace: Duration) {
        let entries: Vec<WorkerEntry> = {
            let mut map = self.lock();
            map.drain().map(|(_, w)| w).collect()
        };
        for w in &entries {
            w.cancel.cancel();
        }
        let deadline = tokio::time::Instant::now() + grace;
        for mut w in entries {
            // `&mut JoinHandle` is a Future; on timeout we still own the
            // handle, so the straggler is aborted and awaited — never leaked.
            if tokio::time::timeout_at(deadline, &mut w.handle)
                .await
                .is_err()
            {
                tracing::warn!("mission worker exceeded the shutdown grace period; aborting");
                w.handle.abort();
                let _ = w.handle.await;
            }
        }
    }
}

/// Body of one mission worker: drive the mission to a resting state, then
/// deregister. Non-cancellation failures mark the mission `failed` so it
/// never sits in `running` with nobody driving it.
async fn run_worker(
    orch: Orchestrator,
    workers: Workers,
    mission_id: String,
    worker_id: String,
    cancel: CancellationToken,
) {
    match orch.run_mission(&mission_id, &cancel).await {
        Ok(status) => {
            tracing::info!(mission_id = %mission_id, status = status.as_str(), "mission worker finished");
        }
        Err(MaestroError::Cancelled) => {
            // pause/cancel/shutdown set the mission status before cancelling.
            tracing::info!(mission_id = %mission_id, "mission worker cancelled");
        }
        Err(e) => {
            tracing::error!(mission_id = %mission_id, error = %e, "mission worker failed");
            let msg = e.to_string();
            let failed =
                orch.store()
                    .set_mission_status(&mission_id, MissionStatus::Failed, Some(&msg));
            if failed.is_ok() {
                let _ = orch.store().append_event(
                    &mission_id,
                    None,
                    kind::MISSION_STATUS,
                    serde_json::json!({ "status": "failed", "reason": msg }),
                );
            }
        }
    }
    workers.remove_if(&mission_id, &worker_id);
    // Cancelled/failed missions are terminal and never re-driven: reclaim
    // their worktrees and branches now that no worker can be touching them.
    // Re-read the store rather than trusting the return value — shutdown
    // also cancels workers but leaves missions resumable, and those keep
    // their checkouts.
    match orch.store().get_mission(&mission_id) {
        Ok(mission)
            if matches!(
                mission.status,
                MissionStatus::Cancelled | MissionStatus::Failed
            ) =>
        {
            // A failed mission keeps its branch: any merged-but-unfinalized
            // work stays reachable. Only an explicit cancel abandons it.
            let delete_branch = mission.status == MissionStatus::Cancelled;
            orch.cleanup_mission_workspaces(&mission_id, delete_branch)
                .await;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(mission_id = %mission_id, error = %e, "could not re-read mission for cleanup");
        }
    }
    // Anything poked between run_mission returning and remove_if above found
    // a live-looking handle and did not spawn; re-check once now that the
    // registration is gone. Skipped when cancelled: pause/cancel/shutdown
    // must not resurrect the worker.
    if !cancel.is_cancelled() {
        workers.respawn_if_needed(&orch, &mission_id);
    }
}

/// Whether a mission at rest still has work a worker would drive:
/// `planned`/`running` always do; `planning`/`blocked` only when a task is
/// ready (e.g. released by an approval or a retry). Paused, awaiting-approval
/// and terminal missions are never re-driven from here — those transitions go
/// through their own API handlers.
fn mission_needs_driving(store: &Store, mission_id: &str) -> bool {
    let Ok(mission) = store.get_mission(mission_id) else {
        return false;
    };
    match mission.status {
        MissionStatus::Planned | MissionStatus::Running => true,
        MissionStatus::Planning | MissionStatus::Blocked => store
            .list_tasks(mission_id)
            .map(|tasks| tasks.iter().any(|t| t.status == TaskStatus::Ready))
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_providers::ProviderRegistry;
    use maestro_types::config::MaestroConfig;
    use maestro_types::domain::{RiskLevel, Role};
    use maestro_types::plan::PlanTask;
    use std::sync::Arc;

    fn seeded_store(dir: &std::path::Path) -> (Store, String) {
        let store = Store::open(dir).unwrap();
        let mission = store.create_mission("drive me").unwrap();
        store
            .insert_tasks(
                &mission.id,
                &[PlanTask {
                    key: "design".into(),
                    role: Role::Designer,
                    title: "Design".into(),
                    description: "Sketch it.".into(),
                    dependencies: vec![],
                    acceptance: vec!["done".into()],
                    risk: RiskLevel::Low,
                    write_scopes: vec![],
                }],
            )
            .unwrap();
        store.refresh_ready(&mission.id).unwrap();
        (store, mission.id)
    }

    #[test]
    fn needs_driving_matches_resting_states() {
        let dir = tempfile::tempdir().unwrap();
        let (store, mission_id) = seeded_store(dir.path());

        store
            .set_mission_status(&mission_id, MissionStatus::Planned, None)
            .unwrap();
        assert!(mission_needs_driving(&store, &mission_id));

        // Blocked counts only while a task is ready.
        store
            .set_mission_status(&mission_id, MissionStatus::Blocked, Some("x"))
            .unwrap();
        assert!(mission_needs_driving(&store, &mission_id));
        let task = &store.list_tasks(&mission_id).unwrap()[0];
        store
            .set_task_status(&task.id, TaskStatus::Blocked, Some("agent declared"), None)
            .unwrap();
        assert!(!mission_needs_driving(&store, &mission_id));

        for status in [
            MissionStatus::Paused,
            MissionStatus::AwaitingApproval,
            MissionStatus::Completed,
            MissionStatus::Failed,
            MissionStatus::Cancelled,
        ] {
            store.set_mission_status(&mission_id, status, None).unwrap();
            assert!(
                !mission_needs_driving(&store, &mission_id),
                "{} must not be re-driven",
                status.as_str()
            );
        }
        assert!(!mission_needs_driving(&store, "no-such-mission"));
    }

    /// Finding: ensure/deregister race — a planned mission with a ready task
    /// and no registered worker must be picked up by the post-exit re-check.
    #[tokio::test(flavor = "multi_thread")]
    async fn respawn_if_needed_spawns_for_a_planned_mission_with_ready_work() {
        let dir = tempfile::tempdir().unwrap();
        let (store, mission_id) = seeded_store(dir.path());
        store
            .set_mission_status(&mission_id, MissionStatus::Planned, None)
            .unwrap();

        let config = MaestroConfig::default();
        let registry = Arc::new(ProviderRegistry::new(&config));
        let orch = Orchestrator::new(
            store.clone(),
            config,
            registry,
            dir.path().to_path_buf(),
            "daemon_test".into(),
        );
        let workers = Workers::new();
        assert!(
            workers.respawn_if_needed(&orch, &mission_id),
            "worker spawned"
        );
        // A second call while that worker is live must not double-spawn.
        assert!(!workers.respawn_if_needed(&orch, &mission_id));
        workers.shutdown_all(Duration::from_secs(5)).await;

        // A completed mission is left alone.
        store
            .set_mission_status(&mission_id, MissionStatus::Completed, None)
            .unwrap();
        assert!(!workers.respawn_if_needed(&orch, &mission_id));
        assert!(workers.snapshot().is_empty());
    }
}
