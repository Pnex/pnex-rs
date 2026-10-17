//! Pages (`docs/architecture/pages.md`, decision #21) — REST side: list,
//! create, rename, delete, named versions. The content itself goes through
//! the websocket (`ws_doc`). Org from the principal (R1), 404 across orgs,
//! writes gated by `can_write` (R2), no internal id in read DTOs (R4).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use base64::Engine as _;
use loco_rs::prelude::*;
use sea_orm::{
    ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
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
        .add("/{id}/versions/{n}/approve", post(approve_version))
        .add("/{id}/versions/{n}/pdf", get(version_pdf))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        create,
        detail,
        rename,
        remove,
        versions,
        create_version,
        version_state,
        approve_version,
        version_pdf
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
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

/// A page without its content: lists and lookups never load the (up to
/// 8 MiB) snapshot.
#[derive(Debug, Clone, sea_orm::FromQueryResult)]
pub(crate) struct DocHead {
    pub id: Uuid,
    pub org_id: i64,
    pub title: String,
    pub created_at: sea_orm::prelude::DateTimeWithTimeZone,
    pub updated_at: sea_orm::prelude::DateTimeWithTimeZone,
}

fn heads() -> sea_orm::Select<docs::Entity> {
    docs::Entity::find().select_only().columns([
        docs::Column::Id,
        docs::Column::OrgId,
        docs::Column::Title,
        docs::Column::CreatedAt,
        docs::Column::UpdatedAt,
    ])
}

fn summary(d: &DocHead) -> DocSummary {
    DocSummary {
        id: d.id,
        title: d.title.clone(),
        created_at: d.created_at.to_rfc3339(),
        updated_at: d.updated_at.to_rfc3339(),
    }
}

/// Page of the caller's org, else 404 (cross-org masked).
pub(crate) async fn find_doc(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<DocHead> {
    heads()
        .filter(docs::Column::Id.eq(id))
        .filter(docs::Column::OrgId.eq(org_id))
        .into_model::<DocHead>()
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
    /// Label filter `name` / `name:value` (D218).
    label: Option<String>,
    /// Title contains (case-insensitive).
    search: Option<String>,
}

/// `GET /api/v1/docs` — pages of the org, most recently edited first.
#[utoipa::path(
    get,
    path = "/api/v1/docs",
    tag = "docs",
    operation_id = "docs_list",
    summary = "List the pages of the organisation",
    description = "Pages (rich-text documents) of the org, most recently edited first, paginated (`limit`/`offset`), filtered by `search` (title) and `label`. Content is not included (it is edited over the websocket). Any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::pages::DocSummary>),
        (status = 400, description = "`field-invalid`: invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut scope = sea_orm::Condition::all().add(docs::Column::OrgId.eq(org.org.id));
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        scope = scope.add(pagination::sql_contains(
            (docs::Entity, docs::Column::Title),
            &pat,
        ));
    }
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_DOC,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            scope = scope.add(docs::Column::Id.is_in(pagination::parse_ids::<Uuid>(&ids)))
        }
        Err(resp) => return Ok(resp),
    }
    let count = docs::Entity::find()
        .filter(scope.clone())
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let rows = heads()
        .filter(scope)
        .order_by_desc(docs::Column::UpdatedAt)
        .limit(page.limit as u64)
        .offset(page.offset as u64)
        .into_model::<DocHead>()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<DocSummary> = rows.iter().map(summary).collect();
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
#[utoipa::path(
    post,
    path = "/api/v1/docs",
    tag = "docs",
    operation_id = "docs_create",
    summary = "Create a page",
    description = "Creates a page, blank or seeded from a page template of an installed pack (`template` key). Owner, admin or member.",
    request_body = pnex_core::pages::CreateDoc,
    responses(
        (status = 201, body = pnex_core::pages::DocSummary),
        (status = 400, description = "`field-invalid`: `title` (`required`, `max_length:<n>`) or `template` (`invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`docs-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    // Same templates as the pages of a case (installed packs, O43).
    let body = match input.template.as_deref().filter(|k| !k.is_empty()) {
        None => None,
        Some(key) => {
            let templates = crate::services::ontology::packs::page_templates(&ctx.db, org.org.id)
                .await
                .map_err(|_| Error::InternalServerError)?;
            match templates.into_iter().find(|t| t.key == key) {
                Some(t) => Some(t.body),
                None => return Ok(field_status("template", err_codes::FIELD_INVALID)),
            }
        }
    };
    let row = docs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org.org.id),
        title: Set(title),
        snapshot: Set(body.map(|b| crate::services::pages::seed::from_markdown(&b))),
        snapshot_seq: Set(0),
        created_by: Set(Some(org.auth.user.id)),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    let head = DocHead {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        created_at: row.created_at,
        updated_at: row.updated_at,
    };
    Ok((StatusCode::CREATED, format::json(summary(&head))).into_response())
}

/// `GET /api/v1/docs/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/docs/{id}",
    tag = "docs",
    operation_id = "docs_get",
    summary = "Get a page",
    description = "Title and timestamps of a page (no content). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Page id")),
    responses(
        (status = 200, body = pnex_core::pages::DocSummary),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    Ok(format::json(summary(&d)).into_response())
}

/// `PATCH /api/v1/docs/{id}` — rename.
#[utoipa::path(
    patch,
    path = "/api/v1/docs/{id}",
    tag = "docs",
    operation_id = "docs_rename",
    summary = "Rename a page",
    description = "Sets the title of a page. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Page id")),
    request_body = pnex_core::pages::PatchDoc,
    responses(
        (status = 200, body = pnex_core::pages::DocSummary),
        (status = 400, description = "`field-invalid`: `title` (`required`, `max_length:<n>`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`docs-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    docs::Entity::update_many()
        .col_expr(docs::Column::Title, sea_orm::sea_query::Expr::value(title))
        .col_expr(
            docs::Column::UpdatedAt,
            sea_orm::sea_query::Expr::current_timestamp(),
        )
        .filter(docs::Column::Id.eq(d.id))
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let row = find_doc(&ctx.db, org.org.id, d.id).await?;
    Ok(format::json(summary(&row)).into_response())
}

/// `DELETE /api/v1/docs/{id}` — 204; log and versions cascade.
#[utoipa::path(
    delete,
    path = "/api/v1/docs/{id}",
    tag = "docs",
    operation_id = "docs_delete",
    summary = "Delete a page",
    description = "Permanently deletes a page with its edit log, named versions, comments and labels. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Page id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`docs-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/docs/{id}/versions",
    tag = "docs",
    operation_id = "docs_versions_list",
    summary = "List the named versions of a page",
    description = "Named versions (label, author, AI flag, approval), newest first, paginated (`limit`/`offset`); frozen content is not included. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Page id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::pages::DocVersionSummary>),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let scope = doc_versions::Column::DocId.eq(d.id);
    let count = doc_versions::Entity::find()
        .filter(scope.clone())
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Labels only: the frozen states stay in the database.
    let rows = doc_versions::Entity::find()
        .select_only()
        .columns([
            doc_versions::Column::VersionNumber,
            doc_versions::Column::Label,
            doc_versions::Column::CreatedBy,
            doc_versions::Column::CreatedAt,
            doc_versions::Column::AiGenerated,
            doc_versions::Column::FlowId,
            doc_versions::Column::ApprovedBy,
            doc_versions::Column::ApprovedAt,
        ])
        .filter(scope)
        .order_by_desc(doc_versions::Column::VersionNumber)
        .limit(page.limit as u64)
        .offset(page.offset as u64)
        .into_model::<VersionHead>()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let names = user_names(
        &ctx.db,
        rows.iter().flat_map(|v| [v.created_by, v.approved_by]),
    )
    .await?;
    let results: Vec<DocVersionSummary> = rows
        .iter()
        .map(|v| DocVersionSummary {
            version_number: v.version_number,
            label: v.label.clone(),
            created_by: v.created_by.and_then(|u| names.get(&u).cloned()),
            created_at: v.created_at.to_rfc3339(),
            ai_generated: v.ai_generated,
            flow_id: v.flow_id,
            approved_by: v.approved_by.and_then(|u| names.get(&u).cloned()),
            approved_at: v.approved_at.map(|t| t.to_rfc3339()),
        })
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/docs/{}/versions", d.id),
        &[],
        page,
        count as i64,
        results,
    ))
    .into_response())
}

/// `limit`/`offset` of a plain paginated list.
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PageQuery {
    #[param(value_type = Option<i64>)]
    pub(crate) limit: Option<String>,
    #[param(value_type = Option<i64>)]
    pub(crate) offset: Option<String>,
}

/// Display names (full name, else email) of the given user ids.
pub(crate) async fn user_names(
    db: &DatabaseConnection,
    ids: impl IntoIterator<Item = Option<i64>>,
) -> Result<std::collections::HashMap<i64, String>> {
    let ids: Vec<i64> = ids.into_iter().flatten().collect();
    if ids.is_empty() {
        return Ok(Default::default());
    }
    Ok(users::Entity::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|u| (u.id, u.full_name.unwrap_or(u.email)))
        .collect())
}

#[derive(sea_orm::FromQueryResult)]
struct VersionHead {
    version_number: i64,
    label: String,
    created_by: Option<i64>,
    created_at: sea_orm::prelude::DateTimeWithTimeZone,
    ai_generated: bool,
    flow_id: Option<i64>,
    approved_by: Option<i64>,
    approved_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
}

/// `POST /api/v1/docs/{id}/versions` — freezes the current content.
#[utoipa::path(
    post,
    path = "/api/v1/docs/{id}/versions",
    tag = "docs",
    operation_id = "docs_versions_create",
    summary = "Freeze a named version of a page",
    description = "Snapshots the current content of the page as a new named version. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Page id")),
    request_body = pnex_core::pages::CreateDocVersion,
    responses(
        (status = 201, body = pnex_core::pages::DocVersionSummary),
        (status = 400, description = "`field-invalid`: `label` (`required`, `max_length:<n>`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`docs-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    let v = pages::create_version(&ctx.db, d.id, d.org_id, &label, Some(org.auth.user.id))
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
        ai_generated: v.ai_generated,
        flow_id: v.flow_id,
        approved_by: None,
        approved_at: None,
    };
    Ok((StatusCode::CREATED, format::json(dto)).into_response())
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PdfQuery {
    /// Language of the PDF notices (`fr`, `en`).
    #[serde(default)]
    lang: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/docs/{id}/versions/{n}/pdf",
    tag = "docs",
    operation_id = "docs_versions_pdf",
    summary = "Download a version as PDF",
    description = "Renders a named version as an A4 PDF attachment; an AI-generated version not yet approved carries a 'not reviewed' notice. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Page id"), ("n" = i64, Path, description = "Version number"), PdfQuery),
    responses(
        (status = 200, content_type = "application/pdf", body = Vec<u8>),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
/// `GET /api/v1/docs/{id}/versions/{n}/pdf?lang=fr` — the frozen version as
/// an A4 PDF (D209). Read access; an AI version not yet approved carries
/// the "not reviewed" notice.
async fn version_pdf(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, n)): Path<(Uuid, i64)>,
    axum::extract::Query(q): axum::extract::Query<PdfQuery>,
) -> Result<Response> {
    let rendered = pages::pdf::render_version(&ctx.db, org.org.id, id, n, &q.lang)
        .await
        .map_err(|e| {
            tracing::warn!(doc_id = %id, error = %e, "pages: pdf not rendered");
            Error::InternalServerError
        })?
        .ok_or(Error::NotFound)?;
    Ok((
        StatusCode::OK,
        [
            ("content-type", "application/pdf".to_string()),
            (
                "content-disposition",
                format!("attachment; filename=\"{}\"", rendered.filename),
            ),
            ("x-content-type-options", "nosniff".to_string()),
            // Same sandbox as user content served by the media library (R12).
            (
                "content-security-policy",
                "default-src 'none'; sandbox".to_string(),
            ),
        ],
        rendered.bytes,
    )
        .into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/docs/{id}/versions/{n}/approve",
    tag = "docs",
    operation_id = "docs_versions_approve",
    summary = "Approve a named version",
    description = "Records the caller as approver of a frozen version (an AI report may leave the platform once approved). Idempotent: the first approval is kept. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Page id"), ("n" = i64, Path, description = "Version number")),
    responses(
        (status = 200, body = pnex_core::pages::DocVersionSummary),
        (status = 403, description = "`docs-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
/// `POST /api/v1/docs/{id}/versions/{n}/approve` — a person approves a
/// frozen version (D210: an AI report of a flow leaves PneX once
/// approved). Editors only; idempotent (the first approval is kept).
async fn approve_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, n)): Path<(Uuid, i64)>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let d = find_doc(&ctx.db, org.org.id, id).await?;
    let v = doc_versions::Entity::find()
        .filter(doc_versions::Column::DocId.eq(d.id))
        .filter(doc_versions::Column::VersionNumber.eq(n))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::NotFound)?;
    let v = if v.approved_at.is_some() {
        v
    } else {
        let mut a: doc_versions::ActiveModel = v.into();
        a.approved_by = sea_orm::Set(Some(org.auth.user.id));
        a.approved_at = sea_orm::Set(Some(chrono::Utc::now().into()));
        sea_orm::ActiveModelTrait::update(a, &ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
    };
    // O75: author and approver are resolved like the list, whoever calls.
    let names = user_names(&ctx.db, [v.created_by, v.approved_by]).await?;
    let name = |uid: Option<i64>| uid.and_then(|u| names.get(&u).cloned());
    Ok(format::json(DocVersionSummary {
        version_number: v.version_number,
        label: v.label,
        created_by: name(v.created_by),
        created_at: v.created_at.to_rfc3339(),
        ai_generated: v.ai_generated,
        flow_id: v.flow_id,
        approved_by: name(v.approved_by),
        approved_at: v.approved_at.map(|t| t.to_rfc3339()),
    })
    .into_response())
}

#[utoipa::path(
    get,
    path = "/api/v1/docs/{id}/versions/{n}",
    tag = "docs",
    operation_id = "docs_versions_get",
    summary = "Get the frozen state of a version",
    description = "Frozen Yjs document state (v1 encoding, base64) of a named version, for read-only viewing, diff and restore. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Page id"), ("n" = i64, Path, description = "Version number")),
    responses(
        (status = 200, body = pnex_core::pages::DocVersionState),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
