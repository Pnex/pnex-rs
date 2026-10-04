//! Écriture des couches d'annotations (D55–D60) — point d'entrée **unique**
//! des versions de couches : le contrôleur HTTP
//! (`controllers/annotation_layers.rs`) passe par ici, jamais d'écriture
//! directe (école `services/tour.rs`).
//!
//! Sémantique : append-only (save = nouvelle version), concurrence optimiste
//! (`expected_version_number` ≠ latest → 409). La validation est en deux
//! étages : structurelle pure (`pnex_core::validate_annotation_doc`) puis
//! **en base** — chaque asset référencé doit exister dans l'org avec le kind
//! requis par sa géométrie (`panorama` pour `Equirect`, `photo`/`floorplan`
//! pour `Flat` — n'importe quelle image plate s'affiche) et chaque device
//! ciblé (slug) doit exister dans `device_registries` de l'org.

use pnex_core::{
    geometry_requires_panorama, validate_annotation_doc, AnnotationDoc, AnnotationGeometry,
    AnnotationTarget, AnnotationViolation, TourDoc,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use std::collections::HashSet;
use uuid::Uuid;

use crate::models::_entities::{
    annotation_layer_versions, annotation_layers, device_registries, media_assets, tour_versions,
    tours,
};

/// Kinds admis par géométrie — un item `Equirect` exige un panorama, un item
/// `Flat` s'affiche sur n'importe quelle image plate (école `KINDS_PLAN`).
const KINDS_FLAT: &[&str] = &["photo", "floorplan"];
const KINDS_EQUIRECT: &[&str] = &["panorama"];

/// Erreurs d'écriture d'une couche — le contrôleur les mappe en HTTP
/// (400 champ/violations, 409 conflit, 500).
#[derive(Debug)]
pub enum AnnotationLayerWriteError {
    /// Nom vide après trim.
    NameRequired,
    /// Nom > 200 caractères.
    NameTooLong,
    /// `validate_annotation_doc` non satisfait (structurel).
    Doc(Vec<AnnotationViolation>),
    /// Asset référencé inexistant dans l'org (ou id non-UUID).
    UnknownAsset { id: String },
    /// Asset référencé avec un kind hors liste (`expected` : kinds admis).
    BadAssetKind {
        id: String,
        expected: &'static [&'static str],
    },
    /// Device ciblé (slug) inexistant dans l'org.
    UnknownDevice { device_id: String },
    /// A `control` item references a control absent from the org.
    UnknownControl { id: uuid::Uuid },
    /// Item ancré sur un média autre que celui déclaré par l'ensemble
    /// (pivot UX : un ensemble = un média ; un ensemble-tour : un média
    /// de scène du tour).
    AnchorMismatch { id: String },
    /// Association ambiguë : média ET tour déclarés (XOR attendu).
    MediaAndTour,
    /// Tour associé inexistant dans l'org.
    UnknownTour { id: String },
    /// Concurrence optimiste perdue.
    Conflict { expected: i64, current: i64 },
    /// Échec base de données.
    Db,
}

/// Validation commune (nom + document structurel) avant toute écriture.
fn validate_layer_write(name: &str, doc: &AnnotationDoc) -> Result<(), AnnotationLayerWriteError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AnnotationLayerWriteError::NameRequired);
    }
    if name.chars().count() > 200 {
        return Err(AnnotationLayerWriteError::NameTooLong);
    }
    let violations = validate_annotation_doc(doc);
    if !violations.is_empty() {
        return Err(AnnotationLayerWriteError::Doc(violations));
    }
    Ok(())
}

/// Kinds requis pour un item selon sa géométrie.
fn kinds_for(geometry: &AnnotationGeometry) -> &'static [&'static str] {
    if geometry_requires_panorama(geometry) {
        KINDS_EQUIRECT
    } else {
        KINDS_FLAT
    }
}

/// Validation **en base** : chaque `media_asset_id` référencé doit exister
/// dans l'org avec le kind requis par sa géométrie — requête unique en vrac
/// (pas de N+1, école `validate_doc_assets`).
async fn validate_doc_assets(
    db: &DatabaseConnection,
    org_id: i64,
    doc: &AnnotationDoc,
) -> Result<(), AnnotationLayerWriteError> {
    let mut usages: Vec<(Uuid, &'static [&'static str])> = Vec::new();
    for item in &doc.items {
        match Uuid::parse_str(&item.media_asset_id) {
            Ok(id) => usages.push((id, kinds_for(&item.geometry))),
            Err(_) => {
                return Err(AnnotationLayerWriteError::UnknownAsset {
                    id: item.media_asset_id.clone(),
                })
            }
        }
    }
    if usages.is_empty() {
        return Ok(());
    }

    let ids: Vec<Uuid> = {
        let mut ids: Vec<Uuid> = usages.iter().map(|(id, _)| *id).collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let rows = media_assets::Entity::find()
        .filter(media_assets::Column::OrgId.eq(org_id))
        .filter(media_assets::Column::Id.is_in(ids))
        .all(db)
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    let kinds: std::collections::HashMap<Uuid, String> =
        rows.into_iter().map(|a| (a.id, a.kind)).collect();

    for (id, want) in &usages {
        match kinds.get(id) {
            None => return Err(AnnotationLayerWriteError::UnknownAsset { id: id.to_string() }),
            Some(kind) if !want.contains(&kind.as_str()) => {
                return Err(AnnotationLayerWriteError::BadAssetKind {
                    id: id.to_string(),
                    expected: want,
                })
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Validation **en base** des cibles devices (D57 : device identifié par
/// slug, existence dans l'org) — batch, école `validate_doc_assets`.
/// La pin n'est pas résolue ici (identité machine : existence du device
/// suffit ; une pin inexistante s'affichera vide, toléré école S7).
async fn validate_doc_devices(
    db: &DatabaseConnection,
    org_id: i64,
    doc: &AnnotationDoc,
) -> Result<(), AnnotationLayerWriteError> {
    let mut slugs: Vec<String> = doc
        .items
        .iter()
        .filter_map(|item| match &item.target {
            AnnotationTarget::Device { device_id } => Some(device_id.clone()),
            AnnotationTarget::Pin { device_id, .. } => Some(device_id.clone()),
            AnnotationTarget::Status { device_id } => Some(device_id.clone()),
            // Controls are checked by `validate_doc_controls`; a reading may
            // target a flow virtual device (`flow_{id}`), never registered.
            AnnotationTarget::Note { .. }
            | AnnotationTarget::Control { .. }
            | AnnotationTarget::Reading { .. } => None,
        })
        .collect();
    if slugs.is_empty() {
        return Ok(());
    }
    slugs.sort();
    slugs.dedup();

    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(device_registries::Column::DeviceId.is_in(slugs))
        .all(db)
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    let known: HashSet<&str> = rows.iter().map(|r| r.device_id.as_str()).collect();
    for item in &doc.items {
        let slug = match &item.target {
            AnnotationTarget::Device { device_id } => Some(device_id),
            AnnotationTarget::Pin { device_id, .. } => Some(device_id),
            AnnotationTarget::Status { device_id } => Some(device_id),
            AnnotationTarget::Note { .. }
            | AnnotationTarget::Control { .. }
            | AnnotationTarget::Reading { .. } => None,
        };
        if let Some(slug) = slug {
            if !known.contains(slug.as_str()) {
                return Err(AnnotationLayerWriteError::UnknownDevice {
                    device_id: slug.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Numéro de la dernière version d'une couche (0 si aucune — ne doit pas
/// arriver : la création pose toujours v1).
/// D131: provisions the controls declared by the `control` items (nil id +
/// kind), keeps the links to existing controls of the org, and returns the
/// document with every control item bound — the version stored. A link to
/// a control absent from the org without a kind to fall back on is
/// refused. Runs inside the save transaction.
async fn bind_doc_controls<C: sea_orm::ConnectionTrait>(
    db: &C,
    org_id: i64,
    layer_id: Uuid,
    doc: &AnnotationDoc,
) -> Result<AnnotationDoc, AnnotationLayerWriteError> {
    use crate::services::surface_controls::{sync_surface, DeclaredControl, SyncError};
    let items: Vec<DeclaredControl> = doc
        .items
        .iter()
        .filter_map(|item| match &item.target {
            AnnotationTarget::Control { control_id, kind } => Some(DeclaredControl {
                item_id: item.id.clone(),
                kind: *kind,
                label: item.label.clone(),
                current: (!control_id.is_nil()).then_some(*control_id),
            }),
            _ => None,
        })
        .collect();
    let outcome = sync_surface(
        db,
        org_id,
        pnex_core::ui_control::ORIGIN_ANNOTATION,
        layer_id,
        &items,
        None,
    )
    .await
    .map_err(|e| match e {
        SyncError::UnknownControl(id) => AnnotationLayerWriteError::UnknownControl { id },
        SyncError::Db => AnnotationLayerWriteError::Db,
    })?;
    let mut bound = doc.clone();
    for item in &mut bound.items {
        if let AnnotationTarget::Control { control_id, .. } = &mut item.target {
            if let Some(id) = outcome.bound.get(&item.id) {
                *control_id = *id;
            }
        }
    }
    Ok(bound)
}

pub async fn latest_version_number<C: sea_orm::ConnectionTrait>(
    db: &C,
    layer_id: Uuid,
) -> Result<i64, AnnotationLayerWriteError> {
    Ok(annotation_layer_versions::Entity::find()
        .filter(annotation_layer_versions::Column::LayerId.eq(layer_id))
        .select_only()
        .column_as(
            annotation_layer_versions::Column::VersionNumber.max(),
            "latest",
        )
        .into_tuple::<Option<i64>>()
        .one(db)
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?
        .flatten()
        .unwrap_or(0))
}

/// Crée une couche **et sa version 1** (une transaction) — document vide.
/// Aucun effet de publication.
#[allow(clippy::too_many_arguments)] // mirrors the API payload fields one-to-one
pub async fn create_annotation_layer(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    media_asset_id: Option<Uuid>,
    tour_id: Option<Uuid>,
    description: Option<String>,
    author: Option<String>,
    note: Option<String>,
) -> Result<(annotation_layers::Model, i64), AnnotationLayerWriteError> {
    validate_layer_write(name, &AnnotationDoc::default())?;
    // XOR : un ensemble est rattaché à un média OU à un tour,
    // jamais aux deux.
    if media_asset_id.is_some() && tour_id.is_some() {
        return Err(AnnotationLayerWriteError::MediaAndTour);
    }
    // Le média déclaré doit exister dans l'org (batch école validate_doc_assets).
    if let Some(media) = media_asset_id {
        let exists = media_assets::Entity::find()
            .filter(media_assets::Column::OrgId.eq(org_id))
            .filter(media_assets::Column::Id.eq(media))
            .one(db)
            .await
            .map_err(|_| AnnotationLayerWriteError::Db)?
            .is_some();
        if !exists {
            return Err(AnnotationLayerWriteError::UnknownAsset {
                id: media.to_string(),
            });
        }
    }
    // Le tour déclaré doit exister dans l'org (école media).
    if let Some(tour) = tour_id {
        let exists = tours::Entity::find()
            .filter(tours::Column::OrgId.eq(org_id))
            .filter(tours::Column::Id.eq(tour))
            .one(db)
            .await
            .map_err(|_| AnnotationLayerWriteError::Db)?
            .is_some();
        if !exists {
            return Err(AnnotationLayerWriteError::UnknownTour {
                id: tour.to_string(),
            });
        }
    }
    let txn = db
        .begin()
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    let layer = annotation_layers::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name.trim().to_string()),
        media_asset_id: Set(media_asset_id),
        tour_id: Set(tour_id),
        description: Set(description),
        published_version_id: Set(None),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| AnnotationLayerWriteError::Db)?;
    annotation_layer_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        layer_id: Set(layer.id),
        version_number: Set(1),
        doc: Set(serde_json::to_value(AnnotationDoc::default())
            .map_err(|_| AnnotationLayerWriteError::Db)?),
        author: Set(author),
        note: Set(note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| AnnotationLayerWriteError::Db)?;
    txn.commit()
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    tracing::info!(
        layer_id = layer.id.to_string().as_str(),
        org_id,
        "couche d'annotations créée (v1)"
    );
    Ok((layer, 1))
}

/// Enregistre une **nouvelle version** (append-only) avec concurrence
/// optimiste : `expected_version_number` doit être la version courante
/// (la dernière — école `append_tour_version`).
/// Returns the stored document (control items bound, D131).
#[allow(clippy::too_many_arguments)]
pub async fn append_annotation_layer_version(
    db: &DatabaseConnection,
    layer: &annotation_layers::Model,
    expected_version_number: i64,
    doc: &AnnotationDoc,
    new_name: Option<String>,
    author: Option<String>,
    note: Option<String>,
) -> Result<(annotation_layers::Model, i64, AnnotationDoc), AnnotationLayerWriteError> {
    let name_for_validation = new_name.as_deref().unwrap_or(&layer.name);
    validate_layer_write(name_for_validation, doc)?;
    validate_doc_assets(db, layer.org_id, doc).await?;
    validate_doc_devices(db, layer.org_id, doc).await?;
    // Pivot UX : un ensemble est ancré sur SON média (D55 renforcé au
    // niveau couche) — tout item ailleurs est refusé. Un ensemble-tour
    // : l'ancre doit être un média de scène du tour (version
    // courante — l'édition se fait sur latest).
    if let Some(media) = layer.media_asset_id {
        for item in &doc.items {
            match Uuid::parse_str(&item.media_asset_id) {
                Ok(id) if id == media => {}
                _ => {
                    return Err(AnnotationLayerWriteError::AnchorMismatch {
                        id: item.media_asset_id.clone(),
                    });
                }
            }
        }
    } else if let Some(tour) = layer.tour_id {
        let latest = tour_versions::Entity::find()
            .filter(tour_versions::Column::TourId.eq(tour))
            .order_by_desc(tour_versions::Column::VersionNumber)
            .one(db)
            .await
            .map_err(|_| AnnotationLayerWriteError::Db)?;
        let allowed: HashSet<String> = match latest {
            Some(v) => serde_json::from_value::<TourDoc>(v.doc)
                .map(|d| d.scenes.iter().map(|s| s.media_asset_id.clone()).collect())
                .unwrap_or_default(),
            None => HashSet::new(),
        };
        for item in &doc.items {
            if !allowed.contains(&item.media_asset_id) {
                return Err(AnnotationLayerWriteError::AnchorMismatch {
                    id: item.media_asset_id.clone(),
                });
            }
        }
    }
    let txn = db
        .begin()
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    // Serialize concurrent saves of this layer: a no-op UPDATE takes the
    // parent row lock (Postgres), so the latest version read below is the
    // committed one and two saves can never both pass the optimistic check
    // (the loser gets a 409, never a 500 on the version unique index).
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    annotation_layers::Entity::update_many()
        .col_expr(
            annotation_layers::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(annotation_layers::Column::Id.eq(layer.id))
        .exec(&txn)
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    let latest = latest_version_number(&txn, layer.id).await?;
    if expected_version_number != latest {
        return Err(AnnotationLayerWriteError::Conflict {
            expected: expected_version_number,
            current: latest,
        });
    }
    let doc = &bind_doc_controls(&txn, layer.org_id, layer.id, doc).await?;
    let mut active: annotation_layers::ActiveModel = layer.clone().into();
    if let Some(name) = new_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        active.name = Set(name.to_string());
    }
    let layer = active
        .update(&txn)
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    let new_version = annotation_layer_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        layer_id: Set(layer.id),
        version_number: Set(latest + 1),
        doc: Set(serde_json::to_value(doc).map_err(|_| AnnotationLayerWriteError::Db)?),
        author: Set(author),
        note: Set(note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|e| {
        if crate::services::db_lock::is_unique_violation(&e) {
            AnnotationLayerWriteError::Conflict {
                expected: expected_version_number,
                current: latest + 1,
            }
        } else {
            AnnotationLayerWriteError::Db
        }
    })?;
    txn.commit()
        .await
        .map_err(|_| AnnotationLayerWriteError::Db)?;
    tracing::info!(
        layer_id = layer.id.to_string().as_str(),
        version = new_version.version_number,
        "couche d'annotations enregistrée (nouvelle version)"
    );
    Ok((layer, new_version.version_number, doc.clone()))
}
