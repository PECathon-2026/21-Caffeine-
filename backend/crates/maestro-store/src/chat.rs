//! Persistent architect chat: sessions and messages linked to missions,
//! tasks, and approvals.

use crate::{db_err, invalid_col, Store};
use maestro_types::domain::{ChatMessage, ChatRole, ChatSession};
use maestro_types::{new_id, now_ms, MaestroError, Result};
use rusqlite::{params, OptionalExtension};

const SESSION_COLS: &str = "id, title, active_mission_id, created_at, updated_at";
const MESSAGE_COLS: &str =
    "id, session_id, role, content, mission_id, task_id, approval_id, created_at";

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatSession> {
    Ok(ChatSession {
        id: row.get(0)?,
        title: row.get(1)?,
        active_mission_id: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    let role: String = row.get(2)?;
    Ok(ChatMessage {
        id: row.get(0)?,
        session_id: row.get(1)?,
        role: ChatRole::parse(&role).ok_or_else(|| invalid_col("chat role", &role))?,
        content: row.get(3)?,
        mission_id: row.get(4)?,
        task_id: row.get(5)?,
        approval_id: row.get(6)?,
        created_at: row.get(7)?,
    })
}

impl Store {
    /// Create a chat session.
    pub fn create_chat_session(&self, title: &str) -> Result<ChatSession> {
        self.with_conn("chat.create_session", |conn| {
            let now = now_ms();
            let session = ChatSession {
                id: new_id(),
                title: title.to_string(),
                active_mission_id: None,
                created_at: now,
                updated_at: now,
            };
            conn.prepare_cached(
                "INSERT INTO chat_sessions (id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(db_err)?
            .execute(params![session.id, session.title, now, now])
            .map_err(db_err)?;
            Ok(session)
        })
    }

    /// Fetch a session, creating it with `title` when absent. Clients (the
    /// TUI) mint session ids locally and send straight into them; the first
    /// message materializes the session rather than bouncing with NotFound.
    pub fn ensure_chat_session(&self, id: &str, title: &str) -> Result<ChatSession> {
        if id.is_empty()
            || id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(MaestroError::Config(format!(
                "invalid chat session id '{id}'"
            )));
        }
        self.with_conn("chat.ensure_session", |conn| {
            let now = now_ms();
            conn.prepare_cached(
                "INSERT OR IGNORE INTO chat_sessions (id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(db_err)?
            .execute(params![id, title, now, now])
            .map_err(db_err)?;
            Ok(())
        })?;
        self.get_chat_session(id)
    }

    /// Fetch one session; `NotFound` if absent.
    pub fn get_chat_session(&self, id: &str) -> Result<ChatSession> {
        self.with_conn("chat.get_session", |conn| {
            let sql = format!("SELECT {SESSION_COLS} FROM chat_sessions WHERE id = ?1");
            conn.prepare_cached(&sql)
                .map_err(db_err)?
                .query_row(params![id], session_from_row)
                .optional()
                .map_err(db_err)?
                .ok_or_else(|| MaestroError::NotFound(format!("chat session {id}")))
        })
    }

    /// All sessions, most recently updated first.
    pub fn list_chat_sessions(&self) -> Result<Vec<ChatSession>> {
        self.with_conn("chat.list_sessions", |conn| {
            let sql = format!(
                "SELECT {SESSION_COLS} FROM chat_sessions ORDER BY updated_at DESC, id DESC"
            );
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt.query_map([], session_from_row).map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }

    /// Point a session at a mission (or clear the link with `None`).
    pub fn set_active_mission(&self, session_id: &str, mission_id: Option<&str>) -> Result<()> {
        self.with_conn("chat.set_active_mission", |conn| {
            let n = conn
                .prepare_cached(
                    "UPDATE chat_sessions SET active_mission_id = ?2, updated_at = ?3
                     WHERE id = ?1",
                )
                .map_err(db_err)?
                .execute(params![session_id, mission_id, now_ms()])
                .map_err(db_err)?;
            if n == 0 {
                return Err(MaestroError::NotFound(format!("chat session {session_id}")));
            }
            Ok(())
        })
    }

    /// Append a message to a session (also bumps the session's `updated_at`).
    /// `links` are optional (mission, task, approval) references.
    pub fn add_chat_message(
        &self,
        session_id: &str,
        role: ChatRole,
        content: &str,
        mission_id: Option<&str>,
        task_id: Option<&str>,
        approval_id: Option<&str>,
    ) -> Result<ChatMessage> {
        self.with_conn("chat.add_message", |conn| {
            let now = now_ms();
            let tx = conn.transaction().map_err(db_err)?;
            let message = ChatMessage {
                id: new_id(),
                session_id: session_id.to_string(),
                role,
                content: content.to_string(),
                mission_id: mission_id.map(str::to_string),
                task_id: task_id.map(str::to_string),
                approval_id: approval_id.map(str::to_string),
                created_at: now,
            };
            tx.prepare_cached(
                "INSERT INTO chat_messages (id, session_id, role, content, mission_id, task_id, \
                 approval_id, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(db_err)?
            .execute(params![
                message.id,
                message.session_id,
                message.role.as_str(),
                message.content,
                message.mission_id,
                message.task_id,
                message.approval_id,
                now
            ])
            .map_err(|e| match &e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    MaestroError::NotFound(format!("chat session {session_id}"))
                }
                _ => db_err(e),
            })?;
            tx.prepare_cached("UPDATE chat_sessions SET updated_at = ?2 WHERE id = ?1")
                .map_err(db_err)?
                .execute(params![session_id, now])
                .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok(message)
        })
    }

    /// The last `limit` messages of a session, in chronological order.
    pub fn list_chat_messages(&self, session_id: &str, limit: i64) -> Result<Vec<ChatMessage>> {
        self.with_conn("chat.list_messages", |conn| {
            let sql = format!(
                "SELECT {MESSAGE_COLS} FROM (
                     SELECT {MESSAGE_COLS}, rowid AS rid FROM chat_messages
                     WHERE session_id = ?1 ORDER BY rid DESC LIMIT ?2
                 ) ORDER BY rid"
            );
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt
                .query_map(params![session_id, limit], message_from_row)
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }
}
