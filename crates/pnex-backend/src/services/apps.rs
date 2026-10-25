//! Applications declared in an org (external-apps.md D433). An
//! application dresses a service account of the org with the scope it
//! reads; the change feed keeps to that scope when the application calls.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use pnex_core::apps::{check, AppInput, AppScope, AppView};
use pnex_core::err_codes::{FIELD_INVALID_UUID, FIELD_NOT_FOUND, FIELD_UNIQUE};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// `{field: token}` of the input.
    #[error("invalid fields")]
    Fields(BTreeMap<String, String>),
    #[error("not found")]
    NotFound,
    #[error("version conflict (current {current})")]
    VersionConflict { current: i32 },
    #[error(transparent)]
    Db(#[from] DbErr),
}

pub type Result<T> = std::result::Result<T, AppError>;

fn field(name: &str, token: &str) -> AppError {
    AppError::Fields(BTreeMap::from([(name.to_string(), token.to_string())]))
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

const SELECT: &str = "SELECT id, key, name, description, service_account_id, scope, version, \
                      created_at, updated_at FROM apps";

#[derive(FromQueryResult)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    service_account_id: Option<Uuid>,
    scope: serde_json::Value,
    version: i32,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
}

impl From<Row> for AppView {
    fn from(r: Row) -> Self {
        AppView {
            id: r.id.to_string(),
            key: r.key,
            name: r.name,
            description: r.description,
            service_account_id: r.service_account_id.map(|i| i.to_string()),
            scope: serde_json::from_value(r.scope).unwrap_or_default(),
            version: r.version,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        }
    }
}

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<AppView>> {
    Ok(Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 ORDER BY name, key"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

pub async fn get(db: &impl ConnectionTrait, org_id: i64, id: Uuid) -> Result<AppView> {
    Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into)
    .ok_or(AppError::NotFound)
}

/// The application a service account signs in for, if any.
pub async fn of_service_account(
    db: &impl ConnectionTrait,
    org_id: i64,
    service_account_id: Uuid,
) -> Result<Option<AppView>> {
    Ok(Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 AND service_account_id = $2"),
        vec![org_id.into(), service_account_id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into))
}

/// Field checks that need the org: the scope names existing types, and the
/// service account is the org's and no other application's.
async fn check_in_org(
    db: &DatabaseConnection,
    org_id: i64,
    input: &AppInput,
    except: Option<Uuid>,
) -> Result<(Option<Uuid>, AppScope)> {
    let errors = check(input);
    if !errors.is_empty() {
        return Err(AppError::Fields(errors));
    }
    let s = crate::services::ontology::schema(db, org_id).await?;
    if input.scope.types.iter().any(|k| s.object_type(k).is_none()) {
        return Err(field("scope.types", FIELD_NOT_FOUND));
    }
    if input
        .scope
        .link_types
        .iter()
        .any(|k| s.link_type(k).is_none())
    {
        return Err(field("scope.link_types", FIELD_NOT_FOUND));
    }
    let account = match input
        .service_account_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(raw) => {
            let id = Uuid::parse_str(raw)
                .map_err(|_| field("service_account_id", FIELD_INVALID_UUID))?;
            if crate::services::iam::service::get_account(db, org_id, id)
                .await
                .is_err()
            {
                return Err(field("service_account_id", FIELD_NOT_FOUND));
            }
            if of_service_account(db, org_id, id)
                .await?
                .is_some_and(|a| Some(a.id.as_str()) != except.map(|e| e.to_string()).as_deref())
            {
                return Err(field("service_account_id", FIELD_UNIQUE));
            }
            Some(id)
        }
    };
    let mut scope = input.scope.clone();
    for list in [
        &mut scope.types,
        &mut scope.link_types,
        &mut scope.media_kinds,
    ] {
        list.sort();
        list.dedup();
    }
    Ok((account, scope))
}

async fn key_taken(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
    except: Option<Uuid>,
) -> Result<bool> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS one FROM apps WHERE org_id = $1 AND key = $2 AND id IS DISTINCT FROM $3",
            vec![org_id.into(), key.into(), except.into()],
        ))
        .await?
        .is_some())
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    input: &AppInput,
) -> Result<AppView> {
    let (account, scope) = check_in_org(db, org_id, input, None).await?;
    if key_taken(db, org_id, &input.key, None).await? {
        return Err(field("key", FIELD_UNIQUE));
    }
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO apps (id, org_id, key, name, description, service_account_id, scope, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        vec![
            id.into(),
            org_id.into(),
            input.key.clone().into(),
            input.name.trim().to_string().into(),
            input.description.clone().into(),
            account.into(),
            serde_json::to_value(&scope).unwrap_or_default().into(),
            user_id.into(),
        ],
    ))
    .await?;
    get(db, org_id, id).await
}

/// Replaces an application on the version it was read on.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &AppInput,
) -> Result<AppView> {
    let current = get(db, org_id, id).await?;
    let Some(expected) = input.expected_version else {
        return Err(field(
            "expected_version",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    };
    let (account, scope) = check_in_org(db, org_id, input, Some(id)).await?;
    if key_taken(db, org_id, &input.key, Some(id)).await? {
        return Err(field("key", FIELD_UNIQUE));
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE apps SET key = $3, name = $4, description = $5, service_account_id = $6, \
               scope = $7, version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $8",
            vec![
                org_id.into(),
                id.into(),
                input.key.clone().into(),
                input.name.trim().to_string().into(),
                input.description.clone().into(),
                account.into(),
                serde_json::to_value(&scope).unwrap_or_default().into(),
                expected.into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(AppError::VersionConflict {
            current: current.version,
        });
    }
    get(db, org_id, id).await
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<()> {
    let done = db
        .execute_raw(stmt(
            "DELETE FROM apps WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}
