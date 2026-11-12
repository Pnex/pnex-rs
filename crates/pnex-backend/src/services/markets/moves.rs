//! Abnormal sessions of the watched series (markets-research.md D734-D738):
//! after each import that stores values of a watched series, the detector
//! runs on the whole stored series and every new abnormal session becomes a
//! `market_event` of kind `market_move`. Deterministic on a primary source:
//! created directly, no proposal (D738). Idempotent: one event per series,
//! session and mode.

use std::collections::{BTreeMap, HashSet};

use chrono::NaiveDate;
use pnex_core::markets::moves::{
    self as mv, MarketSession, MoveMode, SessionMove, SessionWorldEvent, WatchedSeries,
};
use pnex_core::markets::{ObservationTarget, PlannedObservation};
use pnex_core::ontology::api::{LinkInput, ObjectInput};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::services::ontology::{changes, links, objects, schema, Actor, OntologyError};

/// FRED series and tickers watched (D737): the same entries, two sources.
pub const MOVE_SETTINGS: [&str; 2] = ["markets.move_series", "markets.move_tickers"];
pub const EVENT_TYPE: &str = "market_event";
pub const KIND_MOVE: &str = "market_move";
const LINK_ABOUT: &str = "event_about";
pub const GEO_TYPE: &str = "geo_event";

/// Sessions older than this are not announced (a first reading of a ticker).
const NOTICE_DAYS: i64 = 5;

/// Channel names the session notice also goes to (D738).
const NOTIFY_SETTING: &str = "markets.move_notify_channels";

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

/// The metric of the stored values a watched series is read from.
fn metric_of(target: &ObservationTarget, w: &WatchedSeries) -> Option<String> {
    match target.type_key.as_str() {
        "macro_series" if w.mode != MoveMode::Volume => Some(w.code.clone()),
        "instrument" if w.mode == MoveMode::Volume => Some("volume".into()),
        "instrument" => Some("close".into()),
        _ => None,
    }
}

/// The change as people read it: `-5.5 %` for a return, `+0.25` otherwise.
fn shown(mode: MoveMode, v: f64) -> String {
    match mode {
        MoveMode::LogReturn => format!("{:+.1} %", (v.exp() - 1.0) * 100.0),
        MoveMode::Diff => format!("{v:+.2}"),
        MoveMode::Volume => format!("volume x{:.1}", v.exp()),
    }
}

/// Runs after an import stored values: never fails the import, whose values
/// are already stored. A failure is repaired by the next run, which looks at
/// the whole series again.
pub async fn after_store(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    target: &ObservationTarget,
    rows: &[PlannedObservation],
) {
    if rows.is_empty() {
        return;
    }
    match detect_touched(db, org_id, actor, target, rows).await {
        Ok(created) if !created.is_empty() => notify(db, org_id, &created).await,
        Ok(_) => {}
        Err(e) => tracing::warn!(org = org_id, error = %e, "market move detection failed"),
    }
}

/// A created event, for the session notice.
struct Created {
    date: NaiveDate,
    code: String,
    text: String,
    backfilled: bool,
}

async fn detect_touched(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    target: &ObservationTarget,
    rows: &[PlannedObservation],
) -> Result<Vec<Created>, OntologyError> {
    let watched = watched(db, org_id).await?;
    let touched: HashSet<String> = rows.iter().map(|o| o.key.trim().to_uppercase()).collect();
    let mut created = Vec::new();
    for w in watched {
        if !touched.contains(&w.code) {
            continue;
        }
        let Some(metric) = metric_of(target, &w) else {
            continue;
        };
        created.extend(detect_one(db, org_id, actor, target, &w, &metric).await?);
    }
    Ok(created)
}

async fn detect_one(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    target: &ObservationTarget,
    w: &WatchedSeries,
    metric: &str,
) -> Result<Vec<Created>, OntologyError> {
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT id FROM objects WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL \
               AND upper(properties ->> $3) = $4",
            vec![
                org_id.into(),
                target.type_key.clone().into(),
                target.property.clone().into(),
                w.code.clone().into(),
            ],
        ))
        .await?
    else {
        return Ok(Vec::new());
    };
    let series_id: Uuid = row.try_get("", "id")?;
    // The last value known for each session, with when it became known: a
    // value caught up from history takes the date of a dated release that
    // gave the same value (ALFRED, D811), so the move is not backfilled.
    let values = db
        .query_all_raw(stmt(
            "SELECT DISTINCT ON (l.period_end) l.period_end::text AS d, l.value, \
               to_char(COALESCE(r.known_at, l.known_at) AT TIME ZONE 'UTC', \
                 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS k, \
               l.backfilled AND r.known_at IS NULL AS backfilled \
             FROM market_observations l \
             LEFT JOIN LATERAL ( \
               SELECT f.known_at FROM market_observations f \
               WHERE l.backfilled AND f.org_id = l.org_id AND f.object_id = l.object_id \
                 AND f.metric = l.metric AND f.period_end = l.period_end \
                 AND NOT f.backfilled AND f.value = l.value \
               ORDER BY f.known_at DESC LIMIT 1 \
             ) r ON true \
             WHERE l.org_id = $1 AND l.object_id = $2 AND l.metric = $3 \
             ORDER BY l.period_end, l.known_at DESC",
            vec![org_id.into(), series_id.into(), metric.into()],
        ))
        .await?;
    let mut series = Vec::with_capacity(values.len());
    let mut known: BTreeMap<NaiveDate, (String, bool)> = BTreeMap::new();
    for r in &values {
        let d: String = r.try_get("", "d")?;
        let Ok(date) = NaiveDate::parse_from_str(&d, "%Y-%m-%d") else {
            continue;
        };
        series.push((date, r.try_get::<f64>("", "value")?));
        known.insert(date, (r.try_get("", "k")?, r.try_get("", "backfilled")?));
    }
    let moves = mv::detect(&series, w.mode, &w.params);
    if moves.is_empty() {
        return Ok(Vec::new());
    }
    let refs: Vec<String> = moves
        .iter()
        .map(|m| mv::event_ref(&w.code, m.date, w.mode))
        .collect();
    // Archived ones too: an event a member archived is not written again.
    let existing: HashSet<String> = db
        .query_all_raw(stmt(
            "SELECT properties ->> 'ref' AS r FROM objects \
             WHERE org_id = $1 AND type_key = $2 AND properties ->> 'ref' = ANY($3)",
            vec![org_id.into(), EVENT_TYPE.into(), refs.clone().into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get("", "r").ok())
        .collect();
    let mut out = Vec::new();
    let s = schema(db, org_id).await?;
    for (m, r) in moves.iter().zip(refs) {
        if existing.contains(&r) {
            continue;
        }
        let Some((known_at, backfilled)) = known.get(&m.date).cloned() else {
            continue;
        };
        let score = if m.sigma > 0.0 {
            (m.value / m.sigma).abs()
        } else {
            0.0
        };
        let text = shown(w.mode, m.value);
        let mut props = Map::new();
        for (k, v) in [
            ("ref", json!(r)),
            ("kind", json!(KIND_MOVE)),
            ("event_at", json!(m.date.to_string())),
            ("known_at", json!(known_at)),
            ("backfilled", json!(backfilled)),
            ("series_code", json!(w.code)),
            ("asset_class", json!(w.asset_class.as_str())),
            ("move_direction", json!(m.direction.as_str())),
            ("mode", json!(w.mode.as_str())),
            ("measure", json!(m.value)),
            ("sigma", json!(m.sigma)),
            ("threshold", json!(m.threshold)),
            ("score", json!((score * 100.0).round() / 100.0)),
            ("metric", json!(metric)),
        ] {
            props.insert(k.into(), v);
        }
        let input = ObjectInput {
            type_key: EVENT_TYPE.into(),
            title: format!("{} {text} · {}", w.code, m.date),
            properties: props,
            expected_version: None,
        };
        changes::deferred(async {
            let txn = db.begin().await?;
            let event = objects::create_with(&txn, &s, org_id, actor, &input).await?;
            let link = LinkInput {
                link_type: LINK_ABOUT.into(),
                source_id: event.id.clone(),
                target_id: series_id.to_string(),
                attributes: Map::new(),
                valid_from: None,
                valid_to: None,
            };
            links::create_with(&txn, &s, org_id, actor, &link).await?;
            txn.commit().await?;
            changes::flush();
            Ok::<_, OntologyError>(())
        })
        .await?;
        out.push(Created {
            date: m.date,
            code: w.code.clone(),
            text,
            backfilled,
        });
    }
    Ok(out)
}

/// One notice per session for the moves just found (D738); moves caught up
/// from a history are not news.
async fn notify(db: &DatabaseConnection, org_id: i64, created: &[Created]) {
    // A ticker turned on reads years of bars, each known on its own day:
    // only the sessions of the last days are news.
    let recent = chrono::Utc::now().date_naive() - chrono::Duration::days(NOTICE_DAYS);
    let mut by_day: BTreeMap<NaiveDate, Vec<&Created>> = BTreeMap::new();
    for c in created.iter().filter(|c| !c.backfilled && c.date >= recent) {
        by_day.entry(c.date).or_default().push(c);
    }
    for (day, moves) in by_day {
        let list = moves
            .iter()
            .map(|c| format!("{} {}", c.code, c.text))
            .collect::<Vec<_>>()
            .join(", ");
        let msg = pnex_notify::Message {
            subject: Some(format!("Abnormal session {day}")),
            body: format!("{day}: {list}. Measured on the series, not a cause."),
            meta: json!({
                "kind": "market-moves",
                "date": day.to_string(),
                "series": moves.iter().map(|c| c.code.clone()).collect::<Vec<_>>(),
            }),
        };
        crate::services::data_builds::notify_org(db, org_id, "market-moves", msg.clone()).await;
        to_channels(db, org_id, &msg).await;
    }
}

/// The org's channels named in `move_notify_channels` (D738): e-mail, Slack,
/// ntfy… any kind but the websocket, which always gets the notice. An
/// unknown or disabled name is skipped; every send is journaled.
pub(super) async fn to_channels(db: &DatabaseConnection, org_id: i64, msg: &pnex_notify::Message) {
    use crate::models::_entities::notify_channels;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let names = crate::services::ontology::packs::setting_value(db, org_id, NOTIFY_SETTING)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let names: Vec<String> = names
        .split([',', ';'])
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return;
    }
    let Some(ring) = super::keyring() else {
        tracing::warn!(org = org_id, "market move notice: no vault keyring");
        return;
    };
    let channels = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(org_id))
        .filter(notify_channels::Column::Name.is_in(names))
        .filter(notify_channels::Column::Enabled.eq(true))
        .filter(notify_channels::Column::Kind.ne("websocket"))
        .all(db)
        .await
        .unwrap_or_default();
    for ch in channels {
        let Some(sender) = pnex_notify::channel(&ch.kind) else {
            continue;
        };
        let outcome = match crate::services::secrets::notify::sendable(
            db, ring, org_id, &ch.kind, &ch.config,
        )
        .await
        {
            Ok(config) => sender.send(&config, msg).await.map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        let (status, error) = match &outcome {
            Ok(()) => (pnex_core::DELIVERY_SENT, None),
            Err(e) => (pnex_core::DELIVERY_FAILED, Some(e.clone())),
        };
        crate::services::notify_journal::submit(pnex_core::NotifyDeliveryEntry {
            org_id,
            channel_id: ch.id,
            channel_kind: ch.kind.clone(),
            source: "market-moves".into(),
            status: status.into(),
            error,
            subject: msg.subject.clone(),
            ..Default::default()
        });
    }
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
