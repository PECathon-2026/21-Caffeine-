use crate::{db_err, Store};
use maestro_types::{
    history::{History, PendingRestore, Version},
    MaestroError, Result,
};
use rusqlite::{params, OptionalExtension};

impl Store {
    pub fn history(&self) -> Result<History> {
        self.with_conn("history.list", |c| {
            let mut stmt = c
                .prepare("SELECT data FROM history_versions ORDER BY seq DESC")
                .map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(db_err)?;
            let mut versions = Vec::new();
            for row in rows {
                versions.push(serde_json::from_str(&row.map_err(db_err)?)?);
            }
            let current_id = c
                .query_row(
                    "SELECT value FROM history_meta WHERE key='current'",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_err)?;
            Ok(History {
                versions,
                current_id,
            })
        })
    }

    pub fn history_version(&self, id: &str) -> Result<Version> {
        let matches: Vec<_> = self
            .history()?
            .versions
            .into_iter()
            .filter(|v| v.id == id || (id.len() >= 6 && v.id.starts_with(id)))
            .collect();
        if matches.len() != 1 {
            return Err(MaestroError::NotFound(format!("unique version {id}")));
        }
        Ok(matches.into_iter().next().expect("one version"))
    }

    pub fn insert_version(&self, version: &Version, current: bool) -> Result<()> {
        self.with_conn("history.insert", |c| {
            let tx = c.transaction().map_err(db_err)?;
            tx.execute(
                "INSERT INTO history_versions(id,data) VALUES(?1,?2)",
                params![version.id, serde_json::to_string(version)?],
            )
            .map_err(db_err)?;
            if current {
                tx.execute(
                    "INSERT OR REPLACE INTO history_meta VALUES('current',?1)",
                    [&version.id],
                )
                .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)
        })
    }

    pub fn assert_history_mission_current(&self, id: &str) -> Result<()> {
        self.with_conn("history.mission_current", |c| {
            let retired: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM history_retired WHERE mission_id=?1)", [id], |r| r.get(0)).map_err(db_err)?;
            if retired { Err(MaestroError::Conflict("this mission predates a restore; submit a new request against the restored files".into())) } else { Ok(()) }
        })
    }

    pub fn history_undo_target(&self) -> Result<Version> {
        let undone = self.with_conn("history.undone", |c| {
            let mut stmt = c
                .prepare("SELECT mission_id FROM history_undone")
                .map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(db_err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_err);
            rows
        })?;
        self.history()?
            .versions
            .into_iter()
            .find(|v| {
                v.kind == "before" && v.mission_id.as_ref().is_some_and(|m| !undone.contains(m))
            })
            .ok_or_else(|| MaestroError::NotFound("no saved request left to undo".into()))
    }

    pub fn pending_restore(&self) -> Result<Option<PendingRestore>> {
        self.with_conn("history.pending", |c| {
            let data: Option<String> = c
                .query_row(
                    "SELECT value FROM history_meta WHERE key='pending'",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_err)?;
            data.map(|d| serde_json::from_str(&d).map_err(Into::into))
                .transpose()
        })
    }

    /// Fence every old mission before changing files. Old checkpoints, checks
    /// and approvals remain historical evidence, but can never authorize work.
    pub fn prepare_restore(&self, pending: &PendingRestore) -> Result<()> {
        self.with_conn("history.prepare_restore", |c| {
            let tx = c.transaction().map_err(db_err)?;
            tx.execute("INSERT INTO history_meta(key,value) VALUES('pending',?1)", [serde_json::to_string(pending)?]).map_err(db_err)?;
            tx.execute("INSERT OR IGNORE INTO history_retired SELECT id FROM missions", []).map_err(db_err)?;
            tx.execute("UPDATE missions SET status='cancelled',status_reason='superseded by code restore',updated_at=?1 WHERE status NOT IN ('completed','cancelled','failed')", [maestro_types::now_ms()]).map_err(db_err)?;
            tx.execute("UPDATE approvals SET status='rejected',note='superseded by code restore',decided_at=?1 WHERE status='pending'", [maestro_types::now_ms()]).map_err(db_err)?;
            tx.execute("UPDATE tasks SET status='blocked',status_reason='superseded by code restore',lease_owner=NULL,lease_expires_at=NULL,lease_generation=NULL WHERE status IN ('running','ready','proposed')", []).map_err(db_err)?;
            tx.execute("DELETE FROM scope_locks", []).map_err(db_err)?;
            tx.commit().map_err(db_err)
        })
    }

    pub fn finish_restore(&self, pending: &PendingRestore, applied: bool) -> Result<()> {
        self.with_conn("history.finish_restore", |c| {
            let tx = c.transaction().map_err(db_err)?;
            if applied {
                let version = &pending.result.version;
                tx.execute(
                    "INSERT OR IGNORE INTO history_versions(id,data) VALUES(?1,?2)",
                    params![version.id, serde_json::to_string(version)?],
                )
                .map_err(db_err)?;
                tx.execute(
                    "INSERT OR REPLACE INTO history_meta VALUES('current',?1)",
                    [&version.id],
                )
                .map_err(db_err)?;
                if let Some(m) = &pending.undo_mission {
                    tx.execute("INSERT OR IGNORE INTO history_undone VALUES(?1)", [m])
                        .map_err(db_err)?;
                }
            }
            tx.execute("DELETE FROM history_meta WHERE key='pending'", [])
                .map_err(db_err)?;
            tx.commit().map_err(db_err)
        })
    }
}
