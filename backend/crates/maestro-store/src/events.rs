//! Append-only event log + broadcast fan-out. `events.seq` is the SSE cursor.

use crate::{db_err, json_col, Store};
use maestro_types::event::EventRecord;
use maestro_types::{now_ms, Result};
use rusqlite::params;

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    let payload: String = row.get(4)?;
    Ok(EventRecord {
        seq: row.get(0)?,
        mission_id: row.get(1)?,
        task_id: row.get(2)?,
        kind: row.get(3)?,
        payload: json_col("event payload", &payload)?,
        created_at: row.get(5)?,
    })
}

const EVENT_COLS: &str = "seq, mission_id, task_id, kind, payload, created_at";

impl Store {
    /// Insert an event row, then broadcast it on the bus (send errors — no
    /// live subscribers — are ignored). Returns the durable record with its
    /// assigned `seq`.
    pub fn append_event(
        &self,
        mission_id: &str,
        task_id: Option<&str>,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<EventRecord> {
        self.with_conn("event.append", |conn| {
            let created_at = now_ms();
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO events (mission_id, task_id, kind, payload, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(db_err)?;
            stmt.execute(params![
                mission_id,
                task_id,
                kind,
                payload.to_string(),
                created_at
            ])
            .map_err(db_err)?;
            let record = EventRecord {
                seq: conn.last_insert_rowid(),
                mission_id: mission_id.to_string(),
                task_id: task_id.map(str::to_string),
                kind: kind.to_string(),
                payload,
                created_at,
            };
            // Broadcast while still serialized with the insert so subscribers
            // observe seqs in strictly increasing order (`broadcast::send`
            // never blocks). Nobody listening is fine.
            let _ = self.event_bus().send(record.clone());
            Ok(record)
        })
    }

    /// Highest event `seq` in the log (0 when empty) — the SSE cursor head
    /// reported by the daemon's `/health` and `/api/state`.
    pub fn last_event_seq(&self) -> Result<i64> {
        self.with_conn("event.last_seq", |conn| {
            conn.prepare_cached("SELECT COALESCE(MAX(seq), 0) FROM events")
                .map_err(db_err)?
                .query_row([], |r| r.get(0))
                .map_err(db_err)
        })
    }

    /// Replay events with `seq > cursor_seq`, optionally filtered to one
    /// mission, in ascending `seq` order (SSE replay before following the bus).
    ///
    /// Thin wrapper over [`Store::events_page`] with a large default limit;
    /// callers that may face big logs should page explicitly.
    pub fn events_after(
        &self,
        cursor_seq: i64,
        mission_id: Option<&str>,
    ) -> Result<Vec<EventRecord>> {
        self.events_page(cursor_seq, mission_id, 10_000)
    }

    /// Bounded replay page: up to `limit` events with `seq > cursor_seq`,
    /// optionally filtered to one mission, ascending. Callers page by passing
    /// the last returned `seq` as the next cursor.
    pub fn events_page(
        &self,
        cursor_seq: i64,
        mission_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventRecord>> {
        self.with_conn("event.after", |conn| {
            let limit = limit as i64;
            let sql_all =
                format!("SELECT {EVENT_COLS} FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2");
            let sql_mission = format!(
                "SELECT {EVENT_COLS} FROM events WHERE seq > ?1 AND mission_id = ?3 \
                 ORDER BY seq LIMIT ?2"
            );
            let mut stmt = match mission_id {
                None => conn.prepare_cached(&sql_all).map_err(db_err)?,
                Some(_) => conn.prepare_cached(&sql_mission).map_err(db_err)?,
            };
            let rows = match mission_id {
                None => stmt.query_map(params![cursor_seq, limit], event_from_row),
                Some(m) => stmt.query_map(params![cursor_seq, limit, m], event_from_row),
            }
            .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }
}
