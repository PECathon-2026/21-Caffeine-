//! Durable human approvals: pending-dedupe, lazy expiry, single decision.

use crate::{db_err, invalid_col, json_col, Store};
use maestro_types::domain::{Approval, ApprovalStatus};
use maestro_types::{new_id, now_ms, MaestroError, Millis, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Parameters for [`Store::create_approval`].
#[derive(Debug, Clone)]
pub struct ApprovalRequest<'a> {
    pub mission_id: &'a str,
    pub task_id: Option<&'a str>,
    /// e.g. "edit", "integrate", "network".
    pub action: &'a str,
    /// The resource the action targets (path, "mission", url…).
    pub resource: &'a str,
    /// sha256 binding the approval to an exact diff, when applicable.
    pub patch_hash: Option<&'a str>,
    /// Snapshot of the policy context / diff / checks at request time.
    pub policy_snapshot: serde_json::Value,
    pub expires_at: Option<Millis>,
}

const APPROVAL_COLS: &str = "id, mission_id, task_id, action, resource, patch_hash, \
                             policy_snapshot, status, expires_at, decided_at, decided_by, note, \
                             created_at";

fn approval_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Approval> {
    let snapshot: String = row.get(6)?;
    let status: String = row.get(7)?;
    Ok(Approval {
        id: row.get(0)?,
        mission_id: row.get(1)?,
        task_id: row.get(2)?,
        action: row.get(3)?,
        resource: row.get(4)?,
        patch_hash: row.get(5)?,
        policy_snapshot: json_col("approval policy snapshot", &snapshot)?,
        status: ApprovalStatus::parse(&status)
            .ok_or_else(|| invalid_col("approval status", &status))?,
        expires_at: row.get(8)?,
        decided_at: row.get(9)?,
        decided_by: row.get(10)?,
        note: row.get(11)?,
        created_at: row.get(12)?,
    })
}

/// Flip every overdue pending approval to expired. Cheap (indexed) and called
/// lazily by every read path so expiry needs no background timer.
fn expire_overdue(conn: &Connection, now: Millis) -> Result<usize> {
    conn.prepare_cached(
        "UPDATE approvals SET status = 'expired'
         WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at < ?1",
    )
    .map_err(db_err)?
    .execute(params![now])
    .map_err(db_err)
}

fn get_approval_conn(conn: &Connection, id: &str) -> Result<Approval> {
    let sql = format!("SELECT {APPROVAL_COLS} FROM approvals WHERE id = ?1");
    conn.prepare_cached(&sql)
        .map_err(db_err)?
        .query_row(params![id], approval_from_row)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| MaestroError::NotFound(format!("approval {id}")))
}

impl Store {
    /// Create a pending approval, deduplicating by
    /// `(mission_id, task_id, action, resource)` (NULL-safe on `task_id`): if
    /// a live pending approval for the same tuple exists it is returned
    /// unchanged instead of inserting. Distinct tasks asking for the same
    /// action/resource get distinct approvals.
    pub fn create_approval(&self, req: ApprovalRequest<'_>) -> Result<Approval> {
        self.with_conn("approval.create", |conn| {
            let now = now_ms();
            let tx = conn.transaction().map_err(db_err)?;
            expire_overdue(&tx, now)?;
            let existing: Option<Approval> = {
                let sql = format!(
                    "SELECT {APPROVAL_COLS} FROM approvals
                     WHERE mission_id = ?1 AND action = ?2 AND resource = ?3
                       AND (task_id = ?4 OR (?4 IS NULL AND task_id IS NULL))
                       AND status = 'pending'
                     ORDER BY created_at DESC LIMIT 1"
                );
                tx.prepare_cached(&sql)
                    .map_err(db_err)?
                    .query_row(
                        params![req.mission_id, req.action, req.resource, req.task_id],
                        approval_from_row,
                    )
                    .optional()
                    .map_err(db_err)?
            };
            if let Some(a) = existing {
                tx.commit().map_err(db_err)?;
                return Ok(a);
            }
            let approval = Approval {
                id: new_id(),
                mission_id: req.mission_id.to_string(),
                task_id: req.task_id.map(str::to_string),
                action: req.action.to_string(),
                resource: req.resource.to_string(),
                patch_hash: req.patch_hash.map(str::to_string),
                policy_snapshot: req.policy_snapshot,
                status: ApprovalStatus::Pending,
                expires_at: req.expires_at,
                decided_at: None,
                decided_by: None,
                note: None,
                created_at: now,
            };
            tx.prepare_cached(
                "INSERT INTO approvals (id, mission_id, task_id, action, resource, patch_hash, \
                 policy_snapshot, status, expires_at, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )
            .map_err(db_err)?
            .execute(params![
                approval.id,
                approval.mission_id,
                approval.task_id,
                approval.action,
                approval.resource,
                approval.patch_hash,
                approval.policy_snapshot.to_string(),
                approval.status.as_str(),
                approval.expires_at,
                approval.created_at
            ])
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok(approval)
        })
    }

    /// Fetch one approval, lazily expiring it first when overdue.
    pub fn get_approval(&self, id: &str) -> Result<Approval> {
        self.with_conn("approval.get", |conn| {
            expire_overdue(conn, now_ms())?;
            get_approval_conn(conn, id)
        })
    }

    /// Decide a pending approval. `Conflict` when it was already decided or
    /// has expired; `NotFound` when it does not exist.
    pub fn decide_approval(
        &self,
        id: &str,
        approve: bool,
        decided_by: &str,
        note: Option<&str>,
    ) -> Result<Approval> {
        self.with_conn("approval.decide", |conn| {
            let now = now_ms();
            let tx = conn.transaction().map_err(db_err)?;
            expire_overdue(&tx, now)?;
            let status = if approve {
                ApprovalStatus::Approved
            } else {
                ApprovalStatus::Rejected
            };
            let n = tx
                .prepare_cached(
                    "UPDATE approvals SET status = ?2, decided_at = ?3, decided_by = ?4, \
                     note = ?5 WHERE id = ?1 AND status = 'pending'",
                )
                .map_err(db_err)?
                .execute(params![id, status.as_str(), now, decided_by, note])
                .map_err(db_err)?;
            if n == 0 {
                // Distinguish missing from already-decided for a clear error.
                let current = get_approval_conn(&tx, id)?;
                return Err(MaestroError::Conflict(format!(
                    "approval {id} is {} — only pending approvals can be decided",
                    current.status.as_str()
                )));
            }
            let approval = get_approval_conn(&tx, id)?;
            tx.commit().map_err(db_err)?;
            Ok(approval)
        })
    }

    /// Pending approvals (lazily expiring overdue ones first), oldest first,
    /// optionally filtered to one mission.
    pub fn pending_approvals(&self, mission_id: Option<&str>) -> Result<Vec<Approval>> {
        self.with_conn("approval.pending", |conn| {
            expire_overdue(conn, now_ms())?;
            // Two cached statements (as events_page does for its optional
            // filter): the `(?1 IS NULL OR mission_id = ?1)` form defeats
            // idx_approvals_mission_status and plans as a full scan.
            let sql_all = format!(
                "SELECT {APPROVAL_COLS} FROM approvals
                 WHERE status = 'pending'
                 ORDER BY created_at, id"
            );
            let sql_mission = format!(
                "SELECT {APPROVAL_COLS} FROM approvals
                 WHERE status = 'pending' AND mission_id = ?1
                 ORDER BY created_at, id"
            );
            let mut stmt = match mission_id {
                None => conn.prepare_cached(&sql_all).map_err(db_err)?,
                Some(_) => conn.prepare_cached(&sql_mission).map_err(db_err)?,
            };
            let rows = match mission_id {
                None => stmt.query_map([], approval_from_row),
                Some(m) => stmt.query_map(params![m], approval_from_row),
            }
            .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }

    /// The most recent approval matching `(mission, task, action, resource)`,
    /// lazily expiring overdue pending ones first. Used by the orchestrator's
    /// policy decider to honor prior decisions before creating a new request.
    pub fn find_approval(
        &self,
        mission_id: &str,
        task_id: Option<&str>,
        action: &str,
        resource: &str,
    ) -> Result<Option<Approval>> {
        self.with_conn("approval.find", |conn| {
            expire_overdue(conn, now_ms())?;
            let sql = format!(
                "SELECT {APPROVAL_COLS} FROM approvals
                 WHERE mission_id = ?1 AND action = ?2 AND resource = ?3
                   AND (task_id = ?4 OR (?4 IS NULL AND task_id IS NULL))
                 ORDER BY created_at DESC, id DESC LIMIT 1"
            );
            conn.prepare_cached(&sql)
                .map_err(db_err)?
                .query_row(
                    params![mission_id, action, resource, task_id],
                    approval_from_row,
                )
                .optional()
                .map_err(db_err)
        })
    }
}
