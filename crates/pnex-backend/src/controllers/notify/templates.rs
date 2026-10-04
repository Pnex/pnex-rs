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
pub(super) async fn list_templates(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = NotifyTemplates::find()
        .filter(notify_templates::Column::OrgId.eq(org.org.id))
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
pub(super) async fn create_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<TemplateInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let m = match tpl_service::create(&ctx.db, org.org.id, &params).await {
        Ok(m) => m,
        Err(e) => return template_write_response(e),
    };
    Ok((StatusCode::CREATED, format::json(template_dto(m))).into_response())
}

/// `PUT /api/v1/notify/templates/{id}`.
pub(super) async fn update_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<TemplateInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let updated = match tpl_service::update(&ctx.db, m, &params).await {
        Ok(m) => m,
        Err(e) => return template_write_response(e),
    };
    Ok(format::json(template_dto(updated)).into_response())
}

/// `DELETE /api/v1/notify/templates/{id}` — 204 ; les flows déployés
/// gardent leur snapshot (re-déployer pour propager, école « save ≠
/// déployé ») et le journal reste immuable (template_id sans FK).
pub(super) async fn delete_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    m.into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct PreviewBody {
    #[serde(default)]
    vars: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    payload: serde_json::Value,
}

/// `POST /api/v1/notify/templates/{id}/preview` — rendu seul, **jamais
/// d'envoi** ; les vars manquantes prennent leur `example` déclaré (D52).
/// Body en `Bytes` parsé à la main (même école que le test de canal : un
/// `Option<Json<T>>` casse sur les headers Content-Type injectés/dupliqués
/// des clients de test).
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
