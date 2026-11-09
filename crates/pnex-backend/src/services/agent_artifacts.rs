//! Artifacts of agent runs (agent-environments.md D729, D744). S3 is the only store between
//! runs, under `org_<id>/agent-artifacts/`; the server alone talks to it. A sandbox reaches
//! the transfer routes with an artifact token of its run, valid while its job runs and only
//! for the names that run declared.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use loco_rs::config::Config;
use pnex_core::agent_runs::{
    AgentArtifact, ArtifactForm, ArtifactInput, ArtifactOutput, ArtifactState,
    ARTIFACT_TOKEN_PREFIX,
};
use pnex_core::runners::SandboxArtifact;
use rand::RngExt;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, QueryResult, Statement};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::artifact_store::{S3Store, StreamWriteError};

/// Limits and keeping of artifacts: global settings (`settings.agents.artifacts`), chosen by
/// whoever runs the platform, not values of the code (D729, Q-E14).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct ArtifactSettings {
    pub max_bytes: u64,
    /// All the outputs of one run together.
    pub run_max_bytes: u64,
    pub expires_days: u32,
}

impl Default for ArtifactSettings {
    fn default() -> Self {
        Self {
            max_bytes: 1 << 30,
            run_max_bytes: 4 << 30,
            expires_days: 30,
        }
    }
}

impl ArtifactSettings {
    pub fn from_config(config: &Config) -> Self {
        config
            .settings
            .as_ref()
            .and_then(|s| s.get("agents"))
            .and_then(|a| a.get("artifacts"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum ArtifactError {
    NotFound,
    /// Over a limit: refused whole.
    TooLarge,
    /// Not a declared output of this run, or already sent.
    NotDeclared,
    AlreadySent,
    Interrupted,
    Storage(String),
    Db(DbErr),
}

impl From<DbErr> for ArtifactError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

/// A run's transfer containers, while its job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferPrincipal {
    pub run_id: Uuid,
    pub org_id: i64,
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    format!(
        "{ARTIFACT_TOKEN_PREFIX}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

pub fn token_hash(token: &str) -> String {
    hash(token)
}

/// Where an artifact lives: under its org always, then its run or the uploads.
pub fn storage_key(org_id: i64, run: Option<Uuid>, id: Uuid) -> String {
    match run {
        Some(run) => format!("org_{org_id}/agent-artifacts/{run}/{id}"),
        None => format!("org_{org_id}/agent-artifacts/uploads/{id}"),
    }
}

const ISO: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn select() -> String {
    format!(
        "SELECT id, run_id, name, state, form, error, sha256, size, \
           to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
           to_char(expires_at AT TIME ZONE 'UTC', {ISO}) AS expires_at \
         FROM agent_artifacts"
    )
}

fn view(r: &QueryResult) -> Result<AgentArtifact, DbErr> {
    let state: String = r.try_get("", "state")?;
    let form: Option<String> = r.try_get("", "form")?;
    Ok(AgentArtifact {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        run_id: r
            .try_get::<Option<Uuid>>("", "run_id")?
            .map(|u| u.to_string()),
        name: r.try_get("", "name")?,
        state: ArtifactState::parse(&state)
            .ok_or_else(|| DbErr::Custom(format!("unknown artifact state {state}")))?,
        form: form.as_deref().and_then(ArtifactForm::parse),
        error: r.try_get("", "error")?,
        sha256: r.try_get("", "sha256")?,
        size: r.try_get("", "size")?,
        created_at: r.try_get("", "created_at")?,
        expires_at: r.try_get("", "expires_at")?,
    })
}

pub async fn get<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<AgentArtifact, ArtifactError> {
    let row = db
        .query_one_raw(stmt(
            &format!("{} WHERE org_id = $1 AND id = $2", select()),
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(ArtifactError::NotFound)?;
    Ok(view(&row)?)
}

/// The artifacts a run produced, by name.
pub async fn of_run<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    run: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<AgentArtifact>), DbErr> {
    let total: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agent_artifacts WHERE org_id = $1 AND run_id = $2",
            vec![org_id.into(), run.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{} WHERE org_id = $1 AND run_id = $2 ORDER BY name LIMIT $3 OFFSET $4",
                select()
            ),
            vec![org_id.into(), run.into(), limit.into(), offset.into()],
        ))
        .await?;
    Ok((total, rows.iter().map(view).collect::<Result<_, _>>()?))
}

/// A body written to `key` while its SHA-256 is computed. `(size, "sha256:<hex>")`.
async fn write_hashed<S, E>(
    store: &S3Store,
    key: &str,
    body: S,
    max: u64,
) -> Result<(u64, String), ArtifactError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let hasher = Arc::new(Mutex::new(Sha256::new()));
    let tap = hasher.clone();
    let body = body.map(move |chunk| {
        if let Ok(bytes) = &chunk {
            if let Ok(mut h) = tap.lock() {
                h.update(bytes);
            }
        }
        chunk
    });
    let size = store
        .write_stream(key, body, max)
        .await
        .map_err(|e| match e {
            StreamWriteError::TooLarge => ArtifactError::TooLarge,
            StreamWriteError::Interrupted => ArtifactError::Interrupted,
            StreamWriteError::Storage(e) => ArtifactError::Storage(e),
        })?;
    let digest = hasher
        .lock()
        .map(|h| h.clone().finalize())
        .map_err(|_| ArtifactError::Storage("hasher poisoned".into()))?;
    Ok((size, format!("sha256:{}", hex::encode(digest))))
}

/// A file a person gives to later runs.
pub async fn upload<S, E, C: ConnectionTrait>(
    db: &C,
    store: &S3Store,
    settings: ArtifactSettings,
    org_id: i64,
    user_id: i64,
    name: &str,
    form: ArtifactForm,
    body: S,
) -> Result<AgentArtifact, ArtifactError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let id = Uuid::new_v4();
    let key = storage_key(org_id, None, id);
    let (size, sha) = write_hashed(store, &key, body, settings.max_bytes).await?;
    db.execute_raw(stmt(
        "INSERT INTO agent_artifacts \
           (id, org_id, name, state, form, sha256, size, storage_key, created_by, expires_at) \
         VALUES ($1, $2, $3, 'ready', $4, $5, $6, $7, $8, now() + make_interval(days => $9))",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            form.as_str().into(),
            sha.into(),
            (size as i64).into(),
            key.into(),
            user_id.into(),
            (settings.expires_days as i32).into(),
        ],
    ))
    .await?;
    get(db, org_id, id).await
}

/// Checks that each input is a ready artifact of the org, not expired: the rights of the
/// author of the run are those of its org (D381, R1).
pub async fn check_inputs<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    inputs: &[ArtifactInput],
) -> Result<(), ArtifactError> {
    for input in inputs {
        let id = Uuid::parse_str(&input.artifact_id).map_err(|_| ArtifactError::NotFound)?;
        let ready = db
            .query_one_raw(stmt(
                "SELECT 1 AS ok FROM agent_artifacts WHERE org_id = $1 AND id = $2 \
                   AND state = 'ready' AND expires_at > now()",
                vec![org_id.into(), id.into()],
            ))
            .await?;
        if ready.is_none() {
            return Err(ArtifactError::NotFound);
        }
    }
    Ok(())
}

/// One pending row per declared output, at the creation of the run.
pub async fn declare_outputs<C: ConnectionTrait>(
    db: &C,
    settings: ArtifactSettings,
    org_id: i64,
    run: Uuid,
    outputs: &[ArtifactOutput],
) -> Result<(), DbErr> {
    for output in outputs {
        let id = Uuid::new_v4();
        db.execute_raw(stmt(
            "INSERT INTO agent_artifacts (id, org_id, run_id, name, state, storage_key, expires_at) \
             VALUES ($1, $2, $3, $4, 'pending', $5, now() + make_interval(days => $6))",
            vec![
                id.into(),
                org_id.into(),
                run.into(),
                output.name.clone().into(),
                storage_key(org_id, Some(run), id).into(),
                (settings.expires_days as i32).into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

/// What the transfer containers of a run get: each input with the checksum it must have,
/// each output by name.
pub async fn sandbox_lists<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    inputs: &[ArtifactInput],
    outputs: &[ArtifactOutput],
) -> Result<(Vec<SandboxArtifact>, Vec<SandboxArtifact>), DbErr> {
    let mut ins = Vec::new();
    for input in inputs {
        let Ok(id) = Uuid::parse_str(&input.artifact_id) else {
            continue;
        };
        let Some(row) = db
            .query_one_raw(stmt(
                "SELECT sha256, form FROM agent_artifacts WHERE org_id = $1 AND id = $2",
                vec![org_id.into(), id.into()],
            ))
            .await?
        else {
            continue;
        };
        let form: Option<String> = row.try_get("", "form")?;
        ins.push(SandboxArtifact {
            name: input.name.clone(),
            sha256: row.try_get("", "sha256")?,
            form: form.as_deref().and_then(ArtifactForm::parse),
        });
    }
    let outs = outputs
        .iter()
        .map(|o| SandboxArtifact {
            name: o.name.clone(),
            sha256: None,
            form: None,
        })
        .collect();
    Ok((ins, outs))
}

/// The run behind an artifact token, only while its job runs.
pub async fn authenticate<C: ConnectionTrait>(
    db: &C,
    token: &str,
) -> Result<Option<TransferPrincipal>, DbErr> {
    if !token.starts_with(ARTIFACT_TOKEN_PREFIX) {
        return Ok(None);
    }
    let Some(r) = db
        .query_one_raw(stmt(
            "SELECT a.id, a.org_id FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id \
             WHERE a.artifact_token_hash = $1 AND j.state = 'running'",
            vec![hash(token).into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(TransferPrincipal {
        run_id: r.try_get("", "id")?,
        org_id: r.try_get("", "org_id")?,
    }))
}

/// `(storage key, size)` of an input the run declared under `name`.
pub async fn input_of<C: ConnectionTrait>(
    db: &C,
    run: TransferPrincipal,
    name: &str,
) -> Result<(String, u64), ArtifactError> {
    let row = db
        .query_one_raw(stmt(
            "SELECT f.storage_key, f.size FROM agent_runs a \
             CROSS JOIN LATERAL jsonb_array_elements(a.inputs) i \
             JOIN agent_artifacts f ON f.org_id = a.org_id AND f.id = (i->>'artifact_id')::uuid \
             WHERE a.id = $1 AND a.org_id = $2 AND i->>'name' = $3 AND f.state = 'ready'",
            vec![run.run_id.into(), run.org_id.into(), name.into()],
        ))
        .await?
        .ok_or(ArtifactError::NotDeclared)?;
    let size: Option<i64> = row.try_get("", "size")?;
    Ok((row.try_get("", "storage_key")?, size.unwrap_or(0) as u64))
}

/// Receives a declared output, once. Over a limit, the artifact is refused and says so.
pub async fn receive_output<S, E, C: ConnectionTrait>(
    db: &C,
    store: &S3Store,
    settings: ArtifactSettings,
    run: TransferPrincipal,
    name: &str,
    form: ArtifactForm,
    body: S,
) -> Result<AgentArtifact, ArtifactError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    // Taken first: a second sender of the same name finds it no longer pending.
    let row = db
        .query_one_raw(stmt(
            "UPDATE agent_artifacts SET error = 'agent-artifact-receiving' \
             WHERE org_id = $1 AND run_id = $2 AND name = $3 AND state = 'pending' \
               AND error IS DISTINCT FROM 'agent-artifact-receiving' \
             RETURNING id, storage_key",
            vec![run.org_id.into(), run.run_id.into(), name.into()],
        ))
        .await?;
    let Some(row) = row else {
        let exists = db
            .query_one_raw(stmt(
                "SELECT 1 AS ok FROM agent_artifacts WHERE org_id = $1 AND run_id = $2 AND name = $3",
                vec![run.org_id.into(), run.run_id.into(), name.into()],
            ))
            .await?;
        return Err(if exists.is_some() {
            ArtifactError::AlreadySent
        } else {
            ArtifactError::NotDeclared
        });
    };
    let id: Uuid = row.try_get("", "id")?;
    let key: String = row.try_get("", "storage_key")?;
    let used: i64 = db
        .query_one_raw(stmt(
            "SELECT coalesce(sum(size), 0)::bigint AS n FROM agent_artifacts \
             WHERE org_id = $1 AND run_id = $2 AND state = 'ready'",
            vec![run.org_id.into(), run.run_id.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let room = settings
        .run_max_bytes
        .saturating_sub(used.max(0) as u64)
        .min(settings.max_bytes);
    match write_hashed(store, &key, body, room).await {
        Ok((size, sha)) => {
            db.execute_raw(stmt(
                "UPDATE agent_artifacts SET state = 'ready', error = NULL, form = $2, \
                   sha256 = $3, size = $4 WHERE id = $1",
                vec![
                    id.into(),
                    form.as_str().into(),
                    sha.into(),
                    (size as i64).into(),
                ],
            ))
            .await?;
            get(db, run.org_id, id).await
        }
        Err(e) => {
            let code = match &e {
                ArtifactError::TooLarge => "agent-artifact-too-large",
                _ => "agent-artifact-interrupted",
            };
            // An interrupted send may come again; a too large one never fits.
            let state = if matches!(e, ArtifactError::TooLarge) {
                "refused"
            } else {
                "pending"
            };
            db.execute_raw(stmt(
                "UPDATE agent_artifacts SET state = $2, error = $3 WHERE id = $1",
                vec![id.into(), state.into(), code.into()],
            ))
            .await?;
            Err(e)
        }
    }
}

/// At the end of a sandbox job: what was not sent is missing. `Some(true)` when a required
/// output is among them; `None` when no run stands behind the job.
pub async fn close_outputs<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    job: Uuid,
) -> Result<Option<bool>, DbErr> {
    let Some(run) = db
        .query_one_raw(stmt(
            "SELECT id, outputs FROM agent_runs WHERE org_id = $1 AND job_id = $2",
            vec![org_id.into(), job.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let run_id: Uuid = run.try_get("", "id")?;
    let outputs: Vec<ArtifactOutput> =
        serde_json::from_value(run.try_get("", "outputs")?).unwrap_or_default();
    let missing = db
        .query_all_raw(stmt(
            "UPDATE agent_artifacts SET state = 'missing', error = NULL \
             WHERE org_id = $1 AND run_id = $2 AND state = 'pending' RETURNING name",
            vec![org_id.into(), run_id.into()],
        ))
        .await?;
    let missing: Vec<String> = missing
        .iter()
        .filter_map(|r| r.try_get::<String>("", "name").ok())
        .collect();
    let required_missing = outputs
        .iter()
        .any(|o| o.required && missing.contains(&o.name));
    if required_missing {
        db.execute_raw(stmt(
            "UPDATE agent_runs SET error = 'agent-run-output-missing' WHERE id = $1",
            vec![run_id.into()],
        ))
        .await?;
    }
    Ok(Some(required_missing))
}

/// The bytes of a stored artifact, read in parts.
pub fn read_stream(
    store: S3Store,
    key: String,
    size: u64,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    const PART: u64 = 8 * 1024 * 1024;
    futures_util::stream::unfold(0u64, move |at| {
        let (store, key) = (store.clone(), key.clone());
        async move {
            if at >= size {
                return None;
            }
            let end = (at + PART).min(size);
            Some(match store.get_range(&key, at..end).await {
                Ok(bytes) => (Ok(bytes), end),
                Err(e) => (Err(std::io::Error::other(e)), size),
            })
        }
    })
}

/// Removes artifacts past their expiry, objects first. Returns how many went.
pub async fn sweep<C: ConnectionTrait>(db: &C, store: &S3Store) -> Result<usize, String> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT id, storage_key FROM agent_artifacts WHERE expires_at <= now() LIMIT 500",
            vec![],
        ))
        .await
        .map_err(|e| e.to_string())?;
    let mut gone = 0;
    for r in rows {
        let id: Uuid = r.try_get("", "id").map_err(|e| e.to_string())?;
        let key: String = r.try_get("", "storage_key").map_err(|e| e.to_string())?;
        store.delete_key(&key).await?;
        db.execute_raw(stmt(
            "DELETE FROM agent_artifacts WHERE id = $1",
            vec![id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
        gone += 1;
    }
    Ok(gone)
}

/// Limits of dependency caches: global settings (`settings.agents.depcache`, D730, D755).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct CacheSettings {
    pub max_bytes: u64,
    pub expires_days: u32,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            max_bytes: 8 << 30,
            expires_days: 14,
        }
    }
}

impl CacheSettings {
    pub fn from_config(config: &Config) -> Self {
        config
            .settings
            .as_ref()
            .and_then(|s| s.get("agents"))
            .and_then(|a| a.get("depcache"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
}

/// `none`, or the first 32 hex characters of a SHA-256: a path segment as is.
pub fn cache_key_is_valid(key: &str) -> bool {
    key == "none" || (key.len() == 32 && key.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Prefix of the archives of one cache of an environment.
pub fn cache_prefix(org_id: i64, environment: Uuid, name: &str) -> String {
    format!("org_{org_id}/depcache/{environment}/{name}/")
}

/// The environment of the run, if it declared this cache.
pub async fn cache_of<C: ConnectionTrait>(
    db: &C,
    run: TransferPrincipal,
    name: &str,
) -> Result<Uuid, ArtifactError> {
    let row = db
        .query_one_raw(stmt(
            "SELECT environment_id FROM agent_runs WHERE id = $1 AND org_id = $2 \
               AND caches @> jsonb_build_array(jsonb_build_object('name', $3::text))",
            vec![run.run_id.into(), run.org_id.into(), name.into()],
        ))
        .await?
        .ok_or(ArtifactError::NotDeclared)?;
    Ok(row.try_get("", "environment_id")?)
}

/// `(storage key, size, exact)` of the archive to restore: the one of this key, else the
/// latest of the same cache; `None` before the first save.
pub async fn cache_archive(
    store: &S3Store,
    prefix: &str,
    key: &str,
) -> Result<Option<(String, u64, bool)>, ArtifactError> {
    let exact = format!("{prefix}{key}");
    if let Some(size) = store.size(&exact).await.map_err(ArtifactError::Storage)? {
        return Ok(Some((exact, size, true)));
    }
    let mut all = store
        .list_prefix(prefix)
        .await
        .map_err(ArtifactError::Storage)?;
    all.sort_by_key(|(_, _, at)| *at);
    Ok(all.pop().map(|(k, size, _)| (k, size, false)))
}

/// Saves the archive of a cache under its key, refused whole past the limit.
pub async fn save_cache<S, E>(
    store: &S3Store,
    settings: CacheSettings,
    prefix: &str,
    key: &str,
    body: S,
) -> Result<u64, ArtifactError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    store
        .write_stream(&format!("{prefix}{key}"), body, settings.max_bytes)
        .await
        .map_err(|e| match e {
            StreamWriteError::TooLarge => ArtifactError::TooLarge,
            StreamWriteError::Interrupted => ArtifactError::Interrupted,
            StreamWriteError::Storage(e) => ArtifactError::Storage(e),
        })
}

/// Removes cache archives untouched for longer than their expiry, org by org.
pub async fn sweep_caches<C: ConnectionTrait>(
    db: &C,
    store: &S3Store,
    settings: CacheSettings,
) -> Result<usize, String> {
    let orgs = db
        .query_all_raw(stmt("SELECT DISTINCT org_id FROM environments", vec![]))
        .await
        .map_err(|e| e.to_string())?;
    let limit = chrono::Utc::now() - chrono::Duration::days(i64::from(settings.expires_days));
    let mut gone = 0;
    for r in orgs {
        let org: i64 = r.try_get("", "org_id").map_err(|e| e.to_string())?;
        for (key, _, at) in store.list_prefix(&format!("org_{org}/depcache/")).await? {
            if at.is_some_and(|t| t < limit) {
                store.delete_key(&key).await?;
                gone += 1;
            }
        }
    }
    Ok(gone)
}

const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Removes expired artifacts every hour, on one server of the cluster.
pub fn spawn_sweeper(ctx: &loco_rs::app::AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            if !crate::services::singleton::my_turn(&ctx.db, "agent-artifact-sweep", SWEEP_EVERY)
                .await
            {
                continue;
            }
            let store =
                match crate::services::artifact_store::S3Config::from_config(&ctx.config).store() {
                    Ok(store) => store,
                    Err(_) => continue,
                };
            match sweep(&ctx.db, &store).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(n, "expired agent artifacts removed"),
                Err(e) => tracing::warn!("agent artifact sweep failed: {e}"),
            }
            let caches = crate::services::fabric_settings::caches(&ctx).await;
            match sweep_caches(&ctx.db, &store, caches).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(n, "expired dependency caches removed"),
                Err(e) => tracing::warn!("dependency cache sweep failed: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_stay_under_their_org() {
        let id = Uuid::nil();
        assert!(storage_key(7, None, id).starts_with("org_7/agent-artifacts/uploads/"));
        assert!(storage_key(7, Some(id), id).starts_with(&format!("org_7/agent-artifacts/{id}/")));
        assert!(new_token().starts_with(ARTIFACT_TOKEN_PREFIX));
        assert_ne!(new_token(), new_token());
    }

    #[test]
    fn limits_have_defaults_and_read_from_settings() {
        let s = ArtifactSettings::default();
        assert!(s.max_bytes <= s.run_max_bytes && s.expires_days > 0);
        let parsed: ArtifactSettings =
            serde_json::from_value(serde_json::json!({ "max_bytes": 5 })).unwrap();
        assert_eq!(parsed.max_bytes, 5);
        assert_eq!(parsed.expires_days, s.expires_days);
    }
}
