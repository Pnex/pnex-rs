//! Edge references — org-scoped CRUD of WiFi credentials and PNeX server
//! hosts. Feeds the Config step of the device wizard (no more re-typing
//! on every device addition; see `pnex_core::edge`).
//!
//! `controllers/notify.rs` school: org scoping (cross-org 404 masked),
//! writes gated by their IAM permission, 400 field by field. **Deliberate divergence**
//! from notify: the POST is an **upsert** (200/201, never 409) — (org,
//! ssid) / (org, host) is the natural key and the inline addition from the
//! wizard must never conflict: saving again = updating the
//! password (a host is returned as is).
//!
//! The WiFi password lives in the secrets vault (secrets.md S6): only its
//! reference is returned, never the value.

use std::net::Ipv4Addr;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use axum::Json;
use futures_util::StreamExt;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{pnex_hosts, wifi_credentials};
use crate::models::pnex_hosts::PnexHosts;
use crate::models::wifi_credentials::WifiCredentials;
use pnex_api_contract::{ServerInfo, META_VERSION_PATH, SERVICE};
use pnex_core::{
    err_codes, LanScanHit, LanScanResult, LockedHost, PnexHost, PnexHostInput, SecretFieldInput,
    WifiCredential, WifiCredentialInput,
};

/// Meta port of the PNeX server (default binding, config/*.yaml) — target of the
/// LAN scan.
const SCAN_PORT: u16 = 5150;
/// Timeout per probe: an unreachable IP must free its slot fast.
/// (900 ms: typical SYN-drop, tight bound without abusing false negatives.)
const SCAN_TIMEOUT: Duration = Duration::from_millis(900);
/// Parallelism: 96 simultaneous probes — a multi-interface host
/// (docker bridges, virbr) scans several /24: 1270 candidates ≈ 12 s
/// worst case, a single /24 ≈ 2.5 s.
const SCAN_CONCURRENCY: usize = 96;

/// 400 field by field (notify school).
fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
}

/// 409 when the deployment imposes the server host (`PNEX_PROD_HOST`): the
/// host referential is read-only, builds always target the imposed host.
fn host_locked() -> Option<Error> {
    crate::app::prod_host().map(|_| {
        Error::CustomError(
            StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail::new(
                err_codes::EDGE_HOST_LOCKED,
                "The PNeX server host is imposed by this deployment.".to_string(),
            ),
        )
    })
}

/// 409 when the targeted referential key is already used by ANOTHER entry
/// of the org (an in-place edit renames onto an existing (org, ssid) /
/// (org, host), or a concurrent upsert won).
fn conflict(code: &str, msg: &str) -> Response {
    super::coded_error(StatusCode::CONFLICT, code, msg, None).into_response()
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/edge")
        .add("/wifi-credentials", get(list_wifi).post(create_wifi))
        .add(
            "/wifi-credentials/{id}",
            put(update_wifi).delete(delete_wifi),
        )
        .add("/hosts", get(list_hosts).post(create_host))
        .add("/hosts/locked", get(locked_host))
        .add("/hosts/{id}", put(update_host).delete(delete_host))
        .add("/lan-scan", get(lan_scan))
}

// ───────────────────────── WiFi credentials ─────────────────────────

// The password is a vault secret (secrets.md lot S6): the API only shows
// its reference and name; a typed value lands in `wifi/<ssid>/password`.

/// Read model of a row, `name` = vault name of its password.
fn wifi_dto(m: &wifi_credentials::Model, name: Option<String>) -> WifiCredential {
    WifiCredential {
        id: m.id,
        org_id: m.org_id,
        ssid: m.ssid.clone(),
        password: m
            .secret_id
            .zip(name)
            .map(|(secret_id, name)| pnex_core::SecretFieldView { secret_id, name }),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

/// Read model with its secret name resolved.
async fn wifi_view(
    db: &impl sea_orm::ConnectionTrait,
    m: &wifi_credentials::Model,
) -> Result<WifiCredential> {
    let ids: Vec<uuid::Uuid> = m.secret_id.into_iter().collect();
    let names = crate::services::secrets::store::names_of(db, Some(m.org_id), &ids)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(wifi_dto(
        m,
        m.secret_id.and_then(|id| names.get(&id).cloned()),
    ))
}

/// Password part of an input: a pick, a typed value, or `None` =
/// unchanged.
fn password_input(input: &WifiCredentialInput) -> Option<SecretFieldInput> {
    match &input.password {
        Some(SecretFieldInput::Value { value }) if value.is_empty() => None,
        other => other.clone(),
    }
}

fn validate_ssid(input: &WifiCredentialInput) -> Option<Response> {
    let ssid = input.ssid.trim();
    if ssid.is_empty() {
        return Some(field_status("ssid", err_codes::FIELD_REQUIRED));
    }
    // 802.11 : 32 octets max, byte-sensitive.
    if ssid.len() > 32 {
        return Some(field_status(
            "ssid",
            &format!("{}:32", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    None
}

/// Open networks out of v1 scope: a saved entry always has a password.
fn password_required() -> Response {
    field_status("password", err_codes::FIELD_REQUIRED)
}

fn vault_error(e: crate::services::secrets::store::StoreError) -> Result<Response> {
    crate::controllers::secrets::store_error(e)
}

/// `GET /api/v1/edge/wifi-credentials` — the org's references (ssid ASC).
#[utoipa::path(
    get,
    path = "/api/v1/edge/wifi-credentials",
    tag = "edge",
    operation_id = "edge_wifi_list",
    summary = "List the saved WiFi credentials",
    description = "WiFi credentials of the org (provisioning wizard), ordered by SSID, paginated (`limit`/`offset`). Passwords are returned as vault references (id + name), never values. Any org member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::WifiCredential>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_wifi(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = WifiCredentials::find()
        .filter(wifi_credentials::Column::OrgId.eq(org.org.id))
        .order_by_asc(wifi_credentials::Column::Ssid)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    let (skip, take) = page.slice(count as usize);
    let rows: Vec<_> = rows.into_iter().skip(skip).take(take).collect();
    let ids: Vec<uuid::Uuid> = rows.iter().filter_map(|r| r.secret_id).collect();
    let names = crate::services::secrets::store::names_of(&ctx.db, Some(org.org.id), &ids)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<WifiCredential> = rows
        .iter()
        .map(|r| wifi_dto(r, r.secret_id.and_then(|id| names.get(&id).cloned())))
        .collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/edge/wifi-credentials",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `POST /api/v1/edge/wifi-credentials` — **upsert** on (org, ssid)
/// case-sensitive: exists → 200 (password updated), absent → 201.
#[utoipa::path(
    post,
    path = "/api/v1/edge/wifi-credentials",
    tag = "edge",
    operation_id = "edge_wifi_upsert",
    summary = "Save a WiFi credential (upsert on SSID)",
    description = "Upsert on (org, SSID), case-sensitive: an existing SSID is updated (200), a new one created (201, password required). A typed password is stored in the vault secret `wifi/<ssid>/password` (owner/admin only); a member may pick an existing secret. Owner, admin or member.",
    request_body = pnex_core::WifiCredentialInput,
    responses(
        (status = 200, description = "Existing SSID updated", body = pnex_core::WifiCredential),
        (status = 201, description = "Created", body = pnex_core::WifiCredential),
        (status = 400, description = "Invalid field (`field-invalid`, `errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Picked secret not found (`secret-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Concurrent creation of the same SSID (`edge-ssid-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_wifi(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<WifiCredentialInput>,
) -> Result<Response> {
    if let Some(err) = validate_ssid(&input) {
        return Ok(err);
    }
    let ssid = input.ssid.trim().to_string();
    let password = password_input(&input);
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return vault_error(e),
    };
    let writer = crate::controllers::secrets::writer(&org);
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    let existing = WifiCredentials::find()
        .filter(wifi_credentials::Column::OrgId.eq(org.org.id))
        .filter(wifi_credentials::Column::Ssid.eq(&ssid))
        .one(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let (row, status) = match existing {
        Some(m) => (m, StatusCode::OK),
        None => {
            if password.is_none() {
                return Ok(password_required());
            }
            let am = wifi_credentials::ActiveModel {
                org_id: Set(org.org.id),
                ssid: Set(ssid.clone()),
                ..Default::default()
            };
            match am.insert(&txn).await {
                Ok(m) => (m, StatusCode::CREATED),
                // Concurrent upsert of the same SSID: the other one won.
                Err(e) if is_unique_violation(&e) => {
                    return Ok(conflict(
                        err_codes::EDGE_SSID_TAKEN,
                        "Another entry of this organization already uses this SSID.",
                    ))
                }
                Err(_) => return Err(Error::InternalServerError),
            }
        }
    };
    let secret = match crate::services::secrets::wifi::save(
        &txn,
        &ring,
        writer,
        org.can("secrets:write"),
        row.id,
        &ssid,
        None,
        row.secret_id,
        password.as_ref(),
    )
    .await
    {
        Ok(Some(secret)) => secret,
        Ok(None) => return Ok(password_required()),
        Err(e) => return vault_error(e),
    };
    let mut am: wifi_credentials::ActiveModel = row.into();
    am.secret_id = Set(Some(secret));
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    let saved = am
        .update(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    Ok((status, format::json(wifi_view(&ctx.db, &saved).await?)).into_response())
}

/// `PUT /api/v1/edge/wifi-credentials/{id}` — in-place edit (renaming
/// possible, 409 if the targeted ssid exists elsewhere); password absent =
/// unchanged.
#[utoipa::path(
    put,
    path = "/api/v1/edge/wifi-credentials/{id}",
    tag = "edge",
    operation_id = "edge_wifi_update",
    summary = "Update a WiFi credential",
    description = "In-place edit (rename allowed). Password absent or empty = unchanged. Owner, admin or member; typing a password value needs owner/admin.",
    params(("id" = i64, Path, description = "WiFi credential id")),
    request_body = pnex_core::WifiCredentialInput,
    responses(
        (status = 200, body = pnex_core::WifiCredential),
        (status = 400, description = "Invalid field (`field-invalid`, `errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown credential (or other org)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "SSID used by another entry (`edge-ssid-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_wifi(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<WifiCredentialInput>,
) -> Result<Response> {
    if let Some(err) = validate_ssid(&input) {
        return Ok(err);
    }
    let Some(m) = WifiCredentials::find()
        .filter(wifi_credentials::Column::OrgId.eq(org.org.id))
        .filter(wifi_credentials::Column::Id.eq(id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let ssid = input.ssid.trim().to_string();
    // Renaming: the targeted (org, ssid) already exists on ANOTHER row → 409
    // (the unique index would be violated).
    let dup = WifiCredentials::find()
        .filter(wifi_credentials::Column::OrgId.eq(org.org.id))
        .filter(wifi_credentials::Column::Ssid.eq(&ssid))
        .filter(wifi_credentials::Column::Id.ne(id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if dup.is_some() {
        return Ok(conflict(
            err_codes::EDGE_SSID_TAKEN,
            "Another entry of this organization already uses this SSID.",
        ));
    }
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return vault_error(e),
    };
    let password = password_input(&input);
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    let secret = match crate::services::secrets::wifi::save(
        &txn,
        &ring,
        crate::controllers::secrets::writer(&org),
        org.can("secrets:write"),
        m.id,
        &ssid,
        Some(&m.ssid),
        m.secret_id,
        password.as_ref(),
    )
    .await
    {
        Ok(Some(secret)) => secret,
        Ok(None) => return Ok(password_required()),
        Err(e) => return vault_error(e),
    };
    let mut am: wifi_credentials::ActiveModel = m.into();
    am.ssid = Set(ssid);
    am.secret_id = Set(Some(secret));
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    let saved = am
        .update(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    Ok((
        StatusCode::OK,
        format::json(wifi_view(&ctx.db, &saved).await?),
    )
        .into_response())
}

/// `DELETE /api/v1/edge/wifi-credentials/{id}` — 204; cross-org = 404
/// masked (the find is already filtered by org). Its dedicated secret goes
/// with it.
#[utoipa::path(
    delete,
    path = "/api/v1/edge/wifi-credentials/{id}",
    tag = "edge",
    operation_id = "edge_wifi_delete",
    summary = "Delete a WiFi credential",
    description = "Deletes the credential and releases its dedicated vault secret. Owner, admin or member.",
    params(("id" = i64, Path, description = "WiFi credential id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown credential (or other org)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_wifi(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(m) = WifiCredentials::find()
        .filter(wifi_credentials::Column::OrgId.eq(org.org.id))
        .filter(wifi_credentials::Column::Id.eq(id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    if let Err(e) = crate::services::secrets::wifi::release(&txn, org.org.id, m.id, &m.ssid).await {
        return vault_error(e);
    }
    m.into_active_model()
        .delete(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────────── Hosts ───────────────────────────────

// Devices always connect over wss through the edge (D154): a host has no
// scheme flag.

fn host_dto(m: &pnex_hosts::Model) -> PnexHost {
    PnexHost {
        id: m.id,
        org_id: m.org_id,
        host: m.host.clone(),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

fn validate_host(input: &PnexHostInput) -> Option<Response> {
    let host = input.host.trim();
    if host.is_empty() {
        return Some(field_status("host", err_codes::FIELD_REQUIRED));
    }
    if host.len() > 255 {
        return Some(field_status(
            "host",
            &format!("{}:255", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    // `CreateBuild.pnex_host` is a bare host: no whitespace, no scheme
    // (devices always connect over wss).
    if host.contains(char::is_whitespace) || host.contains("://") {
        return Some(field_status("host", err_codes::FIELD_INVALID));
    }
    None
}

/// `GET /api/v1/edge/hosts` — the org's references (host ASC).
#[utoipa::path(
    get,
    path = "/api/v1/edge/hosts",
    tag = "edge",
    operation_id = "edge_hosts_list",
    summary = "List the saved PNeX server hosts",
    description = "Server hosts of the org that devices connect to (over wss), ordered by host, paginated (`limit`/`offset`). Any org member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::PnexHost>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_hosts(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = PnexHosts::find()
        .filter(pnex_hosts::Column::OrgId.eq(org.org.id))
        .order_by_asc(pnex_hosts::Column::Host)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    let (skip, take) = page.slice(count as usize);
    let results: Vec<PnexHost> = rows.iter().map(host_dto).skip(skip).take(take).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/edge/hosts",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `GET /api/v1/edge/hosts/locked` — server host imposed by the deployment,
/// if any. The UI then replaces the host picker with a read-only display.
#[utoipa::path(
    get,
    path = "/api/v1/edge/hosts/locked",
    tag = "edge",
    operation_id = "edge_hosts_locked",
    summary = "Server host imposed by the deployment",
    description = "Host imposed by `PNEX_PROD_HOST`, if any; when set, the host referential is read-only and builds always target it. Any org member.",
    responses(
        (status = 200, body = pnex_core::LockedHost)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn locked_host(_org: OrgContext) -> Result<Response> {
    format::json(LockedHost {
        host: crate::app::prod_host(),
    })
}

/// `POST /api/v1/edge/hosts` — **upsert** on (org, host): exists → 200
/// (unchanged), absent → 201.
#[utoipa::path(
    post,
    path = "/api/v1/edge/hosts",
    tag = "edge",
    operation_id = "edge_hosts_upsert",
    summary = "Save a PNeX server host (upsert)",
    description = "Upsert on (org, host): existing host returned unchanged (200), new one created (201). Bare host, no scheme, no whitespace, max 255 chars. Owner, admin or member.",
    request_body = pnex_core::PnexHostInput,
    responses(
        (status = 200, description = "Already saved", body = pnex_core::PnexHost),
        (status = 201, description = "Created", body = pnex_core::PnexHost),
        (status = 400, description = "Invalid field (`field-invalid`, `errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Host imposed by the deployment (`edge-host-locked`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_host(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<PnexHostInput>,
) -> Result<Response> {
    if let Some(err) = host_locked() {
        return Err(err);
    }
    if let Some(err) = validate_host(&input) {
        return Ok(err);
    }
    let host = input.host.trim();
    let existing = PnexHosts::find()
        .filter(pnex_hosts::Column::OrgId.eq(org.org.id))
        .filter(pnex_hosts::Column::Host.eq(host))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if let Some(m) = existing {
        return Ok((StatusCode::OK, format::json(host_dto(&m))).into_response());
    }
    let am = pnex_hosts::ActiveModel {
        org_id: Set(org.org.id),
        host: Set(host.to_string()),
        ..Default::default()
    };
    match am.insert(&ctx.db).await {
        Ok(saved) => Ok((StatusCode::CREATED, format::json(host_dto(&saved))).into_response()),
        // Upsert race (see create_wifi).
        Err(e) if is_unique_violation(&e) => {
            let m = PnexHosts::find()
                .filter(pnex_hosts::Column::OrgId.eq(org.org.id))
                .filter(pnex_hosts::Column::Host.eq(host))
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?
                .ok_or(Error::InternalServerError)?;
            Ok((StatusCode::OK, format::json(host_dto(&m))).into_response())
        }
        Err(_) => Err(Error::InternalServerError),
    }
}

/// `PUT /api/v1/edge/hosts/{id}` — in-place update (id kept):
/// host renaming. (org, host) conflict with ANOTHER
/// entry → 409.
#[utoipa::path(
    put,
    path = "/api/v1/edge/hosts/{id}",
    tag = "edge",
    operation_id = "edge_hosts_update",
    summary = "Rename a PNeX server host",
    description = "In-place edit, id kept. Owner, admin or member.",
    params(("id" = i64, Path, description = "Host id")),
    request_body = pnex_core::PnexHostInput,
    responses(
        (status = 200, body = pnex_core::PnexHost),
        (status = 400, description = "Invalid field (`field-invalid`, `errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown host (or other org)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Host imposed by the deployment (`edge-host-locked`) or used by another entry (`edge-host-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_host(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<PnexHostInput>,
) -> Result<Response> {
    if let Some(err) = host_locked() {
        return Err(err);
    }
    if let Some(err) = validate_host(&input) {
        return Ok(err);
    }
    let Some(m) = PnexHosts::find()
        .filter(pnex_hosts::Column::OrgId.eq(org.org.id))
        .filter(pnex_hosts::Column::Id.eq(id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let host = input.host.trim();
    let dup = PnexHosts::find()
        .filter(pnex_hosts::Column::OrgId.eq(org.org.id))
        .filter(pnex_hosts::Column::Host.eq(host))
        .filter(pnex_hosts::Column::Id.ne(id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if dup.is_some() {
        return Ok(conflict(
            err_codes::EDGE_HOST_TAKEN,
            "Another entry of this organization already uses this host.",
        ));
    }
    let mut am: pnex_hosts::ActiveModel = m.into();
    am.host = Set(host.to_string());
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    let saved = am
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::OK, format::json(host_dto(&saved))).into_response())
}

/// `DELETE /api/v1/edge/hosts/{id}` — 204; cross-org = 404 masked.
#[utoipa::path(
    delete,
    path = "/api/v1/edge/hosts/{id}",
    tag = "edge",
    operation_id = "edge_hosts_delete",
    summary = "Delete a PNeX server host",
    description = "Owner, admin or member.",
    params(("id" = i64, Path, description = "Host id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown host (or other org)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_host(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(m) = PnexHosts::find()
        .filter(pnex_hosts::Column::OrgId.eq(org.org.id))
        .filter(pnex_hosts::Column::Id.eq(id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    m.into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────────── Shared ──────────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// Upsert race: two concurrent POSTs pass the find together, the unique
/// index fails the INSERT of one of them.
fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    e.to_string().contains("duplicate key")
}

// ───────────────────────── Scan LAN (server-side) ─────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct LanScanQuery {
    /// Explicit private `a.b.c.` prefix (trailing dot); otherwise automatic detection.
    prefix: Option<String>,
}

/// `GET /api/v1/edge/lan-scan` — probes the /24 of the server's private
/// interfaces (or the explicit `?prefix=` range) and returns the servers
/// identifying as `pnex-server` (contract meta probe, not a mere open
/// port). Auth required, read open to all roles: the scan reveals
/// nothing the server's network does not already expose.
///
/// Default range (no `?prefix=`): the /24 of the address the user reached
/// the app at (`X-Forwarded-Host` / `Host`, e.g. `192.168.1.185` behind the
/// edge), plus `PNEX_LAN_PREFIX` (comma-separated `a.b.c.`), plus the
/// server's own private interfaces — except in a container, whose
/// interfaces are Docker bridges (172.x) devices can never reach.
#[utoipa::path(
    get,
    path = "/api/v1/edge/lan-scan",
    tag = "edge",
    operation_id = "edge_lan_scan",
    summary = "Scan the LAN for PNeX servers",
    description = "Server-side probe of private /24 ranges (the explicit `prefix`, else the /24 of the host the user reached, `PNEX_LAN_PREFIX` and the server private interfaces) on port 5150 and 443; only servers identifying as `pnex-server` through the meta endpoint are returned. An explicit `prefix` must be an RFC 1918 `a.b.c.` prefix. Public ranges are never scanned. Can take several seconds. Owner, admin or member.",
    params(LanScanQuery),
    responses(
        (status = 200, body = pnex_core::LanScanResult),
        (status = 400, description = "`prefix` is not an RFC 1918 `a.b.c.` prefix (`field-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn lan_scan(
    // `edge:test`, checked by the extractor (SEC-43).
    _org: OrgContext,
    headers: HeaderMap,
    Query(q): Query<LanScanQuery>,
) -> Result<Response> {
    // SEC-43: probing the server network is user-driven reconnaissance,
    // kept to the roles that manage the edge referentials (R2).
    let explicit = q.prefix.map(|p| p.trim().to_string());
    if explicit.as_deref().is_some_and(|p| !valid_prefix(p)) {
        return Ok(field_status("prefix", err_codes::FIELD_INVALID));
    }
    let prefixes: Vec<String> = match explicit {
        Some(p) => vec![p],
        None => default_prefixes(
            request_host(&headers).as_deref(),
            std::env::var("PNEX_LAN_PREFIX").ok().as_deref(),
            in_container(),
            private_prefixes,
        ),
    }
    .into_iter()
    .filter(|p| valid_prefix(p))
    .collect();
    // Guard: never probe a public range by accident.
    if prefixes.is_empty() {
        return Ok(format::json(LanScanResult {
            prefixes: vec![],
            hits: vec![],
        })
        .into_response());
    }
    // Two doors per address: the backend port (bare-metal / dev stack) and
    // the TLS edge on 443 (Docker install: only the edge is published).
    let mut candidates: Vec<String> = prefixes
        .iter()
        .flat_map(|p| {
            (1..=254).flat_map(move |i| {
                [
                    format!("http://{p}{i}:{SCAN_PORT}"),
                    format!("https://{p}{i}"),
                ]
            })
        })
        .collect();
    candidates.sort();
    candidates.dedup();

    // Discovery only: an unauthenticated GET of the public version info.
    // A local edge serves a certificate of its own CA, unknown here.
    let client = reqwest::Client::builder()
        .timeout(SCAN_TIMEOUT)
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::InternalServerError)?;
    let mut hits = futures_util::stream::iter(candidates)
        .map(|url| {
            // reqwest::Client is an internal Arc: cloning it per future is
            // negligible and makes the closure FnMut.
            let client = client.clone();
            async move { probe_hit(&client, &url).await }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .filter_map(|hit| async move { hit })
        .collect::<Vec<_>>()
        .await;
    hits.sort_by(|a, b| a.host.cmp(&b.host));
    // Same server through both doors: one row.
    hits.dedup_by(|a, b| a.host == b.host);

    Ok(format::json(LanScanResult { prefixes, hits }).into_response())
}

/// Probes `http://{ip}:{SCAN_PORT}{META_VERSION_PATH}` — keeps only the
/// servers whose contract `service` field is `pnex-server`.
async fn probe_hit(client: &reqwest::Client, base: &str) -> Option<LanScanHit> {
    let url = format!("{base}{META_VERSION_PATH}");
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let info: ServerInfo = response.json().await.ok()?;
    // The meta probe hits the backend port, but devices reach the server
    // through the TLS edge on 443 (D70): the registered host is the bare IP.
    let ip = base
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches(&format!(":{SCAN_PORT}"));
    (info.service == SERVICE).then(|| LanScanHit {
        host: ip.to_string(),
        version: info.version,
        contract: info.contract,
    })
}

/// Host the browser reached the app at, without port: `X-Forwarded-Host`
/// (edge) first, then `Host`.
fn request_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))?
        .to_str()
        .ok()?;
    let first = raw.split(',').next()?.trim();
    // `[v6]:port` and `v4:port` → bare host.
    let host = if let Some(rest) = first.strip_prefix('[') {
        rest.split(']').next()?
    } else {
        first.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// `/.dockerenv` (Docker) or `/run/.containerenv` (Podman).
fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
}

/// Default scan ranges, in order: the request host's /24 (when it is a
/// private IPv4), `PNEX_LAN_PREFIX`, then the server's interfaces — the
/// latter skipped in a container unless nothing else is known.
fn default_prefixes(
    host: Option<&str>,
    env: Option<&str>,
    container: bool,
    interfaces: impl FnOnce() -> Vec<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(ip) = host.and_then(|h| h.parse::<Ipv4Addr>().ok()) {
        if ip.is_private() {
            let o = ip.octets();
            out.push(format!("{}.{}.{}.", o[0], o[1], o[2]));
        }
    }
    for p in env.unwrap_or_default().split(',') {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let p = if p.ends_with('.') {
            p.to_string()
        } else {
            format!("{p}.")
        };
        if !out.contains(&p) {
            out.push(p);
        }
    }
    if !container || out.is_empty() {
        for p in interfaces() {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// "a.b.c." prefixes of the server's private IPv4s (auto-detection). On a
/// containerized deployment (bridge 172.x), the range seen is the bridge —
/// see `default_prefixes`.
fn private_prefixes() -> Vec<String> {
    let mut prefixes: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .iter()
        .filter_map(|iface| match iface.ip() {
            std::net::IpAddr::V4(ip) if ip.is_private() => {
                let octets = ip.octets();
                Some(format!("{}.{}.{}.", octets[0], octets[1], octets[2]))
            }
            _ => None,
        })
        .collect();
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

/// Strict `a.b.c.` + RFC 1918 guard (`Ipv4Addr::is_private`: 10/8,
/// 172.16/12, 192.168/16): the prefix must end with a dot and compose a
/// valid address (`192.168.1` without the dot would compose 192.168.10 — a
/// different range).
fn valid_prefix(prefix: &str) -> bool {
    prefix.ends_with('.')
        && format!("{prefix}0")
            .parse::<Ipv4Addr>()
            .map(|ip| ip.is_private())
            .unwrap_or(false)
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list_wifi,
        create_wifi,
        update_wifi,
        delete_wifi,
        list_hosts,
        locked_host,
        create_host,
        update_host,
        delete_host,
        lan_scan
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Vec<String> {
        vec!["172.18.0.".into()]
    }

    #[test]
    fn container_scans_the_lan_the_user_came_from() {
        // Docker install reached at 192.168.1.185: the bridge is skipped.
        assert_eq!(
            default_prefixes(Some("192.168.1.185"), None, true, none),
            vec!["192.168.1.".to_string()]
        );
        // Bare metal: the interfaces are scanned too.
        assert_eq!(
            default_prefixes(Some("192.168.1.185"), None, false, none),
            vec!["192.168.1.".to_string(), "172.18.0.".to_string()]
        );
    }

    #[test]
    fn env_prefix_and_fallbacks() {
        // Reached by name (pnex.local): PNEX_LAN_PREFIX gives the range.
        assert_eq!(
            default_prefixes(Some("pnex.local"), Some("10.0.0, 192.168.1."), true, none),
            vec!["10.0.0.".to_string(), "192.168.1.".to_string()]
        );
        // Nothing known in a container: the interfaces, rather than nothing.
        assert_eq!(
            default_prefixes(Some("pnex.local"), None, true, none),
            vec!["172.18.0.".to_string()]
        );
        // A public address is never turned into a range.
        assert_eq!(
            default_prefixes(Some("8.8.8.8"), None, true, Vec::new),
            Vec::<String>::new()
        );
    }

    #[test]
    fn explicit_prefix_must_be_rfc1918() {
        for ok in ["10.0.0.", "172.16.5.", "172.31.255.", "192.168.1."] {
            assert!(valid_prefix(ok), "{ok}");
        }
        for bad in [
            "8.8.8.",
            "172.32.0.",
            "100.64.0.",
            "127.0.0.",
            "169.254.1.",
            "192.168.1",
            "192.168.",
            "a.b.c.",
            "",
        ] {
            assert!(!valid_prefix(bad), "{bad}");
        }
    }

    #[test]
    fn request_host_strips_port_and_prefers_forwarded() {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::HOST, "10.1.2.3:5150".parse().unwrap());
        assert_eq!(request_host(&h).as_deref(), Some("10.1.2.3"));
        h.insert("x-forwarded-host", "192.168.1.185".parse().unwrap());
        assert_eq!(request_host(&h).as_deref(), Some("192.168.1.185"));
    }
}
