//! Comments of a page (pages.md P12): threads anchored on a text range,
//! replies, resolve / reopen. Org from the principal (R1), 404 across orgs
//! (page lookup), writes need a writer role (R2); a comment is edited by
//! its author only, deleted by its author or an org admin. Every change
//! tells the open editors to reload the comments (websocket, all pods).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::docs::{find_doc, user_names, PageQuery};
use crate::controllers::pagination;
use crate::models::_entities::{doc_comments, doc_versions};
use crate::services::pages;
use pnex_core::err_codes;
use pnex_core::pages::{
    CreateDocComment, DocComment, PatchDocComment, MAX_ANCHOR_CHARS, MAX_COMMENT_CHARS,
    MAX_QUOTE_CHARS,
};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/docs")
        .add("/{id}/comments", get(list).post(create))
        .add("/{id}/comments/{cid}", patch(update).delete(remove))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, create, update, remove))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

/// Id of the created comment.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct DocCommentCreated {
    id: i64,
}

fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
}

fn coded(status: StatusCode, code: &str, msg: &str) -> Error {
    crate::controllers::coded_error(status, code, msg, None)
}

fn forbidden() -> Error {
    coded(
        StatusCode::FORBIDDEN,
        err_codes::DOCS_WRITE_FORBIDDEN,
        "Owner, admin or member role required to comment pages.",
    )
}

fn not_author() -> Error {
    coded(
        StatusCode::FORBIDDEN,
        err_codes::DOC_COMMENT_NOT_AUTHOR,
        "Only the author can change this comment.",
    )
}

/// Body checked: `required`, `max_length:<n>`.
fn checked_body(raw: &str) -> std::result::Result<String, Box<Response>> {
    let body = raw.trim().to_string();
    if body.is_empty() {
        return Err(Box::new(field_status("body", err_codes::FIELD_REQUIRED)));
    }
    if body.chars().count() > MAX_COMMENT_CHARS {
        return Err(Box::new(field_status(
            "body",
            &format!("{}:{MAX_COMMENT_CHARS}", err_codes::FIELD_MAX_LENGTH),
        )));
    }
    Ok(body)
}

async fn find_comment(
    db: &DatabaseConnection,
    doc_id: Uuid,
    cid: i64,
) -> Result<doc_comments::Model> {
    doc_comments::Entity::find_by_id(cid)
        .filter(doc_comments::Column::DocId.eq(doc_id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)
}

fn changed(ctx: &AppContext, doc_id: Uuid) {
    pages::room::comments_changed(pages::valkey_url(&ctx.config), doc_id);
}

#[utoipa::path(
    get,
    path = "/api/v1/docs/{id}/comments",
    tag = "docs",
    operation_id = "docs_comments_list",
    summary = "List the comments of a page",
    description = "Comments of the page (thread roots with their anchor, and replies), oldest first, paginated (`limit`/`offset`); `mine` tells whether the caller wrote it. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Page id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::pages::DocComment>),
        (status = 404, description = "Page or comment not found", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
/// `GET /api/v1/docs/{id}/comments` — all comments, oldest first.
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) = pagination::sql_page(
        &ctx.db,
        doc_comments::Entity::find()
            .filter(doc_comments::Column::DocId.eq(d.id))
            .order_by_asc(doc_comments::Column::Id),
        page,
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    let names = user_names(&ctx.db, rows.iter().map(|c| c.author_id)).await?;
    let me = org.auth.user.id;
    let results: Vec<DocComment> = rows
        .into_iter()
        .map(|c| DocComment {
            id: c.id,
            parent_id: c.parent_id,
            anchor: c.anchor,
            quote: c.quote,
            body: c.body,
            author: c.author_id.and_then(|a| names.get(&a).cloned()),
            mine: c.author_id == Some(me),
            resolved: c.resolved,
            version_number: c.version_number,
            created_at: c.created_at.to_rfc3339(),
        })
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/docs/{}/comments", d.id),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/docs/{id}/comments",
    tag = "docs",
    operation_id = "docs_comments_create",
    summary = "Comment a page",
    description = "Starts a thread anchored on a text range (`anchor`, `quote`) or replies to a thread root (`parent_id`). Open editors are told to reload the comments. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Page id")),
    request_body = pnex_core::pages::CreateDocComment,
    responses(
        (status = 201, body = DocCommentCreated),
        (status = 400, description = "`field-invalid`: `body` (`required`, `max_length:<n>`), `parent_id` (`invalid`), `anchor` (`max_length:<n>`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`docs-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Page or comment not found", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
/// `POST /api/v1/docs/{id}/comments` — new thread (with an anchor) or
/// reply (`parent_id` = a thread root of the same page).
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateDocComment>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let body = match checked_body(&input.body) {
        Ok(b) => b,
        Err(resp) => return Ok(*resp),
    };
    if let Some(parent) = input.parent_id {
        let root = find_comment(&ctx.db, d.id, parent).await;
        if !matches!(root, Ok(ref r) if r.parent_id.is_none()) {
            return Ok(field_status("parent_id", err_codes::FIELD_INVALID));
        }
    }
    let anchor = input
        .anchor
        .filter(|a| !a.is_empty() && input.parent_id.is_none());
    if anchor
        .as_ref()
        .is_some_and(|a| a.chars().count() > MAX_ANCHOR_CHARS)
    {
        return Ok(field_status(
            "anchor",
            &format!("{}:{MAX_ANCHOR_CHARS}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let quote = input
        .quote
        .filter(|_| input.parent_id.is_none())
        .map(|q| q.trim().chars().take(MAX_QUOTE_CHARS).collect::<String>())
        .filter(|q| !q.is_empty());
    let version: Option<i64> = doc_versions::Entity::find()
        .select_only()
        .column_as(doc_versions::Column::VersionNumber.max(), "n")
        .filter(doc_versions::Column::DocId.eq(d.id))
        .into_tuple()
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .flatten();
    let row = doc_comments::ActiveModel {
        doc_id: Set(d.id),
        org_id: Set(org.org.id),
        parent_id: Set(input.parent_id),
        anchor: Set(anchor),
        quote: Set(quote),
        body: Set(body),
        author_id: Set(Some(org.auth.user.id)),
        version_number: Set(version),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    changed(&ctx, d.id);
    Ok((
        StatusCode::CREATED,
        format::json(DocCommentCreated { id: row.id }),
    )
        .into_response())
}

#[utoipa::path(
    patch,
    path = "/api/v1/docs/{id}/comments/{cid}",
    tag = "docs",
    operation_id = "docs_comments_update",
    summary = "Edit or resolve a comment",
    description = "Changes the body (author only) and/or resolves or reopens a thread root (any writer). Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Page id"), ("cid" = i64, Path, description = "Comment id")),
    request_body = pnex_core::pages::PatchDocComment,
    responses(
        (status = 204, description = "Updated"),
        (status = 400, description = "`field-invalid`: `body`, or `resolved` on a reply (`invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`docs-write-forbidden` (viewer) or `doc-comment-not-author`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Page or comment not found", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
/// `PATCH /api/v1/docs/{id}/comments/{cid}` — edit (author), resolve /
/// reopen a thread (any writer).
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, cid)): Path<(Uuid, i64)>,
    Json(input): Json<PatchDocComment>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let c = find_comment(&ctx.db, d.id, cid).await?;
    let mut active: doc_comments::ActiveModel = c.clone().into();
    if let Some(raw) = input.body.as_deref() {
        if c.author_id != Some(org.auth.user.id) {
            return Err(not_author());
        }
        match checked_body(raw) {
            Ok(b) => active.body = Set(b),
            Err(resp) => return Ok(*resp),
        }
    }
    if let Some(resolved) = input.resolved {
        if c.parent_id.is_some() {
            return Ok(field_status("resolved", err_codes::FIELD_INVALID));
        }
        active.resolved = Set(resolved);
        active.resolved_by = Set(resolved.then_some(org.auth.user.id));
    }
    active.updated_at = Set(chrono::Utc::now().into());
    active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    changed(&ctx, d.id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    delete,
    path = "/api/v1/docs/{id}/comments/{cid}",
    tag = "docs",
    operation_id = "docs_comments_delete",
    summary = "Delete a comment",
    description = "Deletes a comment (author, or an org owner/admin); deleting a thread root deletes its replies. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Page id"), ("cid" = i64, Path, description = "Comment id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`docs-write-forbidden` (viewer) or `doc-comment-not-author`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Page or comment not found", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
/// `DELETE /api/v1/docs/{id}/comments/{cid}` — author or org admin;
/// deleting a thread root deletes its replies.
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, cid)): Path<(Uuid, i64)>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let c = find_comment(&ctx.db, d.id, cid).await?;
    if c.author_id != Some(org.auth.user.id) && !org.can_administer() {
        return Err(not_author());
    }
    doc_comments::Entity::delete_by_id(c.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    changed(&ctx, d.id);
    Ok(StatusCode::NO_CONTENT.into_response())
}
