//! Task rows: planning inserts, delegation, readiness propagation, leases
//! (CAS), status transitions, and retries with cascade.

use crate::{db_err, invalid_col, json_col, Store};
use maestro_types::domain::{RiskLevel, Role, Task, TaskStatus};
use maestro_types::plan::PlanTask;
use maestro_types::{new_id, now_ms, MaestroError, Millis, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet, VecDeque};

pub(crate) const TASK_COLS: &str =
    "id, mission_id, task_key, role, title, description, status, dependencies_json, \
     acceptance_json, risk, attempts, status_reason, summary, lease_owner, lease_expires_at, \
     lease_generation, workspace_path, branch_name, base_ref, patch_hash, integrated_at, \
     integration_commit, parent_task_id, delegation_depth, write_scopes_json, model_override, \
     created_at, updated_at";

pub(crate) fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let role: String = row.get(3)?;
    let status: String = row.get(6)?;
    let deps: String = row.get(7)?;
    let acceptance: String = row.get(8)?;
    let risk: String = row.get(9)?;
    let scopes: String = row.get(24)?;
    Ok(Task {
        id: row.get(0)?,
        mission_id: row.get(1)?,
        task_key: row.get(2)?,
        role: Role::parse(&role).ok_or_else(|| invalid_col("task role", &role))?,
        title: row.get(4)?,
        description: row.get(5)?,
        status: TaskStatus::parse(&status).ok_or_else(|| invalid_col("task status", &status))?,
        dependencies: json_col("task dependencies", &deps)?,
        acceptance: json_col("task acceptance", &acceptance)?,
        risk: RiskLevel::parse(&risk).ok_or_else(|| invalid_col("task risk", &risk))?,
        attempts: row.get(10)?,
        status_reason: row.get(11)?,
        summary: row.get(12)?,
        lease_owner: row.get(13)?,
        lease_expires_at: row.get(14)?,
        lease_generation: row.get(15)?,
        workspace_path: row.get(16)?,
        branch_name: row.get(17)?,
        base_ref: row.get(18)?,
        patch_hash: row.get(19)?,
        integrated_at: row.get(20)?,
        integration_commit: row.get(21)?,
        parent_task_id: row.get(22)?,
        delegation_depth: row.get(23)?,
        write_scopes: json_col("task write scopes", &scopes)?,
        model_override: row.get(25)?,
        created_at: row.get(26)?,
        updated_at: row.get(27)?,
    })
}

fn get_task_conn(conn: &Connection, task_id: &str) -> Result<Task> {
    let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1");
    conn.prepare_cached(&sql)
        .map_err(db_err)?
        .query_row(params![task_id], task_from_row)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| MaestroError::NotFound(format!("task {task_id}")))
}

fn insert_task_row(
    conn: &Connection,
    mission_id: &str,
    spec: &PlanTask,
    parent_task_id: Option<&str>,
    delegation_depth: i64,
    now: Millis,
) -> Result<Task> {
    let task = Task {
        id: new_id(),
        mission_id: mission_id.to_string(),
        task_key: spec.key.clone(),
        role: spec.role,
        title: spec.title.clone(),
        description: spec.description.clone(),
        status: TaskStatus::Proposed,
        dependencies: spec.dependencies.clone(),
        acceptance: spec.acceptance.clone(),
        risk: spec.risk,
        attempts: 0,
        status_reason: None,
        summary: None,
        lease_owner: None,
        lease_expires_at: None,
        lease_generation: None,
        workspace_path: None,
        branch_name: None,
        base_ref: None,
        patch_hash: None,
        integrated_at: None,
        integration_commit: None,
        parent_task_id: parent_task_id.map(str::to_string),
        delegation_depth,
        write_scopes: spec.write_scopes.clone(),
        model_override: None,
        created_at: now,
        updated_at: now,
    };
    let mut stmt = conn
        .prepare_cached(
            "INSERT INTO tasks (id, mission_id, task_key, role, title, description, status, \
             dependencies_json, acceptance_json, risk, delegation_depth, parent_task_id, \
             write_scopes_json, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        )
        .map_err(db_err)?;
    stmt.execute(params![
        task.id,
        task.mission_id,
        task.task_key,
        task.role.as_str(),
        task.title,
        task.description,
        task.status.as_str(),
        serde_json::to_string(&task.dependencies)?,
        serde_json::to_string(&task.acceptance)?,
        task.risk.as_str(),
        task.delegation_depth,
        task.parent_task_id,
        serde_json::to_string(&task.write_scopes)?,
        now,
        now
    ])
    .map_err(|e| match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            MaestroError::Conflict(format!(
                "task key {:?} already exists in mission {mission_id}",
                task.task_key
            ))
        }
        _ => db_err(e),
    })?;
    Ok(task)
}

/// Reason written by [`Store::refresh_ready`] when it blocks a dependent.
/// Only tasks blocked with this exact reason are ever auto-unblocked; blocks
/// with other reasons (approval-parked, agent-declared) are left alone.
const DEP_BLOCKED_REASON: &str = "dependency failed or blocked";

/// Lightweight per-mission dependency view used by readiness / retry logic.
struct DepRow {
    id: String,
    key: String,
    role: Role,
    status: TaskStatus,
    status_reason: Option<String>,
    deps: Vec<String>,
}

impl DepRow {
    /// Blocked purely because of dependency state (reversible by refresh).
    fn dep_blocked(&self) -> bool {
        self.status == TaskStatus::Blocked
            && self.status_reason.as_deref() == Some(DEP_BLOCKED_REASON)
    }
}

fn load_dep_rows(conn: &Connection, mission_id: &str) -> Result<Vec<DepRow>> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, task_key, role, status, status_reason, dependencies_json
             FROM tasks WHERE mission_id = ?1",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![mission_id], |row| {
            let role: String = row.get(2)?;
            let status: String = row.get(3)?;
            let deps: String = row.get(5)?;
            Ok(DepRow {
                id: row.get(0)?,
                key: row.get(1)?,
                role: Role::parse(&role).ok_or_else(|| invalid_col("task role", &role))?,
                status: TaskStatus::parse(&status)
                    .ok_or_else(|| invalid_col("task status", &status))?,
                status_reason: row.get(4)?,
                deps: json_col("task dependencies", &deps)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
}

fn clear_scope_locks(conn: &Connection, task_id: &str) -> Result<()> {
    conn.prepare_cached("DELETE FROM scope_locks WHERE task_id = ?1")
        .map_err(db_err)?
        .execute(params![task_id])
        .map_err(db_err)?;
    Ok(())
}

/// Keys of tasks that transitively depend on `root_key` (excludes the root).
fn descendant_keys(rows: &[DepRow], root_key: &str) -> HashSet<String> {
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for row in rows {
        for dep in &row.deps {
            dependents.entry(dep.as_str()).or_default().push(&row.key);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::from([root_key]);
    while let Some(key) = queue.pop_front() {
        if let Some(children) = dependents.get(key) {
            for child in children {
                if seen.insert((*child).to_string()) {
                    queue.push_back(child);
                }
            }
        }
    }
    seen
}

impl Store {
    /// Insert the validated plan's tasks (status `proposed`, fresh ids,
    /// delegation depth 0) atomically. Call [`Store::refresh_ready`] after.
    pub fn insert_tasks(&self, mission_id: &str, plan_tasks: &[PlanTask]) -> Result<Vec<Task>> {
        self.with_conn("task.insert_batch", |conn| {
            let tx = conn.transaction().map_err(db_err)?;
            let now = now_ms();
            let mut out = Vec::with_capacity(plan_tasks.len());
            for spec in plan_tasks {
                out.push(insert_task_row(&tx, mission_id, spec, None, 0, now)?);
            }
            tx.commit().map_err(db_err)?;
            Ok(out)
        })
    }

    /// Insert one delegated task under `parent_task_id`.
    ///
    /// Enforces: delegation depth ≤ `max_depth`, mission task count <
    /// `max_mission_tasks`, and non-empty write scopes for coder tasks.
    pub fn add_delegated_task(
        &self,
        mission_id: &str,
        parent_task_id: &str,
        spec: &PlanTask,
        max_depth: i64,
        max_mission_tasks: i64,
    ) -> Result<Task> {
        self.with_conn("task.delegate", |conn| {
            if spec.role == Role::Coder && spec.write_scopes.is_empty() {
                return Err(MaestroError::PlanInvalid(format!(
                    "delegated coder task {:?} must declare write scopes",
                    spec.key
                )));
            }
            let tx = conn.transaction().map_err(db_err)?;
            let parent = get_task_conn(&tx, parent_task_id)?;
            if parent.mission_id != mission_id {
                return Err(MaestroError::Conflict(format!(
                    "parent task {parent_task_id} belongs to another mission"
                )));
            }
            let depth = parent.delegation_depth + 1;
            if depth > max_depth {
                return Err(MaestroError::Conflict(format!(
                    "delegation depth {depth} exceeds limit {max_depth}"
                )));
            }
            let count: i64 = tx
                .prepare_cached("SELECT COUNT(*) FROM tasks WHERE mission_id = ?1")
                .map_err(db_err)?
                .query_row(params![mission_id], |r| r.get(0))
                .map_err(db_err)?;
            if count >= max_mission_tasks {
                return Err(MaestroError::Conflict(format!(
                    "mission task cap {max_mission_tasks} reached"
                )));
            }
            let task =
                insert_task_row(&tx, mission_id, spec, Some(parent_task_id), depth, now_ms())?;
            tx.commit().map_err(db_err)?;
            Ok(task)
        })
    }

    /// Fetch one task; `NotFound` if absent.
    pub fn get_task(&self, task_id: &str) -> Result<Task> {
        self.with_conn("task.get", |conn| get_task_conn(conn, task_id))
    }

    /// All tasks of a mission in insertion order.
    pub fn list_tasks(&self, mission_id: &str) -> Result<Vec<Task>> {
        self.with_conn("task.list", |conn| {
            let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE mission_id = ?1 ORDER BY rowid");
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt
                .query_map(params![mission_id], task_from_row)
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }

    /// Propagate dependency state within a mission until fixpoint:
    /// `proposed → ready` when every dependency is completed;
    /// `proposed|ready → blocked` when any dependency is failed or blocked.
    /// Tasks previously blocked by this propagation (status_reason
    /// "dependency failed or blocked") are reconsidered: they unblock back to
    /// `proposed`/`ready` when their dependencies recover (e.g. an
    /// approval-parked dependency later completes). Blocks with any other
    /// reason are never touched. Dependencies are task keys within the
    /// mission; unknown keys never complete. Returns the number of tasks
    /// whose status changed.
    pub fn refresh_ready(&self, mission_id: &str) -> Result<usize> {
        self.with_conn("task.refresh_ready", |conn| {
            let tx = conn.transaction().map_err(db_err)?;
            let rows = load_dep_rows(&tx, mission_id)?;
            // Dep-blocked tasks start the fixpoint as `proposed` so the loop
            // re-derives their true status from current dependency state.
            let mut status: HashMap<String, TaskStatus> = rows
                .iter()
                .map(|r| {
                    let start = if r.dep_blocked() {
                        TaskStatus::Proposed
                    } else {
                        r.status
                    };
                    (r.key.clone(), start)
                })
                .collect();
            loop {
                let mut changed = false;
                for row in &rows {
                    let cur = status[&row.key];
                    if !matches!(cur, TaskStatus::Proposed | TaskStatus::Ready) {
                        continue;
                    }
                    let any_bad = row.deps.iter().any(|d| {
                        matches!(
                            status.get(d),
                            Some(TaskStatus::Failed) | Some(TaskStatus::Blocked)
                        )
                    });
                    let all_done = row
                        .deps
                        .iter()
                        .all(|d| matches!(status.get(d), Some(TaskStatus::Completed)));
                    let next = if any_bad {
                        TaskStatus::Blocked
                    } else if cur == TaskStatus::Proposed && all_done {
                        TaskStatus::Ready
                    } else {
                        cur
                    };
                    if next != cur {
                        status.insert(row.key.clone(), next);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let now = now_ms();
            let mut updated = 0usize;
            {
                let mut stmt = tx
                    .prepare_cached(
                        "UPDATE tasks SET status = ?2, status_reason = ?3, updated_at = ?4
                         WHERE id = ?1",
                    )
                    .map_err(db_err)?;
                for row in &rows {
                    let next = status[&row.key];
                    if next == row.status {
                        continue;
                    }
                    let reason = match next {
                        TaskStatus::Blocked => Some(DEP_BLOCKED_REASON),
                        _ => None,
                    };
                    stmt.execute(params![row.id, next.as_str(), reason, now])
                        .map_err(db_err)?;
                    updated += 1;
                }
            }
            tx.commit().map_err(db_err)?;
            Ok(updated)
        })
    }

    /// Compare-and-swap lease acquisition: one conditional UPDATE that moves a
    /// `ready` task (with no live lease) to `running` and stamps the lease
    /// columns. Returns `true` iff this caller won.
    pub fn acquire_lease(
        &self,
        task_id: &str,
        owner: &str,
        generation: &str,
        ttl_ms: i64,
    ) -> Result<bool> {
        self.with_conn("lease.acquire", |conn| {
            let now = now_ms();
            let n = conn
                .prepare_cached(
                    "UPDATE tasks SET status = 'running', lease_owner = ?2, \
                     lease_expires_at = ?3, lease_generation = ?4, updated_at = ?5
                     WHERE id = ?1 AND status = 'ready'
                       AND (lease_expires_at IS NULL OR lease_expires_at < ?5)",
                )
                .map_err(db_err)?
                .execute(params![task_id, owner, now + ttl_ms, generation, now])
                .map_err(db_err)?;
            Ok(n == 1)
        })
    }

    /// Extend a held lease. Returns `false` unless the task is `running` and
    /// `owner` matches the current lease owner.
    pub fn renew_lease(&self, task_id: &str, owner: &str, ttl_ms: i64) -> Result<bool> {
        self.with_conn("lease.renew", |conn| {
            let now = now_ms();
            let n = conn
                .prepare_cached(
                    "UPDATE tasks SET lease_expires_at = ?3, updated_at = ?4
                     WHERE id = ?1 AND status = 'running' AND lease_owner = ?2",
                )
                .map_err(db_err)?
                .execute(params![task_id, owner, now + ttl_ms, now])
                .map_err(db_err)?;
            Ok(n == 1)
        })
    }

    /// Return every `running` task whose lease expired before `now` to
    /// `ready` (lease cleared, scope locks released). Returns the count.
    pub fn recover_expired_leases(&self, now: Millis) -> Result<usize> {
        self.recover_leases_where(
            "lease.recover_expired",
            "status = 'running' AND lease_expires_at IS NOT NULL AND lease_expires_at < ?1",
            params![now],
        )
    }

    /// Return every `running` task whose lease generation differs from the
    /// current daemon generation to `ready` (post-crash recovery).
    pub fn recover_generation(&self, current_generation: &str) -> Result<usize> {
        self.recover_leases_where(
            "lease.recover_generation",
            "status = 'running' AND (lease_generation IS NULL OR lease_generation != ?1)",
            params![current_generation],
        )
    }

    fn recover_leases_where(
        &self,
        op: &'static str,
        predicate: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<usize> {
        self.with_conn(op, |conn| {
            let tx = conn.transaction().map_err(db_err)?;
            let ids: Vec<String> = {
                let sql = format!("SELECT id FROM tasks WHERE {predicate}");
                let mut stmt = tx.prepare_cached(&sql).map_err(db_err)?;
                let rows = stmt.query_map(args, |r| r.get(0)).map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)?
            };
            let now = now_ms();
            {
                let mut stmt = tx
                    .prepare_cached(
                        "UPDATE tasks SET status = 'ready', lease_owner = NULL, \
                         lease_expires_at = NULL, lease_generation = NULL, \
                         status_reason = 'lease recovered', updated_at = ?2
                         WHERE id = ?1",
                    )
                    .map_err(db_err)?;
                for id in &ids {
                    stmt.execute(params![id, now]).map_err(db_err)?;
                    clear_scope_locks(&tx, id)?;
                    tracing::warn!(target: "maestro_store", task_id = %id, "recovered stale lease");
                }
            }
            tx.commit().map_err(db_err)?;
            Ok(ids.len())
        })
    }

    /// Transition a task's status. Any transition to a non-`running` status
    /// clears the lease columns and releases the task's scope locks. `summary`
    /// only overwrites when `Some`; `reason` is stored as given. Returns the
    /// updated task.
    pub fn set_task_status(
        &self,
        task_id: &str,
        status: TaskStatus,
        reason: Option<&str>,
        summary: Option<&str>,
    ) -> Result<Task> {
        self.with_conn("task.set_status", |conn| {
            let tx = conn.transaction().map_err(db_err)?;
            let now = now_ms();
            let n = if status == TaskStatus::Running {
                tx.prepare_cached(
                    "UPDATE tasks SET status = ?2, status_reason = ?3, \
                     summary = COALESCE(?4, summary), updated_at = ?5 WHERE id = ?1",
                )
                .map_err(db_err)?
                .execute(params![task_id, status.as_str(), reason, summary, now])
                .map_err(db_err)?
            } else {
                tx.prepare_cached(
                    "UPDATE tasks SET status = ?2, status_reason = ?3, \
                     summary = COALESCE(?4, summary), lease_owner = NULL, \
                     lease_expires_at = NULL, lease_generation = NULL, updated_at = ?5
                     WHERE id = ?1",
                )
                .map_err(db_err)?
                .execute(params![task_id, status.as_str(), reason, summary, now])
                .map_err(db_err)?
            };
            if n == 0 {
                return Err(MaestroError::NotFound(format!("task {task_id}")));
            }
            if status != TaskStatus::Running {
                clear_scope_locks(&tx, task_id)?;
            }
            let task = get_task_conn(&tx, task_id)?;
            tx.commit().map_err(db_err)?;
            Ok(task)
        })
    }

    /// Owner-checked variant of [`Store::set_task_status`] for attempt
    /// outcomes: a single CAS `UPDATE … WHERE id = ? AND status = 'running'
    /// AND lease_owner = ?`, so a stale worker whose lease expired and was
    /// reassigned can never clobber the new owner's running task (or its
    /// scope locks). Clears the lease columns and releases scope locks like
    /// [`Store::set_task_status`]. Returns `LeaseLost` when the guard
    /// matches no row (task gone, no longer running, or owned by someone
    /// else). `status` must not be `running`.
    pub fn set_task_status_owned(
        &self,
        task_id: &str,
        owner: &str,
        status: TaskStatus,
        reason: Option<&str>,
        summary: Option<&str>,
    ) -> Result<Task> {
        self.with_conn("task.set_status_owned", |conn| {
            if status == TaskStatus::Running {
                return Err(MaestroError::Conflict(
                    "set_task_status_owned records attempt outcomes; \
                     `running` is not an outcome"
                        .into(),
                ));
            }
            let tx = conn.transaction().map_err(db_err)?;
            let now = now_ms();
            let n = tx
                .prepare_cached(
                    "UPDATE tasks SET status = ?3, status_reason = ?4, \
                     summary = COALESCE(?5, summary), lease_owner = NULL, \
                     lease_expires_at = NULL, lease_generation = NULL, updated_at = ?6
                     WHERE id = ?1 AND status = 'running' AND lease_owner = ?2",
                )
                .map_err(db_err)?
                .execute(params![
                    task_id,
                    owner,
                    status.as_str(),
                    reason,
                    summary,
                    now
                ])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::LeaseLost {
                    task_id: task_id.to_string(),
                });
            }
            clear_scope_locks(&tx, task_id)?;
            let task = get_task_conn(&tx, task_id)?;
            tx.commit().map_err(db_err)?;
            Ok(task)
        })
    }

    /// Owner-checked variant of [`Store::increment_attempts`]: only counts
    /// the attempt while the caller still holds the lease (`running` +
    /// matching `lease_owner`), so a stale worker cannot burn the retry
    /// budget of a task that was reassigned. Returns `LeaseLost` otherwise.
    pub fn increment_attempts_owned(&self, task_id: &str, owner: &str) -> Result<i64> {
        self.with_conn("task.increment_attempts_owned", |conn| {
            conn.prepare_cached(
                "UPDATE tasks SET attempts = attempts + 1, updated_at = ?3
                 WHERE id = ?1 AND status = 'running' AND lease_owner = ?2
                 RETURNING attempts",
            )
            .map_err(db_err)?
            .query_row(params![task_id, owner, now_ms()], |r| r.get(0))
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| MaestroError::LeaseLost {
                task_id: task_id.to_string(),
            })
        })
    }

    /// Retry a task, optionally forcing a different model for the new
    /// attempts (`None` clears any previous override).
    ///
    /// Refuses `running` and already-integrated tasks, and requires every
    /// dependency to be completed. Resets attempts to 0, clears the stale
    /// patch hash / integration columns, moves the task to `ready`, returns
    /// its `blocked` and `ready` descendants (tasks transitively depending
    /// on it, same mission) to `proposed` — a `ready` dependent must not run
    /// against a dependency that just left `completed` — reopens `completed`
    /// reviewer/security descendants (their gates are hash-stale once the
    /// task re-runs), and deletes the task's non-merged merge-queue entries.
    /// Call [`Store::refresh_ready`] after so readiness is re-derived from
    /// actual dependency state. Returns the refreshed task.
    pub fn retry_task(&self, task_id: &str, model_override: Option<String>) -> Result<Task> {
        self.assert_history_mission_current(&self.get_task(task_id)?.mission_id)?;
        self.with_conn("task.retry", |conn| {
            let tx = conn.transaction().map_err(db_err)?;
            let task = get_task_conn(&tx, task_id)?;
            let retired: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM history_retired WHERE mission_id=?1)",
                    [&task.mission_id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if retired {
                return Err(MaestroError::Conflict(
                    "mission predates a restore; submit a new request".into(),
                ));
            }
            if task.status == TaskStatus::Running {
                return Err(MaestroError::Conflict(format!(
                    "task {task_id} is running; cannot retry"
                )));
            }
            if task.integrated_at.is_some() {
                return Err(MaestroError::Conflict(format!(
                    "task {task_id} is already integrated; cannot retry"
                )));
            }
            let rows = load_dep_rows(&tx, &task.mission_id)?;
            let by_key: HashMap<&str, TaskStatus> =
                rows.iter().map(|r| (r.key.as_str(), r.status)).collect();
            for dep in &task.dependencies {
                if by_key.get(dep.as_str()) != Some(&TaskStatus::Completed) {
                    return Err(MaestroError::Conflict(format!(
                        "task {task_id} dependency {dep:?} is not completed"
                    )));
                }
            }
            let now = now_ms();
            // The retried task will produce a new patch: clear the old patch
            // identity and integration columns along with the lease.
            tx.prepare_cached(
                "UPDATE tasks SET status = 'ready', attempts = 0, status_reason = NULL, \
                 lease_owner = NULL, lease_expires_at = NULL, lease_generation = NULL, \
                 patch_hash = NULL, integrated_at = NULL, integration_commit = NULL, \
                 model_override = ?2, updated_at = ?3 WHERE id = ?1",
            )
            .map_err(db_err)?
            .execute(params![task_id, model_override, now])
            .map_err(db_err)?;
            clear_scope_locks(&tx, task_id)?;

            // Cascade over transitive dependents: blocked AND ready ones go
            // back to proposed (a ready dependent's readiness was derived
            // from the dependency being completed, which just stopped being
            // true — refresh_ready re-derives it), and completed
            // reviewer/security ones are reopened too — their gates are
            // bound to the old patch hash and would be stale by construction
            // once the retried task re-runs.
            let descendants = descendant_keys(&rows, &task.task_key);
            {
                let mut demote_stmt = tx
                    .prepare_cached(
                        "UPDATE tasks SET status = 'proposed', status_reason = NULL, \
                         lease_owner = NULL, lease_expires_at = NULL, \
                         lease_generation = NULL, updated_at = ?2
                         WHERE id = ?1 AND status IN ('blocked', 'ready')",
                    )
                    .map_err(db_err)?;
                let mut reopen_stmt = tx
                    .prepare_cached(
                        "UPDATE tasks SET status = 'proposed', status_reason = NULL, \
                         updated_at = ?2 WHERE id = ?1 AND status = 'completed'",
                    )
                    .map_err(db_err)?;
                for row in &rows {
                    if !descendants.contains(&row.key) {
                        continue;
                    }
                    let demoted = demote_stmt.execute(params![row.id, now]).map_err(db_err)?;
                    if demoted > 0 {
                        clear_scope_locks(&tx, &row.id)?;
                    }
                    if matches!(row.role, Role::Reviewer | Role::Security) {
                        reopen_stmt.execute(params![row.id, now]).map_err(db_err)?;
                    }
                }
            }
            tx.prepare_cached("DELETE FROM merge_queue WHERE task_id = ?1 AND status != 'merged'")
                .map_err(db_err)?
                .execute(params![task_id])
                .map_err(db_err)?;
            let refreshed = get_task_conn(&tx, task_id)?;
            tx.commit().map_err(db_err)?;
            Ok(refreshed)
        })
    }

    /// Bump the attempt counter, returning the new value.
    pub fn increment_attempts(&self, task_id: &str) -> Result<i64> {
        self.with_conn("task.increment_attempts", |conn| {
            conn.prepare_cached(
                "UPDATE tasks SET attempts = attempts + 1, updated_at = ?2 WHERE id = ?1
                 RETURNING attempts",
            )
            .map_err(db_err)?
            .query_row(params![task_id, now_ms()], |r| r.get(0))
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| MaestroError::NotFound(format!("task {task_id}")))
        })
    }

    /// Record the git worktree assigned to a task.
    pub fn set_task_worktree(
        &self,
        task_id: &str,
        workspace_path: &str,
        branch_name: &str,
        base_ref: &str,
    ) -> Result<()> {
        self.with_conn("task.set_worktree", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE tasks SET workspace_path = ?2, branch_name = ?3, base_ref = ?4, \
                     updated_at = ?5 WHERE id = ?1",
                )
                .map_err(db_err)?
                .execute(params![
                    task_id,
                    workspace_path,
                    branch_name,
                    base_ref,
                    now_ms()
                ])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("task {task_id}")));
            }
            Ok(())
        })
    }

    /// Record the sha256 of the task's current diff.
    /// Clear a task's patch hash: a retried attempt that produced no diff
    /// has nothing to review or merge, and a leftover hash from a prior
    /// attempt would demand a gate forever.
    pub fn clear_task_patch_hash(&self, task_id: &str) -> Result<()> {
        self.with_conn("task.clear_patch_hash", |conn| {
            let n = conn
                .prepare_cached("UPDATE tasks SET patch_hash = NULL, updated_at = ?2 WHERE id = ?1")
                .map_err(db_err)?
                .execute(params![task_id, now_ms()])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("task {task_id}")));
            }
            Ok(())
        })
    }

    pub fn set_task_patch_hash(&self, task_id: &str, patch_hash: &str) -> Result<()> {
        self.with_conn("task.set_patch_hash", |conn| {
            let n = conn
                .prepare_cached("UPDATE tasks SET patch_hash = ?2, updated_at = ?3 WHERE id = ?1")
                .map_err(db_err)?
                .execute(params![task_id, patch_hash, now_ms()])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("task {task_id}")));
            }
            Ok(())
        })
    }

    /// Replace a mission's entire task set with `plan_tasks` (plan editing
    /// before execution). Refuses when any existing task has started
    /// (attempts > 0), is running, or was integrated. Scope locks for the
    /// removed tasks are dropped in the same transaction.
    pub fn replace_mission_tasks(
        &self,
        mission_id: &str,
        plan_tasks: &[PlanTask],
    ) -> Result<Vec<Task>> {
        self.with_conn("task.replace_all", |conn| {
            let tx = conn.transaction().map_err(db_err)?;
            let started: i64 = tx
                .prepare_cached(
                    "SELECT COUNT(*) FROM tasks
                     WHERE mission_id = ?1
                       AND (attempts > 0 OR status = 'running' OR integrated_at IS NOT NULL)",
                )
                .map_err(db_err)?
                .query_row(params![mission_id], |r| r.get(0))
                .map_err(db_err)?;
            if started > 0 {
                return Err(MaestroError::Conflict(format!(
                    "mission {mission_id} has {started} task(s) already started; \
                     the plan can only be replaced before execution"
                )));
            }
            tx.prepare_cached(
                "DELETE FROM scope_locks WHERE task_id IN \
                 (SELECT id FROM tasks WHERE mission_id = ?1)",
            )
            .map_err(db_err)?
            .execute(params![mission_id])
            .map_err(db_err)?;
            tx.prepare_cached("DELETE FROM tasks WHERE mission_id = ?1")
                .map_err(db_err)?
                .execute(params![mission_id])
                .map_err(db_err)?;
            let now = now_ms();
            let mut out = Vec::with_capacity(plan_tasks.len());
            for spec in plan_tasks {
                out.push(insert_task_row(&tx, mission_id, spec, None, 0, now)?);
            }
            tx.commit().map_err(db_err)?;
            Ok(out)
        })
    }
}
