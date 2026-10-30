//! Serial merge queue: positional entries per mission, supersede-on-requeue,
//! crash reconciliation, and task integration stamps.

use crate::{db_err, invalid_col, Store};
use maestro_types::domain::{MergeEntry, MergeStatus};
use maestro_types::{new_id, now_ms, MaestroError, Result};
use rusqlite::{params, OptionalExtension};

const MERGE_COLS: &str = "id, mission_id, task_id, position, patch_hash, status, detail, \
                          merge_commit, created_at, updated_at";

fn merge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MergeEntry> {
    let status: String = row.get(5)?;
    Ok(MergeEntry {
        id: row.get(0)?,
        mission_id: row.get(1)?,
        task_id: row.get(2)?,
        position: row.get(3)?,
        patch_hash: row.get(4)?,
        status: MergeStatus::parse(&status).ok_or_else(|| invalid_col("merge status", &status))?,
        detail: row.get(6)?,
        merge_commit: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

impl Store {
    /// Enqueue a task's patch at the next queue position for its mission.
    /// Any earlier non-merged entry for the same task is marked `superseded`.
    pub fn enqueue_merge(
        &self,
        mission_id: &str,
        task_id: &str,
        patch_hash: &str,
    ) -> Result<MergeEntry> {
        self.with_conn("merge.enqueue", |conn| {
            let now = now_ms();
            let tx = conn.transaction().map_err(db_err)?;
            tx.prepare_cached(
                "UPDATE merge_queue SET status = 'superseded', updated_at = ?2
                 WHERE task_id = ?1 AND status NOT IN ('merged', 'superseded')",
            )
            .map_err(db_err)?
            .execute(params![task_id, now])
            .map_err(db_err)?;
            let position: i64 = tx
                .prepare_cached(
                    "SELECT COALESCE(MAX(position), 0) + 1 FROM merge_queue WHERE mission_id = ?1",
                )
                .map_err(db_err)?
                .query_row(params![mission_id], |r| r.get(0))
                .map_err(db_err)?;
            let entry = MergeEntry {
                id: new_id(),
                mission_id: mission_id.to_string(),
                task_id: task_id.to_string(),
                position,
                patch_hash: patch_hash.to_string(),
                status: MergeStatus::Pending,
                detail: None,
                merge_commit: None,
                created_at: now,
                updated_at: now,
            };
            tx.prepare_cached(
                "INSERT INTO merge_queue (id, mission_id, task_id, position, patch_hash, status, \
                 created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(db_err)?
            .execute(params![
                entry.id,
                entry.mission_id,
                entry.task_id,
                entry.position,
                entry.patch_hash,
                entry.status.as_str(),
                now,
                now
            ])
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok(entry)
        })
    }

    /// The lowest-position `pending` entry for a mission, if any.
    pub fn next_pending_merge(&self, mission_id: &str) -> Result<Option<MergeEntry>> {
        self.with_conn("merge.next_pending", |conn| {
            let sql = format!(
                "SELECT {MERGE_COLS} FROM merge_queue
                 WHERE mission_id = ?1 AND status = 'pending'
                 ORDER BY position LIMIT 1"
            );
            conn.prepare_cached(&sql)
                .map_err(db_err)?
                .query_row(params![mission_id], merge_from_row)
                .optional()
                .map_err(db_err)
        })
    }

    /// Update one queue entry's status (with optional detail text and merge
    /// commit; a `None` merge commit preserves any existing value).
    pub fn set_merge_status(
        &self,
        entry_id: &str,
        status: MergeStatus,
        detail: Option<&str>,
        merge_commit: Option<&str>,
    ) -> Result<()> {
        self.with_conn("merge.set_status", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE merge_queue SET status = ?2, detail = ?3, \
                     merge_commit = COALESCE(?4, merge_commit), updated_at = ?5 WHERE id = ?1",
                )
                .map_err(db_err)?
                .execute(params![
                    entry_id,
                    status.as_str(),
                    detail,
                    merge_commit,
                    now_ms()
                ])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("merge entry {entry_id}")));
            }
            Ok(())
        })
    }

    /// Crash reconciliation: entries stuck in `checking` (a merge was in
    /// flight when the daemon died) go back to `pending`. Returns the count.
    pub fn reconcile_merge_queue(&self, mission_id: &str) -> Result<usize> {
        self.with_conn("merge.reconcile", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE merge_queue SET status = 'pending', \
                     detail = 'reconciled after restart', updated_at = ?2
                     WHERE mission_id = ?1 AND status = 'checking'",
                )
                .map_err(db_err)?
                .execute(params![mission_id, now_ms()])
                .map_err(db_err)?;
            if n > 0 {
                tracing::warn!(
                    target: "maestro_store",
                    %mission_id, count = n, "reconciled in-flight merge entries"
                );
            }
            Ok(n)
        })
    }

    /// Stamp a task as integrated: sets `integrated_at` now and records the
    /// integration commit.
    pub fn mark_task_integrated(&self, task_id: &str, commit: &str) -> Result<()> {
        self.with_conn("merge.mark_integrated", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE tasks SET integrated_at = ?2, integration_commit = ?3, \
                     updated_at = ?2 WHERE id = ?1",
                )
                .map_err(db_err)?
                .execute(params![task_id, now_ms(), commit])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("task {task_id}")));
            }
            Ok(())
        })
    }
}
