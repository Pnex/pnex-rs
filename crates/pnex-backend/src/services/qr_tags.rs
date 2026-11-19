//! QR tags (parts-docs.md D962–D963): blank sheets printed ahead and linked
//! on site, or a tag made for an object; a scan resolves the token inside
//! the caller's org only (R1), so a tag of another org is unknown.

use pnex_core::qr::{token_from, QrState, QrTag, SHEET_MAX, TOKEN_LEN};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum QrError {
    #[error("invalid {field}: {token}")]
    Invalid { field: String, token: String },
    #[error("tag not found")]
    NotFound,
    /// Only a blank tag is linked; a linked or revoked one stays as it is.
    #[error("tag not blank")]
    NotBlank,
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const VIEW: &str = "SELECT q.token, q.state, q.object_id, o.title AS object_title, \
     o.type_key AS object_type, q.sheet_id, q.created_at, q.linked_at \
     FROM qr_tags q LEFT JOIN objects o ON o.id = q.object_id";

fn view(r: &QueryResult) -> Result<QrTag, sea_orm::DbErr> {
    let at = |c: &str| -> Result<Option<String>, sea_orm::DbErr> {
        Ok(
            r.try_get::<Option<chrono::DateTime<chrono::FixedOffset>>>("", c)?
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        )
    };
    let state: String = r.try_get("", "state")?;
    Ok(QrTag {
        token: r.try_get("", "token")?,
        state: QrState::parse(&state).unwrap_or(QrState::Revoked),
        object_id: r
            .try_get::<Option<Uuid>>("", "object_id")?
            .map(|u| u.to_string()),
        object_title: r.try_get("", "object_title")?,
        object_type: r.try_get("", "object_type")?,
        sheet_id: r
            .try_get::<Option<Uuid>>("", "sheet_id")?
            .map(|u| u.to_string()),
        created_at: at("created_at")?.unwrap_or_default(),
        linked_at: at("linked_at")?,
    })
}

fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_LEN];
    rand::RngExt::fill(&mut rand::rng(), &mut bytes);
    token_from(&bytes)
}

/// `count` blank tags of one new sheet, in printing order.
pub async fn sheet(db: &DatabaseConnection, org: i64, count: u32) -> Result<Vec<QrTag>, QrError> {
    if !(1..=SHEET_MAX).contains(&count) {
        return Err(QrError::Invalid {
            field: "count".into(),
            token: format!("range:1..{SHEET_MAX}"),
        });
    }
    let sheet = Uuid::new_v4();
    let tokens: Vec<String> = (0..count).map(|_| new_token()).collect();
    db.execute_raw(stmt(
        "INSERT INTO qr_tags (token, org_id, sheet_id) SELECT t, $2, $3 FROM unnest($1::text[]) t",
        vec![tokens.clone().into(), org.into(), sheet.into()],
    ))
    .await?;
    let rows = db
        .query_all_raw(stmt(
            &format!("{VIEW} WHERE q.org_id = $1 AND q.token = ANY($2)"),
            vec![org.into(), tokens.clone().into()],
        ))
        .await?;
    let mut out: Vec<QrTag> = rows.iter().map(view).collect::<Result<_, _>>()?;
    out.sort_by_key(|t| tokens.iter().position(|x| *x == t.token));
    Ok(out)
}

/// The tag an object opens: its first live one, else a new linked tag.
/// `true` when it was made now.
pub async fn for_object(
    db: &DatabaseConnection,
    org: i64,
    user: i64,
    object: Uuid,
) -> Result<(QrTag, bool), QrError> {
    crate::services::ontology::objects::find(db, org, object)
        .await
        .map_err(|_| QrError::NotFound)?;
    let found = db
        .query_one_raw(stmt(
            &format!(
                "{VIEW} WHERE q.org_id = $1 AND q.object_id = $2 AND q.state = 'linked' \
                 ORDER BY q.linked_at LIMIT 1"
            ),
            vec![org.into(), object.into()],
        ))
        .await?;
    if let Some(r) = found {
        return Ok((view(&r)?, false));
    }
    let token = new_token();
    db.execute_raw(stmt(
        "INSERT INTO qr_tags (token, org_id, object_id, state, linked_at, linked_by) \
         VALUES ($1, $2, $3, 'linked', now(), $4)",
        vec![token.clone().into(), org.into(), object.into(), user.into()],
    ))
    .await?;
    Ok((get(db, org, &token).await?, true))
}

/// The tags of the org, of one object or one state, newest first.
pub async fn list(
    db: &DatabaseConnection,
    org: i64,
    object: Option<Uuid>,
    state: Option<QrState>,
    tokens: Option<Vec<String>>,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<QrTag>), QrError> {
    let filter = "q.org_id = $1 AND ($2::uuid IS NULL OR q.object_id = $2) \
                  AND ($3::text IS NULL OR q.state = $3) AND ($4::text[] IS NULL OR q.token = ANY($4))";
    let state = state.map(|s| s.as_str().to_string());
    let count: i64 = db
        .query_one_raw(stmt(
            &format!("SELECT count(*) AS n FROM qr_tags q WHERE {filter}"),
            vec![
                org.into(),
                object.into(),
                state.clone().into(),
                tokens.clone().into(),
            ],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{VIEW} WHERE {filter} ORDER BY q.created_at DESC, q.token LIMIT $5 OFFSET $6"
            ),
            vec![
                org.into(),
                object.into(),
                state.into(),
                tokens.into(),
                limit.into(),
                offset.into(),
            ],
        ))
        .await?;
    Ok((count, rows.iter().map(view).collect::<Result<_, _>>()?))
}

/// A token of the caller's org; any other is unknown.
pub async fn get(db: &DatabaseConnection, org: i64, token: &str) -> Result<QrTag, QrError> {
    if !pnex_core::qr::valid_token(token) {
        return Err(QrError::NotFound);
    }
    let r = db
        .query_one_raw(stmt(
            &format!("{VIEW} WHERE q.org_id = $1 AND q.token = $2"),
            vec![org.into(), token.into()],
        ))
        .await?
        .ok_or(QrError::NotFound)?;
    Ok(view(&r)?)
}

/// Links a blank tag to an object of the org (association on site, D963).
pub async fn link(
    db: &DatabaseConnection,
    org: i64,
    user: i64,
    token: &str,
    object: Uuid,
) -> Result<QrTag, QrError> {
    get(db, org, token).await?;
    crate::services::ontology::objects::find(db, org, object)
        .await
        .map_err(|_| QrError::Invalid {
            field: "object_id".into(),
            token: "unknown".into(),
        })?;
    let done = db
        .execute_raw(stmt(
            "UPDATE qr_tags SET state = 'linked', object_id = $3, linked_at = now(), linked_by = $4 \
             WHERE org_id = $1 AND token = $2 AND state = 'blank'",
            vec![org.into(), token.into(), object.into(), user.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(QrError::NotBlank);
    }
    get(db, org, token).await
}

/// Revokes a tag: a lost or torn label stops opening its object.
pub async fn revoke(db: &DatabaseConnection, org: i64, token: &str) -> Result<QrTag, QrError> {
    get(db, org, token).await?;
    db.execute_raw(stmt(
        "UPDATE qr_tags SET state = 'revoked', revoked_at = now() \
         WHERE org_id = $1 AND token = $2 AND state <> 'revoked'",
        vec![org.into(), token.into()],
    ))
    .await?;
    get(db, org, token).await
}
