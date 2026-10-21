//! Materializes `recurrence` properties (ontology.md annex B) as time ranges
//! of their object (D182): one range per occurrence on a rolling horizon,
//! keyed by `external_id = rec:<property>:<YYYY-MM-DD>` so that a replay
//! (object save, sweep, second pod) never duplicates nor misses one.
//!
//! An occurrence a user touched is never rewritten nor removed: completed
//! (`actual_*` set), status in `attrs`, or moved (planned date no longer the
//! one of its key). Untouched future occurrences that left the series (rule
//! changed, object archived) are removed. Floating series restart one
//! period after the last completion.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{NaiveDate, NaiveTime, Utc};
use loco_rs::app::AppContext;
use pnex_core::ontology::recurrence::Recurrence;
use pnex_core::ontology::{ObjectTypeDef, PropertyKind};
use pnex_core::time_range::{RangeOrigin, ScopeKind, TimeRangeInput};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use crate::models::_entities::{objects, organizations, time_ranges as tr};
use crate::services::time_ranges::{self, Scope, Writer};

/// Days ahead materialized.
pub const HORIZON_DAYS: u64 = 90;
const SWEEP_EVERY: Duration = Duration::from_secs(6 * 3600);
const KEY_PREFIX: &str = "rec:";

fn recurrence_keys(def: &ObjectTypeDef) -> Vec<&str> {
    def.properties
        .iter()
        .filter(|p| matches!(p.kind, PropertyKind::Recurrence))
        .map(|p| p.key.as_str())
        .collect()
}

fn external_id(key: &str, date: NaiveDate) -> String {
    format!("{KEY_PREFIX}{key}:{date}")
}

fn date_of(external_id: &str, key: &str) -> Option<NaiveDate> {
    external_id
        .strip_prefix(KEY_PREFIX)?
        .strip_prefix(key)?
        .strip_prefix(':')?
        .parse()
        .ok()
}

fn midnight(d: NaiveDate) -> String {
    // shortcut: UTC days, not the org's time zone; add one when plans need
    // local midnights.
    d.and_time(NaiveTime::MIN).and_utc().to_rfc3339()
}

/// Has a user acted on this occurrence (it then belongs to history)?
fn touched(row: &tr::Model, date: NaiveDate) -> bool {
    row.actual_start.is_some()
        || row.attrs.get("status").is_some_and(|s| !s.is_null())
        || row.planned_start.map(|t| t.date_naive()) != Some(date)
}

/// Dates wanted for one property value on `[today, today + horizon]`.
fn wanted(value: Option<&Value>, last_done: Option<NaiveDate>, today: NaiveDate) -> Vec<NaiveDate> {
    let Some(r) = value.and_then(Recurrence::parse) else {
        return Vec::new();
    };
    let r = match (r.floating, last_done) {
        (true, Some(done)) => match r.after_completion(done) {
            Some(r) => r,
            None => return Vec::new(),
        },
        _ => r,
    };
    let to = today
        .checked_add_days(chrono::Days::new(HORIZON_DAYS))
        .unwrap_or(today);
    let mut out = r.dates(today, to, usize::MAX);
    // A floating series waits for its next completion: an overdue first
    // date stays visible (late) instead of being skipped.
    if r.floating && r.start < today && r.until.is_none_or(|u| r.start <= u) {
        out.insert(0, r.start);
    }
    out
}

/// Brings the occurrences of one object in line with its recurrence
/// properties. Returns the number of ranges created plus removed.
pub async fn refresh<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    object_id: Uuid,
) -> Result<usize, DbErr> {
    let s = super::schema(db, org_id).await?;
    let Some(row) = objects::Entity::find_by_id(object_id)
        .filter(objects::Column::OrgId.eq(org_id))
        .one(db)
        .await?
    else {
        return Ok(0);
    };
    let Some(def) = s.object_type(&row.type_key) else {
        return Ok(0);
    };
    sync_object(db, &row, def, Utc::now().date_naive()).await
}

async fn sync_object<C: ConnectionTrait>(
    db: &C,
    row: &objects::Model,
    def: &ObjectTypeDef,
    today: NaiveDate,
) -> Result<usize, DbErr> {
    let scope = Scope {
        kind: ScopeKind::Object,
        id: row.id.to_string(),
    };
    let existing = tr::Entity::find()
        .filter(tr::Column::OrgId.eq(row.org_id))
        .filter(tr::Column::ScopeKind.eq(ScopeKind::Object.wire()))
        .filter(tr::Column::ScopeId.eq(scope.id.as_str()))
        .filter(tr::Column::ExternalId.starts_with(KEY_PREFIX))
        .all(db)
        .await?;
    let live = row.valid_to.is_none();
    let mut changed = 0;
    // Keys of the type, plus keys of occurrences whose property is gone.
    let mut keys: BTreeSet<String> = recurrence_keys(def).into_iter().map(String::from).collect();
    for r in &existing {
        if let Some(rest) = r
            .external_id
            .as_deref()
            .and_then(|e| e.strip_prefix(KEY_PREFIX))
        {
            if let Some((k, _)) = rest.rsplit_once(':') {
                keys.insert(k.to_string());
            }
        }
    }
    for key in keys {
        let mine: BTreeMap<NaiveDate, &tr::Model> = existing
            .iter()
            .filter_map(|r| Some((date_of(r.external_id.as_deref()?, &key)?, r)))
            .collect();
        let last_done = mine
            .values()
            .filter_map(|r| r.actual_end.map(|t| t.date_naive()))
            .max();
        let declared = recurrence_keys(def).contains(&key.as_str());
        let value = row.properties.get(&key).filter(|_| live && declared);
        let want: BTreeSet<NaiveDate> = wanted(value, last_done, today).into_iter().collect();
        for (date, r) in &mine {
            if !want.contains(date) && *date >= today && !touched(r, *date) {
                tr::Entity::delete_by_id(r.id).exec(db).await?;
                changed += 1;
            }
        }
        let rule = value.and_then(Recurrence::parse);
        let days = rule.as_ref().map_or(1, |r| u64::from(r.days));
        let attrs = match rule.as_ref().and_then(|r| r.lead_days) {
            Some(l) => serde_json::json!({"lead_days": l}),
            None => serde_json::json!({}),
        };
        for date in &want {
            // Untouched occurrences follow the title and the lead time.
            let input = match mine.get(date) {
                Some(r) if touched(r, *date) => continue,
                Some(r) if r.label == row.title && r.attrs == attrs => continue,
                Some(_) => TimeRangeInput {
                    label: Some(row.title.clone()),
                    external_id: Some(external_id(&key, *date)),
                    attrs: Some(attrs.clone()),
                    ..Default::default()
                },
                None => {
                    let end = date
                        .checked_add_days(chrono::Days::new(days))
                        .unwrap_or(*date);
                    TimeRangeInput {
                        label: Some(row.title.clone()),
                        external_id: Some(external_id(&key, *date)),
                        category: Some(key.clone()),
                        planned_start: Some(midnight(*date)),
                        planned_end: Some(midnight(end)),
                        attrs: Some(attrs.clone()),
                        ..Default::default()
                    }
                }
            };
            let writer = Writer {
                origin: RangeOrigin::Grid,
                source_ref: Some(format!("recurrence:{}/{key}", row.id)),
            };
            match time_ranges::upsert(db, row.org_id, &scope, &input, &writer).await {
                Ok(_) => changed += 1,
                Err(time_ranges::RangeError::Db(e)) => return Err(e),
                Err(e) => tracing::warn!(object = %row.id, error = %e, "occurrence rejected"),
            }
        }
    }
    Ok(changed)
}

/// One pass over every org: objects of types holding a recurrence.
pub async fn sweep(db: &DatabaseConnection) -> Result<usize, DbErr> {
    let today = Utc::now().date_naive();
    let mut changed = 0;
    for org in organizations::Entity::find().all(db).await? {
        let s = super::schema(db, org.id).await?;
        let types: Vec<&ObjectTypeDef> = s
            .types
            .iter()
            .map(|(t, _, _)| t)
            .filter(|t| !recurrence_keys(t).is_empty())
            .collect();
        if types.is_empty() {
            continue;
        }
        let rows = objects::Entity::find()
            .filter(objects::Column::OrgId.eq(org.id))
            .filter(objects::Column::ValidTo.is_null())
            .filter(objects::Column::TypeKey.is_in(types.iter().map(|t| t.key.clone())))
            .all(db)
            .await?;
        for row in rows {
            if let Some(def) = types.iter().find(|t| t.key == row.type_key) {
                changed += sync_object(db, &row, def, today).await?;
            }
        }
    }
    Ok(changed)
}

/// Rolls the horizon forward, one pod at a time (D106).
pub fn spawn_sweeper(ctx: &AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            if crate::services::singleton::my_turn(&ctx.db, "recurrence-sweep", SWEEP_EVERY).await {
                match sweep(&ctx.db).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(changed = n, "recurrence occurrences synced"),
                    Err(e) => tracing::warn!("recurrence sweep failed: {e}"),
                }
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    });
}

/// Best-effort refresh after a write: the sweep repairs a failure.
/// `refresh_logged` on a row and its type already in hand (no schema load).
pub(super) async fn refresh_row_logged<C: ConnectionTrait>(
    db: &C,
    row: &objects::Model,
    def: &ObjectTypeDef,
) {
    if let Err(e) = sync_object(db, row, def, Utc::now().date_naive()).await {
        tracing::warn!(object = %row.id, error = %e, "recurrence refresh failed");
    }
}

pub async fn refresh_logged<C: ConnectionTrait>(db: &C, org_id: i64, object_id: Uuid) {
    if let Err(e) = refresh(db, org_id, object_id).await {
        tracing::warn!(object = %object_id, error = %e, "recurrence refresh failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn keys_round_trip() {
        let e = external_id("service", d("2026-03-01"));
        assert_eq!(e, "rec:service:2026-03-01");
        assert_eq!(date_of(&e, "service"), Some(d("2026-03-01")));
        assert_eq!(date_of(&e, "serv"), None);
        assert_eq!(date_of("rec:service_b:2026-03-01", "service"), None);
    }

    #[test]
    fn wanted_fixed_and_floating() {
        let fixed = json!({"start": "2026-01-01", "every": 1, "unit": "month"});
        assert_eq!(
            wanted(Some(&fixed), None, d("2026-02-15")),
            [d("2026-03-01"), d("2026-04-01"), d("2026-05-01")]
        );
        let floating =
            json!({"start": "2026-01-01", "every": 3, "unit": "month", "floating": true});
        // Never done, start passed: the first date stays (late).
        assert_eq!(
            wanted(Some(&floating), None, d("2026-02-15"))[0],
            d("2026-01-01")
        );
        // Done late on Feb 10: the next one is three months after.
        assert_eq!(
            wanted(Some(&floating), Some(d("2026-02-10")), d("2026-02-15")),
            [d("2026-05-10")]
        );
        assert!(wanted(None, None, d("2026-02-15")).is_empty());
    }
}
