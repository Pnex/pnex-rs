//! POI links (D39 to D42): `placed_on` edges over the transverse org layer.

use super::*;

// ─────────────────────────── liens (D39 → D42, arêtes `placed_on`) ───────

/// Relation POI → média/dashboard posé sur la carte (spec du kind `map_pin`).
pub const LINK_RELATION: &str = "placed_on";

/// Arêtes sources d'un lot de POI (batch — école pins list ; hydratation
/// `poi_dto` en 1 requête, pas de N+1).
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
        .filter(resource_edges::Column::SourceKind.eq("map_pin"))
        .filter(
            resource_edges::Column::SourceId
                .is_in(pin_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>()),
        )
        .all(db)
        .await
}

/// Noms d'affichage des cibles d'arêtes, par lot (1 requête par kind cible —
/// école batch anti-N+1). Clé `(target_kind, target_id)` ; cible absente des
/// tables = absente de la map → le DTO la marque `target_dead`. Seuls
/// `media_asset`/`dashboard`/`tour` sont des cibles `placed_on` en V1 (spec
/// du kind `map_pin`, registre D42) — tout autre kind reste non résolu.
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
