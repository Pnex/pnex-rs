//! Agents of an org (agent-environments.md D757, D761). Creating one creates a service
//! account of the org and gives it a managed role through the IAM assignment, with its
//! check: never `admin` (D293), never above the caller. The record is versioned; a run
//! starts from it and may only tighten it (D380).

use pnex_core::agents::{
    name_error, run_of, spec_error, Agent, AgentCreate, AgentRole, AgentRunStart, AgentSpec,
    AgentUpdate, AgentVersion, DESCRIPTION_MAX,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::agent_artifacts::ArtifactSettings;
use crate::services::agent_runs::{self, AgentRunError, RunOrigin};
use crate::services::iam::store::IamError;
use crate::services::iam::{service, store, Grants};

#[derive(Debug)]
pub enum AgentError {
    Invalid {
        field: String,
        token: String,
    },
    NotFound,
    NameTaken,
    VersionConflict {
        current: i32,
    },
    /// The tier of the org allows no more agents.
    Quota {
        max: i32,
    },
    /// The role would give what the caller does not hold.
    Escalation {
        permission: String,
    },
    ProviderNotFound,
    Run(AgentRunError),
    Db(DbErr),
}

impl From<DbErr> for AgentError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl From<IamError> for AgentError {
    fn from(e: IamError) -> Self {
        match e {
            IamError::Invalid { field, token } => Self::Invalid { field, token },
            IamError::ServiceAccountNameTaken => Self::NameTaken,
            IamError::Escalation { permission } => Self::Escalation { permission },
            IamError::Db(e) => Self::Db(e),
            other => Self::Db(DbErr::Custom(format!("{other:?}"))),
        }
    }
}

fn invalid(field: &str, token: &str) -> AgentError {
    AgentError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const ISO: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn select() -> String {
    format!(
        "SELECT id, name, description, service_account_id, role, version, spec, \
           to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
           to_char(updated_at AT TIME ZONE 'UTC', {ISO}) AS updated_at \
         FROM agents"
    )
}

fn role_of(s: &str) -> AgentRole {
    if s == "member" {
        AgentRole::Member
    } else {
        AgentRole::Viewer
    }
}

fn role_str(r: AgentRole) -> &'static str {
    match r {
        AgentRole::Viewer => "viewer",
        AgentRole::Member => "member",
    }
}

fn view(r: &QueryResult) -> Result<Agent, DbErr> {
    let role: String = r.try_get("", "role")?;
    Ok(Agent {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        description: r.try_get("", "description")?,
        service_account_id: r.try_get::<Uuid>("", "service_account_id")?.to_string(),
        role: role_of(&role),
        version: r.try_get("", "version")?,
        spec: serde_json::from_value(r.try_get("", "spec")?)
            .map_err(|e| DbErr::Custom(e.to_string()))?,
        created_at: r.try_get("", "created_at")?,
        updated_at: r.try_get("", "updated_at")?,
    })
}

async fn check_spec<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    spec: &AgentSpec,
) -> Result<(), AgentError> {
    if let Some((field, token)) = spec_error(spec) {
        return Err(invalid(&format!("spec.{field}"), token));
    }
    let environment = Uuid::parse_str(&spec.environment_id)
        .map_err(|_| invalid("spec.environment_id", "invalid"))?;
    let found = db
        .query_one_raw(stmt(
            "SELECT 1 AS ok FROM environments WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), environment.into()],
        ))
        .await?;
    if found.is_none() {
        return Err(invalid("spec.environment_id", "not_found"));
    }
    if let Some(p) = &spec.provider_id {
        let id = Uuid::parse_str(p).map_err(|_| invalid("spec.provider_id", "invalid"))?;
        let found = db
            .query_one_raw(stmt(
                "SELECT 1 AS ok FROM llm_providers WHERE org_id = $1 AND id = $2",
                vec![org_id.into(), id.into()],
            ))
            .await?;
        if found.is_none() {
            return Err(AgentError::ProviderNotFound);
        }
    }
    Ok(())
}

/// The cap of agents of the tier of the org; `None`: no cap.
async fn cap_of<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Option<i32>, DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT t.max_agents FROM organizations o \
             LEFT JOIN subscription_tiers t ON t.id = o.subscription_tier_id WHERE o.id = $1",
            vec![org_id.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<Option<i32>>("", "max_agents").ok().flatten()))
}

pub async fn get<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<Agent, AgentError> {
    let row = db
        .query_one_raw(stmt(
            &format!("{} WHERE org_id = $1 AND id = $2", select()),
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(AgentError::NotFound)?;
    Ok(view(&row)?)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Agent>), DbErr> {
    let total: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agents WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{} WHERE org_id = $1 ORDER BY lower(name) LIMIT $2 OFFSET $3",
                select()
            ),
            vec![org_id.into(), limit.into(), offset.into()],
        ))
        .await?;
    Ok((total, rows.iter().map(view).collect::<Result<_, _>>()?))
}

/// Creates the service account, gives it its role, then the record. Any refusal after the
/// account exists removes it again: no identity is left without its agent.
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    caller: &Grants,
    body: &AgentCreate,
) -> Result<Agent, AgentError> {
    if let Some(token) = name_error(&body.name) {
        return Err(invalid("name", token));
    }
    if body.description.chars().count() > DESCRIPTION_MAX {
        return Err(invalid("description", "max_length:1000"));
    }
    check_spec(db, org_id, &body.spec).await?;
    if let Some(max) = cap_of(db, org_id).await? {
        let n: i64 = db
            .query_one_raw(stmt(
                "SELECT count(*) AS n FROM agents WHERE org_id = $1",
                vec![org_id.into()],
            ))
            .await?
            .map(|r| r.try_get("", "n"))
            .transpose()?
            .unwrap_or(0);
        if n >= i64::from(max) {
            return Err(AgentError::Quota { max });
        }
    }
    let name = body.name.trim();
    let taken = db
        .query_one_raw(stmt(
            "SELECT 1 AS ok FROM agents WHERE org_id = $1 AND lower(name) = lower($2)",
            vec![org_id.into(), name.into()],
        ))
        .await?;
    if taken.is_some() {
        return Err(AgentError::NameTaken);
    }
    let account = service::create_account(
        db,
        org_id,
        caller_id,
        &format!("agent: {name}"),
        Some("Identity of a hosted agent (D757)."),
    )
    .await?;
    let managed = match body.role {
        AgentRole::Viewer => OrgMemberRole::Viewer,
        AgentRole::Member => OrgMemberRole::Member,
    };
    let assigned = store::create_assignment(
        db,
        org_id,
        caller_id,
        caller,
        store::Holder::Service(account.user_id),
        store::Given::Managed(managed),
    )
    .await;
    let written = match assigned {
        Ok(_) => insert(db, org_id, caller_id, account.id, name, body).await,
        Err(e) => Err(e.into()),
    };
    match written {
        Ok(id) => get(db, org_id, id).await,
        Err(e) => {
            if let Err(undo) = service::delete_account(db, org_id, account.id).await {
                tracing::error!(error = ?undo, "service account of a refused agent left behind");
            }
            Err(e)
        }
    }
}

async fn insert(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    account: Uuid,
    name: &str,
    body: &AgentCreate,
) -> Result<Uuid, AgentError> {
    let id = Uuid::new_v4();
    let spec = serde_json::to_value(&body.spec).map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    txn.execute_raw(stmt(
        "INSERT INTO agents (id, org_id, service_account_id, name, description, role, version, spec, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $8)",
        vec![
            id.into(),
            org_id.into(),
            account.into(),
            name.into(),
            body.description.trim().into(),
            role_str(body.role).into(),
            spec.clone().into(),
            caller_id.into(),
        ],
    ))
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            AgentError::NameTaken
        } else {
            e.into()
        }
    })?;
    txn.execute_raw(stmt(
        "INSERT INTO agent_versions (agent_id, number, spec, created_by) VALUES ($1, 1, $2, $3)",
        vec![id.into(), spec.into(), caller_id.into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(id)
}

/// A new version of the record, on the one that was read.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    id: Uuid,
    body: &AgentUpdate,
) -> Result<Agent, AgentError> {
    let current = get(db, org_id, id).await?;
    if let Some(d) = &body.description {
        if d.chars().count() > DESCRIPTION_MAX {
            return Err(invalid("description", "max_length:1000"));
        }
    }
    check_spec(db, org_id, &body.spec).await?;
    let spec = serde_json::to_value(&body.spec).map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let next = body.expected_version + 1;
    let done = txn
        .execute_raw(stmt(
            "UPDATE agents SET version = $3, spec = $4, description = coalesce($5, description), \
               updated_at = now() WHERE org_id = $1 AND id = $2 AND version = $6",
            vec![
                org_id.into(),
                id.into(),
                next.into(),
                spec.clone().into(),
                body.description.as_deref().map(str::trim).into(),
                body.expected_version.into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(AgentError::VersionConflict {
            current: current.version,
        });
    }
    txn.execute_raw(stmt(
        "INSERT INTO agent_versions (agent_id, number, spec, created_by) VALUES ($1, $2, $3, $4)",
        vec![id.into(), next.into(), spec.into(), caller_id.into()],
    ))
    .await?;
    txn.commit().await?;
    get(db, org_id, id).await
}

pub async fn versions(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Vec<AgentVersion>, AgentError> {
    get(db, org_id, id).await?;
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT number, spec, to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at \
                 FROM agent_versions WHERE agent_id = $1 ORDER BY number DESC"
            ),
            vec![id.into()],
        ))
        .await?;
    rows.iter()
        .map(|r| {
            Ok(AgentVersion {
                number: r.try_get("", "number")?,
                spec: serde_json::from_value(r.try_get("", "spec")?)
                    .map_err(|e| DbErr::Custom(e.to_string()))?,
                created_at: r.try_get("", "created_at")?,
            })
        })
        .collect::<Result<_, DbErr>>()
        .map_err(Into::into)
}

/// Removes the agent and its identity: its keys and roles go with the service account, its
/// runs keep their trace without it.
pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), AgentError> {
    let agent = get(db, org_id, id).await?;
    let account =
        Uuid::parse_str(&agent.service_account_id).map_err(|e| DbErr::Custom(e.to_string()))?;
    service::delete_account(db, org_id, account).await?;
    Ok(())
}

/// A run of the agent: its record, tightened by the start, as its identity.
pub async fn start_run(
    db: &DatabaseConnection,
    artifacts: ArtifactSettings,
    org_id: i64,
    caller_id: i64,
    id: Uuid,
    start: &AgentRunStart,
) -> Result<pnex_core::agent_runs::AgentRun, AgentError> {
    let agent = get(db, org_id, id).await?;
    let body = run_of(&agent.spec, start).map_err(|(f, t)| invalid(f, t))?;
    let provider = match &agent.spec.provider_id {
        Some(p) => Some(Uuid::parse_str(p).map_err(|_| AgentError::ProviderNotFound)?),
        None => None,
    };
    agent_runs::create_as(
        db,
        artifacts,
        org_id,
        caller_id,
        &body,
        Some(RunOrigin {
            agent_id: id,
            agent_version: agent.version,
            provider_id: provider,
        }),
    )
    .await
    .map_err(AgentError::Run)
}
