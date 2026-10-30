//! Persisted circuit-breaker state per provider: survives daemon restarts.

use crate::{db_err, Store};
use maestro_types::domain::ProviderHealth;
use maestro_types::{now_ms, Result};
use rusqlite::{params, OptionalExtension};

const HEALTH_COLS: &str =
    "provider_id, success_count, failure_count, consecutive_failures, open_until, updated_at";

fn health_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderHealth> {
    Ok(ProviderHealth {
        provider_id: row.get(0)?,
        success_count: row.get(1)?,
        failure_count: row.get(2)?,
        consecutive_failures: row.get(3)?,
        open_until: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

impl Store {
    /// Current health snapshot for one provider. Providers with no recorded
    /// results yet get a zeroed default (not an error).
    pub fn get_provider_health(&self, provider_id: &str) -> Result<ProviderHealth> {
        self.with_conn("provider.health_get", |conn| {
            let sql = format!("SELECT {HEALTH_COLS} FROM provider_health WHERE provider_id = ?1");
            let found = conn
                .prepare_cached(&sql)
                .map_err(db_err)?
                .query_row(params![provider_id], health_from_row)
                .optional()
                .map_err(db_err)?;
            Ok(found.unwrap_or_else(|| ProviderHealth {
                provider_id: provider_id.to_string(),
                ..ProviderHealth::default()
            }))
        })
    }

    /// Record one call outcome and update breaker state.
    ///
    /// Success resets `consecutive_failures` and closes the breaker. Failure
    /// increments the streak; once it reaches `breaker_threshold` the breaker
    /// opens (`open_until = now + cooldown_ms`). Returns the updated snapshot.
    pub fn record_provider_result(
        &self,
        provider_id: &str,
        success: bool,
        breaker_threshold: i64,
        cooldown_ms: i64,
    ) -> Result<ProviderHealth> {
        self.with_conn("provider.health_record", |conn| {
            let now = now_ms();
            let tx = conn.transaction().map_err(db_err)?;
            let sql = format!("SELECT {HEALTH_COLS} FROM provider_health WHERE provider_id = ?1");
            let mut health = tx
                .prepare_cached(&sql)
                .map_err(db_err)?
                .query_row(params![provider_id], health_from_row)
                .optional()
                .map_err(db_err)?
                .unwrap_or_else(|| ProviderHealth {
                    provider_id: provider_id.to_string(),
                    ..ProviderHealth::default()
                });
            if success {
                health.success_count += 1;
                health.consecutive_failures = 0;
                health.open_until = None;
            } else {
                health.failure_count += 1;
                health.consecutive_failures += 1;
                if health.consecutive_failures >= breaker_threshold {
                    health.open_until = Some(now + cooldown_ms);
                    tracing::warn!(
                        target: "maestro_store",
                        %provider_id,
                        consecutive = health.consecutive_failures,
                        cooldown_ms,
                        "provider circuit breaker opened"
                    );
                }
            }
            health.updated_at = now;
            tx.prepare_cached(
                "INSERT INTO provider_health (provider_id, success_count, failure_count, \
                 consecutive_failures, open_until, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT (provider_id) DO UPDATE SET
                    success_count = excluded.success_count,
                    failure_count = excluded.failure_count,
                    consecutive_failures = excluded.consecutive_failures,
                    open_until = excluded.open_until,
                    updated_at = excluded.updated_at",
            )
            .map_err(db_err)?
            .execute(params![
                health.provider_id,
                health.success_count,
                health.failure_count,
                health.consecutive_failures,
                health.open_until,
                health.updated_at
            ])
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok(health)
        })
    }

    /// Health snapshots for every provider seen so far.
    pub fn list_provider_health(&self) -> Result<Vec<ProviderHealth>> {
        self.with_conn("provider.health_list", |conn| {
            let sql = format!("SELECT {HEALTH_COLS} FROM provider_health ORDER BY provider_id");
            let mut stmt = conn.prepare_cached(&sql).map_err(db_err)?;
            let rows = stmt.query_map([], health_from_row).map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }
}
