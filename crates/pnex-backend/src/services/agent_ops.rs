//! Ended agent runs into the ontology (agent-environments.md D708, D761): the environment,
//! the agent, the run and the files it took and left become objects of the `agent_ops` pack,
//! installed on first use. Safe to run again: everything is matched on its natural key.

use pnex_core::agent_runs::ArtifactInput;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::ontology::{self, packs, Actor, OntologyError, OrgSchema, Result};
use crate::services::software_supply::{ensure_all, link, open_links, props};

const PACK: &str = "agent_ops";
const ENVIRONMENT: &str = "agent_environment";
const AGENT: &str = "hosted_agent";
const RUN: &str = "agent_run";
const FILE: &str = "agent_file";
const OF_AGENT: &str = "run_of_agent";
const IN_ENVIRONMENT: &str = "run_in_environment";
const PRODUCED_BY: &str = "file_produced_by";
const READ: &str = "run_read_file";

async fn schema_with_pack(db: &DatabaseConnection, org_id: i64) -> Result<OrgSchema> {
    let schema = ontology::schema(db, org_id).await?;
    if schema.object_type(RUN).is_some() {
        return Ok(schema);
    }
    let pack = packs::shipped()
        .into_iter()
        .find(|p| p.key == PACK)
        .ok_or(OntologyError::NotFound)?;
    packs::install(db, org_id, None, &pack).await?;
    Ok(ontology::schema(db, org_id).await?)
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Links `source` to `target` under `relation` unless it is already open.
async fn ensure_link(
    db: &DatabaseConnection,
    schema: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    (relation, source, target): (&str, Uuid, Uuid),
) -> Result<()> {
    if open_links(db, org_id, source, relation)
        .await?
        .contains_key(&target)
    {
        return Ok(());
    }
    ontology::links::create_with(
        db,
        schema,
        org_id,
        actor,
        &link(relation, source, target, Map::new()),
    )
    .await?;
    Ok(())
}

struct FileRow {
    id: Uuid,
    name: String,
    sha256: Option<String>,
    size: Option<i64>,
    state: String,
}

async fn files(db: &DatabaseConnection, org_id: i64, sql: &str, key: Uuid) -> Result<Vec<FileRow>> {
    let rows = db
        .query_all_raw(stmt(sql, vec![org_id.into(), key.into()]))
        .await?;
    rows.iter()
        .map(|r| {
            Ok(FileRow {
                id: r.try_get("", "id")?,
                name: r.try_get("", "name")?,
                sha256: r.try_get("", "sha256")?,
                size: r.try_get("", "size")?,
                state: r.try_get("", "state")?,
            })
        })
        .collect::<std::result::Result<_, sea_orm::DbErr>>()
        .map_err(Into::into)
}

/// Projects the run behind runner job `job` once it ended. One at a time per org.
pub async fn project_run(db: &DatabaseConnection, org_id: i64, job: Uuid) -> Result<()> {
    use crate::services::db_lock::{ns, TenantLock};
    let lock = TenantLock::acquire(
        db,
        ns::AGENT_OPS,
        org_id,
        std::time::Duration::from_secs(120),
    )
    .await?;
    let out = project_locked(db, org_id, job).await;
    lock.release().await;
    out
}

async fn project_locked(db: &DatabaseConnection, org_id: i64, job: Uuid) -> Result<()> {
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT a.id, a.agent_id, a.agent_version, a.inputs, a.llm_calls, a.input_tokens, \
               a.output_tokens, a.error, j.outcome, j.spec->>'image' AS image, \
               e.id AS environment_id, e.name AS environment_name, g.name AS agent_name \
             FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id \
             LEFT JOIN environments e ON e.id = a.environment_id \
             LEFT JOIN agents g ON g.id = a.agent_id \
             WHERE a.org_id = $1 AND a.job_id = $2",
            vec![org_id.into(), job.into()],
        ))
        .await?
    else {
        return Ok(());
    };
    let run_id: Uuid = row.try_get("", "id")?;
    // None for an engine that runs in the server (D781).
    let environment_id: Option<Uuid> = row.try_get("", "environment_id")?;
    let environment_name: String = row
        .try_get::<Option<String>>("", "environment_name")?
        .unwrap_or_default();
    let agent_id: Option<Uuid> = row.try_get("", "agent_id")?;
    let agent_name: Option<String> = row.try_get("", "agent_name")?;
    let outcome: Option<String> = row.try_get("", "outcome")?;
    let image: Option<String> = row.try_get("", "image")?;
    let inputs: Vec<ArtifactInput> =
        serde_json::from_value(row.try_get::<Value>("", "inputs")?).unwrap_or_default();

    let schema = schema_with_pack(db, org_id).await?;
    let actor = Actor {
        user_id: None,
        role: OrgMemberRole::Admin,
        source_ref: format!("agent-run:{run_id}"),
    };
    let run_key = run_id.to_string();
    let run_props = props(&[
        ("run_id", json!(run_key)),
        (
            "outcome",
            outcome.as_ref().map_or(Value::Null, |o| json!(o)),
        ),
        ("image", image.as_ref().map_or(Value::Null, |i| json!(i))),
        (
            "agent_version",
            row.try_get::<Option<i32>>("", "agent_version")?
                .map_or(Value::Null, |v| json!(v)),
        ),
        ("llm_calls", json!(row.try_get::<i32>("", "llm_calls")?)),
        (
            "input_tokens",
            json!(row.try_get::<i64>("", "input_tokens")?),
        ),
        (
            "output_tokens",
            json!(row.try_get::<i64>("", "output_tokens")?),
        ),
        (
            "error",
            row.try_get::<Option<String>>("", "error")?
                .map_or(Value::Null, |e| json!(e)),
        ),
    ]);
    let run_title = format!(
        "{} {}",
        agent_name.as_deref().unwrap_or(&environment_name),
        &run_key[..8]
    );
    let run = ensure_all(
        db,
        &schema,
        org_id,
        &actor,
        (RUN, "run_id"),
        std::slice::from_ref(&run_key),
        |_| (run_title.clone(), run_props.clone()),
    )
    .await?[&run_key];

    if let Some(environment_id) = environment_id {
        let env_key = environment_id.to_string();
        let environment = ensure_all(
            db,
            &schema,
            org_id,
            &actor,
            (ENVIRONMENT, "environment_id"),
            std::slice::from_ref(&env_key),
            |_| {
                (
                    environment_name.clone(),
                    props(&[
                        ("environment_id", json!(env_key)),
                        ("environment_name", json!(environment_name)),
                    ]),
                )
            },
        )
        .await?[&env_key];
        ensure_link(
            db,
            &schema,
            org_id,
            &actor,
            (IN_ENVIRONMENT, run, environment),
        )
        .await?;
    }

    if let (Some(agent_id), Some(name)) = (agent_id, &agent_name) {
        let agent_key = agent_id.to_string();
        let agent = ensure_all(
            db,
            &schema,
            org_id,
            &actor,
            (AGENT, "agent_id"),
            std::slice::from_ref(&agent_key),
            |_| {
                (
                    name.clone(),
                    props(&[("agent_id", json!(agent_key)), ("agent_name", json!(name))]),
                )
            },
        )
        .await?[&agent_key];
        ensure_link(db, &schema, org_id, &actor, (OF_AGENT, run, agent)).await?;
    }

    let file_props = |f: &FileRow| {
        props(&[
            ("artifact_id", json!(f.id.to_string())),
            ("file_name", json!(f.name)),
            (
                "sha256",
                f.sha256.as_ref().map_or(Value::Null, |s| json!(s)),
            ),
            ("size", f.size.map_or(Value::Null, |s| json!(s))),
            ("file_state", json!(f.state)),
        ])
    };
    let left = files(
        db,
        org_id,
        "SELECT id, name, sha256, size, state FROM agent_artifacts WHERE org_id = $1 AND run_id = $2",
        run_id,
    )
    .await?;
    for f in &left {
        let key = f.id.to_string();
        let file = ensure_all(
            db,
            &schema,
            org_id,
            &actor,
            (FILE, "artifact_id"),
            std::slice::from_ref(&key),
            |_| (f.name.clone(), file_props(f)),
        )
        .await?[&key];
        ensure_link(db, &schema, org_id, &actor, (PRODUCED_BY, file, run)).await?;
    }
    for input in &inputs {
        let Ok(id) = Uuid::parse_str(&input.artifact_id) else {
            continue;
        };
        let read = files(
            db,
            org_id,
            "SELECT id, name, sha256, size, state FROM agent_artifacts WHERE org_id = $1 AND id = $2",
            id,
        )
        .await?;
        for f in &read {
            let key = f.id.to_string();
            let file = ensure_all(
                db,
                &schema,
                org_id,
                &actor,
                (FILE, "artifact_id"),
                std::slice::from_ref(&key),
                |_| (f.name.clone(), file_props(f)),
            )
            .await?[&key];
            ensure_link(db, &schema, org_id, &actor, (READ, run, file)).await?;
        }
    }
    Ok(())
}

/// In the background, after the runner was answered; a failure is logged. The end of the
/// run is then published to the flows, once (D713): the next step reads the projection.
pub fn spawn_projection(ctx: &loco_rs::app::AppContext, org_id: i64, job: Uuid) {
    let db = ctx.db.clone();
    let config = ctx.config.clone();
    let ring = crate::controllers::secrets::keyring(ctx).ok();
    tokio::spawn(async move {
        if let Err(e) = project_run(&db, org_id, job).await {
            tracing::warn!(org_id, %job, error = ?e, "agent run not projected to the ontology");
        }
        // Before the end is published: the next step of a chain sees the merge request.
        if let Err(e) =
            crate::services::agent_runs::open_merge_requests(&db, ring.as_ref(), org_id, job).await
        {
            tracing::warn!(org_id, %job, error = %e, "merge requests not recorded");
        }
        let valkey = crate::services::shared_valkey::conn(&config).await;
        if let Err(e) =
            crate::services::agent_chains::publish_finished(&db, valkey, org_id, job).await
        {
            tracing::warn!(org_id, %job, error = %e, "agent run end not published");
        }
    });
}
