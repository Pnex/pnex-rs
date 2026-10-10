//! Pages (`docs/architecture/pages.md`, decision #21) — REST side: list,
//! create, rename, delete, named versions. The content itself goes through
//! the websocket (`ws_doc`). Org from the principal (R1), 404 across orgs,
//! writes gated by `can_write` (R2), no internal id in read DTOs (R4).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use base64::Engine as _;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{doc_versions, docs, users};
use crate::services::pages;
use pnex_core::err_codes;
use pnex_core::pages::{
    CreateDoc, CreateDocVersion, DocSummary, DocVersionState, DocVersionSummary, PatchDoc,
    MAX_TITLE_CHARS, MAX_VERSION_LABEL_CHARS,
};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/docs")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(rename).delete(remove))
        .add("/{id}/versions", get(versions).post(create_version))
        .add("/{id}/versions/{n}", get(version_state))
}

fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

fn forbidden() -> Error {
    crate::controllers::coded_error(
        StatusCode::FORBIDDEN,
        err_codes::DOCS_WRITE_FORBIDDEN,
        "Owner, admin or member role required to edit pages.",
        None,
    )
}

/// Trimmed text, or the field error (`required`, `max_length:<n>`).
fn checked_text(field: &str, raw: &str, max: usize) -> std::result::Result<String, Box<Response>> {
    let text = raw.trim().to_string();
    if text.is_empty() {
        return Err(Box::new(field_status(field, err_codes::FIELD_REQUIRED)));
    }
    if text.chars().count() > max {
        return Err(Box::new(field_status(
            field,
            &format!("{}:{max}", err_codes::FIELD_MAX_LENGTH),
        )));
    }
    Ok(text)
}

fn summary(d: &docs::Model) -> DocSummary {
    DocSummary {
        id: d.id,
        title: d.title.clone(),
        created_at: d.created_at.to_rfc3339(),
        updated_at: d.updated_at.to_rfc3339(),
    }
}

/// Page of the caller's org, else 404 (cross-org masked).
pub(crate) async fn find_doc(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<docs::Model> {
    docs::Entity::find_by_id(id)
        .filter(docs::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/docs` — pages of the org, most recently edited first.
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = docs::Entity::find()
        .filter(docs::Column::OrgId.eq(org.org.id))
        .order_by_desc(docs::Column::UpdatedAt)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len();
    let (skip, take) = page.slice(count);
    let results: Vec<DocSummary> = rows.iter().skip(skip).take(take).map(summary).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/docs",
        &[],
        page,
        count as i64,
        results,
    ))
    .into_response())
}

/// `POST /api/v1/docs` — empty page; 201 = DocSummary.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<CreateDoc>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let title = match checked_text("title", &input.title, MAX_TITLE_CHARS) {
        Ok(t) => t,
        Err(resp) => return Ok(*resp),
    };
    let row = docs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org.org.id),
        title: Set(title),
        snapshot_seq: Set(0),
        created_by: Set(Some(org.auth.user.id)),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(summary(&row))).into_response())
}

/// `GET /api/v1/docs/{id}`.
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    Ok(format::json(summary(&d)).into_response())
}

/// `PATCH /api/v1/docs/{id}` — rename.
async fn rename(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<PatchDoc>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let title = match checked_text("title", &input.title, MAX_TITLE_CHARS) {
        Ok(t) => t,
        Err(resp) => return Ok(*resp),
    };
    let mut active: docs::ActiveModel = d.into();
    active.title = Set(title);
    active.updated_at = Set(chrono::Utc::now().into());
    let row = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(format::json(summary(&row)).into_response())
}

/// `DELETE /api/v1/docs/{id}` — 204; log and versions cascade.
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    docs::Entity::delete_by_id(d.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    crate::services::resources::purge_for(
        &ctx.db,
        org.org.id,
        pnex_core::resources::KIND_DOC,
        &d.id.to_string(),
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/v1/docs/{id}/versions` — named versions, newest first.
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let rows = doc_versions::Entity::find()
        .filter(doc_versions::Column::DocId.eq(d.id))
        .order_by_desc(doc_versions::Column::VersionNumber)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let authors: Vec<i64> = rows.iter().filter_map(|v| v.created_by).collect();
    let names: std::collections::HashMap<i64, String> = if authors.is_empty() {
        Default::default()
    } else {
        users::Entity::find()
            .filter(users::Column::Id.is_in(authors))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|u| (u.id, u.full_name.unwrap_or(u.email)))
            .collect()
    };
    let results: Vec<DocVersionSummary> = rows
        .iter()
        .map(|v| DocVersionSummary {
            version_number: v.version_number,
            label: v.label.clone(),
            created_by: v.created_by.and_then(|u| names.get(&u).cloned()),
            created_at: v.created_at.to_rfc3339(),
        })
        .collect();
    Ok(
        format::json(serde_json::json!({ "count": results.len(), "results": results }))
            .into_response(),
    )
}

/// `POST /api/v1/docs/{id}/versions` — freezes the current content.
async fn create_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateDocVersion>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let label = match checked_text("label", &input.label, MAX_VERSION_LABEL_CHARS) {
        Ok(t) => t,
        Err(resp) => return Ok(*resp),
    };
    let v = pages::create_version(&ctx.db, &d, &label, org.auth.user.id)
        .await
        .map_err(|e| {
            tracing::warn!(doc_id = %d.id, error = %e, "pages: version not created");
            Error::InternalServerError
        })?;
    let dto = DocVersionSummary {
        version_number: v.version_number,
        label: v.label,
        created_by: Some(
            org.auth
                .user
                .full_name
                .clone()
                .unwrap_or(org.auth.user.email.clone()),
        ),
        created_at: v.created_at.to_rfc3339(),
    };
    Ok((StatusCode::CREATED, format::json(dto)).into_response())
}

/// `GET /api/v1/docs/{id}/versions/{n}` — frozen state of version `n`.
async fn version_state(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, n)): Path<(Uuid, i64)>,
) -> Result<Response> {
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let v = doc_versions::Entity::find()
        .filter(doc_versions::Column::DocId.eq(d.id))
        .filter(doc_versions::Column::VersionNumber.eq(n))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)?;
    Ok(format::json(DocVersionState {
        version_number: v.version_number,
        label: v.label,
        state_b64: base64::engine::general_purpose::STANDARD.encode(&v.snapshot),
    })
    .into_response())
}
