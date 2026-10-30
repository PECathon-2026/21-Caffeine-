//! SQLite persistence layer, event bus, and op metrics for Maestro.
//!
//! Single `rusqlite::Connection` behind a `std::sync::Mutex` inside an `Arc`:
//! [`Store`] is cheap to clone and safe to share across threads/tasks. Every
//! public method is wrapped by the latency recorder ([`Store::metrics`]) and
//! uses prepared-statement caching to stay well inside the <10ms op budget.
//!
//! See `SPEC.md` §Data model. No SQL exists outside this crate.

#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod metrics;

mod approvals;
mod artifacts;
mod chat;
mod daemon;
mod directives;
mod events;
mod gates;
mod history;
mod invocations;
mod merge;
mod migrations;
mod missions;
mod provider_health;
mod scope_locks;
mod tasks;

pub use approvals::ApprovalRequest;
pub use artifacts::Artifact;
pub use invocations::Scorecard;

use maestro_types::event::EventRecord;
use maestro_types::{MaestroError, Result};
use metrics::Metrics;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::broadcast;

/// Capacity of the in-process event bus. Slow subscribers observe
/// `RecvError::Lagged` and must re-sync via [`Store::events_after`].
pub const EVENT_BUS_CAPACITY: usize = 1024;

/// Convert a rusqlite error into the shared storage error.
pub(crate) fn db_err(e: rusqlite::Error) -> MaestroError {
    MaestroError::Storage(e.to_string())
}

/// Build a rusqlite conversion error for an invalid stored value (used inside
/// row-mapping closures, surfaced to callers through [`db_err`]).
pub(crate) fn invalid_col(what: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("invalid {what}: {value:?}").into(),
    )
}

/// Parse a JSON TEXT column inside a row-mapping closure.
pub(crate) fn json_col<T: serde::de::DeserializeOwned>(
    what: &str,
    raw: &str,
) -> std::result::Result<T, rusqlite::Error> {
    serde_json::from_str(raw).map_err(|e| invalid_col(what, &format!("{raw} ({e})")))
}

struct Inner {
    conn: Mutex<Connection>,
    bus: broadcast::Sender<EventRecord>,
    metrics: Metrics,
    db_path: PathBuf,
}

/// Handle to the Maestro SQLite store. Cheap to clone (`Arc` inside); all
/// clones share one connection, one event bus, and one metrics recorder.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("db_path", &self.inner.db_path)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Open (creating if needed) `<state_dir>/maestro.db`, configure WAL /
    /// foreign keys / busy timeout / synchronous NORMAL, and apply pending
    /// migrations. `state_dir` is created if missing.
    pub fn open(state_dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(state_dir)?;
        let db_path = state_dir.join("maestro.db");
        let mut conn = Connection::open(&db_path).map_err(db_err)?;

        // journal_mode returns a row; query it instead of pragma_update.
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .map_err(db_err)?;
        if !mode.eq_ignore_ascii_case("wal") {
            tracing::warn!(target: "maestro_store", %mode, "journal_mode is not WAL");
        }
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(db_err)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(db_err)?;
        conn.busy_timeout(Duration::from_millis(5000))
            .map_err(db_err)?;

        migrations::apply(&mut conn)?;

        let (bus, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        Ok(Store {
            inner: Arc::new(Inner {
                conn: Mutex::new(conn),
                bus,
                metrics: Metrics::new(),
                db_path,
            }),
        })
    }

    /// Path of the underlying database file.
    pub fn db_path(&self) -> &Path {
        &self.inner.db_path
    }

    /// The store's latency recorder (see [`metrics::Metrics::snapshot`]).
    pub fn metrics(&self) -> &Metrics {
        &self.inner.metrics
    }

    /// Subscribe to the live event bus. Capacity is [`EVENT_BUS_CAPACITY`];
    /// handle `Lagged` by replaying from [`Store::events_after`].
    pub fn subscribe(&self) -> broadcast::Receiver<EventRecord> {
        self.inner.bus.subscribe()
    }

    /// Run `PRAGMA integrity_check` (used by `maestro doctor`).
    pub fn integrity_check(&self) -> Result<bool> {
        self.with_conn("store.integrity_check", |conn| {
            let out: String = conn
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .map_err(db_err)?;
            Ok(out.eq_ignore_ascii_case("ok"))
        })
    }

    pub(crate) fn event_bus(&self) -> &broadcast::Sender<EventRecord> {
        &self.inner.bus
    }

    fn lock_conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned mutex only means another thread panicked mid-call;
        // SQLite's own transaction guarantees keep the DB consistent.
        self.inner
            .conn
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Lock the connection, run `f`, and record the wall-clock latency of the
    /// whole call (lock wait included) under `op`.
    pub(crate) fn with_conn<T>(
        &self,
        op: &'static str,
        f: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        self.inner.metrics.timed(op, || {
            let mut conn = self.lock_conn();
            f(&mut conn)
        })
    }
}
