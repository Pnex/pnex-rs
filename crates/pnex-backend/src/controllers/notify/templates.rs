use super::*;

// ─────────────────────────────── Templates ───────────────────────────────

use crate::services::notify_templates::{self as tpl_service, TemplateInput, TemplateWriteError};

/// Service error → the historical HTTP shapes (400 field, 400 render,
/// 409 name conflict, 500).
fn template_write_response(e: TemplateWriteError) -> Result<Response> {
    match e {
        TemplateWriteError::Field(field, token) => Ok(field_status(field, &token)),
        TemplateWriteError::Render(e) => Ok(template_render_error(&e).into_response()),
        TemplateWriteError::NameTaken => Err(conflict(
            "notify-template-name-conflict",
            "A template already uses this name.",
        )),
        TemplateWriteError::Db => Err(Error::InternalServerError),
    }
}

/// `GET /api/v1/notify/templates`.
#[utoipa::path(
    get,
    path = "/api/v1/notify/templates",
    tag = "notify",
    operation_id = "notify_templates_list",
    summary = "List the notification templates of the organisation",
    description = "Paginated, newest first. Templates are minijinja subject/body pairs with declared variables.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::NotifyTemplate>),
        (status = 400, description = "`field-invalid`: invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn list_templates(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut select = NotifyTemplates::find().filter(notify_templates::Column::OrgId.eq(org.org.id));
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        select = select.filter(pagination::sql_contains(
            (notify_templates::Entity, notify_templates::Column::Name),
            &pat,
        ));
    }
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_NOTIFY_TEMPLATE,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            select = select.filter(
                notify_templates::Column::Id.is_in(pagination::parse_ids::<uuid::Uuid>(&ids)),
            )
        }
        Err(resp) => return Ok(resp),
    }
    let rows = select
        .order_by_desc(notify_templates::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<NotifyTemplate> = rows.into_iter().map(template_dto).collect();
    let count = results.len() as i64;
    let (skip, take) = page.slice(results.len());
    let results: Vec<NotifyTemplate> = results.into_iter().skip(skip).take(take).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/notify/templates",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `GET /api/v1/notify/templates/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/notify/templates/{id}",
    tag = "notify",
    operation_id = "notify_templates_get",
    summary = "Get a notification template",
    description = "One template of the organisation; 404 when it does not exist or belongs to another organisation.",
    params(("id" = String, Path, format = "uuid", description = "Template id")),
    responses(
        (status = 200, body = pnex_core::NotifyTemplate),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn template_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(template_dto(m)).into_response())
}

/// `POST /api/v1/notify/templates`.
#[utoipa::path(
    post,
    path = "/api/v1/notify/templates",
    tag = "notify",
    operation_id = "notify_templates_create",
    summary = "Create a notification template",
    description = "Creates a minijinja template (optional subject, body, declared variables). The template must render with its declared examples. Requires owner, admin or member role. Names are unique per organisation (409).",
    request_body = crate::services::notify_templates::TemplateInput,
    responses(
        (status = 201, body = pnex_core::NotifyTemplate),
        (status = 400, description = "`field-invalid` or `notify-template-render` (template does not render, `args.detail`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`notify-template-name-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn create_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<TemplateInput>,
) -> Result<Response> {
    let m = match tpl_service::create(&ctx.db, org.org.id, &params).await {
        Ok(m) => m,
        Err(e) => return template_write_response(e),
    };
    Ok((StatusCode::CREATED, format::json(template_dto(m))).into_response())
}

/// `PUT /api/v1/notify/templates/{id}`.
#[utoipa::path(
    put,
    path = "/api/v1/notify/templates/{id}",
    tag = "notify",
    operation_id = "notify_templates_update",
    summary = "Replace a notification template",
    description = "Edits the template in place. Deployed flows keep the snapshot taken at deploy: redeploy them to use the change. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Template id")),
    request_body = crate::services::notify_templates::TemplateInput,
    responses(
        (status = 200, body = pnex_core::NotifyTemplate),
        (status = 400, description = "`field-invalid` or `notify-template-render` (template does not render, `args.detail`)", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail),
        (status = 409, description = "`notify-template-name-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn update_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<TemplateInput>,
) -> Result<Response> {
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let updated = match tpl_service::update(&ctx.db, m, &params).await {
        Ok(m) => m,
        Err(e) => return template_write_response(e),
    };
    Ok(format::json(template_dto(updated)).into_response())
}

/// `DELETE /api/v1/notify/templates/{id}` — 204; deployed flows
/// keep their snapshot (redeploy to propagate, "save ≠
/// deployed" school) and the log stays immutable (template_id without FK).
#[utoipa::path(
    delete,
    path = "/api/v1/notify/templates/{id}",
    tag = "notify",
    operation_id = "notify_templates_delete",
    summary = "Delete a notification template",
    description = "Deletes the template. Deployed flows keep their snapshot until redeployed; the journal is unchanged. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Template id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn delete_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    m.into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub(super) struct PreviewBody {
    #[serde(default)]
    vars: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    payload: serde_json::Value,
}

/// `POST /api/v1/notify/templates/{id}/preview` — render only, **never
/// a send**; missing vars take their declared `example` (D52).
/// Body as `Bytes` parsed by hand (same school as the channel test: an
/// `Option<Json<T>>` breaks on the injected/duplicated Content-Type headers
/// of test clients).
#[utoipa::path(
    post,
    path = "/api/v1/notify/templates/{id}/preview",
    tag = "notify",
    operation_id = "notify_templates_preview",
    summary = "Render a notification template without sending",
    description = "Renders the template with the given `vars` (missing ones take their declared example) and sample `payload` as the message context. Never sends anything. Body optional. Any role.",
    params(("id" = String, Path, format = "uuid", description = "Template id")),
    request_body(content = PreviewBody, description = "Optional; an empty body uses the declared examples"),
    responses(
        (status = 200, body = pnex_core::PreviewResult),
        (status = 400, description = "`notify-template-render` or `notify-test-body-invalid`", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn preview_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let PreviewBody { vars, payload } = if raw.is_empty() {
        PreviewBody::default()
    } else {
        serde_json::from_slice(&raw).map_err(|e| {
            Error::CustomError(
                StatusCode::BAD_REQUEST,
                loco_rs::controller::ErrorDetail::new(
                    err_codes::NOTIFY_TEST_BODY_INVALID,
                    format!("Invalid JSON body: {e}"),
                ),
            )
        })?
    };
    let rendered =
        tpl_service::preview(&m, vars, payload).map_err(|e| template_render_error(&e))?;
    Ok(format::json(pnex_core::PreviewResult {
        subject: rendered.subject,
        body: rendered.body,
    })
    .into_response())
}
