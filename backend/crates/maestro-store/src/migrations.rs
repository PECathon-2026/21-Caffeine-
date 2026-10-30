//! Versioned schema migrations, tracked via `PRAGMA user_version`.
//!
//! `MIGRATIONS[i]` moves the schema from version `i` to `i + 1`. Every step is
//! written idempotently (`IF NOT EXISTS`) so a partially-applied step can be
//! replayed safely; each step runs inside one transaction.

use crate::db_err;
use maestro_types::{MaestroError, Result};
use rusqlite::Connection;

/// Ordered, idempotent SQL steps. Append-only: never edit a shipped step.
pub(crate) const MIGRATIONS: &[&str] = &[
    // v1 — initial schema.
    r#"
CREATE TABLE IF NOT EXISTS missions (
    id                TEXT PRIMARY KEY,
    objective         TEXT NOT NULL,
    status            TEXT NOT NULL,
    status_reason     TEXT,
    plan_summary      TEXT,
    plan_json         TEXT,
    base_head         TEXT,
    mission_branch    TEXT,
    integration_head  TEXT,
    final_approval_id TEXT,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id                 TEXT PRIMARY KEY,
    mission_id         TEXT NOT NULL REFERENCES missions(id),
    task_key           TEXT NOT NULL,
    role               TEXT NOT NULL,
    title              TEXT NOT NULL,
    description        TEXT NOT NULL,
    status             TEXT NOT NULL,
    dependencies_json  TEXT NOT NULL DEFAULT '[]',
    acceptance_json    TEXT NOT NULL DEFAULT '[]',
    risk               TEXT NOT NULL DEFAULT 'low',
    attempts           INTEGER NOT NULL DEFAULT 0,
    status_reason      TEXT,
    summary            TEXT,
    lease_owner        TEXT,
    lease_expires_at   INTEGER,
    lease_generation   TEXT,
    workspace_path     TEXT,
    branch_name        TEXT,
    base_ref           TEXT,
    patch_hash         TEXT,
    integrated_at      INTEGER,
    integration_commit TEXT,
    parent_task_id     TEXT,
    delegation_depth   INTEGER NOT NULL DEFAULT 0,
    write_scopes_json  TEXT NOT NULL DEFAULT '[]',
    model_override     TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    UNIQUE (mission_id, task_key)
);
CREATE INDEX IF NOT EXISTS idx_tasks_mission_status ON tasks (mission_id, status);

CREATE TABLE IF NOT EXISTS events (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    mission_id TEXT NOT NULL,
    task_id    TEXT,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_mission_seq ON events (mission_id, seq);

CREATE TABLE IF NOT EXISTS invocations (
    id             TEXT PRIMARY KEY,
    mission_id     TEXT NOT NULL,
    task_id        TEXT,
    role           TEXT NOT NULL,
    provider_id    TEXT NOT NULL,
    model          TEXT NOT NULL,
    status         TEXT NOT NULL,
    input_tokens   INTEGER NOT NULL DEFAULT 0,
    output_tokens  INTEGER NOT NULL DEFAULT 0,
    cost_micro_usd INTEGER NOT NULL DEFAULT 0,
    latency_ms     INTEGER NOT NULL DEFAULT 0,
    error          TEXT,
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_invocations_mission ON invocations (mission_id);

CREATE TABLE IF NOT EXISTS gates (
    id             TEXT PRIMARY KEY,
    mission_id     TEXT NOT NULL,
    task_id        TEXT NOT NULL,
    role           TEXT NOT NULL,
    source_task_id TEXT NOT NULL,
    verdict        TEXT NOT NULL,
    reasons_json   TEXT NOT NULL DEFAULT '[]',
    patch_hash     TEXT,
    created_at     INTEGER NOT NULL,
    UNIQUE (task_id, role)
);

CREATE TABLE IF NOT EXISTS directives (
    id         TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL,
    task_id    TEXT,
    text       TEXT NOT NULL,
    created_by TEXT NOT NULL,
    active     INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_directives_mission ON directives (mission_id, active);

CREATE TABLE IF NOT EXISTS approvals (
    id              TEXT PRIMARY KEY,
    mission_id      TEXT NOT NULL,
    task_id         TEXT,
    action          TEXT NOT NULL,
    resource        TEXT NOT NULL,
    patch_hash      TEXT,
    policy_snapshot TEXT NOT NULL DEFAULT '{}',
    status          TEXT NOT NULL,
    expires_at      INTEGER,
    decided_at      INTEGER,
    decided_by      TEXT,
    note            TEXT,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_approvals_mission_status ON approvals (mission_id, status);

CREATE TABLE IF NOT EXISTS scope_locks (
    mission_id TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    scope      TEXT NOT NULL,
    PRIMARY KEY (mission_id, task_id, scope)
);
CREATE INDEX IF NOT EXISTS idx_scope_locks_task ON scope_locks (task_id);

CREATE TABLE IF NOT EXISTS merge_queue (
    id           TEXT PRIMARY KEY,
    mission_id   TEXT NOT NULL,
    task_id      TEXT NOT NULL,
    position     INTEGER NOT NULL,
    patch_hash   TEXT NOT NULL,
    status       TEXT NOT NULL,
    detail       TEXT,
    merge_commit TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_merge_queue_mission_status ON merge_queue (mission_id, status, position);

CREATE TABLE IF NOT EXISTS daemon_instances (
    generation   TEXT PRIMARY KEY,
    pid          INTEGER NOT NULL,
    url          TEXT NOT NULL,
    status       TEXT NOT NULL,
    started_at   INTEGER NOT NULL,
    heartbeat_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chat_sessions (
    id                TEXT PRIMARY KEY,
    title             TEXT NOT NULL,
    active_mission_id TEXT,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chat_messages (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES chat_sessions(id),
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    mission_id  TEXT,
    task_id     TEXT,
    approval_id TEXT,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chat_messages_session ON chat_messages (session_id, created_at);

CREATE TABLE IF NOT EXISTS artifacts (
    id           TEXT PRIMARY KEY,
    mission_id   TEXT NOT NULL,
    task_id      TEXT,
    kind         TEXT NOT NULL,
    content      TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_artifacts_mission ON artifacts (mission_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_task ON artifacts (task_id);

CREATE TABLE IF NOT EXISTS provider_health (
    provider_id          TEXT PRIMARY KEY,
    success_count        INTEGER NOT NULL DEFAULT 0,
    failure_count        INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    open_until           INTEGER,
    updated_at           INTEGER NOT NULL
);
"#,
    // v2 — partial index for expired-lease recovery scans (running tasks by
    // lease expiry).
    r#"
CREATE INDEX IF NOT EXISTS idx_tasks_lease_expiry
    ON tasks(lease_expires_at) WHERE status = 'running';
"#,
    // v3 — partial index for lazy approval expiry: `expire_overdue` runs on
    // every approval op and its predicate (pending + non-NULL expiry) cannot
    // use idx_approvals_mission_status, so it full-scanned the append-only
    // approvals table. Mirrors the v2 lease-expiry pattern.
    r#"
CREATE INDEX IF NOT EXISTS idx_approvals_pending_expiry
    ON approvals(expires_at) WHERE status = 'pending' AND expires_at IS NOT NULL;
"#,
    // v4 — durable code timeline and restore journal, separate from agent state.
    r#"
CREATE TABLE history_versions (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    data TEXT NOT NULL
);
CREATE TABLE history_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE history_retired (mission_id TEXT PRIMARY KEY);
CREATE TABLE history_undone (mission_id TEXT PRIMARY KEY);
"#,
];

/// Apply any pending migrations. Safe to call on every open.
pub(crate) fn apply(conn: &mut Connection) -> Result<()> {
    // The TypeScript prototype used the same filename with an incompatible
    // schema. Detect it before running any Rust migration against user data.
    let legacy_events: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('events') WHERE name = 'payload_json')
             AND NOT EXISTS(SELECT 1 FROM pragma_table_info('events') WHERE name = 'seq')",
            [],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if legacy_events {
        return Err(MaestroError::Storage(
            "this .maestro/maestro.db belongs to the TypeScript prototype; \
             back it up and migrate its history before using the Rust version"
                .into(),
        ));
    }
    let mut version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(db_err)?;
    for (i, step) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i64;
        if version >= target {
            continue;
        }
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute_batch(step).map_err(db_err)?;
        tx.pragma_update(None, "user_version", target)
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        tracing::debug!(target: "maestro_store", version = target, "applied migration");
        version = target;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_database_is_rejected_without_applying_rust_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY, type TEXT, payload_json TEXT);
             INSERT INTO events VALUES (1, 'saved', '{\"keep\":true}');",
        )
        .unwrap();
        let error = apply(&mut conn).unwrap_err().to_string();
        assert!(error.contains("TypeScript prototype"), "{error}");
        let payload: String = conn
            .query_row("SELECT payload_json FROM events WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(payload, "{\"keep\":true}");
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 1, "no partial Rust migration must be applied");
    }

    #[test]
    fn lease_expiry_index_created_and_version_bumped() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_tasks_lease_expiry'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "partial lease-expiry index must exist");
        // Idempotent re-apply.
        apply(&mut conn).unwrap();
    }

    #[test]
    fn approval_expiry_index_serves_expire_overdue() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_approvals_pending_expiry'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "partial approval-expiry index must exist");
        // The exact expire_overdue predicate must be index-driven, not a
        // full-table scan (the pre-v3 plan was `SCAN approvals`).
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN
                 UPDATE approvals SET status = 'expired'
                 WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at < ?1",
                [0i64],
                |r| r.get(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_approvals_pending_expiry"),
            "expire_overdue must use the partial index, got plan: {plan}"
        );
    }
}
