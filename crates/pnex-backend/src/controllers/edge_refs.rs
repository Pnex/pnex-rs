//! Référentiels Edge — CRUD org-scoped des credentials WiFi et des hosts
//! serveur PNeX. Alimente l'étape Config du wizard device (fin de la
//! ressaisie à chaque ajout de device ; cf. `pnex_core::edge`).
//!
//! École `controllers/notify.rs` : scoping org (404 masqué cross-org),
//! écriture gated `can_write()`, 400 champ-par-champ. **Divergence assumée**
//! vs notify : le POST est un **upsert** (200/201, jamais 409) — le (org,
//! ssid) / (org, host) est la clé naturelle et l'ajout inline depuis le
//! wizard ne doit jamais confler : re-sauver = mettre à jour le mot de
//! passe / le flag ws_ssl.
//!
//! The WiFi password lives in the secrets vault (secrets.md S6): only its
//! reference is returned, never the value.

use std::net::Ipv4Addr;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
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

/// Port meta du serveur PNeX (binding par défaut, config/*.yaml) — cible du
/// scan LAN.
const SCAN_PORT: u16 = 5150;
/// Timeout par sonde : une IP injoignable doit libérer le slot vite.
/// (900 ms : SYN-drop typique, borne serrée sans abuser les faux négatifs.)
const SCAN_TIMEOUT: Duration = Duration::from_millis(900);
/// Parallélisme : 96 sondes simultanées — un hôte multi-interfaces
/// (docker bridges, virbr) scanne plusieurs /24 : 1270 candidats ≈ 12 s
/// pire cas, un /24 seul ≈ 2,5 s.
const SCAN_CONCURRENCY: usize = 96;

/// 400 champ-par-champ (école notify).
fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Writes: owner/admin/member (`OrgContext::can_write`). Carries the machine code +
/// canonical English description (frontend resolves `err-<code>`, verbatim
/// fallback otherwise).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
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

/// 409 — la clé référentiel visée est déjà prise par une AUTRE entrée
/// (l'édition in-place renomme : le (org, ssid) / (org, host) visé existe
/// déjà sur une autre ligne).
fn conflict(field: &str, msg: &str) -> Response {
    (
        StatusCode::CONFLICT,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
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

/// Password part of an input: a pick, a typed value (`wifi_password` is
/// the legacy typed form), or `None` = unchanged.
fn password_input(input: &WifiCredentialInput) -> Option<SecretFieldInput> {
    match &input.password {
        Some(SecretFieldInput::Value { value }) if value.is_empty() => None,
        Some(p) => Some(p.clone()),
        None if !input.wifi_password.is_empty() => Some(SecretFieldInput::Value {
            value: input.wifi_password.clone(),
        }),
        None => None,
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

/// Réseaux ouverts hors scope v1 : a saved entry always has a password.
fn password_required() -> Response {
    field_status("wifi_password", err_codes::FIELD_REQUIRED)
}

fn vault_error(e: crate::services::secrets::store::StoreError) -> Result<Response> {
    crate::controllers::secrets::store_error(e)
}

/// `GET /api/v1/edge/wifi-credentials` — référentiel de l'org (ssid ASC).
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

/// `POST /api/v1/edge/wifi-credentials` — **upsert** sur (org, ssid)
/// case-sensitive : existe → 200 (mdp mis à jour), absent → 201.
async fn create_wifi(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<WifiCredentialInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "edge-write-forbidden",
            "Owner, admin or member role required to manage edge references.",
        ));
    }
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
                        "ssid",
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
        org.can_manage_secrets(),
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

/// `PUT /api/v1/edge/wifi-credentials/{id}` — édition in-place (renommage
/// possible, 409 si le ssid visé existe ailleurs) ; password absent =
/// unchanged.
async fn update_wifi(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<WifiCredentialInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "edge-write-forbidden",
            "Owner, admin or member role required to manage edge references.",
        ));
    }
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
    // Renommage : le (org, ssid) visé existe déjà sur une AUTRE ligne → 409
    // (l'index unique serait violé).
    let dup = WifiCredentials::find()
        .filter(wifi_credentials::Column::OrgId.eq(org.org.id))
        .filter(wifi_credentials::Column::Ssid.eq(&ssid))
        .filter(wifi_credentials::Column::Id.ne(id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if dup.is_some() {
        return Ok(conflict(
            "ssid",
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
        org.can_manage_secrets(),
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

/// `DELETE /api/v1/edge/wifi-credentials/{id}` — 204 ; cross-org = 404
/// masqué (le find est déjà filtré par org). Its dedicated secret goes
/// with it.
async fn delete_wifi(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "edge-write-forbidden",
            "Owner, admin or member role required to manage edge references.",
        ));
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

// Devices always connect over TLS through the edge (D70): `ws_ssl` is
// forced to true on every write, whatever the client sends.

fn host_dto(m: &pnex_hosts::Model) -> PnexHost {
    PnexHost {
        id: m.id,
        org_id: m.org_id,
        host: m.host.clone(),
        ws_ssl: m.ws_ssl,
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
    // Le contrat `CreateBuild.pnex_host` est un hôte nu : pas d'espace
    // (école builds.rs) ni de schéma (`ws://` est porté par ws_ssl).
    if host.contains(char::is_whitespace) {
        return Some(field_status("host", "Aucun espace autorisé."));
    }
    if host.contains("://") {
        return Some(field_status(
            "host",
            "Do not include the scheme — devices always connect over wss://.",
        ));
    }
    None
}

/// `GET /api/v1/edge/hosts` — référentiel de l'org (host ASC).
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
async fn locked_host(_org: OrgContext) -> Result<Response> {
    format::json(LockedHost {
        host: crate::app::prod_host(),
    })
}

/// `POST /api/v1/edge/hosts` — **upsert** sur (org, host) : existe → 200
/// (ws_ssl mis à jour), absent → 201.
async fn create_host(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<PnexHostInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "edge-write-forbidden",
            "Owner, admin or member role required to manage edge references.",
        ));
    }
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
        let mut am: pnex_hosts::ActiveModel = m.into();
        am.ws_ssl = Set(true);
        let saved = am
            .update(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        return Ok((StatusCode::OK, format::json(host_dto(&saved))).into_response());
    }
    let am = pnex_hosts::ActiveModel {
        org_id: Set(org.org.id),
        host: Set(host.to_string()),
        ws_ssl: Set(true),
        ..Default::default()
    };
    match am.insert(&ctx.db).await {
        Ok(saved) => Ok((StatusCode::CREATED, format::json(host_dto(&saved))).into_response()),
        // Course d'upsert (cf. create_wifi).
        Err(e) if is_unique_violation(&e) => {
            let m = PnexHosts::find()
                .filter(pnex_hosts::Column::OrgId.eq(org.org.id))
                .filter(pnex_hosts::Column::Host.eq(host))
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?
                .ok_or(Error::InternalServerError)?;
            let mut am: pnex_hosts::ActiveModel = m.into();
            am.ws_ssl = Set(true);
            let saved = am
                .update(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?;
            Ok((StatusCode::OK, format::json(host_dto(&saved))).into_response())
        }
        Err(_) => Err(Error::InternalServerError),
    }
}

/// `PUT /api/v1/edge/hosts/{id}` — mise à jour in-place (id conservé) :
/// renommage d'hôte et/ou flag ws_ssl. Conflit (org, host) avec une AUTRE
/// entrée → 409.
async fn update_host(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<PnexHostInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "edge-write-forbidden",
            "Owner, admin or member role required to manage edge references.",
        ));
    }
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
            "host",
            "Another entry of this organization already uses this host.",
        ));
    }
    let mut am: pnex_hosts::ActiveModel = m.into();
    am.host = Set(host.to_string());
    am.ws_ssl = Set(true);
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    let saved = am
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::OK, format::json(host_dto(&saved))).into_response())
}

/// `DELETE /api/v1/edge/hosts/{id}` — 204 ; cross-org = 404 masqué.
async fn delete_host(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "edge-write-forbidden",
            "Owner, admin or member role required to manage edge references.",
        ));
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
    m.into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────────── Shared ──────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    limit: Option<String>,
    offset: Option<String>,
}

/// Course d'upsert : 2 POST concurrents passent le find ensemble, l'index
/// unique fait échouer l'INSERT de l'un — portable PG (`duplicate key`) +
/// sqlite (`UNIQUE constraint failed`).
fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    let msg = e.to_string();
    msg.contains("duplicate key") || msg.contains("UNIQUE constraint failed")
}

// ───────────────────────── Scan LAN (server-side) ─────────────────────

#[derive(Debug, Default, Deserialize)]
struct LanScanQuery {
    /// Préfixe « a.b.c. » explicite — sinon auto-détection des interfaces
    /// privées du serveur.
    prefix: Option<String>,
}

/// `GET /api/v1/edge/lan-scan` — sonde les /24 des interfaces privées du
/// serveur (ou la plage `?prefix=` explicite) et retourne les serveurs
/// s'identifiant `pnex-server` (sonde meta du contrat, pas un simple port
/// ouvert). Auth requis, lecture ouverte à tous les rôles : le scan ne
/// révèle rien que le réseau du serveur n'expose déjà.
///
/// Déploiement conteneurisé : un conteneur bridge voit ses interfaces
/// 172.x — passer `?prefix=192.168.1.` (l'outbound vers le LAN traverse le
/// NAT docker) ou basculer le service en `network_mode: host`.
async fn lan_scan(_org: OrgContext, Query(q): Query<LanScanQuery>) -> Result<Response> {
    let prefixes: Vec<String> = match q.prefix {
        Some(p) => vec![p],
        None => private_prefixes(),
    }
    .into_iter()
    .filter(|p| valid_prefix(p))
    .collect();
    // Garde : jamais sonder une plage publique par inadvertance.
    if prefixes.is_empty() {
        return Ok(format::json(LanScanResult {
            prefixes: vec![],
            hits: vec![],
        })
        .into_response());
    }
    let mut candidates: Vec<String> = prefixes
        .iter()
        .flat_map(|p| (1..=254).map(move |i| format!("http://{p}{i}:{SCAN_PORT}")))
        .collect();
    candidates.sort();
    candidates.dedup();

    let client = reqwest::Client::builder()
        .timeout(SCAN_TIMEOUT)
        .build()
        .map_err(|_| Error::InternalServerError)?;
    let mut hits = futures_util::stream::iter(candidates)
        .map(|url| {
            // reqwest::Client est un Arc interne : cloner par future est
            // négligeable et rend la closure FnMut.
            let client = client.clone();
            async move { probe_hit(&client, &url).await }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .filter_map(|hit| async move { hit })
        .collect::<Vec<_>>()
        .await;
    hits.sort_by(|a, b| a.host.cmp(&b.host));

    Ok(format::json(LanScanResult { prefixes, hits }).into_response())
}

/// Sonde `http://{ip}:{SCAN_PORT}{META_VERSION_PATH}` — ne retient que les
/// serveurs dont le champ `service` du contrat vaut `pnex-server`.
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
        .trim_end_matches(&format!(":{SCAN_PORT}"));
    (info.service == SERVICE).then(|| LanScanHit {
        host: ip.to_string(),
        version: info.version,
        contract: info.contract,
    })
}

/// Préfixes « a.b.c. » des IPv4 privées du serveur (auto-détection). Sur un
/// déploiement conteneurisé (bridge 172.x), la plage vue est le bridge —
/// cf. `?prefix=` du handler.
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

/// Garde stricte « a.b.c. » + IPv4 privée : le préfixe doit finir par un
/// point et composer une adresse valide (`192.168.1` sans point final
/// composerait 192.168.10 — plage différente).
fn valid_prefix(prefix: &str) -> bool {
    prefix.ends_with('.')
        && format!("{prefix}0")
            .parse::<Ipv4Addr>()
            .map(|ip| ip.is_private())
            .unwrap_or(false)
}
