//! Bibliothèque de widgets du studio SCADA (D41) — CRUD **mutable**
//! (pas de versioning : D24 réserve celui-ci aux documents composites).
//! Une instance posée dans un dashboard copie le template au drag : la
//! modification/suppression d'un template n'affecte jamais les
//! dashboards existants.
//!
//! École `flows.rs` : scoping org (404 masqué), `can_write()` en
//! écriture, 400 champ-par-champ, violations de config en 400
//! `{"violations": [...]}`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::viz_widget_library;
use crate::models::viz_widget_library::VizWidgetLibraries;
use pnex_core::{err_codes, VizViolation};

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Template de l'org, sinon None (→ 404 masqué cross-org).
async fn find_template(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<viz_widget_library::Model>> {
    VizWidgetLibraries::find_by_id(id)
        .filter(viz_widget_library::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

fn dto(m: viz_widget_library::Model) -> Result<pnex_core::VizWidget> {
    let config: pnex_core::WidgetTemplate =
        serde_json::from_value(m.config).map_err(|_| Error::InternalServerError)?;
    Ok(pnex_core::VizWidget {
        id: m.id.to_string(),
        org_id: m.org_id,
        name: m.name,
        kind: m.kind,
        config,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    })
}

/// Valide un template complet (nom, cohérence kind/widget_type, config) —
/// `Some(response)` = invalide (réponse 400 prête), `None` = valide.
fn validate_template(
    name: &str,
    kind: &str,
    config: &pnex_core::WidgetTemplate,
) -> Option<Response> {
    let name = name.trim();
    if name.is_empty() {
        return Some(field_status("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > 255 {
        return Some(field_status(
            "name",
            &format!("{}:255", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    if !pnex_core::VIZ_WIDGET_TYPES.contains(&kind) {
        return Some(field_status("kind", "Type de widget inconnu."));
    }
    if config.widget_type != kind {
        return Some(field_status(
            "kind",
            "kind doit correspondre au type du widget dans config.",
        ));
    }
    let mut violations: Vec<VizViolation> = Vec::new();
    pnex_core::validate_widget(
        "template",
        &config.widget_type,
        &config.source,
        &config.options,
        &mut violations,
    );
    // Le titre du template est obligatoire (un bouton de palette sans
    // libellé est inexploitable).
    if config.title.trim().is_empty() {
        violations.push(VizViolation::new(
            Some("template"),
            "title_missing",
            "The template title is required.",
        ));
    }
    if violations.is_empty() {
        return None;
    }
    Some(
        (
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "violations": violations })),
        )
            .into_response(),
    )
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/viz/widgets")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(update).delete(delete))
}

#[derive(Debug, Default, Deserialize)]
struct ListWidgetsQuery {
    search: Option<String>,
    kind: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/viz/widgets` — templates de l'org (D14), filtres
/// `search` (nom) et `kind`.
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListWidgetsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = VizWidgetLibraries::find()
        .filter(viz_widget_library::Column::OrgId.eq(org.org.id))
        .order_by_desc(viz_widget_library::Column::Id);
    if let Some(kind) = q.kind.as_deref().filter(|k| !k.is_empty()) {
        query = query.filter(viz_widget_library::Column::Kind.eq(kind));
    }
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        query = query.filter(pagination::sql_contains(
            (viz_widget_library::Entity, viz_widget_library::Column::Name),
            &pat,
        ));
    }
    let (count, rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<pnex_core::VizWidget> = rows.into_iter().map(dto).collect::<Result<_>>()?;

    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    if let Some(k) = q.kind.as_deref().filter(|k| !k.is_empty()) {
        filters.push(("kind".to_string(), k.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/viz/widgets",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

/// `GET /api/v1/viz/widgets/{id}`.
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(dto(m)?).into_response())
}

/// `POST /api/v1/viz/widgets` — ajoute un template à la bibliothèque.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateVizWidget>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "viz-write-forbidden",
            "Owner, admin or member role required to manage the widget library.",
        ));
    }
    if let Some(resp) = validate_template(&params.name, &params.kind, &params.config) {
        return Ok(resp);
    }
    // Unicité (org_id, name) : 400 lisible plutôt que 500 d'index.
    let clash = VizWidgetLibraries::find()
        .filter(viz_widget_library::Column::OrgId.eq(org.org.id))
        .filter(viz_widget_library::Column::Name.eq(params.name.trim()))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if clash {
        return Ok(field_status("name", "Un template porte déjà ce nom."));
    }
    let m = viz_widget_library::ActiveModel {
        org_id: Set(org.org.id),
        name: Set(params.name.trim().to_string()),
        kind: Set(params.kind),
        config: Set(serde_json::to_value(&params.config).map_err(|_| Error::InternalServerError)?),
        created_by: Set(Some(org.auth.user.id)),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(dto(m)?)).into_response())
}

/// `PATCH /api/v1/viz/widgets/{id}` — renomme et/ou remplace la config.
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<pnex_core::UpdateVizWidget>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "viz-write-forbidden",
            "Owner, admin or member role required to manage the widget library.",
        ));
    }
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let name = params.name.unwrap_or_else(|| m.name.clone());
    let config = match params.config {
        Some(c) => c,
        None => serde_json::from_value(m.config.clone()).map_err(|_| Error::InternalServerError)?,
    };
    if let Some(resp) = validate_template(&name, &m.kind, &config) {
        return Ok(resp);
    }
    let mut active: viz_widget_library::ActiveModel = m.into();
    active.name = Set(name.trim().to_string());
    active.config = Set(serde_json::to_value(&config).map_err(|_| Error::InternalServerError)?);
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(format::json(dto(updated)?).into_response())
}

/// `DELETE /api/v1/viz/widgets/{id}` — 204. Les dashboards déjà composés
/// gardent leur snapshot (aucune cascade métier).
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "viz-write-forbidden",
            "Owner, admin or member role required to manage the widget library.",
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
