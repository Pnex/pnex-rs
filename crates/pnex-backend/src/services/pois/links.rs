//! POI links (D39 to D42): `placed_on` edges over the transverse org layer.

use super::*;

// ─────────────────────────── links (D39 → D42, `placed_on` edges) ───────

/// POI → media/dashboard relation placed on the map (`map_pin` kind spec).
pub const LINK_RELATION: &str = "placed_on";

/// Source edges of a batch of POIs (batch — pins list pattern; `poi_dto`
/// hydration in 1 query, no N+1).
pub async fn links_for_pins(
    db: &DatabaseConnection,
    org_id: i64,
    pin_ids: &[Uuid],
) -> Result<Vec<resource_edges::Model>, DbErr> {
    if pin_ids.is_empty() {
        return Ok(vec![]);
    }
    resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(LINK_RELATION))
        .filter(resource_edges::Column::ValidTo.is_null())
        .filter(resource_edges::Column::SourceKind.eq("map_pin"))
        .filter(
            resource_edges::Column::SourceId
                .is_in(pin_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>()),
        )
        .all(db)
        .await
}

/// Display names of edge targets, by batch (1 query per target kind —
/// anti-N+1 batch pattern). Key `(target_kind, target_id)`; a target missing from
/// the tables = missing from the map → the DTO marks it `target_dead`. Only
/// `media_asset`/`dashboard`/`tour` are `placed_on` targets in V1 (`map_pin`
/// kind spec, D42 registry) — any other kind stays unresolved.
pub async fn link_target_labels(
    db: &DatabaseConnection,
    org_id: i64,
    links: &[resource_edges::Model],
) -> Result<HashMap<(String, String), Option<String>>, DbErr> {
    use crate::models::_entities::{dashboards, media_assets, tours};
    let mut media_ids = Vec::new();
    let mut dashboard_ids = Vec::new();
    let mut tour_ids = Vec::new();
    for l in links {
        let Ok(id) = Uuid::parse_str(&l.target_id) else {
            continue;
        };
        match l.target_kind.as_str() {
            "media_asset" => media_ids.push(id),
            "dashboard" => dashboard_ids.push(id),
            "tour" => tour_ids.push(id),
            _ => {}
        }
    }
    let mut labels = HashMap::new();
    if !media_ids.is_empty() {
        for row in media_assets::Entity::find()
            .filter(media_assets::Column::OrgId.eq(org_id))
            .filter(media_assets::Column::Id.is_in(media_ids))
            .all(db)
            .await?
        {
            labels.insert(
                ("media_asset".to_string(), row.id.to_string()),
                Some(row.name),
            );
        }
    }
    if !dashboard_ids.is_empty() {
        for row in dashboards::Entity::find()
            .filter(dashboards::Column::OrgId.eq(org_id))
            .filter(dashboards::Column::Id.is_in(dashboard_ids))
            .all(db)
            .await?
        {
            labels.insert(
                ("dashboard".to_string(), row.id.to_string()),
                Some(row.name),
            );
        }
    }
    if !tour_ids.is_empty() {
        for row in tours::Entity::find()
            .filter(tours::Column::OrgId.eq(org_id))
            .filter(tours::Column::Id.is_in(tour_ids))
            .all(db)
            .await?
        {
            labels.insert(("tour".to_string(), row.id.to_string()), Some(row.name));
        }
    }
    Ok(labels)
}
