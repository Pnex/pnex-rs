//! Edges de la couche d'organisation (D42) — liens croisés many-to-many,
//! chaque arête porte son `placement` (opaque pour le moteur, structuré par
//! relation). Absorbe `viz_links` (D39 → D42) : la validité
//! `relation × source × cible` est déclarée **par le kind source** (spec),
//! l'existence+tenancy des deux bouts passe par le **registre**.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};
use serde_json::Value;

use crate::models::_entities::resource_edges;
use crate::services::resources::registry;
use crate::services::resources::ResourceError;
use pnex_core::resources::REL_PLACED_ON;

/// Cap de taille du placement (JSON sérialisé) — une coordonnée/hotspot,
/// pas un document.
const PLACEMENT_MAX_BYTES: usize = 4096;

/// Validation moteur : `placement` = objet JSON borné. La structure fine
/// (hotspot, coords, overrides) est affaire de la relation/du kind — le
/// moteur reste générique.
pub fn validate_placement(placement: &Value) -> Result<(), ResourceError> {
    if !placement.is_object() {
        return Err(ResourceError::PlacementInvalid("objet JSON attendu"));
    }
    if serde_json::to_vec(placement)
        .map(|b| b.len() > PLACEMENT_MAX_BYTES)
        .unwrap_or(true)
    {
        return Err(ResourceError::PlacementInvalid("placement trop volumineux"));
    }
    Ok(())
}

/// Arête de l'org par id.
pub async fn find_edge(
    db: &DatabaseConnection,
    org_id: i64,
    edge_id: i64,
) -> Result<Option<resource_edges::Model>, DbErr> {
    resource_edges::Entity::find_by_id(edge_id)
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .one(db)
        .await
}

/// Arêtes filtrées (tous critères optionnels) — listes API + hydratation
/// batch des POI (école `links_for_sources`).
pub async fn list_edges(
    db: &DatabaseConnection,
    org_id: i64,
    relation: Option<&str>,
    source: Option<(&str, &str)>,
    target: Option<(&str, &str)>,
) -> Result<Vec<resource_edges::Model>, DbErr> {
    let mut q = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .order_by_desc(resource_edges::Column::CreatedAt);
    if let Some(relation) = relation {
        q = q.filter(resource_edges::Column::Relation.eq(relation));
    }
    if let Some((kind, id)) = source {
        q = q
            .filter(resource_edges::Column::SourceKind.eq(kind))
            .filter(resource_edges::Column::SourceId.eq(id));
    }
    if let Some((kind, id)) = target {
        q = q
            .filter(resource_edges::Column::TargetKind.eq(kind))
            .filter(resource_edges::Column::TargetId.eq(id));
    }
    q.all(db).await
}

/// Crée une arête : validité déclarée par le kind **source**, existence +
/// tenancy des deux bouts par le registre, doublon → 409. Signature bas
/// niveau assumée (API moteur générique — `#[allow]` documenté).
#[allow(clippy::too_many_arguments)]
pub async fn create_edge(
    db: &DatabaseConnection,
    org_id: i64,
    relation: &str,
    source_kind: &str,
    source_id: &str,
    target_kind: &str,
    target_id: &str,
    placement: Option<&Value>,
) -> Result<resource_edges::Model, ResourceError> {
    let reg = registry::global();
    // La validité est UNE question posée au registre — jamais de match ici.
    if !reg.allows_relation(relation, source_kind, target_kind) {
        return Err(ResourceError::RelationInvalid);
    }
    for (kind, id) in [(source_kind, source_id), (target_kind, target_id)] {
        let Some(entry) = reg.entry(kind) else {
            return Err(ResourceError::KindInvalid);
        };
        let ok = entry
            .resolver
            .resolve(db, org_id, id)
            .await
            .map_err(|_| ResourceError::Db)?;
        if !ok {
            return Err(ResourceError::ResourceUnknown);
        }
    }
    if let Some(p) = placement {
        validate_placement(p)?;
    }

    // Doublon (index unique) → 409 propre plutôt qu'un Db opaque.
    let dup = list_edges(
        db,
        org_id,
        Some(relation),
        Some((source_kind, source_id)),
        Some((target_kind, target_id)),
    )
    .await
    .map_err(|_| ResourceError::Db)?;
    if !dup.is_empty() {
        return Err(ResourceError::Conflict);
    }

    let am = resource_edges::ActiveModel {
        org_id: Set(org_id),
        relation: Set(relation.to_string()),
        source_kind: Set(source_kind.to_string()),
        source_id: Set(source_id.to_string()),
        target_kind: Set(target_kind.to_string()),
        target_id: Set(target_id.to_string()),
        placement: Set(placement.cloned()),
        ..Default::default()
    };
    am.insert(db).await.map_err(|_| ResourceError::Db)
}

/// PATCH placement d'une arête existante.
pub async fn update_placement(
    db: &DatabaseConnection,
    org_id: i64,
    edge_id: i64,
    placement: &Value,
) -> Result<Option<resource_edges::Model>, ResourceError> {
    validate_placement(placement)?;
    let row = find_edge(db, org_id, edge_id)
        .await
        .map_err(|_| ResourceError::Db)?;
    let Some(row) = row else { return Ok(None) };
    let mut am: resource_edges::ActiveModel = row.into();
    am.placement = Set(Some(placement.clone()));
    am.updated_at = Set(chrono::Utc::now().into());
    Ok(Some(am.update(db).await.map_err(|_| ResourceError::Db)?))
}

/// Supprime ; `false` = arête introuvable dans l'org (→ 404 masqué).
pub async fn delete_edge(
    db: &DatabaseConnection,
    org_id: i64,
    edge_id: i64,
) -> Result<bool, DbErr> {
    let Some(edge) = find_edge(db, org_id, edge_id).await? else {
        return Ok(false);
    };
    resource_edges::Entity::delete_by_id(edge.id)
        .exec(db)
        .await
        .map(|_| ())?;
    Ok(true)
}

/// Relation canonique du POI → média/dashboard (rebranchement D39).
pub const POI_PLACED_ON: &str = REL_PLACED_ON;
