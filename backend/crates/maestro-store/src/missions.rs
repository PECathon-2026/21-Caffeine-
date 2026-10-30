//! Mission rows: lifecycle status, git/integration columns, stored plan JSON.

use crate::{db_err, invalid_col, json_col, Store};
use maestro_types::domain::{Mission, MissionStatus};
use maestro_types::plan::Plan;
use maestro_types::{new_id, now_ms, MaestroError, Result};
use rusqlite::{params, OptionalExtension};

const MISSION_COLS: &str = "id, objective, status, plan_summary, base_head, mission_branch, \
                            integration_head, final_approval_id, created_at, updated_at";

fn mission_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Mission> {
    let status: String = row.get(2)?;
    Ok(Mission {
        id: row.get(0)?,
        objective: row.get(1)?,
        status: MissionStatus::parse(&status)
            .ok_or_else(|| invalid_col("mission status", &status))?,
        plan_summary: row.get(3)?,
        base_head: row.get(4)?,
        mission_branch: row.get(5)?,
        integration_head: row.get(6)?,
        final_approval_id: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

impl Store {
    /// Create a mission in `planning` status.
    pub fn create_mission(&self, objective: &str) -> Result<Mission> {
        self.with_conn("mission.create", |conn| {
            let now = now_ms();
            let mission = Mission {
                id: new_id(),
                objective: objective.to_string(),
                status: MissionStatus::Planning,
                plan_summary: None,
                base_head: None,
                mission_branch: None,
                integration_head: None,
                final_approval_id: None,
                created_at: now,
                updated_at: now,
            };
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO missions (id, objective, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(db_err)?;
            stmt.execute(params![
                mission.id,
                mission.objective,
                mission.status.as_str(),
                now,
                now
            ])
            .map_err(db_err)?;
            Ok(mission)
        })
    }

    /// Fetch one mission; `NotFound` if absent.
    pub fn get_mission(&self, id: &str) -> Result<Mission> {
        self.with_conn("mission.get", |conn| {
            let sql = format!("SELECT {MISSION_COLS} FROM missions WHERE id = ?1");
            conn.prepare_cached(&sql)
                .map_err(db_err)?
                .query_row(params![id], mission_from_row)
                .optional()
                .map_err(db_err)?
                .ok_or_else(|| MaestroError::NotFound(format!("mission {id}")))
        })
    }

    /// All missions, newest first.
    pub fn list_missions(&self) -> Result<Vec<Mission>> {
        self.with_conn("mission.list", |conn| {
            let sql =
                format!("SELECT {MISSION_COLS} FROM missions ORDER BY created_at DESC, id DESC");
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt.query_map([], mission_from_row).map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }

    /// Update status (+ optional human-readable reason) and `updated_at`.
    pub fn set_mission_status(
        &self,
        id: &str,
        status: MissionStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        self.with_conn("mission.set_status", |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "UPDATE missions SET status = ?2, status_reason = ?3, updated_at = ?4
                     WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM history_retired WHERE mission_id=?1)",
                )
                .map_err(db_err)?;
            let n = stmt
                .execute(params![id, status.as_str(), reason, now_ms()])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("mission {id}")));
            }
            Ok(())
        })
    }

    /// Guarded transition to `running` for the scheduler entry path: a
    /// single conditional UPDATE that only fires when the current status is
    /// one that may (re-)enter execution (`planning`, `planned`, `blocked`,
    /// or already `running`). Returns `false` — writing nothing — when a
    /// concurrent cancel/pause (or any other transition) landed first, so a
    /// user's `cancelled` can never be overwritten by a racing worker.
    /// `NotFound` when the mission does not exist.
    pub fn begin_mission_running(&self, id: &str) -> Result<bool> {
        self.with_conn("mission.begin_running", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE missions SET status = 'running', status_reason = NULL, \
                     updated_at = ?2
                     WHERE id = ?1
                       AND status IN ('planning', 'planned', 'running', 'blocked')
                       AND NOT EXISTS (SELECT 1 FROM history_retired WHERE mission_id=?1)",
                )
                .map_err(db_err)?
                .execute(params![id, now_ms()])
                .map_err(db_err)?;
            if n == 1 {
                return Ok(true);
            }
            // Distinguish "guard lost" from "missing mission".
            let exists: bool = conn
                .prepare_cached("SELECT 1 FROM missions WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![id], |_| Ok(()))
                .optional()
                .map_err(db_err)?
                .is_some();
            if !exists {
                return Err(MaestroError::NotFound(format!("mission {id}")));
            }
            Ok(false)
        })
    }

    /// Compare-and-swap status transition: writes `to` (+ reason) only when
    /// the mission is currently in `from`. Returns `false` — writing
    /// nothing — when the status changed concurrently (e.g. a cancel landed
    /// between a read and this write). `NotFound` when the mission does not
    /// exist.
    pub fn set_mission_status_if(
        &self,
        id: &str,
        from: MissionStatus,
        to: MissionStatus,
        reason: Option<&str>,
    ) -> Result<bool> {
        self.with_conn("mission.set_status_if", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE missions SET status = ?3, status_reason = ?4, updated_at = ?5
                     WHERE id = ?1 AND status = ?2 AND NOT EXISTS (SELECT 1 FROM history_retired WHERE mission_id=?1)",
                )
                .map_err(db_err)?
                .execute(params![id, from.as_str(), to.as_str(), reason, now_ms()])
                .map_err(db_err)?;
            if n == 1 {
                return Ok(true);
            }
            let exists: bool = conn
                .prepare_cached("SELECT 1 FROM missions WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![id], |_| Ok(()))
                .optional()
                .map_err(db_err)?
                .is_some();
            if !exists {
                return Err(MaestroError::NotFound(format!("mission {id}")));
            }
            Ok(false)
        })
    }

    /// Record the git baseline: base HEAD commit and mission branch name.
    pub fn set_mission_git_base(
        &self,
        id: &str,
        base_head: &str,
        mission_branch: &str,
    ) -> Result<()> {
        self.mission_setter(
            "mission.set_git_base",
            "UPDATE missions SET base_head = ?2, mission_branch = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, base_head, mission_branch, now_ms()],
            id,
        )
    }

    /// Record the current integration head commit (advances after each merge).
    pub fn set_mission_integration_head(&self, id: &str, integration_head: &str) -> Result<()> {
        self.mission_setter(
            "mission.set_integration_head",
            "UPDATE missions SET integration_head = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, integration_head, now_ms()],
            id,
        )
    }

    /// Link the final integration approval to the mission.
    pub fn set_mission_final_approval(&self, id: &str, approval_id: &str) -> Result<()> {
        self.mission_setter(
            "mission.set_final_approval",
            "UPDATE missions SET final_approval_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, approval_id, now_ms()],
            id,
        )
    }

    /// Store the validated plan JSON (and its summary) on the mission row —
    /// the source of truth for the plan-approval flow.
    pub fn set_mission_plan(&self, id: &str, plan: &Plan) -> Result<()> {
        let plan_json = serde_json::to_string(plan)?;
        self.mission_setter(
            "mission.set_plan",
            "UPDATE missions SET plan_json = ?2, plan_summary = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, plan_json, plan.summary, now_ms()],
            id,
        )
    }

    /// The stored plan, if one has been recorded. `NotFound` if the mission
    /// itself is absent.
    pub fn get_mission_plan(&self, id: &str) -> Result<Option<Plan>> {
        self.with_conn("mission.get_plan", |conn| {
            let raw: Option<Option<String>> = conn
                .prepare_cached("SELECT plan_json FROM missions WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![id], |r| r.get(0))
                .optional()
                .map_err(db_err)?;
            match raw {
                None => Err(MaestroError::NotFound(format!("mission {id}"))),
                Some(None) => Ok(None),
                Some(Some(json)) => Ok(Some(json_col("plan json", &json).map_err(db_err)?)),
            }
        })
    }

    fn mission_setter(
        &self,
        op: &'static str,
        sql: &str,
        args: &[&dyn rusqlite::ToSql],
        id: &str,
    ) -> Result<()> {
        self.with_conn(op, |conn| {
            let n = conn
                .prepare_cached(sql)
                .map_err(db_err)?
                .execute(args)
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("mission {id}")));
            }
            Ok(())
        })
    }
}
