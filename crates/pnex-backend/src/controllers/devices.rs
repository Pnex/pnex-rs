//! Devices — registre scopé org (D2) + catalogue global partagé.
//!
//! Contrats (`docs/phase0/api-rest.md` §4, pagination D14) :
//! - `GET /api/v1/devices` : filtres `device_type` (« all » = no-op),
//!   `capability`, `device_id` (exact), `active` (true|false) + pagination
//!   `limit`/`offset` → enveloppe `{count, next, previous, results}` ;
//! - `POST` : réactivation implicite d'un device inactif connu (200) ou 400
//!   « already registered and active », quota tier par type, sinon création
//!   inactive + DeviceToken auto (token urlsafe 32 octets + clé ChaCha20 en
//!   base64) → 201 avec le DTO complet ;
//! - `PUT/PATCH /{id}` : **metadata uniquement** (toute autre clé → 400) ;
//! - `DELETE /{id}` : nettoie build_records + device_token → 204.
//!
//! Durcissements assumés vs Django POC (multi-tenant D2 + refus par défaut) :
//! - scoping **org** (`X-Org-Id`) à la place du user Django ;
//! - écriture (create/update/delete) réservée owner/admin, lecture à tout
//!   membre ;
//! - `DELETE` répond 204 **sans body** — Django renvoyait un body sur 204,
//!   illisible côté navigateur (le compte de records nettoyés part dans les
//!   logs) ;
//! - catalogue `predefined-devices` authentifié (Django : AllowAny) ;
//! - filtre `revision` fonctionnel (Django filtrait `version=`, champ
//!   inexistant → 500) ;
//! - **pagination obligatoire** (D14) : listes paginées en SQL (catalogue :
//!   SeaORM `count`/`offset`/`limit` ; registre org : filtre puis découpage,
//!   ensemble borné par les quotas tier).

use axum::extract::{Path, Query, RawQuery, State};
use axum::http::StatusCode;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use loco_rs::prelude::*;
use rand::RngCore;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, QueryTrait, Set, TransactionTrait,
};
use sea_orm::sea_query::Expr;
use sea_orm::sea_query::extension::postgres::PgExpr;
use serde::Deserialize;
use std::collections::HashMap;

use super::pagination;
use crate::auth::{AuthUser, OrgContext};
use crate::models::_entities::{
    build_records, device_capabilities, device_registries, device_tokens, device_types,
    mcu_boards, predefined_device_capabilities, predefined_devices, subscription_tiers,
    sea_orm_active_enums::CapabilityMode,
};

// ─────────────────────────── Aides ───────────────────────────

/// Mode de capacité tel qu'exposé dans l'API (l'enum SeaORM générée
/// sérialise en Capitalized, on mappe — cf. `role_str` dans orgs).
pub fn capability_mode_str(mode: CapabilityMode) -> &'static str {
    match mode {
        CapabilityMode::Input => "input",
        CapabilityMode::Output => "output",
        CapabilityMode::InputOutput => "input_output",
    }
}

fn forbidden(msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new("forbidden", msg.to_string()),
    )
}

/// Réponse exacte des vues Django : `{"detail": "..."}`.
fn detail_status(status: StatusCode, msg: &str) -> Response {
    (
        status,
        format::json(serde_json::json!({ "detail": msg })),
    )
        .into_response()
}

/// Erreur champ-par-champ, forme DRF : `{"<champ>": "..."}`.
fn field_status(status: StatusCode, field: &str, msg: &str) -> Response {
    (
        status,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

/// Parité `secrets.token_urlsafe(32)`.
fn generate_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(32))
}

/// Parité `crypto_utils.generate_device_key` — base64 standard (44 chars).
fn generate_device_key() -> String {
    STANDARD.encode(random_bytes(32))
}

/// Noms des mesures découvertes (clés du JSONB) si le dynamic est autorisé.
fn discovered_names(value: &Option<serde_json::Value>) -> Vec<String> {
    value
        .as_ref()
        .and_then(|j| j.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Capacités par predefined device (join table), pour les ids donnés.
async fn capabilities_of(
    db: &DatabaseConnection,
    pd_ids: &[i64],
) -> Result<HashMap<i64, Vec<device_capabilities::Model>>> {
    if pd_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = predefined_device_capabilities::Entity::find()
        .find_also_related(device_capabilities::Entity)
        .filter(
            predefined_device_capabilities::Column::PredefinedDeviceId.is_in(pd_ids.to_vec()),
        )
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mut map: HashMap<i64, Vec<device_capabilities::Model>> = HashMap::new();
    for (link, cap) in rows {
        if let Some(cap) = cap {
            map.entry(link.predefined_device_id).or_default().push(cap);
        }
    }
    Ok(map)
}

/// Assemble le DTO `Device` (pnex-core) depuis le registre + son contexte.
fn device_dto(
    device: device_registries::Model,
    predefined: &predefined_devices::Model,
    type_name: &str,
    capabilities: &[device_capabilities::Model],
    token: Option<&device_tokens::Model>,
) -> pnex_core::Device {
    pnex_core::Device {
        id: device.id,
        org_id: device.org_id,
        device_id: device.device_id,
        metadata: device.metadata,
        predefined_device_name: predefined.name.clone(),
        device_type: type_name.to_string(),
        capabilities: capabilities
            .iter()
            .map(|c| pnex_core::DeviceCapability {
                id: c.id,
                name: c.name.clone(),
                mode: capability_mode_str(c.mode).to_string(),
            })
            .collect(),
        active: device.active,
        device_token: token.map(|t| pnex_core::DeviceTokenInfo {
            token: t.token.clone(),
            encryption_key: t.encryption_key.clone(),
            is_active: t.is_active,
            created: Some(t.created_at.to_rfc3339()),
        }),
        allow_dynamic_measurements: device.allow_dynamic_measurements,
        discovered_measurements: if device.allow_dynamic_measurements {
            discovered_names(&device.discovered_measurements)
        } else {
            Vec::new()
        },
        max_unique_measurements: device.max_unique_measurements,
    }
}

/// DTO complet d'un device isolé (détail / création / update).
async fn device_full(
    db: &DatabaseConnection,
    device: device_registries::Model,
) -> Result<pnex_core::Device> {
    let predefined = predefined_devices::Entity::find_by_id(device.predefined_device_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::InternalServerError)?;
    let type_name = device_types::Entity::find_by_id(predefined.device_type_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|t| t.name)
        .unwrap_or_default();
    let capabilities = capabilities_of(db, &[predefined.id])
        .await?
        .remove(&predefined.id)
        .unwrap_or_default();
    let token = device_tokens::Entity::find()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(device_dto(
        device,
        &predefined,
        &type_name,
        &capabilities,
        token.as_ref(),
    ))
}

async fn find_device(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<Option<device_registries::Model>> {
    device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Token du device : réactivé s'il existe inactif, généré s'il manque
/// (parité `get_or_create` de la réactivation Django).
async fn ensure_token(
    db: &sea_orm::DatabaseTransaction,
    device: &device_registries::Model,
) -> Result<device_tokens::Model> {
    if let Some(existing) = device_tokens::Entity::find()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        if !existing.is_active {
            let mut active: device_tokens::ActiveModel = existing.into();
            active.is_active = Set(true);
            return active.update(db).await.map_err(|_| Error::InternalServerError);
        }
        return Ok(existing);
    }
    device_tokens::ActiveModel {
        token: Set(generate_token()),
        encryption_key: Set(Some(generate_device_key())),
        is_active: Set(true),
        device_registry_id: Set(device.id),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(|_| Error::InternalServerError)
}

/// Quota du tier de l'org pour ce type de device (None = pas de plafond).
async fn tier_limit_for(
    db: &DatabaseConnection,
    org: &OrgContext,
    type_name: &str,
) -> Result<Option<i32>> {
    let Some(tier_id) = org.org.subscription_tier_id else {
        return Ok(None);
    };
    let Some(tier) = subscription_tiers::Entity::find_by_id(tier_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(None);
    };
    Ok(match type_name.to_ascii_lowercase().as_str() {
        "sensor" => Some(tier.max_sensor_devices),
        "actuator" => Some(tier.max_actuator_devices),
        "mixed" => Some(tier.max_mixed_devices),
        _ => None,
    })
}

// ───────────────────── Registre devices (org) ─────────────────────

#[derive(Debug, Default, Deserialize)]
struct ListDevicesQuery {
    /// Nom de type ; « all » = no-op (parité Django).
    device_type: Option<String>,
    capability: Option<String>,
    /// Correspondance exacte sur l'identifiant firmware.
    device_id: Option<String>,
    /// « true » | « false » ; autre/absent = tous.
    active: Option<String>,
    /// Recherche OU sur device_id, modèle (nom/pretty/description), type et
    /// capacités.
    search: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/devices` — devices de l'org, paginés (D14).
///
/// Les filtres `capability`/`search` portent sur la M2M du modèle :
/// l'ensemble org est borné par les quotas tier, on filtre en Rust puis on
/// découpe — le count reflète bien le total filtré.
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListDevicesQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .find_also_related(predefined_devices::Entity)
        .order_by_asc(device_registries::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Contexte batché : types, capacités par predefined, tokens par device.
    let type_names: HashMap<i64, String> = device_types::Entity::find()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let pd_ids: Vec<i64> = rows.iter().filter_map(|(_, pd)| pd.as_ref().map(|p| p.id)).collect();
    let caps = capabilities_of(&ctx.db, &pd_ids).await?;
    let tokens: HashMap<i64, device_tokens::Model> = device_tokens::Entity::find()
        .filter(
            device_tokens::Column::DeviceRegistryId
                .is_in(rows.iter().map(|(d, _)| d.id).collect::<Vec<_>>()),
        )
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.device_registry_id, t))
        .collect();

    let mut devices = Vec::new();
    for (device, predefined) in rows {
        let Some(predefined) = predefined else { continue };
        let type_name = type_names
            .get(&predefined.device_type_id)
            .map(String::as_str)
            .unwrap_or_default();
        if let Some(t) = q.device_type.as_deref() {
            if t != "all" && type_name != t {
                continue;
            }
        }
        if let Some(c) = q.capability.as_deref() {
            let has = caps
                .get(&predefined.id)
                .is_some_and(|list| list.iter().any(|cap| cap.name == c));
            if !has {
                continue;
            }
        }
        if q.device_id.as_deref().is_some_and(|v| v != device.device_id) {
            continue;
        }
        match q.active.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("true") if !device.active => continue,
            Some("false") if device.active => continue,
            _ => {}
        }
        // Recherche multi-champs : device_id, modèle (nom/pretty/description),
        // type, capacités — OU insensible à la casse.
        let term = q
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase);
        if let Some(term) = &term {
            let pd_caps = caps.get(&predefined.id).map(Vec::as_slice).unwrap_or(&[]);
            let text_hit = pagination::rust_search_match(
                &Some(term.clone()),
                &[
                    device.device_id.as_str(),
                    predefined.name.as_str(),
                    predefined.pretty_name.as_deref().unwrap_or_default(),
                    predefined.description.as_deref().unwrap_or_default(),
                    type_name,
                ],
            );
            let cap_hit = pd_caps
                .iter()
                .any(|cap| str::contains(&cap.name.to_lowercase(), term.as_str()));
            if !text_hit && !cap_hit {
                continue;
            }
        }
        let token = tokens.get(&device.id);
        devices.push(device_dto(
            device,
            &predefined,
            type_name,
            caps.get(&predefined.id).map(Vec::as_slice).unwrap_or(&[]),
            token,
        ));
    }

    // Découpage après filtrage + liens conservant les filtres actifs.
    let count = devices.len() as i64;
    let (skip, take) = page.slice(devices.len());
    let mut filters = Vec::new();
    if let Some(t) = q.device_type.as_ref().filter(|t| t.as_str() != "all") {
        filters.push(("device_type".to_string(), t.clone()));
    }
    if let Some(c) = &q.capability {
        filters.push(("capability".to_string(), c.clone()));
    }
    if let Some(v) = &q.device_id {
        filters.push(("device_id".to_string(), v.clone()));
    }
    if let Some(v) = &q.active {
        filters.push(("active".to_string(), v.clone()));
    }
    if let Some(v) = &q.search {
        filters.push(("search".to_string(), v.clone()));
    }
    format::json(pagination::envelope(
        "/api/v1/devices",
        &filters,
        page,
        count,
        devices.into_iter().skip(skip).take(take).collect(),
    ))
}

/// `POST /api/v1/devices` — création inactive + token, ou réactivation.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateDevice>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour gérer les devices"));
    }
    let device_id = params.device_id.trim().to_string();
    if device_id.is_empty() {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "device_id",
            "This field is required.",
        ));
    }

    let Some(predefined) = predefined_devices::Entity::find()
        .filter(predefined_devices::Column::Name.eq(&params.predefined_device_name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "predefined_device_name",
            &format!(
                "PredefinedDevice with name {} does not exist.",
                params.predefined_device_name
            ),
        ));
    };

    // Device connu de l'org : réactivation (200) ou refus (400).
    if let Some(existing) = device_registries::Entity::find()
        .filter(
            device_registries::Column::OrgId
                .eq(org.org.id)
                .and(device_registries::Column::DeviceId.eq(&device_id)),
        )
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        if existing.active {
            return Ok(detail_status(
                StatusCode::BAD_REQUEST,
                "This device is already registered and active.",
            ));
        }
        let txn = ctx.db.begin().await.map_err(|_| Error::InternalServerError)?;
        let mut active: device_registries::ActiveModel = existing.into();
        active.active = Set(true);
        let device = active
            .update(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?;
        ensure_token(&txn, &device).await?;
        txn.commit().await.map_err(|_| Error::InternalServerError)?;
        return Ok(detail_status(
            StatusCode::OK,
            "Device reactivated successfully.",
        ));
    }

    // Quota tier : tous états confondus (parité Django).
    let type_name = device_types::Entity::find_by_id(predefined.device_type_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|t| t.name)
        .unwrap_or_default();
    if let Some(limit) = tier_limit_for(&ctx.db, &org, &type_name).await? {
        let same_type = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org.org.id))
            .find_also_related(predefined_devices::Entity)
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        let count = same_type
            .iter()
            .filter(|(_, pd)| {
                pd.as_ref()
                    .is_some_and(|p| p.device_type_id == predefined.device_type_id)
            })
            .count() as i64;
        if count >= i64::from(limit) {
            return Ok(detail_status(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Device limit reached for {} devices in your subscription tier.",
                    type_name.to_ascii_lowercase()
                ),
            ));
        }
    }

    // Création inactive + token (transaction : jamais de device sans token).
    let allow_dynamic =
        matches!(predefined.name.as_str(), "custom_sensor" | "custom_device");
    let txn = ctx.db.begin().await.map_err(|_| Error::InternalServerError)?;
    let device = device_registries::ActiveModel {
        device_id: Set(device_id),
        metadata: Set(params.metadata),
        active: Set(false),
        allow_dynamic_measurements: Set(allow_dynamic),
        discovered_measurements: Set(None),
        max_unique_measurements: Set(100),
        org_id: Set(org.org.id),
        predefined_device_id: Set(predefined.id),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| Error::InternalServerError)?;
    ensure_token(&txn, &device).await?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;

    let dto = device_full(&ctx.db, device).await?;
    Ok((StatusCode::CREATED, format::json(dto)).into_response())
}

/// `GET /api/v1/devices/{id}` — détail, membres de l'org.
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    format::json(device_full(&ctx.db, device).await?)
}

/// `PUT|PATCH /api/v1/devices/{id}` — metadata uniquement (contrat Django :
/// toute autre clé, ou absence de `metadata`, → 400 « Only metadata updates
/// are allowed. »). La charge est lue en JSON brut pour détecter les clés
/// interdites avant désérialisation.
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour gérer les devices"));
    }
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let only_metadata = body
        .as_object()
        .is_some_and(|obj| obj.len() == 1 && obj.contains_key("metadata"));
    if !only_metadata {
        return Ok(detail_status(
            StatusCode::BAD_REQUEST,
            "Only metadata updates are allowed.",
        ));
    }
    let metadata = body.get("metadata").cloned();
    let mut active: device_registries::ActiveModel = device.into();
    active.metadata = Set(metadata);
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(device_full(&ctx.db, updated).await?)
}

/// `DELETE /api/v1/devices/{id}` — device + token + build_records.
/// 204 sans body (le décompte des records nettoyés part dans les logs).
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour gérer les devices"));
    }
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let cleaned = build_records::Entity::delete_many()
        .filter(
            build_records::Column::OrgId
                .eq(org.org.id)
                .and(build_records::Column::DeviceId.eq(&device.device_id)),
        )
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .rows_affected;
    device_tokens::Entity::delete_many()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    device_registries::Entity::delete_by_id(device.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(device = %device.device_id, firmware_cleaned = cleaned, "device supprimé");
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────── Catalogue global ───────────────────────

#[derive(Debug, Default, Deserialize)]
struct CapabilityQuery {
    /// input | output | input_output (autre valeur → liste vide).
    mode: Option<String>,
    /// Recherche OU sur le nom.
    search: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/device-capabilities` — catalogue, authentifié. Table de
/// référence bornée : le filtre `mode` reste en Rust, puis découpage.
async fn capabilities(
    State(ctx): State<AppContext>,
    _auth: AuthUser,
    Query(q): Query<CapabilityQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = device_capabilities::Entity::find()
        .order_by_asc(device_capabilities::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let caps: Vec<pnex_core::DeviceCapability> = rows
        .into_iter()
        .filter(|c| {
            q.mode
                .as_deref()
                .is_none_or(|m| capability_mode_str(c.mode) == m)
                && pagination::rust_search_match(&q.search, &[c.name.as_str()])
        })
        .map(|c| pnex_core::DeviceCapability {
            id: c.id,
            name: c.name,
            mode: capability_mode_str(c.mode).to_string(),
        })
        .collect();
    let count = caps.len() as i64;
    let (skip, take) = page.slice(caps.len());
    let mut filters = Vec::new();
    if let Some(m) = &q.mode {
        filters.push(("mode".to_string(), m.clone()));
    }
    if let Some(s) = &q.search {
        filters.push(("search".to_string(), s.clone()));
    }
    format::json(pagination::envelope(
        "/api/v1/device-capabilities",
        &filters,
        page,
        count,
        caps.into_iter().skip(skip).take(take).collect(),
    ))
}

/// Query-string en map multi-valeurs (`capabilities=a&capabilities=b`).
fn raw_query_map(raw: Option<&str>) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(q) = raw {
        for (k, v) in form_urlencoded::parse(q.as_bytes()) {
            map.entry(k.into_owned()).or_default().push(v.into_owned());
        }
    }
    map
}

/// `GET /api/v1/predefined-devices` — catalogue global, authentifié,
/// **paginé en SQL** (D14) : count + LIMIT/OFFSET côté base, hydration
/// (noms type/board, capacités) limitée à la seule page renvoyée.
///
/// Filtres : `capabilities` (répétable, OU — sous-requête M2M), `board`,
/// `device_type`, `name`/`pretty_name` (icontains via ILIKE), `revision`
/// (exact), `search` (OU multi-champs : nom, pretty, description, type,
/// board, capacités — ILIKE).
async fn predefined_list(
    State(ctx): State<AppContext>,
    _auth: AuthUser,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let params = raw_query_map(raw.as_deref());
    let first = |k: &str| params.get(k).and_then(|v| v.first().cloned());
    let caps_filter = params.get("capabilities").cloned().unwrap_or_default();
    let (board_f, type_f, name_f, pretty_f, rev_f, search_f) = (
        first("board"),
        first("device_type"),
        first("name"),
        first("pretty_name"),
        first("revision"),
        first("search"),
    );
    let page = pagination::PageParams::from_map(&params);

    // Liens next/previous : rejouer les filtres actifs.
    let mut filters: Vec<(String, String)> = caps_filter
        .iter()
        .map(|c| ("capabilities".to_string(), c.clone()))
        .collect();
    for (key, value) in [
        ("board", &board_f),
        ("device_type", &type_f),
        ("name", &name_f),
        ("pretty_name", &pretty_f),
        ("revision", &rev_f),
        ("search", &search_f),
    ] {
        if let Some(v) = value {
            filters.push((key.to_string(), v.clone()));
        }
    }

    // Résolution des filtres par nom → id (type, board). Nom inconnu →
    // liste vide cohérente avec le count.
    let empty_page = |page: pagination::PageParams, filters: &[(String, String)]| {
        format::json(pagination::envelope(
            "/api/v1/predefined-devices",
            filters,
            page,
            0,
            Vec::<pnex_core::PredefinedDevice>::new(),
        ))
    };
    let type_id = match type_f.as_deref() {
        Some(name) => device_types::Entity::find()
            .filter(device_types::Column::Name.eq(name))
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .map(|t| t.id),
        None => None,
    };
    if type_f.is_some() && type_id.is_none() {
        return empty_page(page, &filters);
    }
    let board_id = match board_f.as_deref() {
        Some(name) => mcu_boards::Entity::find()
            .filter(mcu_boards::Column::Name.eq(name))
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .map(|b| b.id),
        None => None,
    };
    if board_f.is_some() && board_id.is_none() {
        return empty_page(page, &filters);
    }

    let mut query = predefined_devices::Entity::find();
    if let Some(id) = type_id {
        query = query.filter(predefined_devices::Column::DeviceTypeId.eq(id));
    }
    if let Some(id) = board_id {
        query = query.filter(predefined_devices::Column::BoardId.eq(id));
    }
    if let Some(n) = &name_f {
        query = query.filter(
            Expr::col((predefined_devices::Entity, predefined_devices::Column::Name))
                .ilike(format!("%{n}%")),
        );
    }
    if let Some(p) = &pretty_f {
        query = query.filter(
            Expr::col((predefined_devices::Entity, predefined_devices::Column::PrettyName))
                .ilike(format!("%{p}%")),
        );
    }
    if let Some(r) = &rev_f {
        query = query.filter(predefined_devices::Column::Revision.eq(r));
    }
    if !caps_filter.is_empty() {
        // OU sur la M2M : id IN (sous-requête des liens vers les caps visées).
        let sub = predefined_device_capabilities::Entity::find()
            .left_join(device_capabilities::Entity)
            .filter(device_capabilities::Column::Name.is_in(caps_filter))
            .select_only()
            .column(predefined_device_capabilities::Column::PredefinedDeviceId);
        query = query.filter(predefined_devices::Column::Id.in_subquery(sub.into_query()));
    }
    if let Some(s) = search_f
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Recherche OU multi-champs, poussée en SQL (ILIKE PG = insensible à
        // la casse) pour rester compatible avec le LIMIT/OFFSET base.
        let pat = format!("%{s}%");
        let text_or_refs = sea_orm::Condition::any()
            .add(
                Expr::col((predefined_devices::Entity, predefined_devices::Column::Name))
                    .ilike(pat.clone()),
            )
            .add(
                Expr::col((predefined_devices::Entity, predefined_devices::Column::PrettyName))
                    .ilike(pat.clone()),
            )
            .add(
                Expr::col((predefined_devices::Entity, predefined_devices::Column::Description))
                    .ilike(pat.clone()),
            )
            .add(predefined_devices::Column::DeviceTypeId.in_subquery(
                device_types::Entity::find()
                    .filter(
                        Expr::col((device_types::Entity, device_types::Column::Name))
                            .ilike(pat.clone()),
                    )
                    .select_only()
                    .column(device_types::Column::Id)
                    .into_query(),
            ))
            .add(predefined_devices::Column::BoardId.in_subquery(
                mcu_boards::Entity::find()
                    .filter(
                        Expr::col((mcu_boards::Entity, mcu_boards::Column::Name))
                            .ilike(pat.clone()),
                    )
                    .select_only()
                    .column(mcu_boards::Column::Id)
                    .into_query(),
            ))
            .add(predefined_devices::Column::Id.in_subquery(
                predefined_device_capabilities::Entity::find()
                    .left_join(device_capabilities::Entity)
                    .filter(
                        Expr::col((device_capabilities::Entity, device_capabilities::Column::Name))
                            .ilike(pat),
                    )
                    .select_only()
                    .column(predefined_device_capabilities::Column::PredefinedDeviceId)
                    .into_query(),
            ));
        query = query.filter(text_or_refs);
    }

    let count = query
        .clone()
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)? as i64;
    let rows = query
        .order_by_asc(predefined_devices::Column::Id)
        .offset(page.offset as u64)
        .limit(page.limit as u64)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Hydration de la page uniquement.
    let type_names: HashMap<i64, String> = device_types::Entity::find()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let board_names: HashMap<i64, String> = mcu_boards::Entity::find()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|b| (b.id, b.name))
        .collect();
    let caps = capabilities_of(
        &ctx.db,
        &rows.iter().map(|p| p.id).collect::<Vec<_>>(),
    )
    .await?;

    let out: Vec<pnex_core::PredefinedDevice> = rows
        .into_iter()
        .map(|pd| pnex_core::PredefinedDevice {
            name: pd.name,
            pretty_name: pd.pretty_name,
            prestashop_product_id: pd.prestashop_product_id,
            prestashop_buy_url: pd.prestashop_buy_url,
            byod_doc_url: pd.byod_doc_url,
            image_source_url: pd.image_source_url,
            description: pd.description,
            revision: pd.revision,
            device_type: type_names
                .get(&pd.device_type_id)
                .cloned()
                .unwrap_or_default(),
            capabilities: caps
                .get(&pd.id)
                .map(|list| list.iter().map(|c| c.name.clone()).collect())
                .unwrap_or_default(),
            board: board_names
                .get(&pd.board_id)
                .cloned()
                .unwrap_or_default(),
        })
        .collect();
    format::json(pagination::envelope(
        "/api/v1/predefined-devices",
        &filters,
        page,
        count,
        out,
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/devices")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).put(update).patch(update).delete(delete))
}

/// Routes du catalogue global (préfixe /api/v1 commun).
pub fn catalogue_routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/device-capabilities", get(capabilities))
        .add("/predefined-devices", get(predefined_list))
}
