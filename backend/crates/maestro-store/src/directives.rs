//! Steering directives: global to a mission or scoped to one task.

use crate::{db_err, Store};
use maestro_types::domain::Directive;
use maestro_types::{new_id, now_ms, MaestroError, Result};
use rusqlite::params;

fn directive_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Directive> {
    Ok(Directive {
        id: row.get(0)?,
        mission_id: row.get(1)?,
        task_id: row.get(2)?,
        text: row.get(3)?,
        created_by: row.get(4)?,
        active: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
    })
}

const DIRECTIVE_COLS: &str = "id, mission_id, task_id, text, created_by, active, created_at";

impl Store {
    /// Record an active steering directive (mission-global when `task_id` is
    /// `None`).
    pub fn add_directive(
        &self,
        mission_id: &str,
        task_id: Option<&str>,
        text: &str,
        created_by: &str,
    ) -> Result<Directive> {
        self.with_conn("directive.add", |conn| {
            let directive = Directive {
                id: new_id(),
                mission_id: mission_id.to_string(),
                task_id: task_id.map(str::to_string),
                text: text.to_string(),
                created_by: created_by.to_string(),
                active: true,
                created_at: now_ms(),
            };
            conn.prepare_cached(
                "INSERT INTO directives (id, mission_id, task_id, text, created_by, active, \
                 created_at) VALUES (?1,?2,?3,?4,?5,1,?6)",
            )
            .map_err(db_err)?
            .execute(params![
                directive.id,
                directive.mission_id,
                directive.task_id,
                directive.text,
                directive.created_by,
                directive.created_at
            ])
            .map_err(db_err)?;
            Ok(directive)
        })
    }

    /// Active directives visible to a task: mission-global ones plus (when
    /// `task_id` is given) those scoped to that task, oldest first.
    pub fn active_directives(
        &self,
        mission_id: &str,
        task_id: Option<&str>,
    ) -> Result<Vec<Directive>> {
        self.with_conn("directive.active", |conn| {
            let sql = format!(
                "SELECT {DIRECTIVE_COLS} FROM directives
                 WHERE mission_id = ?1 AND active = 1
                   AND (task_id IS NULL OR task_id = ?2)
                 ORDER BY created_at, id"
            );
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt
                .query_map(params![mission_id, task_id], directive_from_row)
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }

    /// Deactivate one directive (it stays in the audit trail).
    pub fn deactivate_directive(&self, id: &str) -> Result<()> {
        self.with_conn("directive.deactivate", |conn| {
            let n = conn
                .prepare_cached("UPDATE directives SET active = 0 WHERE id = ?1")
                .map_err(db_err)?
                .execute(params![id])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("directive {id}")));
            }
            Ok(())
        })
    }
}
