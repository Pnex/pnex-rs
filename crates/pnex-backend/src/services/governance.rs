//! Data governance (governance.md D798–D803): markings of an org, clearances, the
//! markings of tables, and the journal of every governed read and change. Org from
//! the principal (R1); the clearance is checked here, never from the UI.

use std::collections::BTreeSet;

use pnex_core::governance::{Clearance, ClearanceInput, Declassify, Marking, MarkingInput};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
};
use serde_json::json;
use uuid::Uuid;

use crate::services::iam::audit;

/// Stream of governed reads and changes (D803).
pub const GOVERNANCE_STREAM: &str = "governance_reads";

#[derive(Debug)]
pub enum GovernanceError {
    Invalid { field: String, token: String },
    MarkingNotFound,
    KeyTaken,
    InUse,
    ClearanceNotFound,
    Db(DbErr),
}

impl From<DbErr> for GovernanceError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

fn invalid(field: &str, token: &str) -> GovernanceError {
    GovernanceError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const TS: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn marking_of(r: &QueryResult) -> Result<Marking, DbErr> {
    Ok(Marking {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        key: r.try_get("", "key")?,
        name: r.try_get("", "name")?,
        description: r.try_get("", "description")?,
        uses: r.try_get("", "uses")?,
        created_at: r.try_get("", "created_at")?,
    })
}

pub async fn markings(db: &DatabaseConnection, org_id: i64) -> Result<Vec<Marking>, DbErr> {
    let sql = format!(
        "SELECT m.id, m.key, m.name, m.description, \
           (SELECT count(*) FROM dataset_markings d WHERE d.marking_id = m.id) AS uses, \
           to_char(m.created_at AT TIME ZONE 'UTC', {TS}) AS created_at \
         FROM markings m WHERE m.org_id = $1 ORDER BY m.key"
    );
    let rows = db.query_all_raw(stmt(&sql, vec![org_id.into()])).await?;
    rows.iter().map(marking_of).collect()
}

pub async fn create_marking(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    input: &MarkingInput,
) -> Result<Marking, GovernanceError> {
    if let Some((field, token)) = input.violation() {
        return Err(invalid(field, &token));
    }
    let id = Uuid::new_v4();
    let done = db
        .execute_raw(stmt(
            "INSERT INTO markings (id, org_id, key, name, description, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (org_id, key) DO NOTHING",
            vec![
                id.into(),
                org_id.into(),
                input.key.clone().into(),
                input.name.trim().into(),
                input.description.trim().into(),
                by.into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(GovernanceError::KeyTaken);
    }
    journal(
        org_id,
        by,
        "marking_created",
        json!({ "marking": input.key }),
    );
    markings(db, org_id)
        .await?
        .into_iter()
        .find(|m| m.key == input.key)
        .ok_or(GovernanceError::MarkingNotFound)
}

/// A marking still carried by a table is kept: removing it would unmark data.
pub async fn delete_marking(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    id: Uuid,
) -> Result<(), GovernanceError> {
    let m = markings(db, org_id)
        .await?
        .into_iter()
        .find(|m| m.id == id.to_string())
        .ok_or(GovernanceError::MarkingNotFound)?;
    if m.uses > 0 {
        return Err(GovernanceError::InUse);
    }
    db.execute_raw(stmt(
        "DELETE FROM markings WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    journal(org_id, by, "marking_deleted", json!({ "marking": m.key }));
    Ok(())
}

fn clearance_of(r: &QueryResult) -> Result<Clearance, DbErr> {
    Ok(Clearance {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        user_id: r.try_get("", "user_id")?,
        user_email: r.try_get("", "email")?,
        marking_key: r.try_get("", "key")?,
        purpose: r.try_get("", "purpose")?,
        expires_at: r.try_get("", "expires_at")?,
        revoked_at: r.try_get("", "revoked_at")?,
        active: r.try_get("", "active")?,
        created_at: r.try_get("", "created_at")?,
    })
}

fn clearance_select() -> String {
    format!(
        "SELECT c.id, c.user_id, u.email, m.key, c.purpose, \
           to_char(c.expires_at AT TIME ZONE 'UTC', {TS}) AS expires_at, \
           to_char(c.revoked_at AT TIME ZONE 'UTC', {TS}) AS revoked_at, \
           (c.revoked_at IS NULL AND c.expires_at > now()) AS active, \
           to_char(c.created_at AT TIME ZONE 'UTC', {TS}) AS created_at \
         FROM clearances c JOIN markings m ON m.id = c.marking_id JOIN users u ON u.id = c.user_id"
    )
}

pub async fn clearances(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Clearance>), DbErr> {
    let total: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM clearances WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql = format!(
        "{} WHERE c.org_id = $1 ORDER BY active DESC, c.created_at DESC LIMIT $2 OFFSET $3",
        clearance_select()
    );
    let rows = db
        .query_all_raw(stmt(&sql, vec![org_id.into(), limit.into(), offset.into()]))
        .await?;
    Ok((
        total,
        rows.iter().map(clearance_of).collect::<Result<_, _>>()?,
    ))
}

async fn marking_id(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
) -> Result<Uuid, GovernanceError> {
    db.query_one_raw(stmt(
        "SELECT id FROM markings WHERE org_id = $1 AND key = $2",
        vec![org_id.into(), key.into()],
    ))
    .await?
    .map(|r| r.try_get("", "id"))
    .transpose()?
    .ok_or(GovernanceError::MarkingNotFound)
}

/// Clears a member of the org (never a service account, R22) for a marking.
pub async fn grant(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    input: &ClearanceInput,
) -> Result<Clearance, GovernanceError> {
    if let Some((field, token)) = input.violation() {
        return Err(invalid(field, &token));
    }
    let member = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM organization_members om JOIN users u ON u.id = om.user_id \
             WHERE om.org_id = $1 AND om.user_id = $2 \
               AND NOT EXISTS (SELECT 1 FROM iam_service_accounts s WHERE s.user_id = u.id)",
            vec![org_id.into(), input.user_id.into()],
        ))
        .await?
        .is_some();
    if !member {
        return Err(invalid("user_id", "not_member"));
    }
    let marking = marking_id(db, org_id, &input.marking_key).await?;
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO clearances (id, org_id, user_id, marking_id, purpose, expires_at, granted_by) \
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(days => $6::int), $7)",
        vec![
            id.into(),
            org_id.into(),
            input.user_id.into(),
            marking.into(),
            input.purpose.trim().into(),
            (input.days as i32).into(),
            by.into(),
        ],
    ))
    .await?;
    journal(
        org_id,
        by,
        "clearance_granted",
        json!({ "user_id": input.user_id, "marking": input.marking_key,
                "purpose": input.purpose.trim(), "days": input.days }),
    );
    let sql = format!("{} WHERE c.org_id = $1 AND c.id = $2", clearance_select());
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(GovernanceError::ClearanceNotFound)?;
    Ok(clearance_of(&row)?)
}

pub async fn revoke(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    id: Uuid,
) -> Result<(), GovernanceError> {
    let done = db
        .execute_raw(stmt(
            "UPDATE clearances SET revoked_at = coalesce(revoked_at, now()) \
             WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(GovernanceError::ClearanceNotFound);
    }
    journal(
        org_id,
        by,
        "clearance_revoked",
        json!({ "clearance": id.to_string() }),
    );
    Ok(())
}

/// Markings `user` is cleared for now, with the purpose of each clearance.
pub async fn cleared(
    db: &DatabaseConnection,
    org_id: i64,
    user: i64,
) -> Result<Vec<(String, String)>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT m.key, c.purpose FROM clearances c JOIN markings m ON m.id = c.marking_id \
             WHERE c.org_id = $1 AND c.user_id = $2 AND c.revoked_at IS NULL AND c.expires_at > now()",
            vec![org_id.into(), user.into()],
        ))
        .await?;
    rows.iter()
        .map(|r| Ok((r.try_get("", "key")?, r.try_get("", "purpose")?)))
        .collect()
}

/// Keys of the markings a table carries.
pub async fn dataset_markings(
    db: &DatabaseConnection,
    dataset: Uuid,
) -> Result<Vec<String>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT m.key FROM dataset_markings dm JOIN markings m ON m.id = dm.marking_id \
             WHERE dm.dataset_id = $1 ORDER BY m.key",
            vec![dataset.into()],
        ))
        .await?;
    rows.iter().map(|r| r.try_get("", "key")).collect()
}

/// D801: the markings of a table `user` is not cleared for; empty = may read.
/// A read allowed under markings is journaled with the purposes it used (D803).
pub async fn check_dataset_read(
    db: &DatabaseConnection,
    org_id: i64,
    user: i64,
    dataset: Uuid,
    what: &str,
    origin: &str,
) -> Result<Vec<String>, DbErr> {
    let carried = dataset_markings(db, dataset).await?;
    if carried.is_empty() {
        return Ok(Vec::new());
    }
    let cleared = cleared(db, org_id, user).await?;
    let missing: Vec<String> = carried
        .iter()
        .filter(|k| !cleared.iter().any(|(c, _)| c == *k))
        .cloned()
        .collect();
    let purposes: BTreeSet<&str> = cleared
        .iter()
        .filter(|(k, _)| carried.contains(k))
        .map(|(_, p)| p.as_str())
        .collect();
    journal(
        org_id,
        user,
        if missing.is_empty() {
            "read"
        } else {
            "read_refused"
        },
        json!({ "dataset": dataset.to_string(), "what": what, "markings": carried,
                "purposes": purposes, "origin": origin }),
    );
    Ok(missing)
}

/// Adds markings to a table; unknown keys are refused, none is removed.
pub async fn add_dataset_markings(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    dataset: Uuid,
    keys: &[String],
) -> Result<Vec<String>, GovernanceError> {
    let mut ids = Vec::new();
    for k in keys {
        ids.push(marking_id(db, org_id, k).await?);
    }
    for id in ids {
        db.execute_raw(stmt(
            "INSERT INTO dataset_markings (dataset_id, marking_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
            vec![dataset.into(), id.into()],
        ))
        .await?;
    }
    journal(
        org_id,
        by,
        "dataset_marked",
        json!({ "dataset": dataset.to_string(), "markings": keys }),
    );
    Ok(dataset_markings(db, dataset).await?)
}

/// Removes one marking from a table, with its justification in the journal (D802).
/// The next run whose input still carries it puts it back.
pub async fn declassify_dataset(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    dataset: Uuid,
    input: &Declassify,
) -> Result<Vec<String>, GovernanceError> {
    if let Some((field, token)) = input.violation() {
        return Err(invalid(field, &token));
    }
    let id = marking_id(db, org_id, &input.marking).await?;
    db.execute_raw(stmt(
        "DELETE FROM dataset_markings WHERE dataset_id = $1 AND marking_id = $2",
        vec![dataset.into(), id.into()],
    ))
    .await?;
    journal(
        org_id,
        by,
        "dataset_declassified",
        json!({ "dataset": dataset.to_string(), "marking": input.marking,
                "justification": input.justification.trim() }),
    );
    Ok(dataset_markings(db, dataset).await?)
}

/// One journal line (D803): identifiers only, never the data read.
pub fn journal(org_id: i64, user: i64, action: &str, detail: serde_json::Value) {
    let mut doc = json!({
        "_timestamp": chrono::Utc::now().timestamp_micros(),
        "message": action,
        "action": action,
        "user_id": user,
    });
    if let (Some(d), serde_json::Value::Object(extra)) = (doc.as_object_mut(), detail) {
        for (k, v) in extra {
            // Nested values flattened to text: O2 keeps flat columns.
            let v = match v {
                serde_json::Value::String(_)
                | serde_json::Value::Number(_)
                | serde_json::Value::Bool(_) => v,
                other => serde_json::Value::String(other.to_string()),
            };
            d.insert(k, v);
        }
    }
    audit::submit_doc(org_id, GOVERNANCE_STREAM, doc);
}
