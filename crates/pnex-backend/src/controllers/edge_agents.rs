//! Edge agent management (D95, docs/architecture/edge-agent.md).
//!
//! Org-scoped (owner/admin/member for writes):
//! - `GET|PATCH /api/v1/devices/{id}/agent` — overview + distinct keys quota;
//! - `GET /api/v1/devices/{id}/agent-keys` — keys discovered from the agent
//!   (free-form ingestion, no catalogue);
//! - `PATCH|DELETE /api/v1/devices/{id}/agent-keys/{key_id}` — per-key
//!   OpenObserve recording toggle / forget a stray key;
//! - `POST /api/v1/devices/{id}/agent-enrollment` — fresh single-use code.
//!
//! Public (no user session — the agent has none):
//! - `POST /api/v1/agent/enroll` — code → credentials (rotates token + key),
//!   rate-limited per client address (`services::rate_limit`, cross-pod);
//! - `GET /api/v1/agent/download/{target}`, `/SHA256SUMS`, `/install.sh`,
//!   `/install.ps1` — binaries shipped with this server + install scripts.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use sha2::{Digest, Sha256};

use crate::auth::OrgContext;
use crate::models::_entities::{
    agent_enrollments, agent_keys, device_registries, device_tokens, predefined_devices,
};
use pnex_core::agent::{
    agent_file_of, normalize_enroll_code, AgentEnrollRequest, AgentEnrollResponse,
    AgentEnrollmentCreated, AgentInfo, AgentKey, AgentKeyPatch, AgentSettingsPatch,
    AGENT_ENROLL_TTL_SECS,
};

/// Unambiguous alphabet of enrollment codes (no 0/O, 1/I/L, U).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTVWXYZ23456789";
/// Raw code length (display: 3 groups of 4).
const CODE_LEN: usize = 12;
/// Distinct keys quota bounds.
const MAX_KEYS_RANGE: std::ops::RangeInclusive<i32> = 1..=100_000;

const INSTALL_SH: &str = include_str!("../../../../deploy/agent/install.sh");
const INSTALL_PS1: &str = include_str!("../../../../deploy/agent/install.ps1");

fn agent_error(status: StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn forbidden() -> Error {
    agent_error(
        StatusCode::FORBIDDEN,
        "agent-write-forbidden",
        "Owner, admin or member role required to manage edge agents.",
    )
}

/// Hex SHA-256.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Org agent by registry id — `NotFound` outside the org, 400 when the
/// device is not an edge agent.
async fn find_agent(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<device_registries::Model> {
    let Some((device, Some(pd))) = device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .find_also_related(predefined_devices::Entity)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    if pd.name != pnex_core::EDGE_AGENT_PREDEF {
        return Err(agent_error(
            StatusCode::BAD_REQUEST,
            "agent-not-an-agent",
            "This device is not an edge agent.",
        ));
    }
    Ok(device)
}

/// `true` when the registry row is an edge agent (guards of firmware/OTA/pins).
pub(crate) async fn is_agent(db: &DatabaseConnection, device: &device_registries::Model) -> bool {
    predefined_devices::Entity::find_by_id(device.predefined_device_id)
        .one(db)
        .await
        .ok()
        .flatten()
        .is_some_and(|pd| pd.name == pnex_core::EDGE_AGENT_PREDEF)
}

/// 400 `agent-unsupported-action` (firmware build, OTA, pins on an agent).
pub(crate) fn unsupported_action() -> Error {
    agent_error(
        StatusCode::BAD_REQUEST,
        "agent-unsupported-action",
        "This action does not apply to an edge agent.",
    )
}

fn key_dto(r: agent_keys::Model) -> AgentKey {
    AgentKey {
        id: r.id,
        key: r.key,
        unit: r.unit,
        kind: r.kind,
        record_o2: r.record_o2,
        first_seen_at: r.first_seen_at.to_rfc3339(),
        last_seen_at: r.last_seen_at.to_rfc3339(),
    }
}

async fn info_of(db: &DatabaseConnection, device: &device_registries::Model) -> Result<AgentInfo> {
    let last = agent_enrollments::Entity::find()
        .filter(agent_enrollments::Column::DeviceRegistryId.eq(device.id))
        .filter(agent_enrollments::Column::UsedAt.is_not_null())
        .order_by_desc(agent_enrollments::Column::UsedAt)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(AgentInfo {
        max_keys: device.max_unique_measurements,
        agent_version: device.fw_version.clone(),
        hostname: last.as_ref().and_then(|e| e.hostname.clone()),
        os: last.as_ref().and_then(|e| e.os.clone()),
        arch: last.as_ref().and_then(|e| e.arch.clone()),
        enrolled_at: last
            .as_ref()
            .and_then(|e| e.used_at.map(|t| t.to_rfc3339())),
    })
}

// ───────────────────────────── Org-scoped ─────────────────────────────

async fn info(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let device = find_agent(&ctx.db, &org, id).await?;
    format::json(info_of(&ctx.db, &device).await?)
}

async fn update_settings(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(body): Json<AgentSettingsPatch>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    if !MAX_KEYS_RANGE.contains(&body.max_keys) {
        return Err(agent_error(
            StatusCode::BAD_REQUEST,
            "agent-quota-invalid",
            "The key quota must be between 1 and 100000.",
        ));
    }
    let device = find_agent(&ctx.db, &org, id).await?;
    let mut active: device_registries::ActiveModel = device.into();
    active.max_unique_measurements = Set(body.max_keys);
    let device = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(info_of(&ctx.db, &device).await?)
}

async fn list_keys(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let device = find_agent(&ctx.db, &org, id).await?;
    let rows = agent_keys::Entity::find()
        .filter(agent_keys::Column::DeviceRegistryId.eq(device.id))
        .order_by_asc(agent_keys::Column::Key)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(rows.into_iter().map(key_dto).collect::<Vec<_>>())
}

async fn find_key(
    db: &DatabaseConnection,
    device_id: i64,
    key_id: i64,
) -> Result<agent_keys::Model> {
    agent_keys::Entity::find_by_id(key_id)
        .filter(agent_keys::Column::DeviceRegistryId.eq(device_id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)
}

async fn patch_key(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, key_id)): Path<(i64, i64)>,
    Json(body): Json<AgentKeyPatch>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let device = find_agent(&ctx.db, &org, id).await?;
    let row = find_key(&ctx.db, device.id, key_id).await?;
    let mut active: agent_keys::ActiveModel = row.into();
    active.record_o2 = Set(body.record_o2);
    active.updated_at = Set(chrono::Utc::now().fixed_offset());
    let row = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(key_dto(row))
}

async fn delete_key(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, key_id)): Path<(i64, i64)>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let device = find_agent(&ctx.db, &org, id).await?;
    let row = find_key(&ctx.db, device.id, key_id).await?;
    let name = row.key.clone();
    agent_keys::Entity::delete_by_id(row.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Keep the metric pickers mirror in sync.
    if let Some(serde_json::Value::Object(mut map)) = device.discovered_measurements.clone() {
        map.remove(&name);
        let mut active: device_registries::ActiveModel = device.into();
        active.discovered_measurements = Set(Some(serde_json::Value::Object(map)));
        let _ = active.update(&ctx.db).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Random code over [`CODE_ALPHABET`] (raw form, no separators).
fn new_code() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..CODE_LEN)
        .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// `ABCDEFGHJKMN` → `ABCD-EFGH-JKMN`.
fn display_code(raw: &str) -> String {
    raw.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

async fn create_enrollment(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let device = find_agent(&ctx.db, &org, id).await?;
    let now = chrono::Utc::now();
    // One pending code per agent: older unused codes are revoked.
    agent_enrollments::Entity::delete_many()
        .filter(agent_enrollments::Column::DeviceRegistryId.eq(device.id))
        .filter(agent_enrollments::Column::UsedAt.is_null())
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let code = new_code();
    let expires_at = now + chrono::Duration::seconds(AGENT_ENROLL_TTL_SECS);
    agent_enrollments::ActiveModel {
        org_id: Set(org.org.id),
        device_registry_id: Set(device.id),
        code_hash: Set(sha256_hex(code.as_bytes())),
        expires_at: Set(expires_at.fixed_offset()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    let dto = AgentEnrollmentCreated {
        code: display_code(&code),
        expires_at: expires_at.to_rfc3339(),
        ca_sha256: super::meta::local_ca_pem().map(|pem| sha256_hex(pem.as_bytes())),
    };
    Ok((StatusCode::CREATED, format::json(dto)).into_response())
}

// ───────────────────────────── Public ─────────────────────────────

fn clip(v: Option<String>, max: usize) -> Option<String> {
    v.map(|s| s.trim().chars().take(max).collect::<String>())
        .filter(|s| !s.is_empty())
}

async fn enroll(
    State(ctx): State<AppContext>,
    Json(body): Json<AgentEnrollRequest>,
) -> Result<Response> {
    // Per-client attempts are bounded cross-pod by the rate-limit layer
    // (`services::rate_limit`, rule `agent-enroll`).
    let invalid = || {
        agent_error(
            StatusCode::BAD_REQUEST,
            "agent-enroll-code-invalid",
            "Invalid, used or expired enrollment code.",
        )
    };
    let code = normalize_enroll_code(&body.code);
    if code.len() != CODE_LEN {
        return Err(invalid());
    }
    let now = chrono::Utc::now().fixed_offset();
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Atomic consumption: only an unused, unexpired code flips to used.
    let consumed = agent_enrollments::Entity::update_many()
        .col_expr(agent_enrollments::Column::UsedAt, now.into())
        .col_expr(
            agent_enrollments::Column::Hostname,
            clip(body.hostname, 255).into(),
        )
        .col_expr(agent_enrollments::Column::Os, clip(body.os, 32).into())
        .col_expr(agent_enrollments::Column::Arch, clip(body.arch, 32).into())
        .col_expr(
            agent_enrollments::Column::AgentVersion,
            clip(body.agent_version, 32).into(),
        )
        .filter(agent_enrollments::Column::CodeHash.eq(sha256_hex(code.as_bytes())))
        .filter(agent_enrollments::Column::UsedAt.is_null())
        .filter(agent_enrollments::Column::ExpiresAt.gt(now))
        .exec(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if consumed.rows_affected != 1 {
        return Err(invalid());
    }
    let enrollment = agent_enrollments::Entity::find()
        .filter(agent_enrollments::Column::CodeHash.eq(sha256_hex(code.as_bytes())))
        .one(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(invalid)?;
    let device = device_registries::Entity::find_by_id(enrollment.device_registry_id)
        .one(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(invalid)?;
    // Rotation: a (re)installation always gets fresh credentials — any
    // previous installation is disconnected at its next revalidation (4005).
    let token = super::devices::generate_token();
    let key = super::devices::generate_device_key();
    let existing = device_tokens::Entity::find()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .one(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    match existing {
        Some(row) => {
            let mut active: device_tokens::ActiveModel = row.into();
            active.token = Set(token.clone());
            active.encryption_key = Set(Some(key.clone()));
            active.is_active = Set(true);
            active
                .update(&txn)
                .await
                .map_err(|_| Error::InternalServerError)?;
        }
        None => {
            device_tokens::ActiveModel {
                token: Set(token.clone()),
                encryption_key: Set(Some(key.clone())),
                is_active: Set(true),
                device_registry_id: Set(device.id),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?;
        }
    }
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    tracing::info!(device = %device.device_id, "edge agent enrolled");
    format::json(AgentEnrollResponse {
        device_id: device.device_id,
        token,
        encryption_key: key,
        ws_path: "/ws/device".to_string(),
        ca_pem: super::meta::local_ca_pem(),
    })
}

/// Directory of the shipped agent binaries (`PNEX_AGENT_DIST_DIR`, dev
/// default `target/agent-dist`).
fn dist_dir() -> std::path::PathBuf {
    std::env::var("PNEX_AGENT_DIST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("../../target/agent-dist"))
}

fn attachment(bytes: Vec<u8>, content_type: &str, filename: &str) -> Response {
    (
        StatusCode::OK,
        [
            ("content-type", content_type.to_string()),
            (
                "content-disposition",
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response()
}

async fn download(Path(target): Path<String>) -> Result<Response> {
    let not_found = || {
        agent_error(
            StatusCode::NOT_FOUND,
            "agent-binary-not-found",
            "This server does not ship the agent for this platform.",
        )
    };
    // Whitelisted names only (no path from the request reaches the fs).
    let file = if target == "SHA256SUMS" {
        "SHA256SUMS"
    } else {
        agent_file_of(&target).ok_or_else(not_found)?
    };
    // Streamed from disk in fixed-size chunks: agent binaries are tens of
    // MB and concurrent fleet installs must not each buffer a whole copy.
    let handle = tokio::fs::File::open(dist_dir().join(file))
        .await
        .map_err(|_| not_found())?;
    let len = handle.metadata().await.ok().map(|m| m.len());
    let ctype = if file == "SHA256SUMS" {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    };
    let mut resp = (
        StatusCode::OK,
        [
            ("content-type", ctype.to_string()),
            (
                "content-disposition",
                format!("attachment; filename=\"{file}\""),
            ),
        ],
        axum::body::Body::from_stream(file_chunks(handle)),
    )
        .into_response();
    if let Some(len) = len {
        resp.headers_mut()
            .insert(axum::http::header::CONTENT_LENGTH, len.into());
    }
    Ok(resp)
}

/// Chunked reader stream over an open file (64 KiB reads).
fn file_chunks(
    file: tokio::fs::File,
) -> impl futures_util::Stream<Item = std::io::Result<axum::body::Bytes>> + Send {
    futures_util::stream::try_unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 64 * 1024];
        let n = file.read(&mut buf).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some((axum::body::Bytes::from(buf), file)))
    })
}

async fn install_sh() -> Result<Response> {
    Ok(attachment(
        INSTALL_SH.as_bytes().to_vec(),
        "text/x-shellscript; charset=utf-8",
        "install.sh",
    ))
}

async fn install_ps1() -> Result<Response> {
    Ok(attachment(
        INSTALL_PS1.as_bytes().to_vec(),
        "text/plain; charset=utf-8",
        "install.ps1",
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/devices/{id}/agent", get(info).patch(update_settings))
        .add("/devices/{id}/agent-keys", get(list_keys))
        .add(
            "/devices/{id}/agent-keys/{key_id}",
            patch(patch_key).delete(delete_key),
        )
        .add("/devices/{id}/agent-enrollment", post(create_enrollment))
        .add("/agent/enroll", post(enroll))
        .add("/agent/install.sh", get(install_sh))
        .add("/agent/install.ps1", get(install_ps1))
        .add("/agent/download/{target}", get(download))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unambiguous_and_displayed_in_groups() {
        let c = new_code();
        assert_eq!(c.len(), CODE_LEN);
        assert!(c.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        let shown = display_code(&c);
        assert_eq!(shown.len(), CODE_LEN + 2);
        assert_eq!(normalize_enroll_code(&shown), c);
    }
}
