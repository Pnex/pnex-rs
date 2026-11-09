//! Agent environments (agent-environments.md D702, D704): versions in the database,
//! immutable, each with the SHA-256 of its source. A build is a runner job: the server
//! keeps no credential with the job, and hands them over at the claim only (D701).

use pnex_core::environments::{
    Environment, EnvironmentBuild, EnvironmentBuildState, EnvironmentInput, EnvironmentScan,
    EnvironmentScanState, EnvironmentSource, EnvironmentVersion, EnvironmentVersionInput,
    EnvironmentVersionSummary, Isolation, IsolationInput, ScanMode, ScanPolicy, ScanReport,
    ScanThreshold,
};
use pnex_core::runners::{
    digest_is_valid, RunnerBuildInput, RunnerJobCreate, RunnerJobKind, RunnerJobSpec,
    RunnerScanInput, TIMEOUT_MAX_SECS,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::artifact_store::S3Config;
use crate::services::secrets::{store, Keyring};
use crate::services::{build_cache, runners};

/// What a runner announces when its cluster runs a build pod (D701).
pub const BUILD_CAPABILITY: &str = "builder:buildkit";
/// What a runner announces when it can scan an image (D720).
pub const SCAN_CAPABILITY: &str = "scanner:trivy";
/// The scan needs its vulnerability database and the image, nothing more.
const SCAN_TIMEOUT_SECS: u32 = 900;

#[derive(Debug)]
pub enum EnvironmentError {
    Invalid {
        field: String,
        token: String,
    },
    NotFound,
    VersionMissing,
    NameTaken,
    VersionConflict {
        current: Option<i32>,
    },
    /// A credential in the source, at this file or build argument.
    SecretInSource {
        at: String,
    },
    BuildMissing,
    BuildUnavailable,
    NoWriteSecret,
    BuildRunning,
    ScanMissing,
    BuildNotReady,
    /// Over the scan threshold while the org refuses such versions (D748).
    ScanRefused {
        breaches: u32,
    },
    Db(DbErr),
}

impl From<DbErr> for EnvironmentError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const ISO: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn select() -> String {
    format!(
        "SELECT id, name, description, registry_id, latest_version, active_build_id, version, \
           isolation, \
           (SELECT b.repository || '@' || b.digest FROM environment_builds b \
            WHERE b.id = active_build_id) AS active_image, \
           to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
           to_char(updated_at AT TIME ZONE 'UTC', {ISO}) AS updated_at FROM environments"
    )
}

fn view(r: &QueryResult) -> Result<Environment, DbErr> {
    Ok(Environment {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        description: r.try_get("", "description")?,
        registry_id: r.try_get::<Uuid>("", "registry_id")?.to_string(),
        latest_version: r.try_get("", "latest_version")?,
        active_build_id: r
            .try_get::<Option<Uuid>>("", "active_build_id")?
            .map(|u| u.to_string()),
        active_image: r.try_get("", "active_image")?,
        isolation: Isolation::parse(&r.try_get::<String>("", "isolation")?).unwrap_or_default(),
        version: r.try_get("", "version")?,
        created_at: r.try_get("", "created_at")?,
        updated_at: r.try_get("", "updated_at")?,
    })
}

fn invalid(field: &str, token: &str) -> EnvironmentError {
    EnvironmentError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

/// The input, with its registry checked to belong to the org.
async fn checked<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    input: &EnvironmentInput,
) -> Result<Uuid, EnvironmentError> {
    if let Some((field, token)) = input.violation() {
        return Err(invalid(field, &token));
    }
    let registry =
        Uuid::parse_str(&input.registry_id).map_err(|_| invalid("registry_id", "invalid"))?;
    db.query_one_raw(stmt(
        "SELECT 1 AS x FROM image_registries WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), registry.into()],
    ))
    .await?
    .ok_or_else(|| invalid("registry_id", "invalid"))?;
    Ok(registry)
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    other_than: Uuid,
) -> Result<bool, DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM environments WHERE org_id = $1 AND name = $2 AND id <> $3",
            vec![org_id.into(), name.into(), other_than.into()],
        ))
        .await?
        .is_some())
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Environment, EnvironmentError> {
    let sql = format!("{} WHERE org_id = $1 AND id = $2", select());
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(EnvironmentError::NotFound)?;
    Ok(view(&row)?)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Environment>), EnvironmentError> {
    let total = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM environments WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql = format!(
        "{} WHERE org_id = $1 ORDER BY name LIMIT $2 OFFSET $3",
        select()
    );
    let rows = db
        .query_all_raw(stmt(&sql, vec![org_id.into(), limit.into(), offset.into()]))
        .await?;
    Ok((total, rows.iter().map(view).collect::<Result<_, _>>()?))
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    input: &EnvironmentInput,
) -> Result<Environment, EnvironmentError> {
    let registry = checked(db, org_id, input).await?;
    let name = input.name.trim();
    let id = Uuid::new_v4();
    if name_taken(db, org_id, name, id).await? {
        return Err(EnvironmentError::NameTaken);
    }
    db.execute_raw(stmt(
        "INSERT INTO environments (id, org_id, name, description, registry_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            input.description.clone().into(),
            registry.into(),
            by.into(),
        ],
    ))
    .await?;
    get(db, org_id, id).await
}

/// Sets where the runs of the environment execute, on the version that was read (D763).
pub async fn set_isolation(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &IsolationInput,
) -> Result<Environment, EnvironmentError> {
    let current = get(db, org_id, id).await?;
    let done = db
        .execute_raw(stmt(
            "UPDATE environments SET isolation = $3, version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $4",
            vec![
                org_id.into(),
                id.into(),
                input.isolation.as_str().into(),
                input.expected_version.into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(EnvironmentError::VersionConflict {
            current: Some(current.version),
        });
    }
    get(db, org_id, id).await
}

pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &EnvironmentInput,
) -> Result<Environment, EnvironmentError> {
    let registry = checked(db, org_id, input).await?;
    let Some(expected) = input.expected_version else {
        return Err(invalid("expected_version", "required"));
    };
    let current = get(db, org_id, id).await?;
    let name = input.name.trim();
    if name_taken(db, org_id, name, id).await? {
        return Err(EnvironmentError::NameTaken);
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE environments SET name = $3, description = $4, registry_id = $5, \
               version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $6",
            vec![
                org_id.into(),
                id.into(),
                name.into(),
                input.description.clone().into(),
                registry.into(),
                expected.into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(EnvironmentError::VersionConflict {
            current: Some(current.version),
        });
    }
    get(db, org_id, id).await
}

pub async fn delete(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<(), EnvironmentError> {
    let done = db
        .execute_raw(stmt(
            "DELETE FROM environments WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(EnvironmentError::NotFound);
    }
    Ok(())
}

/// SHA-256 of the source in one fixed form: the maps serialize in key order.
fn fingerprint(source: &EnvironmentSource) -> Result<String, DbErr> {
    let bytes = serde_json::to_vec(source).map_err(|e| DbErr::Custom(e.to_string()))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn summary_of(r: &QueryResult) -> Result<EnvironmentVersionSummary, DbErr> {
    Ok(EnvironmentVersionSummary {
        number: r.try_get("", "number")?,
        fingerprint: r.try_get("", "fingerprint")?,
        created_at: r.try_get("", "created_at")?,
    })
}

/// Saves a new version, on the latest one that was read.
pub async fn create_version(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    id: Uuid,
    input: &EnvironmentVersionInput,
) -> Result<EnvironmentVersionSummary, EnvironmentError> {
    if let Some((field, token)) = input.source.violation() {
        return Err(invalid(field, &token));
    }
    if let Some(at) = input.source.secret_at() {
        return Err(EnvironmentError::SecretInSource { at });
    }
    let print = fingerprint(&input.source)?;
    let txn = db.begin().await?;
    let row = txn
        .query_one_raw(stmt(
            "SELECT latest_version FROM environments WHERE org_id = $1 AND id = $2 FOR UPDATE",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(EnvironmentError::NotFound)?;
    let latest: Option<i32> = row.try_get("", "latest_version")?;
    if latest != input.expected_latest {
        return Err(EnvironmentError::VersionConflict { current: latest });
    }
    let number = latest.unwrap_or(0) + 1;
    let saved = txn
        .query_one_raw(stmt(
            &format!(
                "INSERT INTO environment_versions \
                   (environment_id, number, dockerfile, files, build_args, fingerprint, created_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 RETURNING number, fingerprint, \
                   to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at"
            ),
            vec![
                id.into(),
                number.into(),
                input.source.dockerfile.clone().into(),
                serde_json::json!(input.source.files).into(),
                serde_json::json!(input.source.build_args).into(),
                print.into(),
                by.into(),
            ],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("version not inserted".into()))?;
    txn.execute_raw(stmt(
        "UPDATE environments SET latest_version = $2, updated_at = now() WHERE id = $1",
        vec![id.into(), number.into()],
    ))
    .await?;
    let summary = summary_of(&saved)?;
    txn.commit().await?;
    Ok(summary)
}

pub async fn list_versions(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<EnvironmentVersionSummary>), EnvironmentError> {
    let total = get(db, org_id, id).await?.latest_version.unwrap_or(0) as i64;
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT number, fingerprint, \
                   to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at \
                 FROM environment_versions WHERE environment_id = $1 \
                 ORDER BY number DESC LIMIT $2 OFFSET $3"
            ),
            vec![id.into(), limit.into(), offset.into()],
        ))
        .await?;
    Ok((
        total,
        rows.iter().map(summary_of).collect::<Result<_, _>>()?,
    ))
}

pub async fn get_version(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    number: i32,
) -> Result<EnvironmentVersion, EnvironmentError> {
    get(db, org_id, id).await?;
    let row = db
        .query_one_raw(stmt(
            &format!(
                "SELECT number, fingerprint, dockerfile, files, build_args, \
                   to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at \
                 FROM environment_versions WHERE environment_id = $1 AND number = $2"
            ),
            vec![id.into(), number.into()],
        ))
        .await?
        .ok_or(EnvironmentError::VersionMissing)?;
    let map = |column: &str| -> Result<_, DbErr> {
        serde_json::from_value(row.try_get::<serde_json::Value>("", column)?)
            .map_err(|e| DbErr::Custom(e.to_string()))
    };
    Ok(EnvironmentVersion {
        summary: summary_of(&row)?,
        source: EnvironmentSource {
            dockerfile: row.try_get("", "dockerfile")?,
            files: map("files")?,
            build_args: map("build_args")?,
        },
    })
}

const BUILD_SELECT: &str =
    "SELECT b.id, b.number, b.job_id, b.repository, b.digest, j.state, j.outcome, \
    to_char(b.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    to_char(j.finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS finished_at, \
    s.report AS scan_report, sj.state AS scan_job_state \
    FROM environment_builds b JOIN runner_jobs j ON j.id = b.job_id \
    LEFT JOIN LATERAL (SELECT report, job_id FROM environment_scans \
      WHERE build_id = b.id ORDER BY created_at DESC LIMIT 1) s ON true \
    LEFT JOIN runner_jobs sj ON sj.id = s.job_id";

/// A scan reads like a build: its job says where it is, and a job that succeeded without
/// a usable report failed.
fn scan_state(job: &str, has_report: bool) -> EnvironmentScanState {
    match (job, has_report) {
        ("queued" | "running", _) => EnvironmentScanState::Scanning,
        ("succeeded", true) => EnvironmentScanState::Done,
        _ => EnvironmentScanState::Failed,
    }
}

fn report_of(r: &QueryResult, column: &str) -> Result<Option<ScanReport>, DbErr> {
    r.try_get::<Option<serde_json::Value>>("", column)?
        .map(|v| serde_json::from_value(v).map_err(|e| DbErr::Custom(e.to_string())))
        .transpose()
}

/// The state comes from the job, so that the reaper needs to know nothing of builds. A
/// job that succeeded without a digest pushed nothing we can run: it failed.
fn build_view(r: &QueryResult) -> Result<EnvironmentBuild, DbErr> {
    let job: String = r.try_get("", "state")?;
    let digest: Option<String> = r.try_get("", "digest")?;
    let repository: String = r.try_get("", "repository")?;
    let report = report_of(r, "scan_report")?;
    let scan_job: Option<String> = r.try_get("", "scan_job_state")?;
    let state = match (job.as_str(), &digest) {
        ("queued" | "running", _) => EnvironmentBuildState::Building,
        ("succeeded", Some(_)) => EnvironmentBuildState::Ready,
        _ => EnvironmentBuildState::Failed,
    };
    Ok(EnvironmentBuild {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        number: r.try_get("", "number")?,
        state,
        image: digest.map(|d| format!("{repository}@{d}")),
        outcome: r.try_get("", "outcome")?,
        job_id: r.try_get::<Uuid>("", "job_id")?.to_string(),
        created_at: r.try_get("", "created_at")?,
        finished_at: r.try_get("", "finished_at")?,
        scan_state: scan_job.map(|j| scan_state(&j, report.is_some())),
        scan_counts: report.as_ref().map(ScanReport::summary),
    })
}

/// Queues a build of a version. `builder_image` is the pinned image of this deployment.
pub async fn start_build(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    id: Uuid,
    number: i32,
    builder_image: Option<&str>,
) -> Result<EnvironmentBuild, EnvironmentError> {
    let spec = RunnerJobSpec {
        image: builder_image
            .ok_or(EnvironmentError::BuildUnavailable)?
            .to_string(),
        command: vec![],
        timeout_secs: TIMEOUT_MAX_SECS,
    };
    if pnex_core::runners::spec_error(&spec).is_some() {
        return Err(EnvironmentError::BuildUnavailable);
    }
    let txn = db.begin().await?;
    // The row lock makes two simultaneous starts wait for each other.
    let env = txn
        .query_one_raw(stmt(
            "SELECT r.host, r.repository, r.write_secret_id FROM environments e \
             JOIN image_registries r ON r.id = e.registry_id \
             WHERE e.org_id = $1 AND e.id = $2 FOR UPDATE OF e",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(EnvironmentError::NotFound)?;
    txn.query_one_raw(stmt(
        "SELECT 1 AS x FROM environment_versions WHERE environment_id = $1 AND number = $2",
        vec![id.into(), number.into()],
    ))
    .await?
    .ok_or(EnvironmentError::VersionMissing)?;
    if env
        .try_get::<Option<Uuid>>("", "write_secret_id")?
        .is_none()
    {
        return Err(EnvironmentError::NoWriteSecret);
    }
    let running = txn
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM environment_builds b JOIN runner_jobs j ON j.id = b.job_id \
             WHERE b.environment_id = $1 AND j.state IN ('queued', 'running') LIMIT 1",
            vec![id.into()],
        ))
        .await?
        .is_some();
    if running {
        return Err(EnvironmentError::BuildRunning);
    }
    let job = runners::create_job(
        &txn,
        org_id,
        by,
        &RunnerJobCreate {
            kind: RunnerJobKind::EnvironmentBuild,
            spec,
            required_capabilities: vec![BUILD_CAPABILITY.to_string()],
        },
    )
    .await?;
    let host: String = env.try_get("", "host")?;
    let repository: String = env.try_get("", "repository")?;
    let build = Uuid::new_v4();
    txn.execute_raw(stmt(
        "INSERT INTO environment_builds \
           (id, org_id, environment_id, number, job_id, repository, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        vec![
            build.into(),
            org_id.into(),
            id.into(),
            number.into(),
            Uuid::parse_str(&job.id)
                .map_err(|e| DbErr::Custom(e.to_string()))?
                .into(),
            format!("{host}/{repository}").into(),
            by.into(),
        ],
    ))
    .await?;
    txn.commit().await?;
    get_build(db, org_id, id, build).await
}

pub async fn get_build(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    build: Uuid,
) -> Result<EnvironmentBuild, EnvironmentError> {
    let sql = format!("{BUILD_SELECT} WHERE b.org_id = $1 AND b.environment_id = $2 AND b.id = $3");
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into(), build.into()]))
        .await?
        .ok_or(EnvironmentError::BuildMissing)?;
    Ok(build_view(&row)?)
}

pub async fn list_builds(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<EnvironmentBuild>), EnvironmentError> {
    get(db, org_id, id).await?;
    let total = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM environment_builds WHERE environment_id = $1",
            vec![id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql = format!(
        "{BUILD_SELECT} WHERE b.environment_id = $1 ORDER BY b.created_at DESC LIMIT $2 OFFSET $3"
    );
    let rows = db
        .query_all_raw(stmt(&sql, vec![id.into(), limit.into(), offset.into()]))
        .await?;
    Ok((
        total,
        rows.iter().map(build_view).collect::<Result<_, _>>()?,
    ))
}

/// What the build pod of a claimed job needs. `None`: nothing usable, the caller fails
/// the job. The registry is read now, so that a changed secret is the one delivered; a
/// registry moved since the start pushes where `repository` said it would, or not at all.
pub async fn build_input(
    db: &DatabaseConnection,
    ring: &Keyring,
    storage: &S3Config,
    org_id: i64,
    job_id: &str,
) -> Option<RunnerBuildInput> {
    let job = Uuid::parse_str(job_id).ok()?;
    let read = async {
        let row = db
            .query_one_raw(stmt(
                "SELECT b.environment_id, b.number, b.repository, r.host, r.repository AS current, \
                   r.tls, r.write_username, r.write_secret_id \
                 FROM environment_builds b \
                 JOIN environments e ON e.id = b.environment_id \
                 JOIN image_registries r ON r.id = e.registry_id \
                 WHERE b.org_id = $1 AND b.job_id = $2",
                vec![org_id.into(), job.into()],
            ))
            .await?;
        Ok::<_, DbErr>(row)
    };
    let row = read.await.ok().flatten()?;
    let got = async {
        let host: String = row.try_get("", "host")?;
        let repository: String = row.try_get("", "repository")?;
        let current: String = row.try_get("", "current")?;
        let number: i32 = row.try_get("", "number")?;
        let environment: Uuid = row.try_get("", "environment_id")?;
        let username: Option<String> = row.try_get("", "write_username")?;
        let secret: Option<Uuid> = row.try_get("", "write_secret_id")?;
        let tls: bool = row.try_get("", "tls")?;
        Ok::<_, DbErr>((
            host,
            repository,
            current,
            number,
            environment,
            username,
            secret,
            tls,
        ))
    };
    let (host, repository, current, number, environment, username, secret, tls) = got.await.ok()?;
    if repository != format!("{host}/{current}") {
        return None;
    }
    let version = get_version(db, org_id, environment, number).await.ok()?;
    let secret = store::reveal(db, ring, Some(org_id), secret?).await.ok()?;
    // Set only where pods can reach the store: its address, and the network rules around
    // it, are the deployment's to decide.
    let cache = match std::env::var("PNEX_BUILD_CACHE_ENDPOINT") {
        Ok(endpoint) => {
            build_cache::for_prefix(
                storage,
                &endpoint,
                &build_cache::prefix_of(org_id, environment),
                TIMEOUT_MAX_SECS,
            )
            .await
        }
        Err(_) => None,
    };
    Some(RunnerBuildInput {
        source: version.source,
        target: format!("{repository}:v{number}"),
        registry_host: host,
        tls,
        username: username?,
        secret,
        cache,
        resources: None,
    })
}

/// Keeps the digest a runner reports for the job it held. A malformed one is dropped:
/// the build then reads as failed.
pub async fn record_digest(
    db: &DatabaseConnection,
    org_id: i64,
    job: Uuid,
    digest: &str,
) -> Result<(), DbErr> {
    if !digest_is_valid(digest) {
        tracing::warn!(%job, "malformed image digest dropped");
        return Ok(());
    }
    db.execute_raw(stmt(
        "UPDATE environment_builds SET digest = $3 WHERE org_id = $1 AND job_id = $2",
        vec![org_id.into(), job.into(), digest.into()],
    ))
    .await?;
    Ok(())
}

/// Makes a ready build the active one: a person's act, replaceable in one more (D704).
pub async fn activate(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    build: Uuid,
    expected_version: i32,
) -> Result<Environment, EnvironmentError> {
    let current = get(db, org_id, id).await?;
    let seen = get_build(db, org_id, id, build).await?;
    if seen.state != EnvironmentBuildState::Ready {
        return Err(EnvironmentError::BuildNotReady);
    }
    if let Some(breaches) = refused_by_scan(db, org_id, build).await? {
        return Err(EnvironmentError::ScanRefused { breaches });
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE environments SET active_build_id = $3, version = version + 1, \
               updated_at = now() WHERE org_id = $1 AND id = $2 AND version = $4",
            vec![
                org_id.into(),
                id.into(),
                build.into(),
                expected_version.into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(EnvironmentError::VersionConflict {
            current: Some(current.version),
        });
    }
    get(db, org_id, id).await
}

const RESCAN_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);
const RESCANS_PER_ROUND: i64 = 100;

/// A flaw is often published after the build: queues a new scan of each active build whose
/// last one is a day old (D720). Returns how many.
pub async fn rescan_due(db: &DatabaseConnection, scanner: Option<&str>) -> Result<usize, DbErr> {
    if scanner.is_none() {
        return Ok(0);
    }
    let rows = db
        .query_all_raw(stmt(
            "SELECT b.org_id, b.job_id FROM environments e \
             JOIN environment_builds b ON b.id = e.active_build_id \
             WHERE b.digest IS NOT NULL AND NOT EXISTS ( \
               SELECT 1 FROM environment_scans s WHERE s.build_id = b.id \
                 AND s.created_at > now() - interval '24 hours') \
             LIMIT $1",
            vec![RESCANS_PER_ROUND.into()],
        ))
        .await?;
    for row in &rows {
        start_scan(
            db,
            row.try_get("", "org_id")?,
            row.try_get("", "job_id")?,
            scanner,
        )
        .await?;
    }
    Ok(rows.len())
}

pub fn spawn_rescans(ctx: &loco_rs::app::AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RESCAN_EVERY).await;
            if crate::services::singleton::my_turn(&ctx.db, "environment-rescans", RESCAN_EVERY)
                .await
            {
                let scanner = std::env::var("PNEX_IMAGE_SCANNER").ok();
                match rescan_due(&ctx.db, scanner.as_deref()).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(n, "environment images queued for a new scan"),
                    Err(e) => tracing::warn!("environment rescans failed: {e}"),
                }
            }
        }
    });
}

/// Queues the scan of a build that just got its digest. `scanner` is the pinned image of
/// this deployment; without one, images simply go unscanned. Its failure never touches
/// the build: the image is there, and a scan can run again.
pub async fn start_scan(
    db: &DatabaseConnection,
    org_id: i64,
    build_job: Uuid,
    scanner: Option<&str>,
) -> Result<(), DbErr> {
    let Some(scanner) = scanner else {
        return Ok(());
    };
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT id, repository, digest, created_by FROM environment_builds \
             WHERE org_id = $1 AND job_id = $2 AND digest IS NOT NULL",
            vec![org_id.into(), build_job.into()],
        ))
        .await?
    else {
        return Ok(());
    };
    let repository: String = row.try_get("", "repository")?;
    let digest: String = row.try_get("", "digest")?;
    let spec = RunnerJobSpec {
        image: scanner.to_string(),
        command: pnex_core::runners::scan_command(&format!("{repository}@{digest}")),
        timeout_secs: SCAN_TIMEOUT_SECS,
    };
    if pnex_core::runners::spec_error(&spec).is_some() {
        tracing::warn!("scanner image is not pinned: no scan");
        return Ok(());
    }
    let Some(by) = row.try_get::<Option<i64>>("", "created_by")? else {
        return Ok(());
    };
    let txn = db.begin().await?;
    let job = runners::create_job(
        &txn,
        org_id,
        by,
        &RunnerJobCreate {
            kind: RunnerJobKind::EnvironmentScan,
            spec,
            required_capabilities: vec![SCAN_CAPABILITY.to_string()],
        },
    )
    .await?;
    txn.execute_raw(stmt(
        "INSERT INTO environment_scans (id, org_id, build_id, job_id) VALUES ($1, $2, $3, $4)",
        vec![
            Uuid::new_v4().into(),
            org_id.into(),
            row.try_get::<Uuid>("", "id")?.into(),
            Uuid::parse_str(&job.id)
                .map_err(|e| DbErr::Custom(e.to_string()))?
                .into(),
        ],
    ))
    .await?;
    txn.commit().await
}

/// The read login of the registry a scan reads, if it has one.
pub async fn scan_input(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    job_id: &str,
) -> Result<Option<RunnerScanInput>, DbErr> {
    let job = Uuid::parse_str(job_id).map_err(|e| DbErr::Custom(e.to_string()))?;
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT r.host, r.read_username, r.read_secret_id FROM environment_scans s \
             JOIN environment_builds b ON b.id = s.build_id \
             JOIN environments e ON e.id = b.environment_id \
             JOIN image_registries r ON r.id = e.registry_id \
             WHERE s.org_id = $1 AND s.job_id = $2",
            vec![org_id.into(), job.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let username: Option<String> = row.try_get("", "read_username")?;
    let secret: Option<Uuid> = row.try_get("", "read_secret_id")?;
    let (Some(username), Some(secret)) = (username, secret) else {
        return Ok(None);
    };
    let secret = store::reveal(db, ring, Some(org_id), secret)
        .await
        .map_err(|_| DbErr::Custom("read secret unreadable".into()))?;
    Ok(Some(RunnerScanInput {
        registry_host: row.try_get("", "host")?,
        username,
        secret,
    }))
}

/// Keeps what a runner reports for the job it held. An out-of-bounds report is dropped: the
/// scan then reads as failed.
pub async fn record_scan(
    db: &DatabaseConnection,
    org_id: i64,
    job: Uuid,
    report: &ScanReport,
) -> Result<(), DbErr> {
    if !report.is_valid() {
        tracing::warn!(%job, "out-of-bounds scan report dropped");
        return Ok(());
    }
    let policy = scan_policy(db, org_id).await?;
    let breaches = policy.breaches(report);
    let row = db
        .query_one_raw(stmt(
            "UPDATE environment_scans s SET report = $3, breaches = $4 \
             FROM environment_builds b, environments e \
             WHERE s.org_id = $1 AND s.job_id = $2 AND b.id = s.build_id AND e.id = b.environment_id \
             RETURNING e.name, b.number",
            vec![
                org_id.into(),
                job.into(),
                serde_json::json!(report).into(),
                (breaches as i32).into(),
            ],
        ))
        .await?;
    if let (true, Some(row)) = (breaches > 0, row) {
        let name: String = row.try_get("", "name")?;
        let number: i32 = row.try_get("", "number")?;
        signal_breaches(db, org_id, &name, number, breaches, policy.mode).await;
    }
    Ok(())
}

/// In-app notice that a build is over the scan threshold (D748).
async fn signal_breaches(
    db: &DatabaseConnection,
    org_id: i64,
    environment: &str,
    number: i32,
    breaches: u32,
    mode: ScanMode,
) {
    let refused = if mode == ScanMode::Refuse {
        " It cannot be activated nor run until it is rebuilt."
    } else {
        ""
    };
    let msg = pnex_notify::Message {
        subject: Some(format!("Image of {environment} over the scan threshold")),
        body: format!(
            "Build {number} of {environment} has {breaches} known flaws a rebuild can fix.{refused}"
        ),
        meta: serde_json::json!({
            "kind": "environment-scan-breach",
            "environment": environment,
            "build": number,
            "breaches": breaches,
        }),
    };
    crate::services::data_builds::notify_org(db, org_id, "environment-scan", msg).await;
}

/// The scan policy of an org; the default (critical, signal) when it set none.
pub async fn scan_policy<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<ScanPolicy, DbErr> {
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT threshold, mode FROM environment_scan_policies WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
    else {
        return Ok(ScanPolicy::default());
    };
    let threshold: String = row.try_get("", "threshold")?;
    let mode: String = row.try_get("", "mode")?;
    Ok(ScanPolicy {
        threshold: match threshold.as_str() {
            "high" => ScanThreshold::High,
            "none" => ScanThreshold::None,
            _ => ScanThreshold::Critical,
        },
        mode: if mode == "refuse" {
            ScanMode::Refuse
        } else {
            ScanMode::Signal
        },
    })
}

pub async fn set_scan_policy(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    policy: ScanPolicy,
) -> Result<ScanPolicy, DbErr> {
    let threshold = match policy.threshold {
        ScanThreshold::Critical => "critical",
        ScanThreshold::High => "high",
        ScanThreshold::None => "none",
    };
    let mode = match policy.mode {
        ScanMode::Signal => "signal",
        ScanMode::Refuse => "refuse",
    };
    db.execute_raw(stmt(
        "INSERT INTO environment_scan_policies (org_id, threshold, mode, updated_by) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (org_id) DO UPDATE \
         SET threshold = $2, mode = $3, updated_by = $4, updated_at = now()",
        vec![org_id.into(), threshold.into(), mode.into(), user_id.into()],
    ))
    .await?;
    scan_policy(db, org_id).await
}

/// `Some(breaches)` when the org refuses versions over its threshold and the latest scan of
/// this build is over it, counted with the policy of now: a policy change applies at once.
pub async fn refused_by_scan<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    build: Uuid,
) -> Result<Option<u32>, DbErr> {
    let policy = scan_policy(db, org_id).await?;
    if policy.mode != ScanMode::Refuse {
        return Ok(None);
    }
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT report FROM environment_scans \
             WHERE org_id = $1 AND build_id = $2 AND report IS NOT NULL \
             ORDER BY created_at DESC LIMIT 1",
            vec![org_id.into(), build.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let report: Option<ScanReport> = row
        .try_get::<Option<serde_json::Value>>("", "report")?
        .and_then(|v| serde_json::from_value(v).ok());
    Ok(report.map(|r| policy.breaches(&r)).filter(|n| *n > 0))
}

/// The latest scan of a build, with its findings.
pub async fn get_scan(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    build: Uuid,
) -> Result<EnvironmentScan, EnvironmentError> {
    get_build(db, org_id, id, build).await?;
    let row = db
        .query_one_raw(stmt(
            &format!(
                "SELECT s.report, j.state, \
                   to_char(s.created_at AT TIME ZONE 'UTC', {ISO}) AS created_at \
                 FROM environment_scans s JOIN runner_jobs j ON j.id = s.job_id \
                 WHERE s.org_id = $1 AND s.build_id = $2 ORDER BY s.created_at DESC LIMIT 1"
            ),
            vec![org_id.into(), build.into()],
        ))
        .await?
        .ok_or(EnvironmentError::ScanMissing)?;
    let report = report_of(&row, "report")?;
    let job: String = row.try_get("", "state")?;
    Ok(EnvironmentScan {
        state: scan_state(&job, report.is_some()),
        counts: report.as_ref().map(ScanReport::summary),
        report,
        created_at: row.try_get("", "created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_fingerprint_follows_the_content_and_not_the_order() {
        let source = |pairs: &[(&str, &str)]| EnvironmentSource {
            dockerfile: "FROM x".into(),
            files: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            build_args: BTreeMap::new(),
        };
        let a = fingerprint(&source(&[("a", "1"), ("b", "2")])).unwrap();
        let b = fingerprint(&source(&[("b", "2"), ("a", "1")])).unwrap();
        let c = fingerprint(&source(&[("a", "1"), ("b", "3")])).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("sha256:") && a.len() == 71, "{a}");
    }
}
