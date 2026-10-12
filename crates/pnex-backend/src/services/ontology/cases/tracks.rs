//! Tracks of a case (cases-workspace.md K25–K27): the members that have a
//! position source, their positions over a window read once from O2
//! `positions`, normalised, split at signal gaps and generalised for the
//! zoom level, merged with the tracks imported onto members (Postgres, K26).
//! The replay interpolates on the client without further requests.
//!
//! Position sources of a member: itself when it is a device, or the devices
//! linked to it by `tracks`, each bounded to the link's validity (D181
//! school: the object outlives its trackers).

use std::collections::HashMap;

use pnex_core::err_codes::{FIELD_INVALID, FIELD_REQUIRED};
use pnex_core::ontology::api::{CaseTrack, CaseTracks, CaseTracksQuery, ObjectRef};
use pnex_core::ontology::{KIND_EVENT, REL_TRACKS};
use pnex_core::resources::KIND_DEVICE;
use pnex_core::track::{normalize, split_gaps, tdtr, TrackPoint};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::super::{invalid, links, parse_instant, ts, Result};
use super::detect::Thresholds;
use super::reads::{self, Reader};
use super::T_PERSON;
use super::{imports, membership};
use crate::models::_entities::device_registries;
use crate::services::openobserve::Client;
use crate::services::pois::position_log;

/// Widest window served in one request.
pub(super) const MAX_SPAN_DAYS: i64 = pnex_core::track::MAX_WINDOW_DAYS;
/// Raw positions read at most from O2 per request.
const RAW_CAP: i64 = 100_000;
/// Points returned at most, all tracks together.
const OUT_CAP: usize = 20_000;
/// Members examined at most (each costs one links query).
const MEMBER_CAP: usize = 500;

/// Generalisation tolerance for a timeline pixel of `resolution_ms`.
// shortcut: fixed 2 m per second of pixel (walking to driving speeds); derive
// it from the map zoom too if the trails look too coarse when zoomed in.
fn tolerance_m(resolution_ms: Option<i64>) -> f64 {
    let s = resolution_ms.unwrap_or(1_000).max(0) as f64 / 1_000.0;
    (s * 2.0).clamp(1.0, 500.0)
}

/// A device giving the position of `object` between `from_ms` and `to_ms`.
struct Source {
    object: ObjectRef,
    device_obj: Uuid,
    from_ms: i64,
    to_ms: i64,
}

fn ms_of(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// Clean tracks of the members over a window: per member, its raw point
/// count and its gap-free segments.
pub(super) struct Loaded {
    pub available: bool,
    pub truncated: bool,
    pub tracks: Vec<(ObjectRef, usize, Vec<Vec<TrackPoint>>)>,
    /// Effective signal gap threshold, in ms.
    pub gap_ms: i64,
}

/// Reads the members' positions over `[from_ms, to_ms]` once, normalised and
/// split at gaps longer than `signal_gap_s` (else the case threshold).
/// `available` is false when the history cannot be read (no O2): nothing is
/// invented.
pub(super) async fn load(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    case_id: Uuid,
    (from_ms, to_ms): (i64, i64),
    signal_gap_s: Option<i64>,
    reader: Reader,
) -> Result<Loaded> {
    let m = membership(db, org_id, case_id).await?;
    let gap_ms = gap_ms(signal_gap_s.or(Thresholds::of(&m.case).signal_gap_s));
    let mut ids = m.member_ids();
    ids.truncate(MEMBER_CAP);
    let objs = links::refs(db, org_id, ids.iter().copied()).await?;

    let mut sources: Vec<Source> = Vec::new();
    for id in &ids {
        let Some(o) = objs.get(id) else { continue };
        if o.type_key == KIND_DEVICE {
            sources.push(Source {
                object: links::object_ref(o),
                device_obj: *id,
                from_ms,
                to_ms,
            });
            continue;
        }
        if o.type_key == KIND_EVENT {
            continue;
        }
        // Trackers of this object over its history, clipped to the window.
        for l in links::for_object(db, org_id, *id, None, true).await? {
            if l.link_type != REL_TRACKS || l.target.id != o.id.to_string() {
                continue;
            }
            let (Ok(dev), Some(lf)) = (Uuid::parse_str(&l.source.id), ms_of(&l.valid_from)) else {
                continue;
            };
            let lt = l.valid_to.as_deref().and_then(ms_of).unwrap_or(i64::MAX);
            let (a, b) = (lf.max(from_ms), lt.min(to_ms));
            if a < b {
                sources.push(Source {
                    object: links::object_ref(o),
                    device_obj: dev,
                    from_ms: a,
                    to_ms: b,
                });
            }
        }
    }

    // Device objects → telemetry device ids (registry rows of this org, R1).
    let dev_objs = links::refs(db, org_id, sources.iter().map(|s| s.device_obj)).await?;
    let registry_ids: Vec<i64> = dev_objs
        .values()
        .filter_map(|o| o.native_id.parse().ok())
        .collect();
    let device_of: HashMap<i64, String> = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(device_registries::Column::Id.is_in(registry_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.device_id))
        .collect();
    let telemetry_id = |dev: &Uuid| -> Option<String> {
        let native: i64 = dev_objs.get(dev)?.native_id.parse().ok()?;
        device_of.get(&native).cloned()
    };
    let wanted: Vec<String> = {
        let mut v: Vec<String> = sources
            .iter()
            .filter_map(|s| telemetry_id(&s.device_obj))
            .collect();
        v.sort();
        v.dedup();
        v
    };

    let read = position_log::read(
        db,
        client,
        org_id,
        &wanted,
        from_ms * 1_000,
        to_ms * 1_000,
        RAW_CAP,
    )
    .await
    .map_err(|e| {
        tracing::warn!(case = %case_id, "position history read failed: {e}");
        e
    })
    .ok()
    .flatten();
    let o2_read = read.is_some();
    let rows = read.unwrap_or_default();
    let mut truncated = rows.len() as i64 >= RAW_CAP;
    let mut by_device: HashMap<String, Vec<TrackPoint>> = HashMap::new();
    for (dev, p) in rows {
        by_device.entry(dev).or_default().push(p);
    }

    // One track per object, merged from its sources in member order.
    let mut order: Vec<String> = Vec::new();
    let mut per_object: HashMap<String, (ObjectRef, Vec<TrackPoint>)> = HashMap::new();
    for s in &sources {
        let Some(points) = telemetry_id(&s.device_obj).and_then(|d| by_device.get(&d)) else {
            continue;
        };
        let entry = per_object.entry(s.object.id.clone()).or_insert_with(|| {
            order.push(s.object.id.clone());
            (s.object.clone(), Vec::new())
        });
        entry.1.extend(
            points
                .iter()
                .filter(|p| p.t_ms >= s.from_ms && p.t_ms <= s.to_ms)
                .copied(),
        );
    }
    // Imported tracks (Postgres) of the members themselves.
    let imported_for: Vec<Uuid> = ids
        .iter()
        .filter(|id| objs.get(id).is_some_and(|o| o.type_key != KIND_EVENT))
        .copied()
        .collect();
    let imported = imports::points_of(db, org_id, &imported_for, (from_ms, to_ms), RAW_CAP).await?;
    truncated |= imported.len() as i64 >= RAW_CAP;
    let any_import = !imported.is_empty();
    for (id, p) in imported {
        let Some(o) = objs.get(&id) else { continue };
        let key = id.to_string();
        per_object
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                (links::object_ref(o), Vec::new())
            })
            .1
            .push(p);
    }
    let tracks: Vec<(ObjectRef, usize, Vec<Vec<TrackPoint>>)> = order
        .into_iter()
        .filter_map(|id| per_object.remove(&id))
        .map(|(object, points)| {
            let raw = points.len();
            let clean = normalize(points).points;
            (object, raw, split_gaps(&clean, gap_ms))
        })
        .filter(|(_, _, segs)| !segs.is_empty())
        .collect();
    // K38: a read that holds a person is logged.
    let persons: Vec<Uuid> = tracks
        .iter()
        .filter(|(o, _, _)| o.type_key == T_PERSON)
        .filter_map(|(o, _, _)| Uuid::parse_str(&o.id).ok())
        .collect();
    if !persons.is_empty() {
        reads::submit(reads::Entry {
            org_id,
            case_id,
            reader,
            objects: persons,
            window_ms: (from_ms, to_ms),
        });
    }
    Ok(Loaded {
        available: o2_read || any_import,
        truncated,
        tracks,
        gap_ms,
    })
}

pub async fn tracks(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    reader: Reader,
    case_id: Uuid,
    q: &CaseTracksQuery,
) -> Result<CaseTracks> {
    let from =
        parse_instant("from", Some(&q.from))?.ok_or_else(|| invalid("from", FIELD_REQUIRED))?;
    let to = parse_instant("to", Some(&q.to))?.ok_or_else(|| invalid("to", FIELD_REQUIRED))?;
    if to <= from || to - from > chrono::Duration::days(MAX_SPAN_DAYS) {
        return Err(invalid("to", FIELD_INVALID));
    }
    let window = (from.timestamp_millis(), to.timestamp_millis());
    let tolerance = tolerance_m(q.resolution_ms);
    let loaded = load(db, client, org_id, case_id, window, q.signal_gap_s, reader).await?;
    let mut truncated = loaded.truncated;
    let mut out = Vec::new();
    let mut budget = OUT_CAP;
    for (object, raw_points, segs) in loaded.tracks {
        let Some(t0_ms) = segs.first().and_then(|s| s.first()).map(|p| p.t_ms) else {
            continue;
        };
        let mut segments = Vec::new();
        for seg in segs {
            let g = tdtr(&seg, tolerance);
            if g.len() > budget {
                truncated = true;
                break;
            }
            budget -= g.len();
            segments.push(
                g.iter()
                    .map(|p| (p.lon, p.lat, p.alt, p.t_ms - t0_ms))
                    .collect(),
            );
        }
        out.push(CaseTrack {
            object,
            t0_ms,
            segments,
            raw_points,
        });
    }
    Ok(CaseTracks {
        from: ts(&from),
        to: ts(&to),
        available: loaded.available,
        truncated,
        tracks: out,
    })
}

/// Signal gap threshold in ms, from an optional request value in seconds.
pub(super) fn gap_ms(signal_gap_s: Option<i64>) -> i64 {
    signal_gap_s
        .unwrap_or(pnex_core::track::detect::DEFAULT_SIGNAL_GAP_S)
        .clamp(10, 86_400)
        * 1_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_follows_the_pixel_duration_within_bounds() {
        assert_eq!(tolerance_m(None), 2.0);
        assert_eq!(tolerance_m(Some(0)), 1.0);
        assert_eq!(tolerance_m(Some(10_000)), 20.0);
        assert_eq!(tolerance_m(Some(i64::MAX)), 500.0);
    }
}
