//! Per-mission write-scope locks with a conservative glob-overlap check.
//!
//! Overlap is decided on the static prefix of each glob (the text before the
//! first glob metacharacter). Two scopes overlap when either static prefix is
//! a prefix of the other; a scope with an empty static prefix (e.g. `*`,
//! `**/foo`) conservatively overlaps everything.

use crate::{db_err, Store};
use maestro_types::Result;
use rusqlite::params;

/// The static prefix of a glob: everything before the first metacharacter.
fn static_prefix(scope: &str) -> &str {
    match scope.find(['*', '?', '[', '{']) {
        Some(idx) => &scope[..idx],
        None => scope,
    }
}

/// Conservative overlap test between two scope globs.
pub(crate) fn scopes_overlap(a: &str, b: &str) -> bool {
    let pa = static_prefix(a);
    let pb = static_prefix(b);
    if pa.is_empty() || pb.is_empty() {
        return true;
    }
    pa.starts_with(pb) || pb.starts_with(pa)
}

impl Store {
    /// Atomically acquire all `scopes` for `task_id` within a mission.
    ///
    /// Returns `false` (acquiring nothing) if any requested scope overlaps a
    /// lock held by a different task in the same mission. Re-acquiring scopes
    /// the same task already holds is a no-op success.
    pub fn acquire_scope_locks(
        &self,
        mission_id: &str,
        task_id: &str,
        scopes: &[String],
    ) -> Result<bool> {
        self.with_conn("scope.acquire", |conn| {
            let tx = conn.transaction().map_err(db_err)?;
            let held: Vec<(String, String)> = {
                let mut stmt = tx
                    .prepare_cached(
                        "SELECT task_id, scope FROM scope_locks
                         WHERE mission_id = ?1 AND task_id != ?2",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(params![mission_id, task_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    .map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)?
            };
            for wanted in scopes {
                if let Some((other, theirs)) = held.iter().find(|(_, s)| scopes_overlap(wanted, s))
                {
                    tracing::debug!(
                        target: "maestro_store",
                        %task_id, %other, wanted = %wanted, held = %theirs,
                        "scope lock conflict"
                    );
                    return Ok(false);
                }
            }
            {
                let mut stmt = tx
                    .prepare_cached(
                        "INSERT OR IGNORE INTO scope_locks (mission_id, task_id, scope)
                         VALUES (?1, ?2, ?3)",
                    )
                    .map_err(db_err)?;
                for scope in scopes {
                    stmt.execute(params![mission_id, task_id, scope])
                        .map_err(db_err)?;
                }
            }
            tx.commit().map_err(db_err)?;
            Ok(true)
        })
    }

    /// Release every scope lock held by `task_id`. Returns how many were held.
    pub fn release_scope_locks(&self, task_id: &str) -> Result<usize> {
        self.with_conn("scope.release", |conn| {
            conn.prepare_cached("DELETE FROM scope_locks WHERE task_id = ?1")
                .map_err(db_err)?
                .execute(params![task_id])
                .map_err(db_err)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_overlap_rules() {
        assert!(scopes_overlap("src/api/**", "src/api/handlers/*.rs"));
        assert!(scopes_overlap("src/api/handlers/*.rs", "src/api/**"));
        assert!(!scopes_overlap("src/api/**", "docs/**"));
        assert!(scopes_overlap("*", "docs/**")); // empty prefix hits everything
        assert!(scopes_overlap("**/mod.rs", "src/lib.rs"));
        assert!(scopes_overlap("src/main.rs", "src/main.rs"));
        assert!(!scopes_overlap("src/a.rs", "src/b.rs"));
        // "src/a.rs" vs "src/a" — prefix containment is conservative overlap.
        assert!(scopes_overlap("src/a.rs", "src/a*"));
    }
}
