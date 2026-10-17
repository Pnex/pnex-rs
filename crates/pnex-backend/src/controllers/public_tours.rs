//! Tours publics — consommation d'un parcours **publié** via son lien de
//! partage, **sans authentification** (aucun extracteur `OrgContext` — école
//! `controllers/health.rs`).
//!
//! Garde-fous : le token (32 hex) est la seule clé d'entrée ; 404 **uniforme**
//! pour token inconnu, tour dépublié, asset non référencé ou version
//! inconnue — pas d'énumération. Les octets repassent par le MediaStore
//! (`fs`/RustFS, D5/D21), jamais par la base.
//!
//! Cache : le document est `no-store` (le même token peut servir un doc
//! différent après re-publication) ; les octets sont `immutable` (l'URL
//! porte `?v=n` et une version média est append-only — immuable).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::_entities::{media_assets, media_versions, tour_versions, tours};

/// 404 uniforme — token inconnu, non publié, asset non référencé, version
/// inconnue ou doc corrompu : la même réponse partout (pas d'énumération).
fn not_found() -> Error {
    Error::NotFound
}

/// Tour publié depuis son token, sinon None.
async fn find_published_by_token(
    db: &DatabaseConnection,
    token: &str,
) -> Result<Option<(tours::Model, tour_versions::Model)>> {
    let Some(tour) = tours::Entity::find()
        .filter(tours::Column::ShareToken.eq(token))
        .one(db)
        .await
        .map_err(|_| not_found())?
    else {
        return Ok(None);
    };
    let Some(published_id) = tour.published_version_id else {
        return Ok(None); // partagé mais dépublié → 404 uniforme
    };
    let Some(version) = tour_versions::Entity::find_by_id(published_id)
        .one(db)
        .await
        .map_err(|_| not_found())?
    else {
        return Ok(None);
    };
    Ok(Some((tour, version)))
}

/// Document publié typé, sinon None (doc corrompu → 404 uniforme).
fn published_doc(version: &tour_versions::Model) -> Option<pnex_core::TourDoc> {
    serde_json::from_value(version.doc.clone()).ok()
}

/// Assets référencés par le doc (plans d'étage + scènes), dédupliqués.
fn referenced_assets(doc: &pnex_core::TourDoc) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = Vec::new();
    for f in &doc.floors {
        if let Some(plan) = &f.plan {
            if let Ok(id) = Uuid::parse_str(&plan.media_asset_id) {
                ids.push(id);
            }
        }
    }
    for s in &doc.scenes {
        if let Ok(id) = Uuid::parse_str(&s.media_asset_id) {
            ids.push(id);
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// `GET /api/v1/public/tours/{token}` — la version publiée + la carte des
/// assets référencés (version **courante** de chacun au moment de l'appel).
#[utoipa::path(
    get,
    path = "/api/v1/public/tours/{token}",
    tag = "public",
    operation_id = "public_tours_get",
    summary = "Read a shared tour",
    description = "Public, no authentication: the share token is the only key. Returns the published version of the tour and the current version of every media it references. `Cache-Control: no-store`. Uniform 404 for an unknown token, an unpublished tour or a corrupt document.",
    params(("token" = String, Path, description = "Public share token (32 hex)")),
    responses(
        (status = 200, body = pnex_core::PublicTour),
        (status = 404, description = "Unknown token or tour not published", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn detail(State(ctx): State<AppContext>, Path(token): Path<String>) -> Result<Response> {
    let Some((tour, version)) = find_published_by_token(&ctx.db, &token).await? else {
        return Err(not_found());
    };
    let Some(doc) = published_doc(&version) else {
        return Err(not_found());
    };

    // Résolution des versions courantes (2 requêtes en vrac, pas de N+1).
    let ids = referenced_assets(&doc);
    let assets: HashMap<String, pnex_core::PublicAssetRef> = if ids.is_empty() {
        Default::default()
    } else {
        let rows = media_assets::Entity::find()
            .filter(media_assets::Column::Id.is_in(ids.clone()))
            .filter(media_assets::Column::OrgId.eq(tour.org_id))
            .all(&ctx.db)
            .await
            .map_err(|_| not_found())?;
        let current_ids: Vec<Uuid> = rows.iter().filter_map(|a| a.current_version_id).collect();
        let versions: HashMap<Uuid, media_versions::Model> = if current_ids.is_empty() {
            Default::default()
        } else {
            media_versions::Entity::find()
                .filter(media_versions::Column::Id.is_in(current_ids))
                .all(&ctx.db)
                .await
                .map_err(|_| not_found())?
                .into_iter()
                .map(|v| (v.id, v))
                .collect()
        };
        rows.iter()
            .filter_map(|a| {
                let current = a.current_version_id.and_then(|id| versions.get(&id))?;
                Some((
                    a.id.to_string(),
                    pnex_core::PublicAssetRef {
                        version_number: current.version_number,
                        content_type: current.content_type.clone(),
                        filename: current.filename.clone(),
                        size_bytes: current.size_bytes,
                    },
                ))
            })
            .collect()
    };

    let body = pnex_core::PublicTour {
        name: tour.name,
        description: tour.description,
        mode: tour.mode,
        published_version_number: version.version_number,
        published_at: version.created_at.to_rfc3339(),
        doc,
        assets,
    };
    Ok(([("cache-control", "no-store")], format::json(body)).into_response())
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct AssetQuery {
    /// Media version number (default: the asset's current version).
    v: Option<i64>,
}

/// `GET /api/v1/public/tours/{token}/assets/{asset_id}?v=n` — octets d'un
/// asset référencé par le doc publié. `v` absent = version courante.
/// Only the versioned URL is cached as immutable (O74).
#[utoipa::path(
    get,
    path = "/api/v1/public/tours/{token}/assets/{asset_id}",
    tag = "public",
    operation_id = "public_tours_asset",
    summary = "Download a media of a shared tour",
    description = "Public, no authentication: streams the bytes of a media referenced by the published document of the shared tour (`?v=n` for a given media version, default current). Served with the media content type, `nosniff`, a sandbox CSP; one-year immutable cache with `?v=n`, `no-cache` without. Uniform 404 for anything not referenced or unknown.",
    params(("token" = String, Path, description = "Public share token (32 hex)"), ("asset_id" = String, Path, format = "uuid", description = "Media asset id (UUID)"), AssetQuery),
    responses(
        (status = 200, description = "Media bytes (actual type in `Content-Type`)", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 404, description = "Unknown token, asset not referenced or unknown version", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn asset(
    State(ctx): State<AppContext>,
    Path((token, asset_id)): Path<(String, Uuid)>,
    Query(q): Query<AssetQuery>,
) -> Result<Response> {
    let Some((tour, version)) = find_published_by_token(&ctx.db, &token).await? else {
        return Err(not_found());
    };
    let Some(doc) = published_doc(&version) else {
        return Err(not_found());
    };
    // Autorisation : l'asset doit être référencé par le doc publié — et
    // rester dans l'org du tour (re-vérification explicite, pas de confiance
    // dans le doc seul).
    if !referenced_assets(&doc).contains(&asset_id) {
        return Err(not_found());
    }
    let Some(asset) = media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(tour.org_id))
        .one(&ctx.db)
        .await
        .map_err(|_| not_found())?
    else {
        return Err(not_found());
    };

    // Version demandée, sinon courante.
    let version_row = match q.v {
        Some(n) => {
            let Some(v) = media_versions::Entity::find()
                .filter(media_versions::Column::AssetId.eq(asset.id))
                .filter(media_versions::Column::VersionNumber.eq(n))
                .one(&ctx.db)
                .await
                .map_err(|_| not_found())?
            else {
                return Err(not_found());
            };
            v
        }
        None => {
            let Some(current_id) = asset.current_version_id else {
                return Err(not_found());
            };
            let Some(v) = media_versions::Entity::find_by_id(current_id)
                .one(&ctx.db)
                .await
                .map_err(|_| not_found())?
            else {
                return Err(not_found());
            };
            v
        }
    };

    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    let store = settings.store().map_err(|_| not_found())?;
    let bytes = store
        .get(&version_row.storage_key)
        .await
        .map_err(|_| not_found())?;
    // Allowlisted type + nosniff + sandbox CSP (SEC-5): this route is public
    // and served on the app origin.
    let headers = crate::services::media::user_content_headers(
        &version_row.content_type,
        &version_row.filename,
    );
    Ok((
        StatusCode::OK,
        headers,
        [("cache-control", asset_cache_control(q.v).to_string())],
        bytes,
    )
        .into_response())
}

/// Cache policy of a shared asset (O74): only a versioned URL (`?v=n`,
/// append-only bytes) is immutable; the unversioned one follows the current
/// version, so the browser revalidates it.
fn asset_cache_control(version: Option<i64>) -> &'static str {
    if version.is_some() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/public/tours")
        .add("/{token}", get(detail))
        .add("/{token}/assets/{asset_id}", get(asset))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(detail, asset))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
