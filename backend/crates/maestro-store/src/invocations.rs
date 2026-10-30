//! Provider invocation audit rows + aggregates (mission stats, scorecards).

use crate::{db_err, invalid_col, Store};
use maestro_types::domain::{Invocation, InvocationStatus, MissionStats, Role};
use maestro_types::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// Per (role, provider, model) aggregate derived from invocation rows.
/// Served at `GET /api/scorecards` and by `maestro scorecards`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scorecard {
    pub role: Role,
    pub provider_id: String,
    pub model: String,
    pub calls: i64,
    /// Fraction of calls with status `succeeded`, in `[0, 1]`.
    pub success_rate: f64,
    pub p50_latency_ms: i64,
    pub total_tokens: i64,
    pub cost_micro_usd: i64,
}

impl Store {
    /// Persist one provider invocation (id supplied by the caller).
    pub fn record_invocation(&self, inv: &Invocation) -> Result<()> {
        self.with_conn("invocation.record", |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO invocations (id, mission_id, task_id, role, provider_id, model, \
                     status, input_tokens, output_tokens, cost_micro_usd, latency_ms, error, \
                     created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                )
                .map_err(db_err)?;
            stmt.execute(params![
                inv.id,
                inv.mission_id,
                inv.task_id,
                inv.role.as_str(),
                inv.provider_id,
                inv.model,
                inv.status.as_str(),
                inv.input_tokens,
                inv.output_tokens,
                inv.cost_micro_usd,
                inv.latency_ms,
                inv.error,
                inv.created_at
            ])
            .map_err(db_err)?;
            Ok(())
        })
    }

    /// SQL aggregate of a mission's usage and cost.
    pub fn mission_stats(&self, mission_id: &str) -> Result<MissionStats> {
        self.with_conn("invocation.mission_stats", |conn| {
            conn.prepare_cached(
                "SELECT COUNT(*),
                        COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(cost_micro_usd), 0),
                        COALESCE(SUM(status = 'failed'), 0)
                 FROM invocations WHERE mission_id = ?1",
            )
            .map_err(db_err)?
            .query_row(params![mission_id], |row| {
                Ok(MissionStats {
                    model_calls: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cost_micro_usd: row.get(3)?,
                    failed_calls: row.get(4)?,
                })
            })
            .map_err(db_err)
        })
    }

    /// Per (role, provider, model) scorecards across all missions, sorted by
    /// role/provider/model. p50 latency is the true median row per group.
    pub fn scorecards(&self) -> Result<Vec<Scorecard>> {
        self.with_conn("invocation.scorecards", |conn| {
            struct Group {
                role: Role,
                provider_id: String,
                model: String,
                calls: i64,
                ok: i64,
                total_tokens: i64,
                cost_micro_usd: i64,
            }
            let groups: Vec<Group> = {
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT role, provider_id, model, COUNT(*),
                                COALESCE(SUM(status = ?1), 0),
                                COALESCE(SUM(input_tokens + output_tokens), 0),
                                COALESCE(SUM(cost_micro_usd), 0)
                         FROM invocations
                         GROUP BY role, provider_id, model
                         ORDER BY role, provider_id, model",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(params![InvocationStatus::Succeeded.as_str()], |row| {
                        let role: String = row.get(0)?;
                        Ok(Group {
                            role: Role::parse(&role)
                                .ok_or_else(|| invalid_col("invocation role", &role))?,
                            provider_id: row.get(1)?,
                            model: row.get(2)?,
                            calls: row.get(3)?,
                            ok: row.get(4)?,
                            total_tokens: row.get(5)?,
                            cost_micro_usd: row.get(6)?,
                        })
                    })
                    .map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)?
            };
            let mut p50_stmt = conn
                .prepare_cached(
                    "SELECT latency_ms FROM invocations
                     WHERE role = ?1 AND provider_id = ?2 AND model = ?3
                     ORDER BY latency_ms LIMIT 1 OFFSET ?4",
                )
                .map_err(db_err)?;
            let mut out = Vec::with_capacity(groups.len());
            for g in groups {
                let median_offset = (g.calls - 1) / 2;
                let p50_latency_ms: i64 = p50_stmt
                    .query_row(
                        params![g.role.as_str(), g.provider_id, g.model, median_offset],
                        |r| r.get(0),
                    )
                    .map_err(db_err)?;
                out.push(Scorecard {
                    role: g.role,
                    provider_id: g.provider_id,
                    model: g.model,
                    calls: g.calls,
                    success_rate: if g.calls > 0 {
                        g.ok as f64 / g.calls as f64
                    } else {
                        0.0
                    },
                    p50_latency_ms,
                    total_tokens: g.total_tokens,
                    cost_micro_usd: g.cost_micro_usd,
                });
            }
            Ok(out)
        })
    }
}
