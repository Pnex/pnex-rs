//! Browser agents (extension-collector.md D254–D269): pairing, tickets,
//! missions and their leases. Shared by the HTTP controller and the
//! `/ws/extension` link.
//!
//! - an agent is a `device_registries` row of the `browser_agent` model
//!   plus an `extension_agents` row holding its credentials: the install
//!   key (D266), the hash of its token and its Noise key. The token lives
//!   apart from `device_tokens`, so no other device link accepts it;
//! - every request of the agent is signed by its install key (WebCrypto
//!   ECDSA P-256, non extractable): a copied token alone opens nothing;
//! - a mission runs only on the agent holding the Valkey lease of its
//!   target (D257).

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chrono::{DateTime, FixedOffset, Utc};
use loco_rs::config::Config;
use pnex_core::browser_agent::{
    self as ba, BrowserAgent, Mission, MissionCreate, MissionPatch, MissionSpec, MissionState,
    PairRequest, PairResponse,
};
use pnex_core::err_codes;
use rand::RngExt;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, Set, Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::models::_entities::{device_registries, predefined_devices};

/// Lifetime of an unused websocket ticket.
pub const TICKET_TTL_SECS: u64 = 60;
/// Lease of a mission target; renewed by the link well before it expires.
pub const LEASE_TTL_MS: u64 = 45_000;

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("invalid field {field}")]
    Invalid { field: String, token: String },
    #[error("agent not found")]
    AgentNotFound,
    #[error("mission not found")]
    MissionNotFound,
    #[error("video not found")]
    VideoNotFound,
    /// The text store (O2) refused the captions: nothing recorded, retry.
    #[error("text store unavailable")]
    TextStoreUnavailable,
    #[error("stale version")]
    VersionConflict { current: i32 },
    #[error("pairing refused")]
    PairRefused,
    #[error("install paired with another org")]
    PairedElsewhere,
    /// Org cap on agents or missions reached.
    #[error("limit reached")]
    LimitReached { max: i64 },
    #[error(transparent)]
    Db(#[from] DbErr),
}

impl From<(String, String)> for ExtensionError {
    fn from((field, token): (String, String)) -> Self {
        Self::Invalid { field, token }
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DbBackend::Postgres, sql, values)
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn random_b64(url_safe: bool) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    if url_safe {
        URL_SAFE_NO_PAD.encode(bytes)
    } else {
        STANDARD.encode(bytes)
    }
}

/// Raw uncompressed P-256 point (65 bytes, `0x04‖x‖y`).
fn decode_install_key(raw: &str) -> Option<Vec<u8>> {
    let key = URL_SAFE_NO_PAD.decode(raw.trim()).ok()?;
    (key.len() == 65 && key[0] == 0x04).then_some(key)
}

/// ECDSA P-256 SHA-256 signature (WebCrypto's 64-byte r‖s) of `message`
/// by `key`, made within the accepted clock skew.
fn signed_now(key: &[u8], message: &str, ts_ms: i64, signature: &str) -> bool {
    if (Utc::now().timestamp_millis() - ts_ms).abs() > ba::SIGNED_SKEW_MS {
        return false;
    }
    let Ok(sig) = URL_SAFE_NO_PAD.decode(signature.trim()) else {
        return false;
    };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, key)
        .verify(message.as_bytes(), &sig)
        .is_ok()
}

// ───────────────────────── Agents ─────────────────────────

#[derive(Debug, FromQueryResult)]
struct AgentRow {
    device_registry_id: i64,
    org_id: i64,
    device_id: String,
    install_key: String,
    encryption_key: String,
    name: String,
    fingerprint: serde_json::Value,
    mode: String,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
}

const AGENT_SELECT: &str = "SELECT a.device_registry_id, a.org_id, d.device_id, a.install_key,
    a.encryption_key, a.name, a.fingerprint, a.mode, a.created_at, a.updated_at
    FROM extension_agents a JOIN device_registries d ON d.id = a.device_registry_id";

impl AgentRow {
    fn view(&self, online: bool) -> BrowserAgent {
        BrowserAgent {
            id: self.device_registry_id,
            device_id: self.device_id.clone(),
            name: self.name.clone(),
            fingerprint: serde_json::from_value(self.fingerprint.clone()).unwrap_or_default(),
            mode: self.mode.clone(),
            online,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
        }
    }
}

/// The authenticated agent of a link or a ticket request.
#[derive(Debug, Clone)]
pub struct AgentAuth {
    pub org_id: i64,
    pub device_registry_id: i64,
    pub device_id: String,
    pub encryption_key: [u8; 32],
    /// `shared` or `dedicated:<platform>` (D269), read at connection.
    pub mode: String,
}

/// Pairs an install with the org (D266): creates its agent, or rotates the
/// credentials of the install's existing agent in this org.
pub async fn pair(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    req: &PairRequest,
) -> Result<PairResponse, ExtensionError> {
    let key = decode_install_key(&req.install_key).ok_or(ExtensionError::PairRefused)?;
    if !signed_now(
        &key,
        &ba::pair_message(org_id, req.ts),
        req.ts,
        &req.signature,
    ) {
        return Err(ExtensionError::PairRefused);
    }
    let install_key = URL_SAFE_NO_PAD.encode(&key);
    let fingerprint = req.fingerprint.bounded();
    let token = random_b64(true);
    let encryption_key = random_b64(false);

    let txn = db.begin().await?;
    let existing = AgentRow::find_by_statement(stmt(
        &format!("{AGENT_SELECT} WHERE a.install_key = $1 FOR UPDATE OF a"),
        vec![install_key.clone().into()],
    ))
    .one(&txn)
    .await?;
    let device_registry_id =
        match existing {
            Some(row) if row.org_id != org_id => return Err(ExtensionError::PairedElsewhere),
            Some(row) => {
                txn.execute_raw(stmt(
                    "UPDATE extension_agents SET token_hash = $2, encryption_key = $3,
                    fingerprint = $4, updated_at = now() WHERE device_registry_id = $1",
                    vec![
                        row.device_registry_id.into(),
                        sha256_hex(token.as_bytes()).into(),
                        encryption_key.clone().into(),
                        serde_json::to_value(&fingerprint)
                            .unwrap_or_default()
                            .into(),
                    ],
                ))
                .await?;
                row.device_registry_id
            }
            None => {
                let count = txn
                    .query_one_raw(stmt(
                        "SELECT count(*) AS n FROM extension_agents WHERE org_id = $1",
                        vec![org_id.into()],
                    ))
                    .await?
                    .map(|r| r.try_get::<i64>("", "n"))
                    .transpose()?
                    .unwrap_or(0);
                if count >= ba::AGENTS_MAX {
                    return Err(ExtensionError::LimitReached {
                        max: ba::AGENTS_MAX,
                    });
                }
                let model = predefined_devices::Entity::find()
                    .filter(predefined_devices::Column::Name.eq(pnex_core::BROWSER_AGENT_PREDEF))
                    .one(&txn)
                    .await?
                    .ok_or_else(|| DbErr::RecordNotFound("browser_agent model".into()))?;
                let device = device_registries::ActiveModel {
                    device_id: Set(format!("ext_{}", &sha256_hex(&key)[..12])),
                    active: Set(false),
                    allow_dynamic_measurements: Set(true),
                    discovered_measurements: Set(None),
                    max_unique_measurements: Set(100),
                    org_id: Set(org_id),
                    predefined_device_id: Set(model.id),
                    board_id: Set(Some(model.board_id)),
                    ..Default::default()
                }
                .insert(&txn)
                .await?;
                txn.execute_raw(stmt(
                "INSERT INTO extension_agents (device_registry_id, org_id, install_key, token_hash,
                    encryption_key, name, fingerprint, paired_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                vec![
                    device.id.into(),
                    org_id.into(),
                    install_key.clone().into(),
                    sha256_hex(token.as_bytes()).into(),
                    encryption_key.clone().into(),
                    fingerprint.default_name().into(),
                    serde_json::to_value(&fingerprint).unwrap_or_default().into(),
                    user_id.into(),
                ],
            ))
            .await?;
                device.id
            }
        };
    let row = agent_row(&txn, org_id, device_registry_id)
        .await?
        .ok_or(ExtensionError::AgentNotFound)?;
    txn.commit().await?;
    Ok(PairResponse {
        agent: row.view(false),
        token,
        encryption_key,
    })
}

async fn agent_row<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: i64,
) -> Result<Option<AgentRow>, DbErr> {
    AgentRow::find_by_statement(stmt(
        &format!("{AGENT_SELECT} WHERE a.org_id = $1 AND a.device_registry_id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await
}

/// Is a browser agent of the org online now?
pub(crate) async fn any_online(db: &DatabaseConnection, org_id: i64) -> bool {
    let Ok(rows) = AgentRow::find_by_statement(stmt(
        &format!("{AGENT_SELECT} WHERE a.org_id = $1"),
        vec![org_id.into()],
    ))
    .all(db)
    .await
    else {
        return false;
    };
    let ids: Vec<i64> = rows.iter().map(|r| r.device_registry_id).collect();
    online_of(db, &ids).await.values().any(|on| *on)
}

async fn online_of(db: &DatabaseConnection, ids: &[i64]) -> std::collections::HashMap<i64, bool> {
    crate::services::device_liveness::seen_many(db, ids)
        .await
        .map(|m| m.into_iter().map(|(id, s)| (id, s.connected)).collect())
        .unwrap_or_default()
}

/// Agents of the org, newest first.
pub async fn list_agents(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<BrowserAgent>, ExtensionError> {
    let rows = AgentRow::find_by_statement(stmt(
        &format!("{AGENT_SELECT} WHERE a.org_id = $1 ORDER BY a.created_at DESC"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.device_registry_id).collect();
    let online = online_of(db, &ids).await;
    Ok(rows
        .iter()
        .map(|r| r.view(online.get(&r.device_registry_id).copied().unwrap_or(false)))
        .collect())
}

/// Renames an agent.
pub async fn update_agent(
    db: &DatabaseConnection,
    org_id: i64,
    id: i64,
    patch: &ba::BrowserAgentPatch,
) -> Result<BrowserAgent, ExtensionError> {
    let name = patch.name.as_deref().map(ba::check_name).transpose()?;
    let mode = patch.mode.as_deref().map(ba::check_mode).transpose()?;
    // A new mode reaches the agent at its next connection.
    let done = db
        .execute_raw(stmt(
            "UPDATE extension_agents
             SET name = coalesce($3, name), mode = coalesce($4, mode), updated_at = now()
             WHERE org_id = $1 AND device_registry_id = $2",
            vec![org_id.into(), id.into(), name.into(), mode.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(ExtensionError::AgentNotFound);
    }
    let row = agent_row(db, org_id, id)
        .await?
        .ok_or(ExtensionError::AgentNotFound)?;
    let online = online_of(db, &[id]).await;
    Ok(row.view(online.get(&id).copied().unwrap_or(false)))
}

/// Agent of a bearer token, if `signature` is its install key's signature
/// of `message(device_id)` made within the clock skew.
pub async fn authenticate(
    db: &DatabaseConnection,
    token: &str,
    ts_ms: i64,
    signature: &str,
    message: impl FnOnce(&str) -> String,
) -> Result<Option<AgentAuth>, DbErr> {
    let Some(row) = AgentRow::find_by_statement(stmt(
        &format!("{AGENT_SELECT} WHERE a.token_hash = $1"),
        vec![sha256_hex(token.trim().as_bytes()).into()],
    ))
    .one(db)
    .await?
    else {
        return Ok(None);
    };
    let Some(key) = decode_install_key(&row.install_key) else {
        return Ok(None);
    };
    if !signed_now(&key, &message(&row.device_id), ts_ms, signature) {
        return Ok(None);
    }
    Ok(auth_of(&row))
}

fn auth_of(row: &AgentRow) -> Option<AgentAuth> {
    let encryption_key = STANDARD
        .decode(row.encryption_key.trim())
        .ok()
        .and_then(|k| <[u8; 32]>::try_from(k).ok())?;
    Some(AgentAuth {
        org_id: row.org_id,
        device_registry_id: row.device_registry_id,
        device_id: row.device_id.clone(),
        encryption_key,
        mode: row.mode.clone(),
    })
}

/// Agent still paired (not deleted, credentials not rotated since).
pub async fn still_paired(db: &DatabaseConnection, auth: &AgentAuth) -> bool {
    let row = AgentRow::find_by_statement(stmt(
        &format!("{AGENT_SELECT} WHERE a.device_registry_id = $1"),
        vec![auth.device_registry_id.into()],
    ))
    .one(db)
    .await;
    matches!(row, Ok(Some(r)) if auth_of(&r).is_some_and(|a| a.encryption_key == auth.encryption_key))
}

// ───────────────────────── Tickets ─────────────────────────

fn ticket_key(ticket: &str) -> String {
    format!("pnex:ws-ticket:ext:{ticket}")
}

/// One-time ticket of `/ws/extension` for an agent; `None` when Valkey is
/// unreachable or the signature was already used (replay).
pub async fn issue_ticket(config: &Config, agent: &AgentAuth, signature: &str) -> Option<String> {
    if !first_use(config, signature).await? {
        return None;
    }
    let mut conn = crate::services::shared_valkey::conn(config).await?;
    let ticket = random_b64(true);
    redis::cmd("SET")
        .arg(ticket_key(&ticket))
        .arg(agent.device_registry_id)
        .arg("EX")
        .arg(TICKET_TTL_SECS)
        .query_async::<()>(&mut conn)
        .await
        .ok()?;
    Some(ticket)
}

/// `Some(true)` the first time a signature is seen (kept past its validity
/// window), `Some(false)` on a replay, `None` when Valkey is unreachable.
pub async fn first_use(config: &Config, signature: &str) -> Option<bool> {
    // Keyed on `r`: the text may differ (padding, spaces) and `(r, n−s)`
    // verifies too, but `r` is the same for every form of one signature.
    let Some(r) = URL_SAFE_NO_PAD
        .decode(signature.trim())
        .ok()
        .filter(|sig| sig.len() == 64)
    else {
        return Some(false);
    };
    let mut conn = crate::services::shared_valkey::conn(config).await?;
    let fresh: Option<String> = redis::cmd("SET")
        .arg(format!("pnex:ext:sig:{}", sha256_hex(&r[..32])))
        .arg(1)
        .arg("NX")
        .arg("PX")
        .arg(ba::SIGNED_SKEW_MS * 2)
        .query_async(&mut conn)
        .await
        .ok()?;
    Some(fresh.is_some())
}

/// Consumes a ticket: the agent's device registry id, once.
pub async fn redeem_ticket(config: &Config, ticket: &str) -> Option<i64> {
    let ticket = ticket.trim();
    if ticket.is_empty() || ticket.len() > 64 {
        return None;
    }
    let mut conn = crate::services::shared_valkey::conn(config).await?;
    redis::cmd("GETDEL")
        .arg(ticket_key(ticket))
        .query_async::<Option<i64>>(&mut conn)
        .await
        .ok()
        .flatten()
}

/// Agent of a redeemed ticket.
pub async fn agent_by_id(db: &DatabaseConnection, id: i64) -> Result<Option<AgentAuth>, DbErr> {
    Ok(AgentRow::find_by_statement(stmt(
        &format!("{AGENT_SELECT} WHERE a.device_registry_id = $1"),
        vec![id.into()],
    ))
    .one(db)
    .await?
    .as_ref()
    .and_then(auth_of))
}

// ───────────────────────── Missions ─────────────────────────

#[derive(Debug, FromQueryResult)]
pub(crate) struct MissionRow {
    id: Uuid,
    pub(crate) name: String,
    spec: serde_json::Value,
    target: String,
    interval_secs: i32,
    agent_id: Option<i64>,
    enabled: bool,
    state: String,
    state_code: Option<String>,
    version: i32,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
    /// Server-side progress (page digest, history depth), never sent as is.
    cursor: Option<serde_json::Value>,
}

const MISSION_COLUMNS: &str = "id, name, spec, target, interval_secs, agent_id, enabled, state,
    state_code, version, created_at, updated_at, cursor";

/// History progress of a `follow_account` (D283), kept in the cursor.
fn history_of(cursor: Option<&serde_json::Value>) -> (u32, bool) {
    let c = cursor.cloned().unwrap_or_default();
    (
        c["history_depth"].as_u64().unwrap_or(0) as u32,
        c["history_done"].as_bool().unwrap_or(false),
    )
}

impl MissionRow {
    /// `None` for a spec this server no longer reads (kind removed).
    fn view(&self) -> Option<Mission> {
        Some(Mission {
            id: self.id.to_string(),
            name: self.name.clone(),
            spec: serde_json::from_value(self.spec.clone()).ok()?,
            target: self.target.clone(),
            interval_secs: self.interval_secs,
            agent_id: self.agent_id,
            enabled: self.enabled,
            state: MissionState::parse(&self.state).unwrap_or_default(),
            state_code: self.state_code.clone(),
            version: self.version,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
        })
    }
}

/// Pinned agent must be a browser agent of the org.
async fn check_agent<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    agent_id: Option<i64>,
) -> Result<(), ExtensionError> {
    let Some(id) = agent_id else {
        return Ok(());
    };
    match agent_row(db, org_id, id).await? {
        Some(_) => Ok(()),
        None => Err(ExtensionError::Invalid {
            field: "agent_id".into(),
            token: err_codes::FIELD_NOT_FOUND.into(),
        }),
    }
}

pub async fn list_missions(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<Mission>, ExtensionError> {
    let rows = MissionRow::find_by_statement(stmt(
        &format!(
            // Proof tasks are the server's (D308): shown with their pages.
            "SELECT {MISSION_COLUMNS} FROM extension_missions WHERE org_id = $1 \
             AND kind <> 'capture_proof' ORDER BY created_at DESC"
        ),
        vec![org_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows.iter().filter_map(MissionRow::view).collect())
}

async fn mission_row<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<MissionRow, ExtensionError> {
    MissionRow::find_by_statement(stmt(
        &format!("SELECT {MISSION_COLUMNS} FROM extension_missions WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .ok_or(ExtensionError::MissionNotFound)
}

pub async fn get_mission(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Mission, ExtensionError> {
    mission_row(db, org_id, id)
        .await?
        .view()
        .ok_or(ExtensionError::MissionNotFound)
}

pub async fn create_mission(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    input: &MissionCreate,
) -> Result<Mission, ExtensionError> {
    let name = ba::check_name(&input.name)?;
    let (spec, target) = ba::check_spec(&input.spec)?;
    let interval = ba::check_interval(input.interval_secs)?;
    check_agent(db, org_id, input.agent_id).await?;
    // shortcut: count then insert is racy by a few rows, fine for an abuse cap.
    let count = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM extension_missions WHERE org_id = $1 AND kind <> 'capture_proof'",
            vec![org_id.into()],
        ))
        .await?
        .map(|r| r.try_get::<i64>("", "n"))
        .transpose()?
        .unwrap_or(0);
    if count >= ba::MISSIONS_MAX {
        return Err(ExtensionError::LimitReached {
            max: ba::MISSIONS_MAX,
        });
    }
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO extension_missions (id, org_id, name, kind, spec, target, interval_secs,
            agent_id, enabled, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            spec.kind().into(),
            serde_json::to_value(&spec).unwrap_or_default().into(),
            target.into(),
            interval.into(),
            input.agent_id.into(),
            input.enabled.into(),
            user_id.into(),
        ],
    ))
    .await?;
    get_mission(db, org_id, id).await
}

/// Partial update on the version the caller read (optimistic concurrency).
pub async fn update_mission(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    patch: &MissionPatch,
) -> Result<Mission, ExtensionError> {
    let txn = db.begin().await?;
    let row = MissionRow::find_by_statement(stmt(
        &format!(
            "SELECT {MISSION_COLUMNS} FROM extension_missions WHERE org_id = $1 AND id = $2 FOR UPDATE"
        ),
        vec![org_id.into(), id.into()],
    ))
    .one(&txn)
    .await?
    .ok_or(ExtensionError::MissionNotFound)?;
    if row.version != patch.expected_version {
        return Err(ExtensionError::VersionConflict {
            current: row.version,
        });
    }
    let name = match &patch.name {
        Some(n) => ba::check_name(n)?,
        None => row.name.clone(),
    };
    let (spec, target) = match &patch.spec {
        Some(s) => ba::check_spec(s)?,
        None => (
            serde_json::from_value::<MissionSpec>(row.spec.clone()).map_err(|_| {
                ExtensionError::Invalid {
                    field: "spec".into(),
                    token: err_codes::FIELD_REQUIRED.into(),
                }
            })?,
            row.target.clone(),
        ),
    };
    let interval = match patch.interval_secs {
        Some(i) => ba::check_interval(i)?,
        None => row.interval_secs,
    };
    let agent_id = patch.agent_id.unwrap_or(row.agent_id);
    check_agent(&txn, org_id, agent_id).await?;
    let enabled = patch.enabled.unwrap_or(row.enabled);
    // A re-enabled or changed mission starts over: its last state is stale.
    txn.execute_raw(stmt(
        "UPDATE extension_missions SET name = $3, kind = $4, spec = $5, target = $6,
            interval_secs = $7, agent_id = $8, enabled = $9, state = 'idle', state_code = NULL,
            version = version + 1, updated_at = now()
         WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            name.into(),
            spec.kind().into(),
            serde_json::to_value(&spec).unwrap_or_default().into(),
            target.into(),
            interval.into(),
            agent_id.into(),
            enabled.into(),
        ],
    ))
    .await?;
    let row = mission_row(&txn, org_id, id).await?;
    txn.commit().await?;
    row.view().ok_or(ExtensionError::MissionNotFound)
}

pub async fn delete_mission(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<(), ExtensionError> {
    let done = db
        .execute_raw(stmt(
            "DELETE FROM extension_missions WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(ExtensionError::MissionNotFound);
    }
    Ok(())
}

/// Enabled missions an agent may run (pinned on it or on no agent), oldest
/// first: candidates of the lease.
pub async fn candidates(
    db: &DatabaseConnection,
    agent: &AgentAuth,
) -> Result<Vec<ba::AssignedMission>, DbErr> {
    let rows = MissionRow::find_by_statement(stmt(
        &format!(
            "SELECT {MISSION_COLUMNS} FROM extension_missions
             WHERE org_id = $1 AND enabled AND (agent_id IS NULL OR agent_id = $2)
             ORDER BY created_at"
        ),
        vec![agent.org_id.into(), agent.device_registry_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let m = row.view()?;
            // D269: exclusive platforms only on an agent dedicated to them.
            if !ba::mode_allows(&agent.mode, ba::platform_of(&m.target)) {
                return None;
            }
            // A server crawl is the server's (D307).
            if matches!(&m.spec, MissionSpec::CrawlSite(p) if p.executor == ba::CrawlExecutor::Server)
            {
                return None;
            }
            let scroll_depth = match &m.spec {
                MissionSpec::FollowAccount(_) => {
                    let (depth, done) = history_of(row.cursor.as_ref());
                    ba::next_depth(depth, done)
                }
                _ => 0,
            };
            Some(ba::AssignedMission {
                id: m.id,
                spec: m.spec,
                target: m.target,
                interval_secs: m.interval_secs,
                scroll_depth,
            })
        })
        .collect())
}

/// State reported by the agent for one of its missions; unknown codes are
/// stored empty (an untrusted agent never writes free text).
pub async fn report_state(
    db: &DatabaseConnection,
    agent: &AgentAuth,
    mission_id: Uuid,
    state: MissionState,
    code: Option<&str>,
) -> Result<(), DbErr> {
    let code = code.filter(|c| ba::AGENT_STATE_CODES.contains(c));
    db.execute_raw(stmt(
        "UPDATE extension_missions SET state = $3, state_code = $4
         WHERE org_id = $1 AND id = $2 AND enabled",
        vec![
            agent.org_id.into(),
            mission_id.into(),
            state.wire().into(),
            code.map(str::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

// ───────────────────────── Page captures (D261) ─────────────────────────

/// Markdown document of a page: title, source, readable text.
pub fn page_markdown(snapshot: &ba::PageSnapshot) -> String {
    let title: String = snapshot
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect();
    let title = if title.is_empty() {
        snapshot.url.clone()
    } else {
        title
    };
    format!(
        "# {title}\n\n<{}>\n\n{}\n",
        snapshot.url,
        snapshot.text.trim()
    )
}

/// Stores the text of a `collect_page` mission of this agent as a version
/// of the mission's document (created on the first capture); an unchanged
/// text adds no version. The document is indexed by the document search.
pub async fn store_page(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    snapshot: &ba::PageSnapshot,
) -> Result<ba::PageStored, ExtensionError> {
    use crate::models::_entities::media_assets;
    let (id, mission, spec) = own_mission(&ctx.db, agent, &snapshot.mission_id).await?;
    if !matches!(spec, MissionSpec::CollectPage(_)) {
        return Err(ExtensionError::MissionNotFound);
    }
    if snapshot.text.trim().is_empty() {
        return Err(ExtensionError::Invalid {
            field: "text".into(),
            token: err_codes::FIELD_REQUIRED.into(),
        });
    }
    if snapshot.text.len() > ba::PAGE_TEXT_MAX {
        return Err(ExtensionError::Invalid {
            field: "text".into(),
            token: format!("{}:{}", err_codes::FIELD_MAX_LENGTH, ba::PAGE_TEXT_MAX),
        });
    }
    let markdown = page_markdown(snapshot);
    let digest = sha256_hex(markdown.as_bytes());
    let cursor: serde_json::Value = mission_cursor(&ctx.db, id).await?;
    let known = cursor["asset_id"]
        .as_str()
        .and_then(|a| Uuid::parse_str(a).ok());
    let asset = match known {
        Some(asset_id) => {
            media_assets::Entity::find_by_id(asset_id)
                .filter(media_assets::Column::OrgId.eq(agent.org_id))
                .one(&ctx.db)
                .await?
        }
        None => None,
    };
    if let Some(asset) = &asset {
        if cursor["sha256"].as_str() == Some(digest.as_str()) {
            let version = cursor["version"].as_i64().unwrap_or(0);
            return Ok(ba::PageStored {
                asset_id: asset.id.to_string(),
                version,
                changed: false,
            });
        }
    }
    let asset = match asset {
        Some(asset) => asset,
        // shortcut: the asset row is written before its first version, like
        // the media upload; a failed write leaves an empty document behind.
        None => {
            media_assets::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(agent.org_id),
                kind: Set("document".into()),
                name: Set(mission.name.clone()),
                description: Set(None),
                metadata: Set(Some(serde_json::json!({
                    "source": "browser_agent",
                    "mission_id": id.to_string(),
                    "source_url": snapshot.url,
                }))),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await?
        }
    };
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    let version = crate::controllers::media::write_version(
        ctx,
        agent.org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: "page.md".into(),
            content_type: "text/markdown".into(),
            note: None,
            bytes: markdown.into_bytes().into(),
        },
        &settings,
    )
    .await
    .map_err(|e| ExtensionError::Db(DbErr::Custom(format!("page version: {e}"))))?;
    ctx.db
        .execute_raw(stmt(
            "UPDATE extension_missions SET cursor = $2, state = 'running', state_code = NULL
             WHERE id = $1",
            vec![
                id.into(),
                serde_json::json!({
                    "asset_id": asset.id.to_string(),
                    "sha256": digest,
                    "version": version.version_number,
                })
                .into(),
            ],
        ))
        .await?;
    Ok(ba::PageStored {
        asset_id: asset.id.to_string(),
        version: version.version_number,
        changed: true,
    })
}

/// An enabled mission this agent may run, with its spec; anything else is
/// "not found" (an agent never learns about other missions).
pub(crate) async fn own_mission(
    db: &DatabaseConnection,
    agent: &AgentAuth,
    mission_id: &str,
) -> Result<(Uuid, MissionRow, MissionSpec), ExtensionError> {
    let id = Uuid::parse_str(mission_id).map_err(|_| ExtensionError::MissionNotFound)?;
    let mission = mission_row(db, agent.org_id, id).await?;
    let pinned_elsewhere = mission
        .agent_id
        .is_some_and(|a| a != agent.device_registry_id);
    let spec = serde_json::from_value::<MissionSpec>(mission.spec.clone())
        .map_err(|_| ExtensionError::MissionNotFound)?;
    if !mission.enabled || pinned_elsewhere {
        return Err(ExtensionError::MissionNotFound);
    }
    Ok((id, mission, spec))
}

// ───────────────────────── Video registry (D259) ─────────────────────────

fn http_url(raw: &str) -> bool {
    raw.len() <= ba::URL_MAX && (raw.starts_with("https://") || raw.starts_with("http://"))
}

/// A post as stored: bounded, typed, never trusted.
fn clean_post(p: &ba::SeenPost) -> Option<ba::SeenPost> {
    let id_ok = !p.platform_id.is_empty()
        && p.platform_id.len() <= 64
        && p.platform_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !id_ok || !http_url(&p.url) {
        return None;
    }
    let published_at = p
        .published_at
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.to_rfc3339());
    Some(ba::SeenPost {
        platform_id: p.platform_id.clone(),
        url: p.url.clone(),
        author: p.author.chars().take(64).collect(),
        text: p.text.chars().take(ba::POST_TEXT_MAX).collect(),
        published_at,
        video_url: p.video_url.clone().filter(|u| http_url(u)),
        duration_ms: p.duration_ms.filter(|d| *d >= 0),
    })
}

/// Records what a `follow_account` mission saw; returns the posts whose
/// video the agent should capture: new in the registry, with a video, and
/// a policy that keeps files. A post already known (by any agent) is never
/// captured twice.
pub async fn record_seen(
    db: &DatabaseConnection,
    agent: &AgentAuth,
    mission_id: &str,
    posts: &[ba::SeenPost],
    depth: u32,
    end_reached: bool,
) -> Result<(Vec<String>, Vec<String>), ExtensionError> {
    let (id, mission, spec) = own_mission(db, agent, mission_id).await?;
    let MissionSpec::FollowAccount(params) = spec else {
        return Err(ExtensionError::MissionNotFound);
    };
    #[derive(FromQueryResult)]
    struct Inserted {
        platform_id: String,
        video_url: Option<String>,
    }
    // shortcut: one count per batch (index scan), fine at 60 batches a minute.
    let known = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM social_videos WHERE org_id = $1",
            vec![agent.org_id.into()],
        ))
        .await?
        .map(|r| r.try_get::<i64>("", "n"))
        .transpose()?
        .unwrap_or(0);
    if known >= ba::VIDEOS_MAX {
        report_state(
            db,
            agent,
            id,
            MissionState::Blocked,
            Some(err_codes::EXT_REGISTRY_FULL),
        )
        .await?;
        return Ok((Vec::new(), Vec::new()));
    }
    let mut capture = Vec::new();
    let mut captions = Vec::new();
    for post in posts.iter().take(ba::SEEN_MAX).filter_map(clean_post) {
        let inserted = Inserted::find_by_statement(stmt(
            "INSERT INTO social_videos (id, org_id, platform, platform_id, url, author, text,
                published_at, video_url, duration_ms, mission_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz, $9, $10, $11)
             ON CONFLICT (org_id, platform, platform_id) DO NOTHING
             RETURNING platform_id, video_url",
            vec![
                Uuid::new_v4().into(),
                agent.org_id.into(),
                params.platform.wire().into(),
                post.platform_id.clone().into(),
                post.url.clone().into(),
                post.author.clone().into(),
                post.text.clone().into(),
                post.published_at.clone().into(),
                post.video_url.clone().into(),
                post.duration_ms.into(),
                id.into(),
            ],
        ))
        .one(db)
        .await?;
        if let Some(row) = inserted {
            if row.video_url.is_some() {
                // D284: text first (captions), the file only when forced.
                captions.push(row.platform_id.clone());
                if params.force_video {
                    capture.push(row.platform_id);
                }
            }
        }
    }
    // D283: the history goes deeper each visit until the list ends.
    let (known_depth, done) = history_of(mission.cursor.as_ref());
    let history_depth = known_depth.max(depth.min(ba::HISTORY_MAX_DEPTH));
    let history_done = done || end_reached || history_depth >= ba::HISTORY_MAX_DEPTH;
    db.execute_raw(stmt(
        "UPDATE extension_missions
         SET cursor = coalesce(cursor, '{}'::jsonb) || $2::jsonb
         WHERE id = $1",
        vec![
            id.into(),
            serde_json::json!({ "history_depth": history_depth, "history_done": history_done })
                .into(),
        ],
    ))
    .await?;
    report_state(db, agent, id, MissionState::Running, None).await?;
    Ok((capture, captions))
}

#[derive(Debug, FromQueryResult)]
struct VideoRow {
    id: Uuid,
    platform: String,
    platform_id: String,
    url: String,
    author: String,
    text: String,
    published_at: Option<DateTime<FixedOffset>>,
    video_url: Option<String>,
    state: String,
    media_asset_id: Option<Uuid>,
    mission_id: Option<Uuid>,
    first_seen_at: DateTime<FixedOffset>,
    captured_at: Option<DateTime<FixedOffset>>,
}

const VIDEO_COLUMNS: &str = "id, platform, platform_id, url, author, text, published_at, video_url,
    state, media_asset_id, mission_id, first_seen_at, captured_at";

impl VideoRow {
    fn view(&self) -> ba::SocialVideo {
        ba::SocialVideo {
            platform: ba::SocialPlatform::parse(&self.platform).unwrap_or(ba::SocialPlatform::X),
            platform_id: self.platform_id.clone(),
            url: self.url.clone(),
            author: self.author.clone(),
            text: self.text.clone(),
            published_at: self.published_at.map(|t| t.to_rfc3339()),
            has_video: self.video_url.is_some(),
            state: match self.state.as_str() {
                "captured" => ba::VideoState::Captured,
                "captioned" => ba::VideoState::Captioned,
                _ => ba::VideoState::Seen,
            },
            media_asset_id: self.media_asset_id.map(|u| u.to_string()),
            mission_id: self.mission_id.map(|u| u.to_string()),
            first_seen_at: self.first_seen_at.to_rfc3339(),
            captured_at: self.captured_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Stores the video of a seen post as a `video` media of the library
/// (D281: reviewed like any media), with the post's provenance.
pub async fn store_video(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    mission_id: &str,
    platform_id: &str,
    part: u32,
    bytes: axum::body::Bytes,
) -> Result<ba::SocialVideo, ExtensionError> {
    if !(1..=ba::VIDEO_PARTS_MAX).contains(&part) {
        return Err(ExtensionError::Invalid {
            field: "part".into(),
            token: err_codes::FIELD_INVALID.into(),
        });
    }
    let (id, _, spec) = own_mission(&ctx.db, agent, mission_id).await?;
    let MissionSpec::FollowAccount(params) = spec else {
        return Err(ExtensionError::MissionNotFound);
    };
    let row = VideoRow::find_by_statement(stmt(
        &format!(
            "SELECT {VIDEO_COLUMNS} FROM social_videos
             WHERE org_id = $1 AND platform = $2 AND platform_id = $3"
        ),
        vec![
            agent.org_id.into(),
            params.platform.wire().into(),
            platform_id.into(),
        ],
    ))
    .one(&ctx.db)
    .await?
    // Part 1 captures the post; the next ones follow a captured post (O92).
    .filter(|r| match part {
        1 => matches!(r.state.as_str(), "seen" | "captioned") && r.video_url.is_some(),
        _ => r.state == "captured",
    })
    .ok_or(ExtensionError::VideoNotFound)?;
    // A recording from the browser is WebM (EBML magic), a fetched file mp4.
    let ext = if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "webm"
    } else {
        "mp4"
    };
    if bytes.is_empty() || bytes.len() > ba::VIDEO_MAX {
        return Err(ExtensionError::Invalid {
            field: "body".into(),
            token: format!("{}:{}", err_codes::FIELD_MAX_LENGTH, ba::VIDEO_MAX),
        });
    }
    let asset = if part == 1 {
        post_asset(ctx, agent, id, &row, params.publisher_id.as_deref()).await?
    } else {
        part_asset(ctx, agent, id, &row, part).await?
    };
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    crate::controllers::media::write_version(
        ctx,
        agent.org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: match part {
                1 => format!("{}.{ext}", row.platform_id),
                _ => format!("{}-part{part}.{ext}", row.platform_id),
            },
            content_type: format!("video/{ext}"),
            note: None,
            bytes,
        },
        &settings,
    )
    .await
    .map_err(|e| ExtensionError::Db(DbErr::Custom(format!("video version: {e}"))))?;
    if part > 1 {
        return Ok(row.view());
    }
    let updated = VideoRow::find_by_statement(stmt(
        &format!(
            "UPDATE social_videos SET state = 'captured', media_asset_id = $2, captured_at = now()
             WHERE id = $1 RETURNING {VIDEO_COLUMNS}"
        ),
        vec![row.id.into(), asset.id.into()],
    ))
    .one(&ctx.db)
    .await?
    .ok_or(ExtensionError::VideoNotFound)?;
    Ok(updated.view())
}

/// The media of a post: the one its captions created, else a new `video`
/// media with the post's provenance (D281), file to come or never.
async fn post_asset(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    mission_id: Uuid,
    row: &VideoRow,
    publisher: Option<&str>,
) -> Result<crate::models::_entities::media_assets::Model, ExtensionError> {
    use crate::models::_entities::media_assets;
    if let Some(existing) = row.media_asset_id {
        if let Some(asset) = media_assets::Entity::find_by_id(existing)
            .filter(media_assets::Column::OrgId.eq(agent.org_id))
            .one(&ctx.db)
            .await?
        {
            return Ok(asset);
        }
    }
    // shortcut: the asset row is written before its version, like the media
    // upload; a refused file leaves a media with no file behind (the same
    // state as a captions-only post, D281).
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(agent.org_id),
        kind: Set("video".into()),
        name: Set(format!("@{} · {}", row.author, row.platform_id)),
        description: Set((!row.text.is_empty()).then(|| row.text.chars().take(2000).collect())),
        metadata: Set(Some(serde_json::json!({
            "source": "browser_agent",
            "mission_id": mission_id.to_string(),
            "source_url": row.url,
            "platform": row.platform,
            "platform_id": row.platform_id,
            "author_handle": row.author,
            "published_at": row.published_at.map(|t| t.to_rfc3339()),
            "captured_at": chrono::Utc::now().to_rfc3339(),
            "published_by_object": publisher,
        }))),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await?;
    ctx.db
        .execute_raw(stmt(
            "UPDATE social_videos SET media_asset_id = $2 WHERE id = $1",
            vec![row.id.into(), asset.id.into()],
        ))
        .await?;
    if let Some(publisher) = publisher {
        link_publisher(ctx, agent, asset.id, publisher).await;
    }
    Ok(asset)
}

/// A new `video` media for part `part` (2 and up) of a captured post: the
/// post's provenance plus `part` and `part_of` (the first part's media).
/// A part already stored is refused (an agent retrying a sent part).
async fn part_asset(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    mission_id: Uuid,
    row: &VideoRow,
    part: u32,
) -> Result<crate::models::_entities::media_assets::Model, ExtensionError> {
    use crate::models::_entities::media_assets;
    let first = row.media_asset_id.ok_or(ExtensionError::VideoNotFound)?;
    let exists = ctx
        .db
        .query_one_raw(stmt(
            "SELECT 1 AS one FROM media_assets WHERE org_id = $1
             AND metadata->>'part_of' = $2 AND metadata->>'part' = $3",
            vec![
                agent.org_id.into(),
                first.to_string().into(),
                part.to_string().into(),
            ],
        ))
        .await?;
    if exists.is_some() {
        return Err(ExtensionError::VideoNotFound);
    }
    Ok(media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(agent.org_id),
        kind: Set("video".into()),
        name: Set(format!("@{} · {} · {part}", row.author, row.platform_id)),
        metadata: Set(Some(serde_json::json!({
            "source": "browser_agent",
            "mission_id": mission_id.to_string(),
            "source_url": row.url,
            "platform": row.platform,
            "platform_id": row.platform_id,
            "author_handle": row.author,
            "published_at": row.published_at.map(|t| t.to_rfc3339()),
            "captured_at": chrono::Utc::now().to_rfc3339(),
            "part": part.to_string(),
            "part_of": first.to_string(),
        }))),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await?)
}

/// `published_by` from the media to the account's person or organization
/// (D278): read by the review's expected participants (D274). An org
/// without that link type (pack absent) or an object gone: no link.
async fn link_publisher(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    asset_id: Uuid,
    publisher: &str,
) {
    use crate::services::ontology;
    let media_object = ctx
        .db
        .query_one_raw(stmt(
            "SELECT object_id FROM media_assets WHERE id = $1 AND org_id = $2",
            vec![asset_id.into(), agent.org_id.into()],
        ))
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<Option<Uuid>>("", "object_id").ok().flatten());
    let Some(media_object) = media_object else {
        return;
    };
    let actor = ontology::Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: format!("agent:{}", agent.device_id),
    };
    let input = pnex_core::ontology::api::LinkInput {
        link_type: "published_by".into(),
        source_id: media_object.to_string(),
        target_id: publisher.to_string(),
        attributes: serde_json::Map::new(),
        valid_from: None,
        valid_to: None,
    };
    if let Err(e) = ontology::links::create(&ctx.db, agent.org_id, &actor, &input).await {
        if !matches!(e, ontology::OntologyError::Invalid { .. }) {
            tracing::warn!(error = %e, "published_by link not written");
        }
    }
}

/// Stores a post's platform captions as a transcription run of origin
/// `platform` on its media (D281), created without a file when needed.
pub async fn store_captions(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    upload: &ba::CaptionsUpload,
) -> Result<ba::SocialVideo, ExtensionError> {
    use crate::services::media_runs::{self, PlatformCue, RunError};
    let (id, _, spec) = own_mission(&ctx.db, agent, &upload.mission_id).await?;
    let MissionSpec::FollowAccount(params) = spec else {
        return Err(ExtensionError::MissionNotFound);
    };
    let row = VideoRow::find_by_statement(stmt(
        &format!(
            "SELECT {VIDEO_COLUMNS} FROM social_videos
             WHERE org_id = $1 AND platform = $2 AND platform_id = $3"
        ),
        vec![
            agent.org_id.into(),
            params.platform.wire().into(),
            upload.platform_id.clone().into(),
        ],
    ))
    .one(&ctx.db)
    .await?
    .filter(|r| r.video_url.is_some())
    .ok_or(ExtensionError::VideoNotFound)?;
    let cues: Vec<PlatformCue> = upload
        .cues
        .iter()
        .map(|c| PlatformCue {
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            text: c.text.clone(),
        })
        .collect();
    let asset = post_asset(ctx, agent, id, &row, params.publisher_id.as_deref()).await?;
    match media_runs::store_platform_run(ctx, agent.org_id, asset.id, &upload.lang, &cues).await {
        Ok(_) => {}
        Err(RunError::Invalid(why)) => {
            return Err(ExtensionError::Invalid {
                field: "cues".into(),
                token: format!("{}:{why}", err_codes::FIELD_INVALID),
            })
        }
        Err(RunError::NotFound) => return Err(ExtensionError::VideoNotFound),
        Err(RunError::Store(e)) => {
            tracing::warn!(error = %e, "platform captions not stored");
            return Err(ExtensionError::TextStoreUnavailable);
        }
        Err(RunError::Db(e)) => return Err(e.into()),
    }
    let updated = VideoRow::find_by_statement(stmt(
        &format!(
            "UPDATE social_videos
             SET state = CASE WHEN state = 'seen' THEN 'captioned' ELSE state END
             WHERE id = $1 RETURNING {VIDEO_COLUMNS}"
        ),
        vec![row.id.into()],
    ))
    .one(&ctx.db)
    .await?
    .ok_or(ExtensionError::VideoNotFound)?;
    Ok(updated.view())
}

/// Stores one part of a recorded live (`watch_live`, D260) as a `video`
/// media of the library, with the live's provenance.
pub async fn store_live_part(
    ctx: &loco_rs::app::AppContext,
    agent: &AgentAuth,
    mission_id: &str,
    part: u32,
    started_at: Option<&str>,
    bytes: axum::body::Bytes,
) -> Result<ba::LivePartStored, ExtensionError> {
    use crate::models::_entities::media_assets;
    let (id, mission, spec) = own_mission(&ctx.db, agent, mission_id).await?;
    let MissionSpec::WatchLive(params) = spec else {
        return Err(ExtensionError::MissionNotFound);
    };
    // Only browser recordings (WebM) come in parts.
    if bytes.len() > ba::VIDEO_MAX || !bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Err(ExtensionError::Invalid {
            field: "body".into(),
            token: err_codes::FIELD_INVALID.into(),
        });
    }
    let started_at = started_at
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.to_rfc3339());
    let day = started_at.as_deref().map_or("", |t| &t[..10.min(t.len())]);
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(agent.org_id),
        kind: Set("video".into()),
        name: Set(format!("{} · {day} · {part}", mission.name)
            .chars()
            .take(200)
            .collect()),
        description: Set(None),
        metadata: Set(Some(serde_json::json!({
            "source": "browser_agent",
            "mission_id": id.to_string(),
            "source_url": params.url,
            "platform": ba::platform_of(&mission.target),
            "live": true,
            "part": part,
            "started_at": started_at,
            "captured_at": chrono::Utc::now().to_rfc3339(),
        }))),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await?;
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    crate::controllers::media::write_version(
        ctx,
        agent.org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: format!("live-{part}.webm"),
            content_type: "video/webm".into(),
            note: None,
            bytes,
        },
        &settings,
    )
    .await
    .map_err(|e| ExtensionError::Db(DbErr::Custom(format!("live part: {e}"))))?;
    report_state(&ctx.db, agent, id, MissionState::Running, None).await?;
    Ok(ba::LivePartStored {
        asset_id: asset.id.to_string(),
        part,
    })
}

/// Registry of the org, newest first (SQL pagination: it grows).
pub async fn list_videos(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<ba::SocialVideo>), ExtensionError> {
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    let count = Count::find_by_statement(stmt(
        "SELECT count(*) AS n FROM social_videos WHERE org_id = $1",
        vec![org_id.into()],
    ))
    .one(db)
    .await?
    .map_or(0, |c| c.n);
    let rows = VideoRow::find_by_statement(stmt(
        &format!(
            "SELECT {VIDEO_COLUMNS} FROM social_videos WHERE org_id = $1
             ORDER BY first_seen_at DESC, id LIMIT $2 OFFSET $3"
        ),
        vec![org_id.into(), limit.into(), offset.into()],
    ))
    .all(db)
    .await?;
    Ok((count, rows.iter().map(VideoRow::view).collect()))
}

async fn mission_cursor(db: &DatabaseConnection, id: Uuid) -> Result<serde_json::Value, DbErr> {
    #[derive(FromQueryResult)]
    struct Cursor {
        cursor: Option<serde_json::Value>,
    }
    Ok(Cursor::find_by_statement(stmt(
        "SELECT cursor FROM extension_missions WHERE id = $1",
        vec![id.into()],
    ))
    .one(db)
    .await?
    .and_then(|c| c.cursor)
    .unwrap_or(serde_json::Value::Null))
}

// ───────────────────────── Leases (D257) ─────────────────────────

/// Granted when free or already this session's; renews the TTL.
const CLAIM: &str = r"
local cur = redis.call('GET', KEYS[1])
if cur and cur ~= ARGV[1] then return 0 end
redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
return 1";

const RELEASE: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end
return 0";

fn lease_key(org_id: i64, target: &str) -> String {
    format!(
        "pnex:ext:v1:{org_id}:lease:{}",
        sha256_hex(target.as_bytes())
    )
}

/// Claims (or renews) the lease of `target` for `session`.
pub async fn claim_lease(config: &Config, org_id: i64, target: &str, session: &str) -> bool {
    let Some(mut conn) = crate::services::shared_valkey::conn(config).await else {
        return false;
    };
    redis::Script::new(CLAIM)
        .key(lease_key(org_id, target))
        .arg(session)
        .arg(LEASE_TTL_MS)
        .invoke_async::<i64>(&mut conn)
        .await
        .is_ok_and(|granted| granted == 1)
}

/// Releases the lease of `target` if `session` still holds it.
pub async fn release_lease(config: &Config, org_id: i64, target: &str, session: &str) {
    if let Some(mut conn) = crate::services::shared_valkey::conn(config).await {
        let _ = redis::Script::new(RELEASE)
            .key(lease_key(org_id, target))
            .arg(session)
            .invoke_async::<i64>(&mut conn)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_key_shape() {
        let mut key = vec![0x04u8];
        key.extend([7u8; 64]);
        assert!(decode_install_key(&URL_SAFE_NO_PAD.encode(&key)).is_some());
        key[0] = 0x02;
        assert!(decode_install_key(&URL_SAFE_NO_PAD.encode(&key)).is_none());
        assert!(decode_install_key("not base64!").is_none());
    }

    #[test]
    fn signature_is_checked_and_bounded_in_time() {
        use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let public = pair.public_key().as_ref().to_vec();
        let now = Utc::now().timestamp_millis();
        let sign = |msg: &str| URL_SAFE_NO_PAD.encode(pair.sign(&rng, msg.as_bytes()).unwrap());
        let msg = ba::pair_message(7, now);
        assert!(signed_now(&public, &msg, now, &sign(&msg)));
        // Another org, or a stale timestamp, never verifies.
        assert!(!signed_now(
            &public,
            &ba::pair_message(8, now),
            now,
            &sign(&msg)
        ));
        let old = now - ba::SIGNED_SKEW_MS - 1;
        let stale = ba::pair_message(7, old);
        assert!(!signed_now(&public, &stale, old, &sign(&stale)));
    }
}
