//! Écriture des tours (Studio) — point d'entrée **unique** des versions de
//! parcours : le contrôleur HTTP (`controllers/tours.rs`) passe par ici,
//! jamais d'écriture directe (école `services/flow.rs`).
//!
//! Sémantique : append-only (save = nouvelle version), concurrence optimiste
//! (`expected_version_number` ≠ latest → 409). La validation est en deux
//! étages : structurelle pure (`pnex_core::validate_tour_doc`) puis **en
//! base** — chaque asset référencé doit exister dans l'org avec le kind
//! requis (`panorama` pour les scènes, image quelconque — `floorplan` ou
//! `photo` — pour les plans : n'importe quelle image s'affiche comme plan).

use pnex_core::{TourDoc, TourViolation, TOUR_MODE_PANORAMA};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{media_assets, tour_versions, tours};

/// Erreurs d'écriture d'un tour — le contrôleur les mappe en HTTP
/// (400 champ/violations, 409 conflit, 500).
#[derive(Debug)]
pub enum TourWriteError {
    /// Nom vide après trim.
    NameRequired,
    /// Nom > 200 caractères.
    NameTooLong,
    /// `validate_tour_doc` non satisfait (structurel).
    Doc(Vec<TourViolation>),
    /// Asset référencé inexistant dans l'org (ou id non-UUID).
    UnknownAsset { id: String },
    /// Asset référencé avec un kind hors liste (`expected` : kinds admis).
    BadAssetKind {
        id: String,
        expected: &'static [&'static str],
    },
    /// Concurrence optimiste perdue.
    Conflict { expected: i64, current: i64 },
    /// Échec base de données.
    Db,
}

/// Validation commune (nom + document structurel) avant toute écriture.
fn validate_tour_write(name: &str, doc: &TourDoc) -> Result<(), TourWriteError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TourWriteError::NameRequired);
    }
    if name.chars().count() > 200 {
        return Err(TourWriteError::NameTooLong);
    }
    let violations = pnex_core::validate_tour_doc(doc);
    if !violations.is_empty() {
        return Err(TourWriteError::Doc(violations));
    }
    Ok(())
}

/// Kinds admis par usage — un plan d'étage s'affiche quel que soit son kind
/// image (`floorplan` ou `photo` : n'importe quelle image convient), une
/// scène exige un panorama (rendu équirectangulaire).
const KINDS_PLAN: &[&str] = &["floorplan", "photo"];
const KINDS_SCENE: &[&str] = &["panorama"];

/// Validation **en base** : chaque `media_asset_id` référencé (plans et
/// scènes) doit exister dans l'org avec un kind admis — **par usage** : un
/// même asset cité comme plan ET comme scène est rejeté (aucune liste de
/// kinds ne peut satisfaire les deux). Requête unique en vrac (pas de N+1).
async fn validate_doc_assets(
    db: &DatabaseConnection,
    org_id: i64,
    doc: &TourDoc,
) -> Result<(), TourWriteError> {
    // Usages dans l'ordre du document (plans d'abord, scènes ensuite) —
    // l'erreur rapporte le premier usage fautif.
    let mut usages: Vec<(Uuid, &'static [&'static str])> = Vec::new();
    for f in &doc.floors {
        if let Some(plan) = &f.plan {
            match Uuid::parse_str(&plan.media_asset_id) {
                Ok(id) => usages.push((id, KINDS_PLAN)),
                Err(_) => {
                    return Err(TourWriteError::UnknownAsset {
                        id: plan.media_asset_id.clone(),
                    })
                }
            }
        }
    }
    for s in &doc.scenes {
        match Uuid::parse_str(&s.media_asset_id) {
            Ok(id) => usages.push((id, KINDS_SCENE)),
            Err(_) => {
                return Err(TourWriteError::UnknownAsset {
                    id: s.media_asset_id.clone(),
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
        .map_err(|_| TourWriteError::Db)?;
    let kinds: std::collections::HashMap<Uuid, String> =
        rows.into_iter().map(|a| (a.id, a.kind)).collect();

    for (id, want) in &usages {
        match kinds.get(id) {
            None => return Err(TourWriteError::UnknownAsset { id: id.to_string() }),
            Some(kind) if !want.contains(&kind.as_str()) => {
                return Err(TourWriteError::BadAssetKind {
                    id: id.to_string(),
                    expected: want,
                })
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Numéro de la dernière version d'un tour (0 si aucune — ne doit pas
/// arriver : la création pose toujours v1).
pub async fn latest_version_number<C: sea_orm::ConnectionTrait>(
    db: &C,
    tour_id: Uuid,
) -> Result<i64, TourWriteError> {
    Ok(tour_versions::Entity::find()
        .filter(tour_versions::Column::TourId.eq(tour_id))
        .select_only()
        .column_as(tour_versions::Column::VersionNumber.max(), "latest")
        .into_tuple::<Option<i64>>()
        .one(db)
        .await
        .map_err(|_| TourWriteError::Db)?
        .flatten()
        .unwrap_or(0))
}

/// Crée un tour **et sa version 1** (une transaction) — document minimal
/// (un étage « RDC », aucune scène). Aucun effet de publication.
pub async fn create_tour(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    description: Option<String>,
    author: Option<String>,
    note: Option<String>,
) -> Result<(tours::Model, i64), TourWriteError> {
    validate_tour_write(name, &TourDoc::minimal())?;
    let txn = db.begin().await.map_err(|_| TourWriteError::Db)?;
    let tour = tours::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name.trim().to_string()),
        description: Set(description),
        mode: Set(TOUR_MODE_PANORAMA.to_string()),
        share_token: Set(None),
        published_version_id: Set(None),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| TourWriteError::Db)?;
    tour_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        tour_id: Set(tour.id),
        version_number: Set(1),
        doc: Set(serde_json::to_value(TourDoc::minimal()).map_err(|_| TourWriteError::Db)?),
        author: Set(author),
        note: Set(note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| TourWriteError::Db)?;
    txn.commit().await.map_err(|_| TourWriteError::Db)?;
    tracing::info!(
        tour_id = tour.id.to_string().as_str(),
        org_id,
        "tour créé (v1)"
    );
    Ok((tour, 1))
}

/// Enregistre une **nouvelle version** (append-only) avec concurrence
/// optimiste : `expected_version_number` doit être la version courante
/// (la dernière — école `append_version` des flows).
#[allow(clippy::too_many_arguments)]
pub async fn append_tour_version(
    db: &DatabaseConnection,
    tour: &tours::Model,
    expected_version_number: i64,
    doc: &TourDoc,
    new_name: Option<String>,
    author: Option<String>,
    note: Option<String>,
) -> Result<(tours::Model, i64), TourWriteError> {
    let name_for_validation = new_name.as_deref().unwrap_or(&tour.name);
    validate_tour_write(name_for_validation, doc)?;
    validate_doc_assets(db, tour.org_id, doc).await?;
    let txn = db.begin().await.map_err(|_| TourWriteError::Db)?;
    // Serialize concurrent saves of this tour: a no-op UPDATE takes the
    // parent row lock (Postgres), so the latest version read below is the
    // committed one and two saves can never both pass the optimistic check
    // (the loser gets a 409, never a 500 on the version unique index).
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    tours::Entity::update_many()
        .col_expr(
            tours::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(tours::Column::Id.eq(tour.id))
        .exec(&txn)
        .await
        .map_err(|_| TourWriteError::Db)?;
    let latest = latest_version_number(&txn, tour.id).await?;
    if expected_version_number != latest {
        return Err(TourWriteError::Conflict {
            expected: expected_version_number,
            current: latest,
        });
    }
    let mut active: tours::ActiveModel = tour.clone().into();
    if let Some(name) = new_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        active.name = Set(name.to_string());
    }
    let tour = active.update(&txn).await.map_err(|_| TourWriteError::Db)?;
    let new_version = tour_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        tour_id: Set(tour.id),
        version_number: Set(latest + 1),
        doc: Set(serde_json::to_value(doc).map_err(|_| TourWriteError::Db)?),
        author: Set(author),
        note: Set(note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|e| {
        if crate::services::db_lock::is_unique_violation(&e) {
            TourWriteError::Conflict {
                expected: expected_version_number,
                current: latest + 1,
            }
        } else {
            TourWriteError::Db
        }
    })?;
    txn.commit().await.map_err(|_| TourWriteError::Db)?;
    tracing::info!(
        tour_id = tour.id.to_string().as_str(),
        version = new_version.version_number,
        "tour enregistré (nouvelle version)"
    );
    Ok((tour, new_version.version_number))
}
