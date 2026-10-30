//! Verdict gates: one row per (task, reviewing role), upserted.

use crate::{db_err, invalid_col, json_col, Store};
use maestro_types::domain::{Gate, Role, Verdict};
use maestro_types::Result;
use rusqlite::params;

fn gate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Gate> {
    let role: String = row.get(3)?;
    let verdict: String = row.get(5)?;
    let reasons: String = row.get(6)?;
    Ok(Gate {
        id: row.get(0)?,
        mission_id: row.get(1)?,
        task_id: row.get(2)?,
        role: Role::parse(&role).ok_or_else(|| invalid_col("gate role", &role))?,
        source_task_id: row.get(4)?,
        verdict: Verdict::parse(&verdict).ok_or_else(|| invalid_col("gate verdict", &verdict))?,
        reasons: json_col("gate reasons", &reasons)?,
        patch_hash: row.get(7)?,
        created_at: row.get(8)?,
    })
}

const GATE_COLS: &str =
    "id, mission_id, task_id, role, source_task_id, verdict, reasons_json, patch_hash, created_at";

impl Store {
    /// Insert or replace the gate for `(gate.task_id, gate.role)`. On upsert
    /// the original row id is kept; verdict, reasons, patch hash, source task
    /// and timestamp are replaced.
    pub fn record_gate(&self, gate: &Gate) -> Result<()> {
        self.with_conn("gate.record", |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO gates (id, mission_id, task_id, role, source_task_id, verdict, \
                     reasons_json, patch_hash, created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                     ON CONFLICT (task_id, role) DO UPDATE SET
                        source_task_id = excluded.source_task_id,
                        verdict = excluded.verdict,
                        reasons_json = excluded.reasons_json,
                        patch_hash = excluded.patch_hash,
                        created_at = excluded.created_at",
                )
                .map_err(db_err)?;
            stmt.execute(params![
                gate.id,
                gate.mission_id,
                gate.task_id,
                gate.role.as_str(),
                gate.source_task_id,
                gate.verdict.as_str(),
                serde_json::to_string(&gate.reasons)?,
                gate.patch_hash,
                gate.created_at
            ])
            .map_err(db_err)?;
            Ok(())
        })
    }

    /// All gates recorded against a coder task (at most one per role).
    pub fn gates_for_task(&self, task_id: &str) -> Result<Vec<Gate>> {
        self.with_conn("gate.for_task", |conn| {
            let sql = format!("SELECT {GATE_COLS} FROM gates WHERE task_id = ?1 ORDER BY role");
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt
                .query_map(params![task_id], gate_from_row)
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }
}
