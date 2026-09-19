//! Postgres I/O for the intelligence layer (S7): events by window in, daily values, baselines
//! and observations out. Thin on purpose — all logic lives in [`super::patterns`] and
//! [`super::baseline`], which never touch a database or a clock.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::Domain;
use crate::permissions::Effect;

use super::baseline::{Baseline, DriftObservation, ExistingObservation};
use super::ledger::ActivityEvent;
use super::patterns::DailyValue;

fn midnight_utc(day: NaiveDate) -> DateTime<Utc> {
    day.and_hms_opt(0, 0, 0).expect("valid time").and_utc()
}

/// Ledger events for `user_id` with `from <= ts::date <= to_inclusive` (UTC days), oldest first.
/// This is the Postgres read the ring-only `LedgerService::query` defers to S7.
pub async fn fetch_events(pool: &PgPool, user_id: &str, from: NaiveDate, to_inclusive: NaiveDate) -> anyhow::Result<Vec<ActivityEvent>> {
    let rows = sqlx::query(
        "SELECT id, user_id, ts, domain, agent_id, kind, effect, duration_ms, subject_hash, meta, trace_id \
         FROM activity_events WHERE user_id = $1 AND ts >= $2 AND ts < $3 ORDER BY ts",
    )
    .bind(user_id)
    .bind(midnight_utc(from))
    .bind(midnight_utc(to_inclusive + Duration::days(1)))
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let domain: String = r.try_get("domain")?;
            let effect: Option<String> = r.try_get("effect")?;
            Ok(ActivityEvent {
                id: r.try_get::<i64, _>("id").ok(),
                user_id: r.try_get("user_id")?,
                ts: r.try_get("ts")?,
                domain: Domain::parse(&domain).unwrap_or_default(),
                agent_id: r.try_get("agent_id")?,
                kind: r.try_get("kind")?,
                effect: effect.as_deref().and_then(Effect::parse),
                duration_ms: r.try_get("duration_ms")?,
                subject_hash: r.try_get("subject_hash")?,
                meta: r.try_get("meta")?,
                trace_id: r.try_get("trace_id")?,
            })
        })
        .collect()
}

/// Upsert daily values (idempotent: re-running a day overwrites it).
pub async fn upsert_daily(pool: &PgPool, rows: &[DailyValue]) -> anyhow::Result<usize> {
    for r in rows {
        sqlx::query(
            "INSERT INTO activity_daily (user_id, day, dimension, value, sample_n, computed_at) \
             VALUES ($1, $2, $3, $4, $5, NOW()) \
             ON CONFLICT (user_id, day, dimension) \
             DO UPDATE SET value = EXCLUDED.value, sample_n = EXCLUDED.sample_n, computed_at = NOW()",
        )
        .bind(&r.user_id)
        .bind(r.day)
        .bind(&r.dimension)
        .bind(r.value)
        .bind(r.sample_n as i32)
        .execute(pool)
        .await?;
    }
    Ok(rows.len())
}

/// Upsert baselines for a window; a user's `confirmed` flag survives recomputation.
pub async fn upsert_baselines(pool: &PgPool, rows: &[Baseline]) -> anyhow::Result<usize> {
    for b in rows {
        sqlx::query(
            "INSERT INTO baselines (user_id, dimension, day_class, median, mad, sample_n, window_end, confirmed, computed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW()) \
             ON CONFLICT (user_id, dimension, day_class, window_end) \
             DO UPDATE SET median = EXCLUDED.median, mad = EXCLUDED.mad, sample_n = EXCLUDED.sample_n, computed_at = NOW()",
        )
        .bind(&b.user_id)
        .bind(&b.dimension)
        .bind(b.day_class.as_str())
        .bind(b.median)
        .bind(b.mad)
        .bind(b.sample_n as i32)
        .bind(b.window_end)
        .bind(b.confirmed)
        .execute(pool)
        .await?;
    }
    Ok(rows.len())
}

/// Store a drift observation as `new`. `created_at` is the evaluation date (midnight UTC), not
/// the wall clock, so cooldown arithmetic matches [`super::baseline::evaluate`].
pub async fn insert_observation(pool: &PgPool, o: &DriftObservation) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO observations (id, user_id, kind, domain, dimensions, window_days, facts, text, offer, status, created_at) \
         VALUES ($1, $2, $3, 'shared', $4, $5, $6, $7, $8, 'new', $9)",
    )
    .bind(id)
    .bind(&o.user_id)
    .bind(&o.kind)
    .bind(&o.dimensions)
    .bind(o.window_days as i32)
    .bind(&o.facts)
    .bind(&o.text)
    .bind(&o.offer)
    .bind(midnight_utc(o.as_of))
    .execute(pool)
    .await?;
    Ok(id)
}

/// The ledger row for an emitted observation (ADR-004): kind and dimension count only, never the
/// text. Timestamped at the evaluation date so it lines up with the observation's `created_at`.
pub fn ledger_event(o: &DriftObservation) -> ActivityEvent {
    let mut ev = ActivityEvent::new(o.user_id.clone(), Domain::Shared, "baseline", "observation")
        .meta(serde_json::json!({ "kind": o.kind, "count": o.dimensions.len(), "status": "new" }));
    ev.ts = midnight_utc(o.as_of);
    ev
}

/// Drift observations created on or after `since`, for the cooldown.
pub async fn recent_observations(pool: &PgPool, user_id: &str, since: NaiveDate) -> anyhow::Result<Vec<ExistingObservation>> {
    let rows = sqlx::query("SELECT dimensions, created_at FROM observations WHERE user_id = $1 AND kind = 'drift' AND created_at >= $2")
        .bind(user_id)
        .bind(midnight_utc(since))
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|r| {
            let created_at: DateTime<Utc> = r.try_get("created_at")?;
            Ok(ExistingObservation { dimensions: r.try_get("dimensions")?, created_on: created_at.date_naive() })
        })
        .collect()
}
