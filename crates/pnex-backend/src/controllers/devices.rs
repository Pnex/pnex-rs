//! Devices — registre scopé org (D2) + catalogue global partagé.
//!
//! Parité des contrats Django (`docs/phase0/api-rest.md` §4) :
//! - `GET /api/v1/devices` : filtres `device_type` (« all » = no-op),
//!   `capability`, `device_id` (exact), `active` (true|false) — liste non
//!   paginée ;
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
//!   inexistant → 500).

use axum::extract::{Path, Query, RawQuery, State};
use axum::http::StatusCode;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use loco_rs::prelude::*;
use rand::RngCore;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use serde::Deserialize;
use std::collections::HashMap;

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
}

/// `GET /api/v1/devices` — devices de l'org, non paginé.
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListDevicesQuery>,
) -> Result<Response> {
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
        let token = tokens.get(&device.id);
        devices.push(device_dto(
            device,
            &predefined,
            type_name,
            caps.get(&predefined.id).map(Vec::as_slice).unwrap_or(&[]),
            token,
        ));
    }
    format::json(devices)
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
}

/// `GET /api/v1/device-capabilities` — catalogue, authentifié.
async fn capabilities(
    State(ctx): State<AppContext>,
    _auth: AuthUser,
    Query(q): Query<CapabilityQuery>,
) -> Result<Response> {
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
        })
        .map(|c| pnex_core::DeviceCapability {
            id: c.id,
            name: c.name,
            mode: capability_mode_str(c.mode).to_string(),
        })
        .collect();
    format::json(caps)
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

/// `GET /api/v1/predefined-devices` — catalogue global, authentifié.
/// Filtres : `capabilities` (répétable, OU), `board`, `device_type`,
/// `name`/`pretty_name` (icontains), `revision` (exact).
async fn predefined_list(
    State(ctx): State<AppContext>,
    _auth: AuthUser,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let params = raw_query_map(raw.as_deref());
    let first = |k: &str| params.get(k).and_then(|v| v.first().cloned());
    let caps_filter = params.get("capabilities").cloned().unwrap_or_default();
    let (board_f, type_f, name_f, pretty_f, rev_f) = (
        first("board"),
        first("device_type"),
        first("name"),
        first("pretty_name"),
        first("revision"),
    );

    let rows = predefined_devices::Entity::find()
        .order_by_asc(predefined_devices::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
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

    let mut out = Vec::new();
    for pd in rows {
        let type_name = type_names
            .get(&pd.device_type_id)
            .map(String::as_str)
            .unwrap_or_default();
        let board_name = board_names
            .get(&pd.board_id)
            .map(String::as_str)
            .unwrap_or_default();
        let cap_names: Vec<String> = caps
            .get(&pd.id)
            .map(|list| list.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();

        if !caps_filter.is_empty() && !caps_filter.iter().any(|c| cap_names.contains(c)) {
            continue;
        }
        if board_f.as_deref().is_some_and(|b| b != board_name) {
            continue;
        }
        if type_f.as_deref().is_some_and(|t| t != type_name) {
            continue;
        }
        if name_f
            .as_deref()
            .is_some_and(|n| !pd.name.to_lowercase().contains(&n.to_lowercase()))
        {
            continue;
        }
        if pretty_f.as_deref().is_some_and(|p| {
            !pd.pretty_name
                .as_deref()
                .is_some_and(|v| v.to_lowercase().contains(&p.to_lowercase()))
        }) {
            continue;
        }
        if rev_f.as_deref().is_some_and(|r| pd.revision != r) {
            continue;
        }

        out.push(pnex_core::PredefinedDevice {
            name: pd.name,
            pretty_name: pd.pretty_name,
            prestashop_product_id: pd.prestashop_product_id,
            prestashop_buy_url: pd.prestashop_buy_url,
            byod_doc_url: pd.byod_doc_url,
            image_source_url: pd.image_source_url,
            description: pd.description,
            revision: pd.revision,
            device_type: type_name.to_string(),
            capabilities: cap_names,
            board: board_name.to_string(),
        });
    }
    format::json(out)
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
