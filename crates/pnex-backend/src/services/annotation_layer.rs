//! Writing of annotation layers (D55–D60) — **single** entry point
//! for layer versions: the HTTP controller
//! (`controllers/annotation_layers.rs`) goes through here, never a direct
//! write (`services/tour.rs` pattern).
//!
//! Semantics: append-only (save = new version), optimistic concurrency
//! (`expected_version_number` ≠ latest → 409). Validation has two
//! stages: pure structural (`pnex_core::validate_annotation_doc`) then
//! **in the DB** — each referenced asset must exist in the org with the kind
//! required by its geometry (`panorama` for `Equirect`, `photo`/`floorplan`
//! for `Flat` — any flat image is displayed) and each targeted device
//! (slug) must exist in the org's `device_registries`.

use pnex_core::{
    validate_annotation_doc, AnnotationDoc, AnnotationGeometry, AnnotationTarget,
    AnnotationViolation, TourDoc,
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

/// Write errors of a layer — the controller maps them to HTTP
/// (400 field/violations, 409 conflict, 500).
#[derive(Debug)]
pub enum AnnotationLayerWriteError {
    /// Empty name after trim.
    NameRequired,
    /// Name > 200 characters.
    NameTooLong,
    /// `validate_annotation_doc` non satisfait (structurel).
    Doc(Vec<AnnotationViolation>),
    /// Referenced asset missing from the org (or non-UUID id).
    UnknownAsset { id: String },
    /// Referenced asset with a kind outside the list (`expected`: allowed kinds).
    BadAssetKind {
        id: String,
        expected: &'static [&'static str],
    },
    /// Targeted device (slug) missing from the org.
    UnknownDevice { device_id: String },
    /// A `control` item references a control absent from the org.
    UnknownControl { id: uuid::Uuid },
    /// Item anchored on a media other than the one declared by the set
    /// (UX pivot: one set = one media; a tour set: a scene media
    /// of the tour).
    AnchorMismatch { id: String },
    /// Ambiguous association: media AND tour declared (XOR expected).
    MediaAndTour,
    /// Associated tour missing from the org.
    UnknownTour { id: String },
    /// Concurrence optimiste perdue.
    Conflict { expected: i64, current: i64 },
    /// Database failure.
    Db,
}

/// Common validation (name + structural document) before any write.
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

/// Media kinds required by an item's geometry (single source: pnex-core).
fn kinds_for(geometry: &AnnotationGeometry) -> &'static [&'static str] {
    pnex_core::geometry_media_kinds(geometry)
}

/// **In-DB** validation: each referenced `media_asset_id` must exist
/// in the org with the kind required by its geometry — single bulk query
/// (no N+1, `validate_doc_assets` pattern).
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

/// **In-DB** validation of the device targets (D57: device identified by
/// slug, existence in the org) — batch, `validate_doc_assets` pattern.
/// The pin is not resolved here (machine identity: the device's existence
/// is enough; a missing pin will display empty, tolerated per S7).
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

/// Number of the latest version of a layer (0 if none — should not
/// happen: creation always sets v1).
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
                spec: None,
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

/// Creates a layer **and its version 1** (one transaction) — empty document.
/// No publication effect.
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
    // XOR: a set is attached to a media OR to a tour,
    // never to both.
    if media_asset_id.is_some() && tour_id.is_some() {
        return Err(AnnotationLayerWriteError::MediaAndTour);
    }
    // The declared media must exist in the org (batch, validate_doc_assets pattern).
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
    // The declared tour must exist in the org (media pattern).
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
        "annotation layer created (v1)"
    );
    Ok((layer, 1))
}

/// Saves a **new version** (append-only) with optimistic
/// concurrency: `expected_version_number` must be the current version
/// (the latest — `append_tour_version` pattern).
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
    // UX pivot: a set is anchored on ITS media (D55 reinforced at the
    // layer level) — any item elsewhere is refused. A tour set:
    // the anchor must be a scene media of the tour (current
    // version — editing happens on latest).
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
        "annotation layer saved (new version)"
    );
    Ok((layer, new_version.version_number, doc.clone()))
}
