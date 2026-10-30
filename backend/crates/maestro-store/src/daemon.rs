//! Daemon instance registry: generations, heartbeats, liveness probe.

use crate::{db_err, Store};
use maestro_types::domain::DaemonInstance;
use maestro_types::{now_ms, MaestroError, Result};
use rusqlite::{params, OptionalExtension};

/// A daemon is considered alive when its heartbeat is within this window.
pub const DAEMON_LIVENESS_WINDOW_MS: i64 = 15_000;

const DAEMON_COLS: &str = "generation, pid, url, status, started_at, heartbeat_at";

fn daemon_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DaemonInstance> {
    Ok(DaemonInstance {
        generation: row.get(0)?,
        pid: row.get(1)?,
        url: row.get(2)?,
        status: row.get(3)?,
        started_at: row.get(4)?,
        heartbeat_at: row.get(5)?,
    })
}

impl Store {
    /// Register this daemon generation as alive and mark every other
    /// instance dead (single-writer invariant).
    pub fn register_daemon(&self, generation: &str, pid: i64, url: &str) -> Result<DaemonInstance> {
        self.with_conn("daemon.register", |conn| {
            let now = now_ms();
            let tx = conn.transaction().map_err(db_err)?;
            tx.prepare_cached("UPDATE daemon_instances SET status = 'dead' WHERE generation != ?1")
                .map_err(db_err)?
                .execute(params![generation])
                .map_err(db_err)?;
            tx.prepare_cached(
                "INSERT INTO daemon_instances (generation, pid, url, status, started_at, \
                 heartbeat_at) VALUES (?1, ?2, ?3, 'alive', ?4, ?4)
                 ON CONFLICT (generation) DO UPDATE SET
                    pid = excluded.pid, url = excluded.url, status = 'alive',
                    heartbeat_at = excluded.heartbeat_at",
            )
            .map_err(db_err)?
            .execute(params![generation, pid, url, now])
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok(DaemonInstance {
                generation: generation.to_string(),
                pid,
                url: url.to_string(),
                status: "alive".to_string(),
                started_at: now,
                heartbeat_at: now,
            })
        })
    }

    /// Refresh the heartbeat timestamp for a registered generation.
    pub fn heartbeat_daemon(&self, generation: &str) -> Result<()> {
        self.with_conn("daemon.heartbeat", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE daemon_instances SET heartbeat_at = ?2, status = 'alive'
                     WHERE generation = ?1",
                )
                .map_err(db_err)?
                .execute(params![generation, now_ms()])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!(
                    "daemon generation {generation}"
                )));
            }
            Ok(())
        })
    }

    /// The most recently heartbeaten alive daemon, if its heartbeat is within
    /// [`DAEMON_LIVENESS_WINDOW_MS`].
    pub fn alive_daemon(&self) -> Result<Option<DaemonInstance>> {
        self.with_conn("daemon.alive", |conn| {
            let cutoff = now_ms() - DAEMON_LIVENESS_WINDOW_MS;
            let sql = format!(
                "SELECT {DAEMON_COLS} FROM daemon_instances
                 WHERE status = 'alive' AND heartbeat_at >= ?1
                 ORDER BY heartbeat_at DESC LIMIT 1"
            );
            conn.prepare_cached(&sql)
                .map_err(db_err)?
                .query_row(params![cutoff], daemon_from_row)
                .optional()
                .map_err(db_err)
        })
    }
}
