//! Typed handoff artifacts (design / patch / test_report / review_findings /
//! security_findings / worktree / note). Content is free-form JSON hashed
//! with sha256 for tamper-evident handoffs.

use crate::{db_err, json_col, Store};
use maestro_types::{new_id, now_ms, Millis, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One artifact row. `kind` is a free string; well-known kinds are listed in
/// SPEC.md §Extended scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: String,
    pub mission_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub kind: String,
    pub content: serde_json::Value,
    /// Hex sha256 of the serialized content.
    pub content_hash: String,
    pub created_at: Millis,
}

const ARTIFACT_COLS: &str = "id, mission_id, task_id, kind, content, content_hash, created_at";

fn artifact_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    let content: String = row.get(4)?;
    Ok(Artifact {
        id: row.get(0)?,
        mission_id: row.get(1)?,
        task_id: row.get(2)?,
        kind: row.get(3)?,
        content: json_col("artifact content", &content)?,
        content_hash: row.get(5)?,
        created_at: row.get(6)?,
    })
}

impl Store {
    /// Store an artifact, computing the sha256 of its serialized content.
    pub fn add_artifact(
        &self,
        mission_id: &str,
        task_id: Option<&str>,
        kind: &str,
        content: serde_json::Value,
    ) -> Result<Artifact> {
        self.with_conn("artifact.add", |conn| {
            let serialized = content.to_string();
            let content_hash = hex::encode(Sha256::digest(serialized.as_bytes()));
            let artifact = Artifact {
                id: new_id(),
                mission_id: mission_id.to_string(),
                task_id: task_id.map(str::to_string),
                kind: kind.to_string(),
                content,
                content_hash,
                created_at: now_ms(),
            };
            conn.prepare_cached(
                "INSERT INTO artifacts (id, mission_id, task_id, kind, content, content_hash, \
                 created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(db_err)?
            .execute(params![
                artifact.id,
                artifact.mission_id,
                artifact.task_id,
                artifact.kind,
                serialized,
                artifact.content_hash,
                artifact.created_at
            ])
            .map_err(db_err)?;
            Ok(artifact)
        })
    }

    /// Artifacts attached to one task, oldest first.
    pub fn artifacts_for_task(&self, task_id: &str) -> Result<Vec<Artifact>> {
        self.with_conn("artifact.for_task", |conn| {
            let sql = format!(
                "SELECT {ARTIFACT_COLS} FROM artifacts WHERE task_id = ?1 ORDER BY created_at, id"
            );
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt
                .query_map(params![task_id], artifact_from_row)
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }

    /// All artifacts of a mission, oldest first.
    pub fn artifacts_for_mission(&self, mission_id: &str) -> Result<Vec<Artifact>> {
        self.with_conn("artifact.for_mission", |conn| {
            let sql = format!(
                "SELECT {ARTIFACT_COLS} FROM artifacts WHERE mission_id = ?1
                 ORDER BY created_at, id"
            );
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt
                .query_map(params![mission_id], artifact_from_row)
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }
}
