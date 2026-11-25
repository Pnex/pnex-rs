//! Candidate causal chains of the moves of a bar series, as known at a date
//! (markets-research.md §3.14, read by `GET /analysis/chains`, PE8): an input
//! of an external model, computed at reading time and never stored as a
//! cause (D742).
//!
//! Effects: 1-hour returns of the series (rows of a bar Table whose
//! `instrument` is the key) beyond the threshold (`price.move`).
//! Causes: world events (`world.event`, or `supply.disruption` for a blockade
//! or sanctions) on their country, macro releases on their series, and
//! measured disruptions (a drop beyond a series' `disruption_drop_pct`). Paths:
//! the `exposed_to` links (any link flagged `propagates_failure`).

use chrono::{DateTime, Duration, Utc};
use pnex_core::markets::decisions as dec;
use pnex_core::rca::{self, Chain, Signal, SignalKind};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use uuid::Uuid;

use super::decisions::{self as svc, BarsError, BarsQuery};
use crate::services::artifact_store::S3Store;

/// Longest window of effects read at once.
pub const WINDOW_MAX_DAYS: i64 = 31;
/// Default window of effects before `as_of`.
pub const WINDOW_DEFAULT_DAYS: i64 = 7;
/// Threshold of a price move when neither the instrument nor the org sets one.
pub const DEFAULT_THRESHOLD_PCT: f64 = 1.5;

#[derive(Debug)]
pub enum ChainsError {
    /// No instrument object with the key as ticker.
    UnknownKey,
    Bars(BarsError),
    Db(sea_orm::DbErr),
    Onto(crate::services::ontology::OntologyError),
}

impl From<BarsError> for ChainsError {
    fn from(e: BarsError) -> Self {
        Self::Bars(e)
    }
}

impl From<sea_orm::DbErr> for ChainsError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Db(e)
    }
}

impl From<crate::services::ontology::OntologyError> for ChainsError {
    fn from(e: crate::services::ontology::OntologyError) -> Self {
        Self::Onto(e)
    }
}

pub struct ChainsQuery<'a> {
    /// Value of the Table's `instrument` column, and ticker of the effect object.
    pub key: &'a str,
    /// Value of the `source` column; empty reads every source.
    pub source: &'a str,
    pub table: Uuid,
    pub from: DateTime<Utc>,
    pub as_of: DateTime<Utc>,
    /// Threshold of a move; the instrument's, then the org's, when `None`.
    pub threshold_pct: Option<f64>,
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

/// The instrument object (`ticker`) and its own threshold. The threshold
/// is a setting of the reading, not a dated fact: the current one applies
/// to every instant, so a feature means the same thing across a backtest.
async fn instrument(
    db: &DatabaseConnection,
    org: i64,
    ticker: &str,
    at: DateTime<Utc>,
) -> Result<(Uuid, Option<f64>), ChainsError> {
    let row = db
        .query_one_raw(stmt(
            "SELECT o.id, o.properties ->> 'move_threshold_pct' AS t FROM objects o \
             WHERE o.org_id = $1 AND o.type_key = 'instrument' AND o.properties ->> 'ticker' = $2 \
               AND (o.valid_to IS NULL OR o.valid_to > $3) LIMIT 1",
            vec![org.into(), ticker.into(), at.into()],
        ))
        .await?
        .ok_or(ChainsError::UnknownKey)?;
    let threshold = row
        .try_get::<Option<String>>("", "t")?
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|t| *t > 0.0);
    Ok((row.try_get("", "id")?, threshold))
}

/// 1-hour returns at least `threshold_pct` (percent), known at their bar's
/// close, in `[from, as_of]`.
pub fn price_moves(bars: &[dec::Bar], on: &str, ticker: &str, threshold_pct: f64) -> Vec<Signal> {
    const HOUR_US: i64 = 3_600_000_000;
    bars.windows(2)
        .filter_map(|w| {
            let (prev, bar) = (w[0].close as f64, w[1].close as f64);
            if prev <= 0.0 {
                return None;
            }
            let r = (bar / prev - 1.0) * 100.0;
            (r.abs() >= threshold_pct).then(|| {
                let close = (w[1].ts_us + HOUR_US) / 1_000_000;
                Signal {
                    id: format!("price.move:{ticker}:{close}"),
                    at: close,
                    on: on.to_string(),
                    kind: SignalKind::PriceMove,
                    severity: (r.abs() / (3.0 * threshold_pct)).min(1.0),
                    source_ref: format!("bars:{ticker}:1h:{close}"),
                }
            })
        })
        .collect()
}

/// World events known in `[from, as_of]`, as they were at `as_of`, on their
/// country object; a dismissed one is no signal.
async fn world_events(
    db: &DatabaseConnection,
    org: i64,
    from: DateTime<Utc>,
    as_of: DateTime<Utc>,
) -> Result<Vec<Signal>, ChainsError> {
    let rows = db
        .query_all_raw(stmt(
            "WITH e AS ( \
               SELECT o.id, COALESCE(v.properties, o.properties) AS p FROM objects o \
               LEFT JOIN LATERAL (SELECT properties FROM object_versions w \
                 WHERE w.object_id = o.id AND w.at <= $3 ORDER BY w.at DESC LIMIT 1) v ON TRUE \
               WHERE o.org_id = $1 AND o.type_key = 'geo_event' \
                 AND (o.valid_to IS NULL OR o.valid_to > $3)) \
             SELECT e.id, c.id AS area_id, e.p ->> 'kind' AS kind, e.p ->> 'status' AS status, \
                    (e.p ->> 'known_at')::timestamptz AS known_at \
             FROM e JOIN objects c ON c.org_id = $1 AND c.type_key = 'nation' \
               AND c.properties ->> 'code' = e.p ->> 'country' AND c.valid_to IS NULL \
             WHERE (e.p ->> 'known_at')::timestamptz BETWEEN $2 AND $3",
            vec![org.into(), from.into(), as_of.into()],
        ))
        .await?;
    let mut out = Vec::new();
    for r in rows {
        let status: Option<String> = r.try_get("", "status")?;
        if status.as_deref() == Some("dismissed") {
            continue;
        }
        let id: Uuid = r.try_get("", "id")?;
        let nation: Uuid = r.try_get("", "area_id")?;
        let kind: Option<String> = r.try_get("", "kind")?;
        let at: DateTime<chrono::FixedOffset> = r.try_get("", "known_at")?;
        let kind = match kind.as_deref() {
            Some("blockade" | "sanctions") => SignalKind::SupplyDisruption,
            _ => SignalKind::WorldEvent,
        };
        out.push(Signal {
            id: format!("world.event:{id}"),
            at: at.timestamp(),
            on: nation.to_string(),
            kind,
            severity: if status.as_deref() == Some("confirmed") {
                1.0
            } else {
                0.5
            },
            source_ref: format!("object:{id}"),
        });
    }
    Ok(out)
}

/// Macro values first known in `[from, as_of]`, on their series.
async fn macro_releases(
    db: &DatabaseConnection,
    org: i64,
    from: DateTime<Utc>,
    as_of: DateTime<Utc>,
) -> Result<Vec<Signal>, ChainsError> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT DISTINCT m.object_id, m.known_at FROM market_observations m \
             JOIN objects o ON o.id = m.object_id AND o.type_key = 'macro_series' \
             WHERE m.org_id = $1 AND m.known_at BETWEEN $2 AND $3",
            vec![org.into(), from.into(), as_of.into()],
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: Uuid = r.try_get("", "object_id")?;
        let at: DateTime<chrono::FixedOffset> = r.try_get("", "known_at")?;
        out.push(Signal {
            id: format!("macro.release:{id}:{}", at.timestamp()),
            at: at.timestamp(),
            on: id.to_string(),
            kind: SignalKind::MacroRelease,
            severity: 1.0,
            source_ref: format!("observations:{id}:{}", at.timestamp()),
        });
    }
    Ok(out)
}

/// Measured supply disruptions known in `[from, as_of]`: a value of a
/// series that declares `disruption_drop_pct` at least that far below the
/// mean of its 7 previous values (same metric), all as known at `as_of`.
/// The threshold is the current property, like an instrument's (markets-research.md §3.12).
async fn measured_disruptions(
    db: &DatabaseConnection,
    org: i64,
    from: DateTime<Utc>,
    as_of: DateTime<Utc>,
) -> Result<Vec<Signal>, ChainsError> {
    let rows = db
        .query_all_raw(stmt(
            "WITH s AS ( \
               SELECT o.id, (o.properties ->> 'disruption_drop_pct')::float8 AS t FROM objects o \
               WHERE o.org_id = $1 AND o.type_key = 'macro_series' AND o.valid_to IS NULL \
                 AND jsonb_typeof(o.properties -> 'disruption_drop_pct') = 'number'), \
             v AS ( \
               SELECT DISTINCT ON (m.object_id, m.metric, m.period_end) \
                      m.object_id, m.metric, m.period_end, m.value, m.known_at \
               FROM market_observations m JOIN s ON s.id = m.object_id \
               WHERE m.org_id = $1 AND m.known_at <= $3 \
               ORDER BY m.object_id, m.metric, m.period_end, m.known_at DESC), \
             w AS ( \
               SELECT v.*, AVG(v.value) OVER (PARTITION BY v.object_id, v.metric \
                 ORDER BY v.period_end ROWS BETWEEN 7 PRECEDING AND 1 PRECEDING) AS base FROM v) \
             SELECT w.object_id, w.metric, w.known_at, (w.base - w.value) / w.base * 100 AS drop, s.t \
             FROM w JOIN s ON s.id = w.object_id \
             WHERE w.known_at BETWEEN $2 AND $3 AND w.base > 0 \
               AND (w.base - w.value) / w.base * 100 >= s.t",
            vec![org.into(), from.into(), as_of.into()],
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: Uuid = r.try_get("", "object_id")?;
        let metric: String = r.try_get("", "metric")?;
        let at: DateTime<chrono::FixedOffset> = r.try_get("", "known_at")?;
        let drop: f64 = r.try_get("", "drop")?;
        let t: f64 = r.try_get("", "t")?;
        out.push(Signal {
            id: format!("supply.disruption:{id}:{metric}:{}", at.timestamp()),
            at: at.timestamp(),
            on: id.to_string(),
            kind: SignalKind::SupplyDisruption,
            severity: (drop / (3.0 * t)).min(1.0),
            source_ref: format!("observations:{id}:{}", at.timestamp()),
        });
    }
    Ok(out)
}

/// Candidate chains of the series' moves in `[from, as_of]`, as
/// known at `as_of`, every one with its lift (never filtered: the model
/// weighs them, markets-research.md §3.14).
pub async fn chains(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    q: &ChainsQuery<'_>,
) -> Result<Vec<Chain>, ChainsError> {
    let pairs: Vec<_> = crate::services::ontology::packs::causal_pairs(db, org)
        .await?
        .into_iter()
        .filter(|p| p.effect == SignalKind::PriceMove)
        .collect();
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let (instrument_id, own) = instrument(db, org, q.key, q.as_of).await?;
    let threshold = match q.threshold_pct.or(own) {
        Some(t) => t,
        None => crate::services::ontology::packs::setting_value(
            db,
            org,
            "markets.price_move_threshold_pct",
        )
        .await?
        .and_then(|v| v.as_f64())
        .unwrap_or(DEFAULT_THRESHOLD_PCT),
    };
    let hour_us = 3_600_000_000;
    let bars = svc::bars(
        db,
        store,
        org,
        q.table,
        &BarsQuery {
            instrument: q.key,
            source: q.source,
            interval_us: hour_us,
            from_us: Some(q.from.timestamp_micros() - 2 * hour_us),
            to_us: None,
            limit: dec::BARS_MAX,
            as_of_us: q.as_of.timestamp_micros(),
        },
    )
    .await?;
    let effects: Vec<Signal> = price_moves(&bars, &instrument_id.to_string(), q.key, threshold)
        .into_iter()
        .filter(|s| s.at >= q.from.timestamp() && s.at <= q.as_of.timestamp())
        .collect();
    if effects.is_empty() {
        return Ok(Vec::new());
    }
    // Causes up to the longest gap before the window, and back far enough
    // for the placebo (D908).
    let longest = pairs.iter().map(|p| p.max_gap).max().unwrap_or(0);
    let back = Duration::seconds(longest + rca::PLACEBO_SHIFTS.iter().max().copied().unwrap_or(0));
    let causes_from = q.from - back;
    let mut signals = world_events(db, org, causes_from, q.as_of).await?;
    signals.extend(macro_releases(db, org, causes_from, q.as_of).await?);
    signals.extend(measured_disruptions(db, org, causes_from, q.as_of).await?);
    signals.extend(effects.iter().cloned());
    let topology = crate::services::rca::topology(db, org, causes_from, q.as_of).await?;
    Ok(rca::chains(&effects, &signals, &topology, &pairs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(h: i64, close: i128) -> dec::Bar {
        dec::Bar {
            ts_us: h * 3_600_000_000,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1,
        }
    }

    #[test]
    fn moves_beyond_the_threshold_dated_at_the_close() {
        let bars = vec![bar(0, 1000), bar(1, 1010), bar(2, 1030), bar(3, 1010)];
        let moves = price_moves(&bars, "i", "GOLD", 1.5);
        // +1.0 % no; +1.98 % yes; −1.94 % yes.
        assert_eq!(moves.len(), 2);
        assert_eq!(moves[0].at, 3 * 3_600);
        assert_eq!(moves[1].at, 4 * 3_600);
        assert!(moves
            .iter()
            .all(|m| m.kind == SignalKind::PriceMove && m.on == "i"));
    }
}
