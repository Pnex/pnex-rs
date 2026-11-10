//! Detections of a case (cases-workspace.md K30): the detectors of
//! `pnex_core::track::detect` run on the members' tracks over a window and
//! write proposals of kind `case_event` in the shared inbox (proposals.md
//! P3). A key (detector, members, start to the minute) makes a
//! recomputation insert only what is new: a rejected proposal is never
//! proposed again. Retaining is the shared acceptance, which writes the
//! event; retaining twice returns the same event. Nothing is ever added
//! to the case without a human gesture. Thresholds are case properties set
//! in the case frame, the defaults below when unset.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::geo_area::GeoArea;
use pnex_core::ontology::api::{
    CaseAttachInput, CaseDetected, CaseEventInput, CaseProposal, CaseWorkspace, DetectQuery,
    ObjectRef, ProposalReject, ProposalRetain,
};
use pnex_core::ontology::proposal::{DecisionKind, ProposalDecision, ProposalInput, ProposalKind};
use pnex_core::ontology::{
    CASE_CAUSAL_MIN_LIFT, CASE_COPRESENCE_DISTANCE_M, CASE_COPRESENCE_MIN_S, CASE_SIGNAL_GAP_S,
    CASE_STOP_DIAMETER_M, CASE_STOP_MIN_S, EVENT_LOCATION, EVENT_NOTE, KIND_EVENT,
};
use pnex_core::resources::KIND_MAP_PIN;
use pnex_core::track::detect::{
    co_presence, signal_losses, stops, zone_visits, DEFAULT_CO_DISTANCE_M as CO_DISTANCE_M,
    DEFAULT_CO_MIN_S as CO_MIN_S, DEFAULT_STOP_DIAMETER_M as STOP_DIAMETER_M,
    DEFAULT_STOP_MIN_S as STOP_MIN_S,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::{invalid, links, parse_instant, proposals, ts, Actor, OntologyError, Result};
use super::signals::events_span;
use super::tracks::{load, MAX_SPAN_DAYS};
use super::{create_event_in, membership, workspace, zones_of, T_PLACE};
use crate::services::openobserve::Client;

const CO_PRESENCE: &str = "co_presence@1";
const STOP: &str = "stop@1";
const SIGNAL_LOSS: &str = "signal_loss@1";
const ZONE: &str = "zone@1";
pub(crate) const CAUSAL_CHAIN: &str = "causal_chain@1";
const DEFAULT_SPAN_HOURS: i64 = 24;

/// Detector thresholds of a case: its properties, else the defaults.
pub(super) struct Thresholds {
    pub co_distance_m: f64,
    pub co_min_s: i64,
    pub stop_diameter_m: f64,
    pub stop_min_s: i64,
    /// `None` = the track reader's default.
    pub signal_gap_s: Option<i64>,
    pub causal_min_lift: f64,
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
            causal_min_lift: num(CASE_CAUSAL_MIN_LIFT).unwrap_or(pnex_core::rca::DEFAULT_MIN_LIFT),
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
    /// Identity across recomputations when not the default fingerprint
    /// (a chain: detector, root signal, effect signal, D909).
    key: Option<String>,
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
    hex::encode(Sha256::digest(format!(
        "{}|{}|{minute}",
        kind_of(detector),
        ids.join(",")
    )))
}

fn instant(ms: i64) -> DateTimeWithTimeZone {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .into()
}

/// Whether the org turned the causal chains of its cases on (pack setting,
/// off by default until measured, D909).
async fn causal_enabled(db: &DatabaseConnection, org_id: i64) -> Result<bool> {
    Ok(crate::services::ontology::packs::setting_value(
        db,
        org_id,
        "maintenance.causal_chains_enabled",
    )
    .await?
    .and_then(|v| v.as_bool())
    .unwrap_or(false))
}

/// Runs the detectors over a window (default: the span of the events, then
/// the last 24 h) and writes the new proposals.
pub async fn detect(
    db: &DatabaseConnection,
    client: Option<&Client>,
    store: Option<&crate::services::artifact_store::S3Store>,
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
                    key: None,
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
                key: None,
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
                    key: None,
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
                    key: None,
                });
            }
        }
    }

    // Causal chains (cases-workspace.md D909): leads over the members'
    // typed signals, only when the org turned them on and they beat the
    // placebo by the case's lift.
    if causal_enabled(db, org_id).await? {
        let pairs = crate::services::ontology::packs::causal_pairs(db, org_id).await?;
        if !pairs.is_empty() {
            let longest = pairs.iter().map(|p| p.max_gap).max().unwrap_or(0);
            let shift = pnex_core::rca::PLACEBO_SHIFTS
                .iter()
                .max()
                .copied()
                .unwrap_or(0);
            let back = chrono::Duration::seconds(longest + shift);
            let (f, t) = (
                from.with_timezone(&chrono::Utc),
                to.with_timezone(&chrono::Utc),
            );
            let signals = super::causal::signals(db, client, org_id, case_id, f - back, t).await?;
            let effects: Vec<_> = pnex_core::rca::effects_of(&signals, &pairs)
                .into_iter()
                .filter(|s| s.at >= f.timestamp() && s.at <= t.timestamp())
                .collect();
            let topology = crate::services::rca::topology(db, org_id, f - back, t).await?;
            let mut ranks: std::collections::HashMap<String, u32> = Default::default();
            for c in pnex_core::rca::chains(&effects, &signals, &topology, &pairs) {
                // Rank among the chains of the same effect (D911).
                let rank = ranks.entry(c.effect().id.clone()).or_insert(0);
                *rank += 1;
                let rank = *rank;
                if c.lift < th.causal_min_lift {
                    continue;
                }
                let mut refs: Vec<Uuid> = Vec::new();
                for h in &c.hops {
                    for on in [&h.cause.on, &h.effect.on] {
                        if let Ok(id) = Uuid::parse_str(on) {
                            if !refs.contains(&id) {
                                refs.push(id);
                            }
                        }
                    }
                }
                let (root, effect) = (c.root().clone(), c.effect().clone());
                found.push(Found {
                    detector: CAUSAL_CHAIN,
                    params: json!({ "min_lift": th.causal_min_lift }),
                    key: Some(hex::encode(Sha256::digest(format!(
                        "{}|{}|{}",
                        kind_of(CAUSAL_CHAIN),
                        root.id,
                        effect.id
                    )))),
                    refs,
                    from_ms: root.at * 1_000,
                    to_ms: Some(effect.at * 1_000),
                    payload: json!({
                        "hops": c.hops.iter().map(|h| json!({
                            "cause": h.cause, "effect": h.effect, "gap_s": h.gap,
                            "distance": h.distance, "score": h.score,
                            "rule": format!("{} → {}", h.cause.kind.as_str(), h.effect.kind.as_str()),
                        })).collect::<Vec<_>>(),
                        "confidence": c.confidence,
                        "placebo": c.placebo,
                        "lift": c.lift,
                        "rank": rank,
                        "root": root.on,
                        "effect": effect.on,
                    }),
                });
            }
        }
    }

    // Early bets on the prediction markets of the case (markets-research.md
    // D924): a proposal with the figures, never an accusation.
    let early = crate::services::markets::prediction::case_detections(
        db,
        org_id,
        &members,
        from.timestamp(),
        to.timestamp(),
    )
    .await?;
    for (market, b) in early {
        let detector = crate::services::markets::prediction::EARLY_BET;
        found.push(Found {
            detector,
            params: json!({}),
            key: Some(hex::encode(Sha256::digest(format!(
                "{}|{}|{market}",
                kind_of(detector),
                b.wallet
            )))),
            refs: vec![market],
            from_ms: b.first_bet * 1_000,
            to_ms: Some(b.trigger_at * 1_000),
            payload: serde_json::to_value(&b).unwrap_or_default(),
        });
    }

    // Public trading accounts entering before moves or around releases of an
    // instrument of the case (markets-research.md D930): figures only.
    let timing = crate::services::markets::accounts::case_detections(
        db,
        store,
        org_id,
        &members,
        from.timestamp(),
        to.timestamp(),
    )
    .await?;
    for (instrument, t) in timing {
        let detector = crate::services::markets::accounts::ENTRY_TIMING;
        found.push(Found {
            detector,
            params: json!({}),
            key: Some(hex::encode(Sha256::digest(format!(
                "{}|{}|{instrument}|{}|{}",
                kind_of(detector),
                t.account_ref,
                t.from,
                t.to
            )))),
            refs: vec![instrument],
            from_ms: t.from * 1_000,
            to_ms: Some(t.to * 1_000),
            payload: serde_json::to_value(&t).unwrap_or_default(),
        });
    }

    let mut inserted = 0;
    for f in found {
        let mut refs = f.refs.clone();
        refs.sort();
        let names: Vec<String> = f
            .refs
            .iter()
            .map(|r| {
                members
                    .get(r)
                    .map_or_else(|| r.to_string(), |o| o.title.clone())
            })
            .collect();
        let input = ProposalInput {
            kind: Some(ProposalKind::CaseEvent),
            type_key: KIND_EVENT.into(),
            target_id: Some(case_id.to_string()),
            title: Some(format!(
                "{}: {}",
                kind_of(f.detector),
                names.join(if f.detector == CAUSAL_CHAIN {
                    " → "
                } else {
                    " · "
                })
            )),
            values: json!({
                "key": f.key.clone().unwrap_or_else(|| fingerprint(f.detector, &refs, f.from_ms)),
                "detector": f.detector,
                "params": f.params,
                "members": refs.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                "from_ms": f.from_ms,
                "to_ms": f.to_ms,
                "payload": f.payload,
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
            origin: format!("detector:{}", f.detector),
            ..Default::default()
        };
        if proposals::propose(db, org_id, &input).await?.is_some() {
            inserted += 1;
        }
    }
    Ok(CaseDetected {
        from: ts(&from),
        to: ts(&to),
        available: loaded.available,
        truncated: loaded.truncated,
        inserted,
    })
}

/// A detection as the case shows it, members not yet resolved.
fn row_of(r: &QueryResult) -> Option<(CaseProposal, Vec<Uuid>)> {
    let vals: Value = r.try_get("", "vals").ok()?;
    let ms = |k: &str| vals.get(k).and_then(Value::as_i64);
    let detector = vals.get("detector")?.as_str()?.to_string();
    let refs: Vec<Uuid> = vals
        .get("members")?
        .as_array()?
        .iter()
        .filter_map(|m| Uuid::parse_str(m.as_str()?).ok())
        .collect();
    let status: String = r.try_get("", "status").ok()?;
    Some((
        CaseProposal {
            id: r.try_get::<Uuid>("", "id").ok()?.to_string(),
            kind: kind_of(&detector).to_string(),
            params: vals.get("params").cloned().unwrap_or_default(),
            t_start: ts(&instant(ms("from_ms")?)),
            t_end: ms("to_ms").map(|t| ts(&instant(t))),
            refs: Vec::new(),
            payload: vals.get("payload").cloned().unwrap_or_default(),
            // The case keeps its own words for an accepted detection.
            status: if status == "accepted" {
                "retained".into()
            } else {
                status
            },
            reject_reason: r.try_get("", "reason").ok()?,
            event_id: r.try_get("", "applied").ok()?,
            detector,
        },
        refs,
    ))
}

/// The detections of a case, by start; members gone from the org are left out.
pub async fn proposals(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<CaseProposal>> {
    membership(db, org_id, case_id).await?;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id, vals, status, reason, applied FROM proposals \
             WHERE org_id = $1 AND kind = 'case_event' AND target_id = $2 \
               AND status <> 'superseded' \
             ORDER BY (vals ->> 'from_ms')::bigint, id",
            [org_id.into(), case_id.to_string().into()],
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

/// `fuzzy_time` value of a detection span (bounds in UTC with `Z`).
fn occurred(from_ms: i64, to_ms: Option<i64>) -> Value {
    let z = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms)
            .unwrap_or_default()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    let from = z(from_ms);
    match to_ms.map(z) {
        Some(to) if to != from => json!({ "from": from, "to": to, "precision": "range" }),
        _ => json!({ "from": from, "to": from, "precision": "instant" }),
    }
}

/// The event of an accepted detection (proposals.md P3): the members
/// attached as actors, the place and the detector cited in the note.
pub(crate) async fn event_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    vals: &Map<String, Value>,
    title: &str,
) -> Result<Uuid> {
    let refs: Vec<Uuid> = vals
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| Uuid::parse_str(m.as_str()?).ok())
        .collect();
    let from_ms = vals
        .get("from_ms")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid("values.from_ms", FIELD_INVALID))?;
    let detector = vals
        .get("detector")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if detector == CAUSAL_CHAIN {
        return causal_event(db, org_id, actor, case_id, vals, from_ms, title).await;
    }
    if detector == crate::services::markets::prediction::EARLY_BET {
        let market = *refs
            .first()
            .ok_or_else(|| invalid("values.members", FIELD_INVALID))?;
        let payload = vals.get("payload").cloned().unwrap_or_default();
        let (wallet, note) = crate::services::markets::prediction::retain_early_bet(
            db, org_id, actor, market, &payload,
        )
        .await?;
        let mut props = Map::new();
        props.insert(EVENT_NOTE.into(), note.into());
        let event = CaseEventInput {
            title: title.to_string(),
            occurred: occurred(from_ms, vals.get("to_ms").and_then(Value::as_i64)),
            origin: Some("detection".into()),
            attach: [(wallet, "context"), (market, "context")]
                .iter()
                .map(|(id, role)| CaseAttachInput {
                    object_id: id.to_string(),
                    role: Some((*role).into()),
                    ..Default::default()
                })
                .collect(),
        };
        return create_event_in(db, org_id, actor, case_id, &event, props).await;
    }
    if detector == crate::services::markets::accounts::ENTRY_TIMING {
        let instrument = *refs
            .first()
            .ok_or_else(|| invalid("values.members", FIELD_INVALID))?;
        let payload = vals.get("payload").cloned().unwrap_or_default();
        let (account, note) =
            crate::services::markets::accounts::retain_entry_timing(db, org_id, actor, &payload)
                .await?;
        let mut props = Map::new();
        props.insert(EVENT_NOTE.into(), note.into());
        let event = CaseEventInput {
            title: title.to_string(),
            occurred: occurred(from_ms, vals.get("to_ms").and_then(Value::as_i64)),
            origin: Some("detection".into()),
            attach: [(account, "context"), (instrument, "context")]
                .iter()
                .map(|(id, role)| CaseAttachInput {
                    object_id: id.to_string(),
                    role: Some((*role).into()),
                    ..Default::default()
                })
                .collect(),
        };
        return create_event_in(db, org_id, actor, case_id, &event, props).await;
    }
    let params = vals.get("params").cloned().unwrap_or_default();
    let mut props = Map::new();
    props.insert(EVENT_NOTE.into(), format!("{detector} {params}").into());
    if let Some([lon, lat]) = vals.get("payload").and_then(|p| p.get("at")).and_then(|a| {
        let a = a.as_array()?;
        Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
    }) {
        props.insert(EVENT_LOCATION.into(), json!({ "lon": lon, "lat": lat }));
    }
    // Tracked members act; the area of a visit is context.
    let types = links::refs(db, org_id, refs.iter().copied()).await?;
    let event = CaseEventInput {
        title: title.to_string(),
        occurred: occurred(from_ms, vals.get("to_ms").and_then(Value::as_i64)),
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
    create_event_in(db, org_id, actor, case_id, &event, props).await
}

/// The event of a retained causal chain (D909): the root cause as context,
/// the effect as evidence, the flow that alerted as source, a note citing
/// every hop; with the Investigation pack, a hypothesis whose hops are the
/// claims for it.
async fn causal_event<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    vals: &Map<String, Value>,
    from_ms: i64,
    title: &str,
) -> Result<Uuid> {
    let payload = vals.get("payload").cloned().unwrap_or_default();
    let hops = payload
        .get("hops")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let text = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let root = text(&payload, "root");
    let effect = text(&payload, "effect");
    // The flow that alerted: `notification:<ts>:<flow object>`.
    let flow = hops
        .last()
        .and_then(|h| h.get("effect"))
        .map(|e| text(e, "source_ref"))
        .and_then(|r| r.strip_prefix("notification:").map(str::to_string))
        .and_then(|r| r.split_once(':').map(|(_, f)| f.to_string()));
    let mut lines: Vec<String> = hops
        .iter()
        .map(|h| {
            format!(
                "{}: {} before, {} link(s) apart, score {:.3}",
                text(h, "rule"),
                pnex_core::rca::gap_text(h.get("gap_s").and_then(Value::as_i64).unwrap_or(0)),
                h.get("distance").and_then(Value::as_u64).unwrap_or(0),
                h.get("score").and_then(Value::as_f64).unwrap_or(0.0),
            )
        })
        .collect();
    let num = |k: &str| payload.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    lines.push(format!(
        "{CAUSAL_CHAIN}: confidence {:.3}, lift {:.1} against the placebo. A lead, not a fact.",
        num("confidence"),
        num("lift")
    ));
    let mut attach: Vec<CaseAttachInput> = Vec::new();
    let mut add = |id: &str, role: &str| {
        if !id.is_empty() && attach.iter().all(|a| a.object_id != id) {
            attach.push(CaseAttachInput {
                object_id: id.to_string(),
                role: Some(role.into()),
                ..Default::default()
            });
        }
    };
    add(&root, "context");
    add(&effect, "evidence");
    if let Some(f) = &flow {
        add(f, "source");
    }
    for h in &hops {
        for end in ["cause", "effect"] {
            add(&text(h.get(end).unwrap_or(&Value::Null), "on"), "context");
        }
    }
    // Investigation pack installed: a hypothesis the hops support.
    let s = super::super::schema(db, org_id).await?;
    if s.object_type("hypothesis").is_some() && s.object_type("claim").is_some() {
        let hypothesis = super::super::objects::create(
            db,
            org_id,
            actor,
            &pnex_core::ontology::api::ObjectInput {
                type_key: "hypothesis".into(),
                title: title.to_string(),
                properties: json!({ "statement": title, "status": "open" })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                expected_version: None,
            },
        )
        .await?;
        for line in lines.iter().take(hops.len()) {
            let claim = super::super::objects::create(
                db,
                org_id,
                actor,
                &pnex_core::ontology::api::ObjectInput {
                    type_key: "claim".into(),
                    title: line.chars().take(200).collect(),
                    properties: json!({ "statement": line, "status": "unverified" })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                    expected_version: None,
                },
            )
            .await?;
            links::create(
                db,
                org_id,
                actor,
                &pnex_core::ontology::api::LinkInput {
                    link_type: "supports".into(),
                    source_id: claim.id.clone(),
                    target_id: hypothesis.id.clone(),
                    ..Default::default()
                },
            )
            .await?;
        }
        add(&hypothesis.id, "hypothesis");
    }
    let to_ms = vals.get("to_ms").and_then(Value::as_i64);
    let mut props = Map::new();
    props.insert(EVENT_NOTE.into(), lines.join("\n").into());
    let event = CaseEventInput {
        title: title.to_string(),
        occurred: occurred(from_ms, to_ms),
        origin: Some("detection".into()),
        attach,
    };
    create_event_in(db, org_id, actor, case_id, &event, props).await
}

/// A detection of this case, else not found (another case's id).
async fn of_case(db: &DatabaseConnection, org_id: i64, case_id: Uuid, id: Uuid) -> Result<()> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 AS one FROM proposals \
         WHERE id = $1 AND org_id = $2 AND kind = 'case_event' AND target_id = $3",
        [id.into(), org_id.into(), case_id.to_string().into()],
    ))
    .await?
    .map(|_| ())
    .ok_or(OntologyError::NotFound)
}

/// The shared decision (D373), in the case's words: deciding a detection
/// the other way is `case-proposal-stale`.
async fn decide_in_case(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    id: Uuid,
    d: &ProposalDecision,
) -> Result<()> {
    of_case(db, org_id, case_id, id).await?;
    match proposals::decide(db, org_id, actor, id, d).await {
        Ok(_) => Ok(()),
        Err(OntologyError::ProposalDecided) => Err(OntologyError::CaseProposalStale),
        Err(e) => Err(e),
    }
}

/// Retains a detection: the proposal is accepted under the given title and
/// its event written, provenance `proposal:<id>` (D184). Idempotent.
pub async fn retain(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    id: Uuid,
    input: &ProposalRetain,
) -> Result<CaseWorkspace> {
    let d = ProposalDecision {
        decision: DecisionKind::Accept,
        reason: None,
        values: Some(
            json!({ "title": input.title })
                .as_object()
                .cloned()
                .unwrap_or_default(),
        ),
    };
    decide_in_case(db, org_id, actor, case_id, id, &d).await?;
    workspace(db, org_id, case_id).await
}

/// Rejects a detection, kept with its reason so it is never proposed again.
/// Idempotent; a retained one is not rejected (remove its event instead).
pub async fn reject(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    id: Uuid,
    input: &ProposalReject,
) -> Result<Vec<CaseProposal>> {
    let d = ProposalDecision {
        decision: DecisionKind::Reject,
        reason: input.reason.clone(),
        values: None,
    };
    decide_in_case(db, org_id, actor, case_id, id, &d).await?;
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
