//! Detections of a case (cases-workspace.md K30): the detectors of
//! `pnex_core::track::detect` run on the members' tracks over a window and
//! write proposals. A fingerprint (detector, members, start to the minute)
//! makes a recomputation insert only what is new: a rejected proposal is
//! never proposed again. Retaining makes an event in one transaction;
//! retaining twice returns the same event (row lock). Nothing is ever added
//! to the case without a human gesture. Thresholds are case properties set
//! in the case frame, the defaults below when unset.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::geo_area::GeoArea;
use pnex_core::ontology::api::{
    CaseAttachInput, CaseDetected, CaseEventInput, CaseProposal, CaseWorkspace, DetectQuery,
    ObjectRef, ProposalReject, ProposalRetain,
};
use pnex_core::ontology::{
    CASE_COPRESENCE_DISTANCE_M, CASE_COPRESENCE_MIN_S, CASE_SIGNAL_GAP_S, CASE_STOP_DIAMETER_M,
    CASE_STOP_MIN_S, EVENT_LOCATION, EVENT_NOTE,
};
use pnex_core::resources::KIND_MAP_PIN;
use pnex_core::track::detect::{
    co_presence, signal_losses, stops, zone_visits, DEFAULT_CO_DISTANCE_M as CO_DISTANCE_M,
    DEFAULT_CO_MIN_S as CO_MIN_S, DEFAULT_STOP_DIAMETER_M as STOP_DIAMETER_M,
    DEFAULT_STOP_MIN_S as STOP_MIN_S,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement, TransactionTrait,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::{invalid, links, parse_instant, ts, Actor, OntologyError, Result};
use super::signals::events_span;
use super::tracks::{load, MAX_SPAN_DAYS};
use super::{create_event_in, membership, workspace, zones_of, T_PLACE};
use crate::services::openobserve::Client;

const CO_PRESENCE: &str = "co_presence@1";
const STOP: &str = "stop@1";
const SIGNAL_LOSS: &str = "signal_loss@1";
const ZONE: &str = "zone@1";
const DEFAULT_SPAN_HOURS: i64 = 24;
const REASON_MAX: usize = 500;

/// Detector thresholds of a case: its properties, else the defaults.
pub(super) struct Thresholds {
    pub co_distance_m: f64,
    pub co_min_s: i64,
    pub stop_diameter_m: f64,
    pub stop_min_s: i64,
    /// `None` = the track reader's default.
    pub signal_gap_s: Option<i64>,
}

impl Thresholds {
    pub fn of(case: &crate::models::_entities::objects::Model) -> Self {
        let num = |k: &str| case.properties.get(k).and_then(Value::as_f64);
        Self {
            co_distance_m: num(CASE_COPRESENCE_DISTANCE_M).unwrap_or(CO_DISTANCE_M),
            co_min_s: num(CASE_COPRESENCE_MIN_S).map_or(CO_MIN_S, |v| v as i64),
            stop_diameter_m: num(CASE_STOP_DIAMETER_M).unwrap_or(STOP_DIAMETER_M),
            stop_min_s: num(CASE_STOP_MIN_S).map_or(STOP_MIN_S, |v| v as i64),
            signal_gap_s: num(CASE_SIGNAL_GAP_S).map(|v| v as i64),
        }
    }
}

/// A detection before it is written.
struct Found {
    detector: &'static str,
    params: Value,
    refs: Vec<Uuid>,
    from_ms: i64,
    to_ms: Option<i64>,
    payload: Value,
}

fn kind_of(detector: &str) -> &str {
    detector.split('@').next().unwrap_or(detector)
}

/// Identity of a detection across recomputations: detector, sorted members,
/// start rounded to the minute.
// shortcut: a start sitting on a minute boundary may round differently after
// a recomputation on another window; widen the rounding if duplicates show.
fn fingerprint(detector: &str, refs: &[Uuid], from_ms: i64) -> String {
    let mut ids: Vec<String> = refs.iter().map(Uuid::to_string).collect();
    ids.sort();
    let minute = from_ms.div_euclid(60_000);
    format!(
        "{:x}",
        Sha256::digest(format!("{}|{}|{minute}", kind_of(detector), ids.join(",")))
    )
}

fn instant(ms: i64) -> DateTimeWithTimeZone {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .into()
}

/// Runs the detectors over a window (default: the span of the events, then
/// the last 24 h) and writes the new proposals.
pub async fn detect(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    user_id: Option<i64>,
    case_id: Uuid,
    q: &DetectQuery,
) -> Result<CaseDetected> {
    let m = membership(db, org_id, case_id).await?;
    let events: Vec<_> = m.events.iter().map(|(_, o)| o.clone()).collect();
    let span = events_span(&events);
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let to = parse_instant("to", q.to.as_deref())?
        .or(span.map(|(_, t)| t))
        .unwrap_or(now);
    let from = parse_instant("from", q.from.as_deref())?
        .or(span.map(|(f, _)| f))
        .unwrap_or(to - chrono::Duration::hours(DEFAULT_SPAN_HOURS));
    if from >= to || to - from > chrono::Duration::days(MAX_SPAN_DAYS) {
        return Err(invalid("from", FIELD_INVALID));
    }
    let th = Thresholds::of(&m.case);
    let window = (from.timestamp_millis(), to.timestamp_millis());
    let reader = super::reads::Reader {
        user_id,
        purpose: "detect",
    };
    let loaded = load(db, client, org_id, case_id, window, None, reader).await?;
    let gap = loaded.gap_ms;
    let ids: Vec<Option<Uuid>> = loaded
        .tracks
        .iter()
        .map(|(o, _, _)| Uuid::parse_str(&o.id).ok())
        .collect();

    let mut found = Vec::new();
    for ((_, _, segs), id) in loaded.tracks.iter().zip(&ids) {
        let Some(id) = *id else { continue };
        for seg in segs {
            for s in stops(seg, th.stop_diameter_m, th.stop_min_s * 1_000) {
                found.push(Found {
                    detector: STOP,
                    params: json!({ "diameter_m": th.stop_diameter_m, "min_s": th.stop_min_s }),
                    refs: vec![id],
                    from_ms: s.from_ms,
                    to_ms: Some(s.to_ms),
                    payload: json!({ "at": [s.at.0, s.at.1] }),
                });
            }
        }
        let until = window.1.min(now.timestamp_millis());
        for l in signal_losses(segs, until, gap) {
            found.push(Found {
                detector: SIGNAL_LOSS,
                params: json!({ "gap_s": gap / 1_000 }),
                refs: vec![id],
                from_ms: l.from_ms,
                to_ms: l.to_ms,
                payload: json!({ "at": [l.at.0, l.at.1] }),
            });
        }
    }
    // Areas: every member holding a `geo_area` value (a place, a POI zone).
    let members = links::refs(db, org_id, m.member_ids()).await?;
    let zones: Vec<(Uuid, GeoArea)> = zones_of(&members)
        .into_iter()
        .map(|(o, a, _)| (o.id, a))
        .collect();
    for ((_, _, segs), id) in loaded.tracks.iter().zip(&ids) {
        let Some(id) = *id else { continue };
        for (zone, area) in zones.iter().filter(|(z, _)| *z != id) {
            let at = area.center();
            for v in zone_visits(segs, |lon, lat| area.contains(lon, lat)) {
                found.push(Found {
                    detector: ZONE,
                    params: json!({}),
                    refs: vec![id, *zone],
                    from_ms: v.from_ms,
                    to_ms: Some(v.to_ms),
                    payload: json!({
                        "at": [at.0, at.1], "zone": zone, "entered": v.entered, "left": v.left,
                    }),
                });
            }
        }
    }
    for i in 0..loaded.tracks.len() {
        for j in i + 1..loaded.tracks.len() {
            let (Some(a), Some(b)) = (ids[i], ids[j]) else {
                continue;
            };
            let (sa, sb) = (&loaded.tracks[i].2, &loaded.tracks[j].2);
            for c in co_presence(sa, sb, th.co_distance_m, th.co_min_s * 1_000) {
                found.push(Found {
                    detector: CO_PRESENCE,
                    params: json!({ "distance_m": th.co_distance_m, "min_s": th.co_min_s }),
                    refs: vec![a, b],
                    from_ms: c.from_ms,
                    to_ms: Some(c.to_ms),
                    payload: json!({
                        "at": [c.at.0, c.at.1],
                        "min_distance_m": (c.min_distance_m * 10.0).round() / 10.0,
                    }),
                });
            }
        }
    }

    let mut inserted = 0;
    for f in found {
        let mut refs = f.refs.clone();
        refs.sort();
        let r = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO case_proposals \
                   (id, org_id, case_id, kind, detector, params, fingerprint, t_start, t_end, refs, payload) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
                 ON CONFLICT (case_id, fingerprint) DO NOTHING",
                [
                    Uuid::new_v4().into(),
                    org_id.into(),
                    case_id.into(),
                    kind_of(f.detector).into(),
                    f.detector.into(),
                    f.params.into(),
                    fingerprint(f.detector, &refs, f.from_ms).into(),
                    instant(f.from_ms).into(),
                    f.to_ms.map(instant).into(),
                    refs.into(),
                    f.payload.into(),
                ],
            ))
            .await?;
        inserted += r.rows_affected() as usize;
    }
    Ok(CaseDetected {
        from: ts(&from),
        to: ts(&to),
        available: loaded.available,
        truncated: loaded.truncated,
        inserted,
    })
}

const COLUMNS: &str = "id, kind, detector, params, t_start, t_end, refs, payload, status, \
                       reject_reason, event_id";

/// One row, members not yet resolved.
fn row_of(r: &QueryResult) -> Option<(CaseProposal, Vec<Uuid>)> {
    let refs: Vec<Uuid> = r.try_get("", "refs").ok()?;
    let t_end: Option<DateTimeWithTimeZone> = r.try_get("", "t_end").ok()?;
    let event_id: Option<Uuid> = r.try_get("", "event_id").ok()?;
    Some((
        CaseProposal {
            id: r.try_get::<Uuid>("", "id").ok()?.to_string(),
            kind: r.try_get("", "kind").ok()?,
            detector: r.try_get("", "detector").ok()?,
            params: r.try_get("", "params").ok()?,
            t_start: ts(&r.try_get("", "t_start").ok()?),
            t_end: t_end.as_ref().map(ts),
            refs: Vec::new(),
            payload: r.try_get("", "payload").ok()?,
            status: r.try_get("", "status").ok()?,
            reject_reason: r.try_get("", "reject_reason").ok()?,
            event_id: event_id.map(|u| u.to_string()),
        },
        refs,
    ))
}

/// The proposals of a case, by start; members gone from the org are left out.
pub async fn proposals(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<CaseProposal>> {
    membership(db, org_id, case_id).await?;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "SELECT {COLUMNS} FROM case_proposals WHERE org_id = $1 AND case_id = $2 \
                 ORDER BY t_start, id"
            ),
            [org_id.into(), case_id.into()],
        ))
        .await?;
    let rows: Vec<(CaseProposal, Vec<Uuid>)> = rows.iter().filter_map(row_of).collect();
    let objs = links::refs(db, org_id, rows.iter().flat_map(|(_, r)| r.iter().copied())).await?;
    Ok(rows
        .into_iter()
        .map(|(mut p, refs)| {
            p.refs = refs
                .iter()
                .filter_map(|id| objs.get(id).map(links::object_ref))
                .collect::<Vec<ObjectRef>>();
            p
        })
        .collect())
}

/// The proposal row, locked for the review.
async fn locked<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    case_id: Uuid,
    id: Uuid,
) -> Result<(CaseProposal, Vec<Uuid>)> {
    let r = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "SELECT {COLUMNS} FROM case_proposals \
                 WHERE id = $1 AND org_id = $2 AND case_id = $3 FOR UPDATE"
            ),
            [id.into(), org_id.into(), case_id.into()],
        ))
        .await?
        .ok_or(OntologyError::NotFound)?;
    row_of(&r).ok_or(OntologyError::NotFound)
}

/// `fuzzy_time` value of a detection span (bounds in UTC with `Z`).
fn occurred(p: &CaseProposal) -> Value {
    let z = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| {
                t.with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            })
            .unwrap_or_default()
    };
    let from = z(&p.t_start);
    match p.t_end.as_deref().map(z) {
        Some(to) if to != from => json!({ "from": from, "to": to, "precision": "range" }),
        _ => json!({ "from": from, "to": from, "precision": "instant" }),
    }
}

/// Retains a proposal: its event, the members attached as actors, the place
/// and the detector cited in the note, provenance `detector:<key>@<version>`
/// (D184). Idempotent.
pub async fn retain(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    id: Uuid,
    input: &ProposalRetain,
) -> Result<CaseWorkspace> {
    let txn = db.begin().await?;
    let (p, refs) = locked(&txn, org_id, case_id, id).await?;
    match p.status.as_str() {
        "retained" => {
            txn.commit().await?;
            return workspace(db, org_id, case_id).await;
        }
        "rejected" => return Err(OntologyError::CaseProposalStale),
        _ => {}
    }
    let mut props = Map::new();
    props.insert(
        EVENT_NOTE.into(),
        format!("{} {}", p.detector, p.params).into(),
    );
    if let Some([lon, lat]) = p.payload.get("at").and_then(|a| {
        let a = a.as_array()?;
        Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
    }) {
        props.insert(EVENT_LOCATION.into(), json!({ "lon": lon, "lat": lat }));
    }
    // Tracked members act; the area of a visit is context.
    let types = links::refs(&txn, org_id, refs.iter().copied()).await?;
    let event = CaseEventInput {
        title: input.title.clone(),
        occurred: occurred(&p),
        origin: Some("detection".into()),
        attach: refs
            .iter()
            .map(|r| {
                let place = types
                    .get(r)
                    .is_some_and(|o| o.type_key == KIND_MAP_PIN || o.type_key == T_PLACE);
                CaseAttachInput {
                    object_id: r.to_string(),
                    role: Some(if place { "context" } else { "actor" }.into()),
                    ..Default::default()
                }
            })
            .collect(),
    };
    let by = Actor {
        source_ref: format!("detector:{}", p.detector),
        ..actor.clone()
    };
    let event_id = create_event_in(&txn, org_id, &by, case_id, &event, props).await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE case_proposals SET status = 'retained', event_id = $1, reviewed_by = $2, \
         reviewed_at = now() WHERE id = $3",
        [event_id.into(), actor.user_id.into(), id.into()],
    ))
    .await?;
    txn.commit().await?;
    workspace(db, org_id, case_id).await
}

/// Rejects a proposal, kept with its reason so it is never proposed again.
/// Idempotent; a retained proposal is not rejected (remove its event instead).
pub async fn reject(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    id: Uuid,
    input: &ProposalReject,
) -> Result<Vec<CaseProposal>> {
    let txn = db.begin().await?;
    let (p, _) = locked(&txn, org_id, case_id, id).await?;
    match p.status.as_str() {
        "retained" => return Err(OntologyError::CaseProposalStale),
        "rejected" => {}
        _ => {
            let reason: Option<String> = input
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(|r| r.chars().take(REASON_MAX).collect());
            txn.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE case_proposals SET status = 'rejected', reject_reason = $1, \
                 reviewed_by = $2, reviewed_at = now() WHERE id = $3",
                [reason.into(), actor.user_id.into(), id.into()],
            ))
            .await?;
        }
    }
    txn.commit().await?;
    proposals(db, org_id, case_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_member_order_and_seconds() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let f = fingerprint(CO_PRESENCE, &[a, b], 120_000);
        assert_eq!(f, fingerprint("co_presence@2", &[b, a], 179_999));
        assert_ne!(f, fingerprint(CO_PRESENCE, &[a, b], 180_000));
        assert_ne!(f, fingerprint(STOP, &[a, b], 120_000));
    }
}
