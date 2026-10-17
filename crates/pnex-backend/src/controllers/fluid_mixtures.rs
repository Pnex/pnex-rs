//! Custom fluid mixtures: org-scoped CRUD.
//!
//! Org scoping (cross-org masked as 404), `can_write()` on writes, 400
//! `field-invalid` / `mixture-invalid` violations from `pnex-core`, CoolProp
//! rejections as `coolprop-error` (engine detail in `args.detail`). The
//! backend links `pnex-coolprop`: mass-to-mole conversion + CoolProp check
//! on save.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::fluid_mixtures;
use crate::models::fluid_mixtures::FluidMixtures;
use pnex_core::{err_codes, FluidMixtureComposition, MixtureBasis};

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
}

/// 400 `coolprop-error`: CoolProp refused the mixture (detail verbatim).
fn coolprop_rejected(detail: &str) -> Response {
    super::coded_error(
        StatusCode::BAD_REQUEST,
        err_codes::COOLPROP_ERROR,
        format!("CoolProp rejected the mixture: {detail}"),
        Some(serde_json::json!({ "detail": detail })),
    )
    .into_response()
}

/// Mixture of the org, else None (cross-org masked as 404).
async fn find_mixture(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<Option<fluid_mixtures::Model>> {
    FluidMixtures::find_by_id(id)
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// API answer of a mixture: stored composition (converted
/// `mole_fractions`) + ready-to-use inline CoolProp spec.
fn dto(m: fluid_mixtures::Model) -> Result<pnex_core::FluidMixture> {
    let composition: FluidMixtureComposition =
        serde_json::from_value(m.composition).map_err(|_| Error::InternalServerError)?;
    Ok(pnex_core::FluidMixture {
        id: m.id,
        name: m.name,
        description: m.description,
        spec: composition.to_coolprop_spec().ok(),
        composition,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    })
}

/// Decodes the body composition, validates it (pnex-core) and converts it
/// to the mole basis (mass: x_i = w_i/M_i normalised) + molar mass.
/// `Err(response)` = invalid (ready 400).
async fn validate_and_convert_composition(
    raw: serde_json::Value,
) -> std::result::Result<FluidMixtureComposition, Box<Response>> {
    let comp: FluidMixtureComposition = match serde_json::from_value(raw) {
        Ok(c) => c,
        Err(_) => {
            return Err(Box::new(field_status(
                "composition",
                err_codes::FIELD_INVALID,
            )));
        }
    };
    let violations: Vec<serde_json::Value> = comp
        .validate()
        .into_iter()
        .map(|v| serde_json::to_value(&v).unwrap_or_default())
        .collect();
    if !violations.is_empty() {
        return Err(Box::new(super::violations_error(
            pnex_core::err_codes::MIXTURE_INVALID,
            violations,
        )));
    }
    // Uniform basis (guaranteed by the type): molar mass of each pure
    // component through CoolProp.
    let fluids: Vec<String> = comp.components.iter().map(|c| c.fluid.clone()).collect();
    // CoolProp FFI is blocking: run it on the blocking pool under the
    // per-pod CoolProp cap (503 server-busy when saturated).
    let masses = crate::services::compute_limits::run_coolprop(move || {
        let refs: Vec<&str> = fluids.iter().map(String::as_str).collect();
        pnex_coolprop::molar_masses(&refs)
    })
    .await
    .map_err(|e| Box::new(e.into_response()))?;
    let masses = match masses {
        Ok(ms) => ms,
        Err(e) => return Err(Box::new(coolprop_rejected(&e.0))),
    };
    let mole_fractions = if comp.basis == MixtureBasis::Mass {
        match comp.to_mole_fractions(&masses) {
            Ok(x) => x,
            Err(_) => {
                return Err(Box::new(field_status(
                    "composition",
                    err_codes::FIELD_INVALID,
                )))
            }
        }
    } else {
        comp.components.iter().map(|c| c.fraction).collect()
    };
    let molar_mass = comp
        .mixture_molar_mass(&masses)
        .map_err(|_| Box::new(field_status("composition", err_codes::FIELD_INVALID)))?;
    Ok(FluidMixtureComposition {
        basis: comp.basis,
        components: comp.components,
        mole_fractions,
        molar_mass: Some(molar_mass),
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/fluid-mixtures")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).put(update).delete(delete))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, detail, create, update, delete))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Case-insensitive substring of the name.
    search: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/fluid-mixtures`: mixtures of the org.
#[utoipa::path(
    get,
    path = "/api/v1/fluid-mixtures",
    tag = "fluid-mixtures",
    operation_id = "fluid_mixtures_list",
    summary = "List the fluid mixtures of the organisation",
    description = "Paginated (`limit`/`offset`), newest first. Each mixture carries its stored composition (mole fractions converted) and its ready-to-use CoolProp spec.",
    params(ListQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::FluidMixture>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = FluidMixtures::find()
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .order_by_desc(fluid_mixtures::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let all: Vec<pnex_core::FluidMixture> = rows
        .into_iter()
        .filter(|m| pagination::rust_search_match(&q.search, &[&m.name]))
        .map(dto)
        .collect::<Result<_>>()?;
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    let results: Vec<pnex_core::FluidMixture> = all.into_iter().skip(skip).take(take).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/fluid-mixtures",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `GET /api/v1/fluid-mixtures/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/fluid-mixtures/{id}",
    tag = "fluid-mixtures",
    operation_id = "fluid_mixtures_get",
    summary = "Get a fluid mixture",
    description = "Returns one mixture of the organisation with its CoolProp spec.",
    params(("id" = i64, Path, description = "Mixture id")),
    responses(
        (status = 200, body = pnex_core::FluidMixture),
        (status = 404, description = "Unknown mixture or mixture of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(m) = find_mixture(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(dto(m)?).into_response())
}

/// Create/replace body.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct MixtureBody {
    /// Unique within the org, 1..=255 characters.
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Composition `{basis: "mole"|"mass", components: [{fluid, fraction}]}`.
    #[schema(value_type = pnex_core::FluidMixtureComposition)]
    composition: serde_json::Value,
}

/// `POST /api/v1/fluid-mixtures` — crée un mélange. La composition est
/// validée (pnex-core), convertie en base molaire si massique, puis la
/// spec résultante est instanciée par CoolProp (400 avec son message si
/// rejet). Le doublon de nom dans l'org répond 400 `name` (école
/// `viz_widgets`).
#[utoipa::path(
    post,
    path = "/api/v1/fluid-mixtures",
    tag = "fluid-mixtures",
    operation_id = "fluid_mixtures_create",
    summary = "Create a fluid mixture",
    description = "Validates the composition, converts mass fractions to mole fractions, checks the resulting spec with CoolProp, then stores the mixture. Requires owner, admin or member role.",
    request_body = MixtureBody,
    responses(
        (status = 201, body = pnex_core::FluidMixture),
        (status = 400, description = "`field-invalid` (`name`: `required`, `max_length:255`, `unique`; `composition`: `invalid`), `mixture-invalid` (`errors.violations`) or `coolprop-error` (CoolProp rejected the mixture, `args.detail`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`fluid-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 503, description = "CoolProp compute pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<MixtureBody>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "fluid-write-forbidden",
            "Owner, admin or member role required to manage fluid mixtures.",
        ));
    }
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Ok(field_status("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > 255 {
        return Ok(field_status(
            "name",
            &format!("{}:255", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let composition = match validate_and_convert_composition(body.composition).await {
        Ok(c) => c,
        Err(resp) => return Ok(*resp),
    };
    let spec = match composition.to_coolprop_spec() {
        Ok(s) => s,
        Err(_) => return Ok(field_status("composition", err_codes::FIELD_INVALID)),
    };
    let checked = {
        let spec = spec.clone();
        crate::services::compute_limits::run_coolprop(move || {
            pnex_coolprop::validate_fluid_spec(&spec)
        })
        .await?
    };
    if let Err(e) = checked {
        return Ok(coolprop_rejected(&e.0));
    }
    // Unique (org_id, name): a readable 400 instead of an index 500.
    let clash = FluidMixtures::find()
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .filter(fluid_mixtures::Column::Name.eq(&name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if clash {
        return Ok(field_status("name", err_codes::FIELD_UNIQUE));
    }
    let m = fluid_mixtures::ActiveModel {
        org_id: Set(org.org.id),
        name: Set(name),
        description: Set(body.description.filter(|d| !d.trim().is_empty())),
        composition: Set(
            serde_json::to_value(&composition).map_err(|_| Error::InternalServerError)?
        ),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(dto(m)?)).into_response())
}

/// `PUT /api/v1/fluid-mixtures/{id}` — remplacement complet
/// (nom, description, composition revalidée + reconvertie).
#[utoipa::path(
    put,
    path = "/api/v1/fluid-mixtures/{id}",
    tag = "fluid-mixtures",
    operation_id = "fluid_mixtures_update",
    summary = "Replace a fluid mixture",
    description = "Full replacement of name, description and composition (revalidated and reconverted like on create). Flows and widgets that froze the former spec are not changed. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Mixture id")),
    request_body = MixtureBody,
    responses(
        (status = 200, body = pnex_core::FluidMixture),
        (status = 400, description = "`field-invalid` (`name`: `required`, `max_length:255`, `unique`; `composition`: `invalid`), `mixture-invalid` (`errors.violations`) or `coolprop-error` (CoolProp rejected the mixture, `args.detail`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`fluid-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown mixture or mixture of another org", body = crate::openapi::ErrorDetail),
        (status = 503, description = "CoolProp compute pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(body): Json<MixtureBody>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "fluid-write-forbidden",
            "Owner, admin or member role required to manage fluid mixtures.",
        ));
    }
    let Some(m) = find_mixture(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Ok(field_status("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > 255 {
        return Ok(field_status(
            "name",
            &format!("{}:255", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let composition = match validate_and_convert_composition(body.composition).await {
        Ok(c) => c,
        Err(resp) => return Ok(*resp),
    };
    let spec = match composition.to_coolprop_spec() {
        Ok(s) => s,
        Err(_) => return Ok(field_status("composition", err_codes::FIELD_INVALID)),
    };
    let checked = {
        let spec = spec.clone();
        crate::services::compute_limits::run_coolprop(move || {
            pnex_coolprop::validate_fluid_spec(&spec)
        })
        .await?
    };
    if let Err(e) = checked {
        return Ok(coolprop_rejected(&e.0));
    }
    // Renaming to a name already used in the org: 400 `name`.
    let clash = FluidMixtures::find()
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .filter(fluid_mixtures::Column::Name.eq(&name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .filter(|row| row.id != m.id)
        .is_some();
    if clash {
        return Ok(field_status("name", err_codes::FIELD_UNIQUE));
    }
    let mut active: fluid_mixtures::ActiveModel = m.into();
    active.name = Set(name);
    active.description = Set(body.description.filter(|d| !d.trim().is_empty()));
    active.composition =
        Set(serde_json::to_value(&composition).map_err(|_| Error::InternalServerError)?);
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(format::json(dto(updated)?).into_response())
}

/// `DELETE /api/v1/fluid-mixtures/{id}` — 204. Les flows/widgets qui ont
/// figé la spec inline restent valides (la spec CoolProp ne dépend pas de
/// la ligne en base).
#[utoipa::path(
    delete,
    path = "/api/v1/fluid-mixtures/{id}",
    tag = "fluid-mixtures",
    operation_id = "fluid_mixtures_delete",
    summary = "Delete a fluid mixture",
    description = "Deletes the mixture. Flows and widgets that froze its inline spec keep working. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Mixture id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`fluid-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown mixture or mixture of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "fluid-write-forbidden",
            "Owner, admin or member role required to manage fluid mixtures.",
        ));
    }
    let Some(m) = find_mixture(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    m.into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
