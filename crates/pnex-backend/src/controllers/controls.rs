//! Org controls (D125–D127, `docs/architecture/surfaces-controls.md`):
//! CRUD of the control definitions, value write and batch read.
//!
//! A control is an org entity (not a dashboard property): the same control
//! can sit on a mobile dashboard, a desktop dashboard and an annotation.
//! Writing it stores and publishes a value for the `control-source` flow
//! nodes — never a device command (D128).
//!
//! School `viz_widgets.rs`: org scoping (masked 404), `can_write()` for
//! every write (operating a control included), field errors as tokens.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::controls;
use crate::models::controls::Controls;
use crate::services::controls as store;
use pnex_core::err_codes;
use pnex_core::ui_control::{
    check_control_label, valid_control_key, valid_control_via, ControlEvent, ControlListener,
    ControlSpec, ControlValue, ControlValuesResponse, CreateUiControl, UiControl, UpdateUiControl,
    WriteControlValue, CONTROL_VALUES_MAX_IDS,
};

/// Machine code + canonical English description, optional `args`.
fn custom_error(
    status: StatusCode,
    code: &str,
    msg: &str,
    args: Option<serde_json::Value>,
) -> Error {
    Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail {
            error: Some(code.to_string()),
            description: Some(msg.to_string()),
            errors: args.map(|a| serde_json::json!({ "args": a })),
        },
    )
}

fn write_forbidden() -> Error {
    custom_error(
        StatusCode::FORBIDDEN,
        err_codes::CONTROL_WRITE_FORBIDDEN,
        "Owner, admin or member role required to manage or operate controls.",
        None,
    )
}

async fn find_control(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<controls::Model>> {
    Controls::find_by_id(id)
        .filter(controls::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

fn spec_of(m: &controls::Model) -> Result<ControlSpec> {
    serde_json::from_value(m.spec.clone()).map_err(|_| Error::InternalServerError)
}

fn dto(
    m: controls::Model,
    listened_by: Vec<ControlListener>,
    origin: Option<pnex_core::ui_control::ControlOrigin>,
) -> Result<UiControl> {
    let spec = spec_of(&m)?;
    Ok(UiControl {
        id: m.id,
        org_id: m.org_id,
        key: m.key,
        label: m.label,
        spec,
        listened_by,
        origin,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    })
}

/// Display origin of the surface-declared controls among `rows` (D131).
async fn origins_of(
    db: &DatabaseConnection,
    org_id: i64,
    rows: &[controls::Model],
) -> Result<std::collections::HashMap<Uuid, pnex_core::ui_control::ControlOrigin>> {
    crate::services::surface_controls::resolve_origins(db, org_id, rows)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Field checks of a full definition: `Some(response)` = invalid.
fn validate_definition(key: &str, label: &str, spec: &ControlSpec) -> Option<Response> {
    if key.is_empty() {
        return Some(super::field_error("key", err_codes::FIELD_REQUIRED));
    }
    if !valid_control_key(key) {
        return Some(super::field_error("key", err_codes::FIELD_INVALID));
    }
    if label.trim().is_empty() {
        return Some(super::field_error("label", err_codes::FIELD_REQUIRED));
    }
    if check_control_label(label).is_some() {
        return Some(super::field_error("label", err_codes::FIELD_INVALID));
    }
    if let Some((code, _)) = spec.check() {
        return Some(super::field_error("spec", code));
    }
    None
}

/// 409 when another control of the org already uses `key`.
async fn ensure_key_free(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
    except: Option<Uuid>,
) -> Result<()> {
    let mut q = Controls::find()
        .filter(controls::Column::OrgId.eq(org_id))
        .filter(controls::Column::Key.eq(key));
    if let Some(id) = except {
        q = q.filter(controls::Column::Id.ne(id));
    }
    let taken = q
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if taken {
        return Err(custom_error(
            StatusCode::CONFLICT,
            err_codes::CONTROL_KEY_TAKEN,
            "Another control of the organization already uses this key.",
            None,
        ));
    }
    Ok(())
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/controls")
        .add("", get(list).post(create))
        .add("/values", get(values))
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/value", post(write_value))
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListControlsQuery {
    /// Case-insensitive search on the label or key.
    search: Option<String>,
    /// Control kind filter (`switch`, `slider`…).
    kind: Option<String>,
    /// Label filter `name` / `name:value` (D218).
    label: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/controls` — controls of the org (D14), filters `search`
/// (label or key) and `kind`; each row carries its listening flows.
#[utoipa::path(
    get,
    path = "/api/v1/controls",
    tag = "controls",
    operation_id = "controls_list",
    summary = "List the controls of the organisation",
    description = "Sorted by label, paginated (`limit`/`offset`). Filters: `search` (label or key), `kind`, `label` (`name` or `name:value`). Each row lists the deployed flows listening to it and, for a control declared by a dashboard or annotation widget, its origin. Any member.",
    params(ListControlsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ui_control::UiControl>),
        (status = 400, description = "`field-invalid`: invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListControlsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = Controls::find()
        .filter(controls::Column::OrgId.eq(org.org.id))
        .order_by_asc(controls::Column::Label);
    if let Some(kind) = q.kind.as_deref().filter(|k| !k.is_empty()) {
        query = query.filter(controls::Column::Kind.eq(kind));
    }
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_CONTROL,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            query =
                query.filter(controls::Column::Id.is_in(pagination::parse_ids::<uuid::Uuid>(&ids)))
        }
        Err(resp) => return Ok(resp),
    }
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        query = query.filter(
            sea_orm::Condition::any()
                .add(pagination::sql_contains(
                    (controls::Entity, controls::Column::Label),
                    &pat,
                ))
                .add(pagination::sql_contains(
                    (controls::Entity, controls::Column::Key),
                    &pat,
                )),
        );
    }
    let (count, rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mut listeners = store::listeners_by_control(&ctx, org.org.id).await?;
    let mut origins = origins_of(&ctx.db, org.org.id, &rows).await?;
    let results: Vec<UiControl> = rows
        .into_iter()
        .map(|m| {
            let by = listeners.remove(&m.id).unwrap_or_default();
            let origin = origins.remove(&m.id);
            dto(m, by, origin)
        })
        .collect::<Result<_>>()?;

    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    if let Some(k) = q.kind.as_deref().filter(|k| !k.is_empty()) {
        filters.push(("kind".to_string(), k.to_string()));
    }
    if let Some(l) = q.label.as_deref().filter(|l| !l.is_empty()) {
        filters.push(("label".to_string(), l.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/controls",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

/// `GET /api/v1/controls/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/controls/{id}",
    tag = "controls",
    operation_id = "controls_get",
    summary = "Get a control",
    description = "Definition, listening flows and origin. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Control id (UUID)")),
    responses(
        (status = 200, body = pnex_core::ui_control::UiControl),
        (status = 404, description = "Unknown control (or another org's)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find_control(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let by = store::listeners_by_control(&ctx, org.org.id)
        .await?
        .remove(&id)
        .unwrap_or_default();
    let origin = origins_of(&ctx.db, org.org.id, std::slice::from_ref(&m))
        .await?
        .remove(&id);
    Ok(format::json(dto(m, by, origin)?).into_response())
}

/// `POST /api/v1/controls` — 201.
#[utoipa::path(
    post,
    path = "/api/v1/controls",
    tag = "controls",
    operation_id = "controls_create",
    summary = "Create a control",
    description = "Defines an org control (switch, slider, button…) that dashboards and annotations can show and `control-source` flow nodes listen to. The key must be unique in the org. Requires a writer role.",
    request_body = pnex_core::ui_control::CreateUiControl,
    responses(
        (status = 201, body = pnex_core::ui_control::UiControl),
        (status = 400, description = "`field-invalid` (`errors.fields`: `key`, `label`, `spec`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`control-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`control-key-taken`: another control of the org uses this key", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<CreateUiControl>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    let key = params.key.trim();
    if let Some(resp) = validate_definition(key, &params.label, &params.spec) {
        return Ok(resp);
    }
    ensure_key_free(&ctx.db, org.org.id, key, None).await?;
    let m = controls::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org.org.id),
        key: Set(key.to_string()),
        label: Set(params.label.trim().to_string()),
        kind: Set(params.spec.kind.as_str().to_string()),
        spec: Set(serde_json::to_value(&params.spec).map_err(|_| Error::InternalServerError)?),
        created_by: Set(Some(org.auth.user.id)),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(dto(m, Vec::new(), None)?)).into_response())
}

/// `PATCH /api/v1/controls/{id}` — key, label and/or spec. Changing the
/// kind is allowed: flows reference the id, the new domain applies to the
/// next writes only.
#[utoipa::path(
    patch,
    path = "/api/v1/controls/{id}",
    tag = "controls",
    operation_id = "controls_update",
    summary = "Update a control",
    description = "Changes the key, label and/or spec (absent fields keep their value). Changing the kind is allowed: flows reference the id, the new domain applies to the next values only. Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Control id (UUID)")),
    request_body = pnex_core::ui_control::UpdateUiControl,
    responses(
        (status = 200, body = pnex_core::ui_control::UiControl),
        (status = 400, description = "`field-invalid` (`errors.fields`: `key`, `label`, `spec`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`control-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown control (or another org's)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`control-key-taken`: another control of the org uses this key", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<UpdateUiControl>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    let Some(m) = find_control(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let key = params
        .key
        .as_deref()
        .map(str::trim)
        .unwrap_or(&m.key)
        .to_string();
    let label = params.label.clone().unwrap_or_else(|| m.label.clone());
    let spec = match params.spec {
        Some(s) => s,
        None => spec_of(&m)?,
    };
    if let Some(resp) = validate_definition(&key, &label, &spec) {
        return Ok(resp);
    }
    if key != m.key {
        ensure_key_free(&ctx.db, org.org.id, &key, Some(id)).await?;
    }
    let mut active: controls::ActiveModel = m.into();
    active.key = Set(key);
    active.label = Set(label.trim().to_string());
    active.kind = Set(spec.kind.as_str().to_string());
    active.spec = Set(serde_json::to_value(&spec).map_err(|_| Error::InternalServerError)?);
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let by = store::listeners_by_control(&ctx, org.org.id)
        .await?
        .remove(&id)
        .unwrap_or_default();
    let origin = origins_of(&ctx.db, org.org.id, std::slice::from_ref(&updated))
        .await?
        .remove(&id);
    Ok(format::json(dto(updated, by, origin)?).into_response())
}

/// `DELETE /api/v1/controls/{id}` — 204; 409 while deployed flows listen to
/// it (they would silently lose their trigger).
#[utoipa::path(
    delete,
    path = "/api/v1/controls/{id}",
    tag = "controls",
    operation_id = "controls_delete",
    summary = "Delete a control",
    description = "Permanently deletes the control and its stored value. Refused (409) while deployed flows listen to it. Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Control id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`control-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown control (or another org's)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`control-in-use` (arg `flow`: names of the listening flows)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    let Some(m) = find_control(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let listeners = store::listeners_by_control(&ctx, org.org.id)
        .await?
        .remove(&id)
        .unwrap_or_default();
    if !listeners.is_empty() {
        let names: Vec<String> = listeners.into_iter().map(|l| l.flow_name).collect();
        return Err(custom_error(
            StatusCode::CONFLICT,
            err_codes::CONTROL_IN_USE,
            "Deployed flows listen to this control: stop them or remove it from their nodes first.",
            Some(serde_json::json!({ "flow": names.join(", ") })),
        ));
    }
    m.into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    store::forget_value(
        crate::services::shared_valkey::conn(&ctx.config).await,
        org.org.id,
        id,
    )
    .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/v1/controls/{id}/value` — operate a control: validated,
/// stored, published to the listening flows, audited. Returns the stored
/// value (snapped to the step grid).
#[utoipa::path(
    post,
    path = "/api/v1/controls/{id}/value",
    tag = "controls",
    operation_id = "controls_write_value",
    summary = "Operate a control (send a value)",
    description = "Side effect: validates the value against the control spec (snapped to the step grid), stores it, publishes it to the deployed flows listening to the control and records an audit event. This is the user acting on the control; it never sends a device command directly (only a flow does). Rate limited. Returns the stored value. Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Control id (UUID)")),
    request_body = pnex_core::ui_control::WriteControlValue,
    responses(
        (status = 200, body = pnex_core::ui_control::ControlValue),
        (status = 400, description = "`control-value-invalid` (arg `reason`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`control-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown control (or another org's)", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`control-rate-limited`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`control-store-unavailable`: the value was not sent", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn write_value(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<WriteControlValue>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    let Some(m) = find_control(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let spec = spec_of(&m)?;
    let v = spec.accepts(params.value).map_err(|e| {
        custom_error(
            StatusCode::BAD_REQUEST,
            err_codes::CONTROL_VALUE_INVALID,
            "Value refused by the control.",
            Some(serde_json::json!({ "reason": e.code() })),
        )
    })?;
    let via = params.via.filter(|v| valid_control_via(v));
    let user = &org.auth.user;
    let by = user
        .full_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| user.email.clone());
    let event = ControlEvent {
        control_id: id,
        key: m.key,
        value: ControlValue {
            v,
            ts_ms: chrono::Utc::now().timestamp_millis(),
            by: Some(by),
            via,
            option: spec.symbol_of(v),
        },
    };
    let conn = crate::services::shared_valkey::conn(&ctx.config).await;
    match store::write_value(conn, org.org.id, &event).await {
        Ok(()) => {}
        Err(store::WriteError::RateLimited) => {
            return Err(custom_error(
                StatusCode::TOO_MANY_REQUESTS,
                err_codes::CONTROL_RATE_LIMITED,
                "Too fast: wait a moment before sending another value.",
                None,
            ));
        }
        Err(store::WriteError::Unavailable) => {
            return Err(custom_error(
                StatusCode::SERVICE_UNAVAILABLE,
                err_codes::CONTROL_STORE_UNAVAILABLE,
                "The control store (Valkey) is unavailable: the value was not sent.",
                None,
            ));
        }
    }
    store::record_write_event(&ctx, org.org.id, &event);
    Ok(format::json(event.value).into_response())
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ControlValuesQuery {
    /// Comma-separated control ids (UUIDs), at most 100.
    ids: Option<String>,
}

/// Parses the comma-separated `ids`; `Err` = a ready 400.
fn parse_value_ids(raw: Option<&str>) -> std::result::Result<Vec<Uuid>, Response> {
    let ids: Vec<Uuid> = raw
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Uuid::parse_str)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| super::field_error("ids", err_codes::FIELD_INVALID_UUID))?;
    if ids.len() > CONTROL_VALUES_MAX_IDS {
        return Err(super::field_error(
            "ids",
            &format!("{}:{CONTROL_VALUES_MAX_IDS}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    Ok(ids)
}

/// `GET /api/v1/controls/values?ids=` — last commanded values for a
/// surface (viewer included). Ids of another org read as `null`.
#[utoipa::path(
    get,
    path = "/api/v1/controls/values",
    tag = "controls",
    operation_id = "controls_values",
    summary = "Read the last values of several controls",
    description = "Any member. Returns, for each requested id (`ids`, comma-separated, at most 100), the last commanded value or `null` (never set, unknown or another org's control).",
    params(ControlValuesQuery),
    responses(
        (status = 200, body = pnex_core::ui_control::ControlValuesResponse),
        (status = 400, description = "`field-invalid`: `ids` not UUIDs (`invalid-uuid`) or too many (`max_length:100`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn values(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ControlValuesQuery>,
) -> Result<Response> {
    let ids = match parse_value_ids(q.ids.as_deref()) {
        Ok(ids) => ids,
        Err(resp) => return Ok(resp),
    };
    // Only ids of this org are read: an unknown id answers null without a
    // Valkey round trip.
    let known: Vec<Uuid> = if ids.is_empty() {
        Vec::new()
    } else {
        Controls::find()
            .filter(controls::Column::OrgId.eq(org.org.id))
            .filter(controls::Column::Id.is_in(ids.clone()))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|m| m.id)
            .collect()
    };
    let conn = crate::services::shared_valkey::conn(&ctx.config).await;
    let mut values = store::read_values(conn, org.org.id, &known).await;
    for id in ids {
        values.entry(id).or_insert(None);
    }
    Ok(format::json(ControlValuesResponse { values }).into_response())
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, detail, create, update, delete, write_value, values))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
