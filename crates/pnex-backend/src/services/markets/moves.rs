//! Abnormal sessions of the watched series (markets-research.md D734-D738):
//! found by the pack's pipeline `markets/market_moves` and created by the
//! import "Abnormal sessions" (pipelines.md D946, D949). Here: the
//! watched entries and the session view.

use chrono::NaiveDate;
use pnex_core::markets::moves::{
    self as mv, MarketSession, SessionMove, SessionWorldEvent, WatchedSeries,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use serde_json::Value;
use uuid::Uuid;

/// FRED series and tickers watched (D737): the same entries, two sources.
pub const MOVE_SETTINGS: [&str; 2] = ["markets.move_series", "markets.move_tickers"];
pub const EVENT_TYPE: &str = "market_event";
pub const KIND_MOVE: &str = "market_move";
pub const GEO_TYPE: &str = "geo_event";

/// Every watched series of the org, FRED codes then tickers.
pub async fn watched<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Vec<WatchedSeries>, DbErr> {
    let mut out = Vec::new();
    for key in MOVE_SETTINGS {
        let raw = crate::services::ontology::packs::setting_value(db, org_id, key)
            .await?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        out.extend(mv::watched_series(&raw));
    }
    Ok(out)
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().into()
}

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// The moves and world events around one session (D738, D742). A world
/// event is "in the same window", never a cause.
pub async fn session<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    date: NaiveDate,
) -> Result<MarketSession, DbErr> {
    let (from, to) = mv::session_window(date);
    let rows = db
        .query_all_raw(stmt(
            "SELECT id, title, type_key, properties FROM objects \
             WHERE org_id = $1 AND valid_to IS NULL AND type_key IN ($2, $3) \
               AND properties ->> 'event_at' BETWEEN $4 AND $5 \
               AND (type_key = $3 OR properties ->> 'kind' = $6) \
               AND coalesce(properties ->> 'status', '') <> 'dismissed' \
             LIMIT 500",
            vec![
                org_id.into(),
                EVENT_TYPE.into(),
                GEO_TYPE.into(),
                from.to_string().into(),
                to.to_string().into(),
                KIND_MOVE.into(),
            ],
        ))
        .await?;
    let (mut moves, mut world) = (Vec::new(), Vec::new());
    for r in rows {
        let id: Uuid = r.try_get("", "id")?;
        let title: String = r.try_get("", "title")?;
        let ty: String = r.try_get("", "type_key")?;
        let p: Value = r.try_get("", "properties")?;
        if ty == GEO_TYPE {
            world.push(SessionWorldEvent {
                id: id.to_string(),
                title,
                event_at: text(&p, "event_at"),
                kind: text(&p, "kind"),
                country: text(&p, "country"),
                status: match text(&p, "status").as_str() {
                    "confirmed" => "confirmed".into(),
                    _ => "unconfirmed".into(),
                },
            });
        } else {
            moves.push(SessionMove {
                id: id.to_string(),
                title,
                event_at: text(&p, "event_at"),
                series_code: text(&p, "series_code"),
                asset_class: text(&p, "asset_class"),
                move_direction: text(&p, "move_direction"),
                mode: text(&p, "mode"),
                measure: num(&p, "measure"),
                sigma: num(&p, "sigma"),
                score: num(&p, "score"),
                backfilled: p.get("backfilled").and_then(Value::as_bool) == Some(true),
            });
        }
    }
    let day = date.to_string();
    moves.sort_by(|a, b| {
        (b.event_at == day)
            .cmp(&(a.event_at == day))
            .then(b.score.total_cmp(&a.score))
            .then(a.title.cmp(&b.title))
    });
    world.sort_by(|a, b| a.event_at.cmp(&b.event_at).then(a.title.cmp(&b.title)));
    Ok(MarketSession {
        date: day,
        window_from: from.to_string(),
        window_to: to.to_string(),
        moves,
        world_events: world,
    })
}

/// The map's world events: those placed inside the bounding box, from the
/// last `days` days, dismissed ones left out, the most significant first
/// (deaths, then sources, then newest): a world view of 30 days holds
/// thousands, the map shows a page (measured 2026-11-06: 12 883) (D739).
pub async fn world_events_in<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    bbox: (f64, f64, f64, f64),
    days: i64,
    limit: i64,
) -> Result<(i64, Vec<mv::WorldEventPoint>), DbErr> {
    let since = (chrono::Utc::now().date_naive() - chrono::Duration::days(days)).to_string();
    let (west, south, east, north) = bbox;
    let filter = "FROM objects WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL \
           AND properties ->> 'event_at' >= $3 \
           AND coalesce(properties ->> 'status', '') <> 'dismissed' \
           AND jsonb_typeof(properties -> 'location' -> 'lon') = 'number' \
           AND (properties -> 'location' ->> 'lon')::float8 BETWEEN $4 AND $6 \
           AND (properties -> 'location' ->> 'lat')::float8 BETWEEN $5 AND $7";
    let values = || -> Vec<sea_orm::Value> {
        vec![
            org_id.into(),
            GEO_TYPE.into(),
            since.clone().into(),
            west.into(),
            south.into(),
            east.into(),
            north.into(),
        ]
    };
    let count: i64 = db
        .query_one_raw(stmt(&format!("SELECT count(*) AS n {filter}"), values()))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let mut v = values();
    v.push(limit.into());
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT id, title, properties {filter} \
                 ORDER BY coalesce((properties ->> 'deaths')::float8, 0) DESC, \
                   coalesce((properties ->> 'source_count')::float8, 0) DESC, \
                   properties ->> 'event_at' DESC, id LIMIT $8"
            ),
            v,
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: Uuid = r.try_get("", "id")?;
        let p: Value = r.try_get("", "properties")?;
        let (Some(lon), Some(lat)) = (
            p.pointer("/location/lon").and_then(Value::as_f64),
            p.pointer("/location/lat").and_then(Value::as_f64),
        ) else {
            continue;
        };
        out.push(mv::WorldEventPoint {
            id: id.to_string(),
            title: r.try_get("", "title")?,
            kind: text(&p, "kind"),
            status: if text(&p, "status") == "confirmed" {
                "confirmed".into()
            } else {
                "unconfirmed".into()
            },
            event_at: text(&p, "event_at"),
            lon,
            lat,
        });
    }
    Ok((count, out))
}
