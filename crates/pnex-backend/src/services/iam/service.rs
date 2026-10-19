//! Service accounts and access keys (iam.md §7, lot I6).
//!
//! A service account is a non-human principal of one org: a `users` row
//! marked `is_service` (so history and audit reference an identity, D299),
//! never a member, whose rights are only the roles assigned to it. It
//! calls org operations with `Authorization: Bearer pnex_sk_…`, read-only
//! in V1, never the access service, never the MCP (iam.md §14).
//!
//! A key is 32 random bytes shown once; only its sha256 is stored (R15,
//! R16), with a short display prefix. Expiry is mandatory (one year at
//! most) and at most two keys are active at once (rotation without cut).

use chrono::{DateTime, Duration, FixedOffset, Utc};
use rand::RngExt;
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, Set, Statement as Sql, TransactionTrait,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::store::{IamError, Result, DESCRIPTION_MAX, NAME_MAX};
use crate::models::_entities::users;

/// Prefix of every access key secret.
pub const KEY_PREFIX: &str = "pnex_sk_";
/// Longest lifetime of a key.
pub const KEY_MAX_DAYS: i64 = 366;
/// Keys active at once per account (current + next, for rotation).
pub const ACTIVE_KEYS_MAX: i64 = 2;
/// Characters of the secret kept for display (`pnex_sk_` + 8).
const DISPLAY_PREFIX_LEN: usize = 16;

fn sql(text: &str, values: Vec<sea_orm::Value>) -> Sql {
    Sql::from_sql_and_values(DatabaseBackend::Postgres, text, values)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(secret: &str) -> String {
    hex(&Sha256::digest(secret.as_bytes()))
}

/// The service account behind an access key.
#[derive(Clone, Debug)]
pub struct ServicePrincipal {
    pub id: Uuid,
    pub org_id: i64,
    pub key_id: Uuid,
    pub expires_at: DateTime<FixedOffset>,
}

#[derive(FromQueryResult)]
struct KeyHit {
    key_id: Uuid,
    account_id: Uuid,
    org_id: i64,
    user_id: i64,
    expires_at: DateTime<FixedOffset>,
}

/// Resolves a `pnex_sk_…` secret: `None` when unknown, revoked or expired.
pub async fn authenticate(
    db: &DatabaseConnection,
    secret: &str,
) -> std::result::Result<Option<(users::Model, ServicePrincipal)>, DbErr> {
    let Some(hit) = KeyHit::find_by_statement(sql(
        "SELECT k.id AS key_id, sa.id AS account_id, sa.org_id, sa.user_id, k.expires_at \
         FROM iam_access_keys k JOIN iam_service_accounts sa ON sa.id = k.service_account_id \
         WHERE k.secret_sha256 = $1 AND k.revoked_at IS NULL AND k.expires_at > now()",
        vec![sha256_hex(secret).into()],
    ))
    .one(db)
    .await?
    else {
        return Ok(None);
    };
    // At most one write a minute per key.
    db.execute_raw(sql(
        "UPDATE iam_access_keys SET last_used_at = now() WHERE id = $1 \
         AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
        vec![hit.key_id.into()],
    ))
    .await?;
    let Some(user) = users::Entity::find_by_id(hit.user_id).one(db).await? else {
        return Ok(None);
    };
    Ok(Some((
        user,
        ServicePrincipal {
            id: hit.account_id,
            org_id: hit.org_id,
            key_id: hit.key_id,
            expires_at: hit.expires_at,
        },
    )))
}

// ── Accounts ────────────────────────────────────────────────────────

/// A service account of the org.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ServiceAccount {
    #[schema(value_type = String, format = "uuid")]
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// Identity of the account in history and audit (never a member).
    pub user_id: i64,
    /// Keys neither revoked nor expired.
    pub active_keys: i64,
    pub created_by: Option<i64>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<FixedOffset>,
}

#[derive(FromQueryResult)]
struct AccountRow {
    id: Uuid,
    name: String,
    description: String,
    user_id: i64,
    active_keys: i64,
    created_by: Option<i64>,
    created_at: DateTime<FixedOffset>,
}

impl From<AccountRow> for ServiceAccount {
    fn from(a: AccountRow) -> Self {
        Self {
            id: a.id,
            name: a.name,
            description: a.description,
            user_id: a.user_id,
            active_keys: a.active_keys,
            created_by: a.created_by,
            created_at: a.created_at,
        }
    }
}

const ACCOUNT_SELECT: &str = "SELECT sa.id, sa.name, sa.description, sa.user_id, sa.created_by, \
     sa.created_at, (SELECT count(*) FROM iam_access_keys k WHERE k.service_account_id = sa.id \
       AND k.revoked_at IS NULL AND k.expires_at > now()) AS active_keys \
     FROM iam_service_accounts sa";

pub async fn list_accounts(db: &DatabaseConnection, org_id: i64) -> Result<Vec<ServiceAccount>> {
    Ok(AccountRow::find_by_statement(sql(
        &format!("{ACCOUNT_SELECT} WHERE sa.org_id = $1 ORDER BY sa.name"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

pub async fn get_account(
    db: &impl ConnectionTrait,
    org_id: i64,
    id: Uuid,
) -> Result<ServiceAccount> {
    AccountRow::find_by_statement(sql(
        &format!("{ACCOUNT_SELECT} WHERE sa.org_id = $1 AND sa.id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into)
    .ok_or(IamError::ServiceAccountNotFound)
}

fn text(field: &str, value: &str, max: usize, required: bool) -> Result<String> {
    let v = value.trim();
    if required && v.is_empty() {
        return Err(IamError::Invalid {
            field: field.into(),
            token: "required".into(),
        });
    }
    if v.chars().count() > max {
        return Err(IamError::Invalid {
            field: field.into(),
            token: format!("max_length:{max}"),
        });
    }
    Ok(v.to_string())
}

fn name_taken(e: DbErr) -> IamError {
    if e.to_string().contains("duplicate key") {
        IamError::ServiceAccountNameTaken
    } else {
        e.into()
    }
}

pub async fn create_account(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    name: &str,
    description: Option<&str>,
) -> Result<ServiceAccount> {
    let name = text("name", name, NAME_MAX, true)?;
    let description = text(
        "description",
        description.unwrap_or(""),
        DESCRIPTION_MAX,
        false,
    )?;
    let id = Uuid::new_v4();
    let txn = db.begin().await?;
    // `.invalid` (RFC 2606) never reaches an identity provider.
    let user = users::ActiveModel {
        email: Set(format!("sa-{id}@service-accounts.invalid")),
        full_name: Set(Some(name.clone())),
        is_service: Set(true),
        ..Default::default()
    }
    .insert(&txn)
    .await?;
    txn.execute_raw(sql(
        "INSERT INTO iam_service_accounts (id, org_id, user_id, name, description, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
        vec![
            id.into(),
            org_id.into(),
            user.id.into(),
            name.into(),
            description.into(),
            caller_id.into(),
        ],
    ))
    .await
    .map_err(name_taken)?;
    let account = get_account(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(account)
}

pub async fn update_account(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<ServiceAccount> {
    let current = get_account(db, org_id, id).await?;
    let name = match name {
        Some(n) => text("name", n, NAME_MAX, true)?,
        None => current.name,
    };
    let description = match description {
        Some(d) => text("description", d, DESCRIPTION_MAX, false)?,
        None => current.description,
    };
    db.execute_raw(sql(
        "UPDATE iam_service_accounts SET name = $3, description = $4 WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            name.clone().into(),
            description.into(),
        ],
    ))
    .await
    .map_err(name_taken)?;
    db.execute_raw(sql(
        "UPDATE users SET full_name = $2 WHERE id = $1 AND is_service",
        vec![current.user_id.into(), name.into()],
    ))
    .await?;
    get_account(db, org_id, id).await
}

/// Deletes the account, its keys and its assignments (its identity row).
pub async fn delete_account(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<()> {
    let account = get_account(db, org_id, id).await?;
    db.execute_raw(sql(
        "DELETE FROM users WHERE id = $1 AND is_service",
        vec![account.user_id.into()],
    ))
    .await?;
    Ok(())
}

/// Identity (`users.id`) of a service account of the org.
pub async fn account_user(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<i64> {
    Ok(get_account(db, org_id, id).await?.user_id)
}

// ── Keys ────────────────────────────────────────────────────────────

/// An access key, without its secret.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct AccessKey {
    #[schema(value_type = String, format = "uuid")]
    pub id: Uuid,
    /// First characters of the secret, to recognise it.
    pub prefix: String,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<FixedOffset>,
    pub created_by: Option<i64>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<FixedOffset>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_used_at: Option<DateTime<FixedOffset>>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub revoked_at: Option<DateTime<FixedOffset>>,
}

#[derive(FromQueryResult)]
struct KeyRow {
    id: Uuid,
    prefix: String,
    expires_at: DateTime<FixedOffset>,
    created_by: Option<i64>,
    created_at: DateTime<FixedOffset>,
    last_used_at: Option<DateTime<FixedOffset>>,
    revoked_at: Option<DateTime<FixedOffset>>,
}

impl From<KeyRow> for AccessKey {
    fn from(k: KeyRow) -> Self {
        Self {
            id: k.id,
            prefix: k.prefix,
            expires_at: k.expires_at,
            created_by: k.created_by,
            created_at: k.created_at,
            last_used_at: k.last_used_at,
            revoked_at: k.revoked_at,
        }
    }
}

/// A key just created: the only answer that ever carries the secret.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CreatedKey {
    pub key: AccessKey,
    /// `pnex_sk_…`: shown once, keep it on the server side of the
    /// application (environment variable, secret store).
    pub secret: String,
}

const KEY_COLUMNS: &str =
    "id, prefix, expires_at, created_by, created_at, last_used_at, revoked_at";

pub async fn list_keys(
    db: &DatabaseConnection,
    org_id: i64,
    account: Uuid,
) -> Result<Vec<AccessKey>> {
    get_account(db, org_id, account).await?;
    Ok(KeyRow::find_by_statement(sql(
        &format!(
            "SELECT {KEY_COLUMNS} FROM iam_access_keys WHERE service_account_id = $1 \
             ORDER BY created_at DESC, id"
        ),
        vec![account.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    format!("{KEY_PREFIX}{}", hex(&bytes))
}

pub async fn create_key(
    db: &DatabaseConnection,
    org_id: i64,
    account: Uuid,
    caller_id: i64,
    expires_at: DateTime<FixedOffset>,
) -> Result<CreatedKey> {
    get_account(db, org_id, account).await?;
    let now = Utc::now();
    let token = if expires_at <= now {
        Some("in_past".to_string())
    } else if expires_at > now + Duration::days(KEY_MAX_DAYS) {
        Some(format!("max_days:{KEY_MAX_DAYS}"))
    } else {
        None
    };
    if let Some(token) = token {
        return Err(IamError::Invalid {
            field: "expires_at".into(),
            token,
        });
    }
    let txn = db.begin().await?;
    // One rotation at a time per account.
    txn.execute_raw(sql(
        "SELECT pg_advisory_xact_lock(hashtext($1::text))",
        vec![account.to_string().into()],
    ))
    .await?;
    let active = txn
        .query_one_raw(sql(
            "SELECT count(*) AS n FROM iam_access_keys WHERE service_account_id = $1 \
             AND revoked_at IS NULL AND expires_at > now()",
            vec![account.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    if active >= ACTIVE_KEYS_MAX {
        return Err(IamError::KeyLimit);
    }
    let secret = new_secret();
    let id = Uuid::new_v4();
    let key = KeyRow::find_by_statement(sql(
        &format!(
            "INSERT INTO iam_access_keys (id, service_account_id, prefix, secret_sha256, expires_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {KEY_COLUMNS}"
        ),
        vec![
            id.into(),
            account.into(),
            secret[..DISPLAY_PREFIX_LEN].to_string().into(),
            sha256_hex(&secret).into(),
            expires_at.into(),
            caller_id.into(),
        ],
    ))
    .one(&txn)
    .await?
    .ok_or(IamError::KeyNotFound)?;
    txn.commit().await?;
    Ok(CreatedKey {
        key: key.into(),
        secret,
    })
}

/// Revokes a key at once (the next call with it answers 401).
pub async fn revoke_key(
    db: &DatabaseConnection,
    org_id: i64,
    account: Uuid,
    key: Uuid,
) -> Result<()> {
    get_account(db, org_id, account).await?;
    let res = db
        .execute_raw(sql(
            "UPDATE iam_access_keys SET revoked_at = now() \
             WHERE id = $1 AND service_account_id = $2 AND revoked_at IS NULL",
            vec![key.into(), account.into()],
        ))
        .await?;
    if res.rows_affected() == 0 {
        return Err(IamError::KeyNotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_random_prefixed_and_hashed() {
        let (a, b) = (new_secret(), new_secret());
        assert_ne!(a, b);
        assert!(a.starts_with(KEY_PREFIX));
        assert_eq!(a.len(), KEY_PREFIX.len() + 64);
        assert_eq!(sha256_hex(&a).len(), 64);
        assert_ne!(sha256_hex(&a), a);
    }
}
