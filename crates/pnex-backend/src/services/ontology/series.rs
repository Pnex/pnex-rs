//! Time belongs to objects (D181): a `series` property is recomposed from
//! the successive `measures` links that fed it. Replacing a sensor closes
//! one link and opens another; the object's curve stays continuous.
//!
//! Points are read through the dashboards path (`series_points`: validated
//! selector, cache, timeout, degraded on O2 trouble), then clipped to each
//! link's validity.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::api::{LatestValue, QueryRow, SeriesPoint, SeriesSegment, SeriesView};
use pnex_core::ontology::{PropertyKind, MEASURES_METRIC, REL_MEASURES};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use super::links::object_ref;
use super::{invalid, OntologyError, OrgSchema, Result};
use crate::models::_entities::{device_registries, objects, resource_edges};
use crate::services::openobserve::client::Client;
use crate::services::visualization::{series_points, WINDOWS};

/// Rows of joined last values are capped: one O2 read per row and property.
const LATEST_ROWS_MAX: usize = 100;

/// `measures` links of `object.property`, oldest first; `since` keeps the
/// links still open at or after it.
async fn bindings(
    db: &DatabaseConnection,
    org_id: i64,
    object: Uuid,
    property: &str,
    since: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> std::result::Result<Vec<resource_edges::Model>, sea_orm::DbErr> {
    let mut q = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(REL_MEASURES))
        .filter(resource_edges::Column::TargetObjectId.eq(object))
        .filter(Expr::cust_with_values(
            "attributes ->> 'property' = $1",
            [property],
        ))
        .order_by_asc(resource_edges::Column::ValidFrom);
    if let Some(t) = since {
        q = q.filter(
            Condition::any()
                .add(resource_edges::Column::ValidTo.is_null())
                .add(resource_edges::Column::ValidTo.gte(t)),
        );
    }
    q.all(db).await
}

/// The telemetry `device_id` label of a device identity (its registry name).
async fn device_label(db: &DatabaseConnection, org_id: i64, native_id: &str) -> Option<String> {
    device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(device_registries::Column::Id.eq(native_id.parse::<i64>().ok()?))
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|d| d.device_id)
}

fn series_unit(s: &OrgSchema, type_key: &str, property: &str) -> Result<Option<String>> {
    let def = s.object_type(type_key).ok_or(OntologyError::NotFound)?;
    match def
        .properties
        .iter()
        .find(|p| p.key == property)
        .map(|p| &p.kind)
    {
        Some(PropertyKind::Series { unit, .. }) => Ok(unit.clone()),
        _ => Err(invalid("property", FIELD_INVALID)),
    }
}

/// Recomposed series of `object.property` over a window preset.
pub async fn read(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    s: &OrgSchema,
    object: &objects::Model,
    property: &str,
    window: &str,
) -> Result<SeriesView> {
    let unit = series_unit(s, &object.type_key, property)?;
    let Some(&(_, window_secs)) = WINDOWS.iter().find(|(k, _)| *k == window) else {
        return Err(invalid("window", FIELD_INVALID));
    };
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::seconds(window_secs)).fixed_offset();
    let links = bindings(db, org_id, object.id, property, Some(since)).await?;
    let devices =
        super::links::refs(db, org_id, links.iter().filter_map(|l| l.source_object_id)).await?;
    let mut view = SeriesView {
        property: property.into(),
        unit,
        ..Default::default()
    };
    for l in &links {
        let Some(dev) = l.source_object_id.and_then(|id| devices.get(&id)) else {
            continue;
        };
        let metric = l
            .attributes
            .get(MEASURES_METRIC)
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        view.segments.push(SeriesSegment {
            device: object_ref(dev),
            metric: metric.into(),
            from: l.valid_from.to_rfc3339(),
            to: l.valid_to.map(|t| t.to_rfc3339()),
        });
        let Some(label) = device_label(db, org_id, &dev.native_id).await else {
            continue;
        };
        // Invalid names degrade to "no data" like a dashboard source.
        let Ok(resp) = series_points(db, client, org_id, metric, &label, window).await else {
            continue;
        };
        let from = l.valid_from.timestamp() as f64;
        let to = l.valid_to.map_or(f64::MAX, |t| t.timestamp() as f64);
        view.points.extend(
            resp.points
                .into_iter()
                .filter(|p| p.ts >= from && p.ts < to)
                .filter_map(|p| {
                    Some(SeriesPoint {
                        t: chrono::DateTime::from_timestamp(p.ts as i64, 0)?.to_rfc3339(),
                        v: p.value,
                        source: dev.id.to_string(),
                    })
                }),
        );
    }
    view.points.sort_by(|a, b| a.t.cmp(&b.t));
    Ok(view)
}

/// Current sensor of every bound `series` property of an object (open
/// `measures` links): what a type dashboard reads (D187).
pub async fn current_bindings(
    db: &DatabaseConnection,
    org_id: i64,
    object: Uuid,
) -> Result<Vec<pnex_core::SeriesBinding>> {
    let open = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(REL_MEASURES))
        .filter(resource_edges::Column::TargetObjectId.eq(object))
        .filter(resource_edges::Column::ValidTo.is_null())
        .all(db)
        .await?;
    let mut out = Vec::new();
    for l in open {
        let attr = |k: &str| {
            l.attributes
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let Some(dev) = l.source_object_id else {
            continue;
        };
        let Ok(dev) = super::objects::find(db, org_id, dev).await else {
            continue;
        };
        let Some(label) = device_label(db, org_id, &dev.native_id).await else {
            continue;
        };
        out.push(pnex_core::SeriesBinding {
            property: attr(pnex_core::ontology::MEASURES_PROPERTY),
            device_id: label,
            metric: attr(MEASURES_METRIC),
        });
    }
    Ok(out)
}

/// Joins the last value of `props` on query rows (D185), through the open
/// `measures` link of each property.
pub async fn join_latest(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    s: &OrgSchema,
    props: &[String],
    rows: &mut [QueryRow],
) -> Result<()> {
    for row in rows.iter_mut().take(LATEST_ROWS_MAX) {
        let Ok(id) = Uuid::parse_str(&row.object.id) else {
            continue;
        };
        for p in props {
            if series_unit(s, &row.object.type_key, p).is_err() {
                continue;
            }
            let open = bindings(db, org_id, id, p, None)
                .await?
                .into_iter()
                .find(|l| l.valid_to.is_none());
            let Some(l) = open else { continue };
            let Some(dev) = l.source_object_id else {
                continue;
            };
            let Ok(dev) = super::objects::find(db, org_id, dev).await else {
                continue;
            };
            let Some(label) = device_label(db, org_id, &dev.native_id).await else {
                continue;
            };
            let metric = l
                .attributes
                .get(MEASURES_METRIC)
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if let Ok(resp) = series_points(db, client, org_id, metric, &label, "1h").await {
                if let Some(last) = resp.points.last() {
                    if let Some(at) = chrono::DateTime::from_timestamp(last.ts as i64, 0) {
                        row.latest.insert(
                            p.clone(),
                            LatestValue {
                                value: last.value,
                                at: at.to_rfc3339(),
                            },
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
