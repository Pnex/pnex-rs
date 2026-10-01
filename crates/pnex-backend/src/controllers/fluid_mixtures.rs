//! Mélanges de fluides personnalisés — CRUD org-scoped.
//!
//! École `viz_widgets.rs` : scoping org (404 masqué cross-org),
//! `can_write()` en écriture, 400 champ-par-champ / violations
//! `pnex-core`. Le backend lie `pnex-coolprop` : conversion
//! massique→molaire + validation CoolProp à la sauvegarde.

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
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Mélange de l'org, sinon None (→ 404 masqué cross-org).
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

/// Réponse d'un mélange : composition stockée (avec `mole_fractions`
/// converties) + spec CoolProp inline prête à l'emploi.
fn dto(m: fluid_mixtures::Model) -> Result<pnex_core::FluidMixture> {
    let composition: FluidMixtureComposition =
        serde_json::from_value(m.composition).map_err(|_| Error::InternalServerError)?;
    Ok(pnex_core::FluidMixture {
        id: m.id.to_string(),
        name: m.name,
        description: m.description,
        spec: composition.to_coolprop_spec().ok(),
        composition,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    })
}

/// Décode la composition du body, la valide (pnex-core) et la convertit
/// en base molaire (massique → x_i = w_i/M_i normalisé) + masse molaire.
/// `Some(response)` = invalide (réponse 400 prête).
async fn validate_and_convert_composition(
    raw: serde_json::Value,
) -> std::result::Result<FluidMixtureComposition, Box<Response>> {
    let comp: FluidMixtureComposition = match serde_json::from_value(raw) {
        Ok(c) => c,
        Err(e) => {
            return Err(Box::new(field_status(
                "composition",
                &format!("composition invalide : {e}"),
            )));
        }
    };
    let violations: Vec<serde_json::Value> = comp
        .validate()
        .into_iter()
        .map(|v| serde_json::to_value(&v).unwrap_or_default())
        .collect();
    if !violations.is_empty() {
        return Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                format::json(serde_json::json!({ "violations": violations })),
            )
                .into_response(),
        ));
    }
    // Base uniforme (structurellement garanti par le type) — masse
    // molaire de chaque composant pur via CoolProp.
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
        Err(e) => {
            return Err(Box::new(field_status(
                "composition",
                &format!("fluide inconnu : {}", e.0),
            )));
        }
    };
    let mole_fractions = if comp.basis == MixtureBasis::Mass {
        match comp.to_mole_fractions(&masses) {
            Ok(x) => x,
            Err(msg) => return Err(Box::new(field_status("composition", &msg))),
        }
    } else {
        comp.components.iter().map(|c| c.fraction).collect()
    };
    let molar_mass = comp
        .mixture_molar_mass(&masses)
        .map_err(|msg| Box::new(field_status("composition", &msg)))?;
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

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    search: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/fluid-mixtures` — mélanges de l'org.
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

/// Corps de create/update.
#[derive(Debug, Deserialize)]
struct MixtureBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    composition: serde_json::Value,
}

/// `POST /api/v1/fluid-mixtures` — crée un mélange. La composition est
/// validée (pnex-core), convertie en base molaire si massique, puis la
/// spec résultante est instanciée par CoolProp (400 avec son message si
/// rejet). Le doublon de nom dans l'org répond 400 `name` (école
/// `viz_widgets`).
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
        Err(msg) => return Ok(field_status("composition", &msg)),
    };
    let checked = {
        let spec = spec.clone();
        crate::services::compute_limits::run_coolprop(move || {
            pnex_coolprop::validate_fluid_spec(&spec)
        })
        .await?
    };
    if let Err(e) = checked {
        return Ok(field_status(
            "composition",
            &format!("CoolProp a rejeté le mélange : {}", e.0),
        ));
    }
    // Unicité (org_id, name) : 400 lisible plutôt que 500 d'index.
    let clash = FluidMixtures::find()
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .filter(fluid_mixtures::Column::Name.eq(&name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if clash {
        return Ok(field_status("name", "Un mélange porte déjà ce nom."));
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
        Err(msg) => return Ok(field_status("composition", &msg)),
    };
    let checked = {
        let spec = spec.clone();
        crate::services::compute_limits::run_coolprop(move || {
            pnex_coolprop::validate_fluid_spec(&spec)
        })
        .await?
    };
    if let Err(e) = checked {
        return Ok(field_status(
            "composition",
            &format!("CoolProp a rejeté le mélange : {}", e.0),
        ));
    }
    // Renommer vers un nom déjà pris dans l'org → 400 `name`.
    let clash = FluidMixtures::find()
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .filter(fluid_mixtures::Column::Name.eq(&name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .filter(|row| row.id != m.id)
        .is_some();
    if clash {
        return Ok(field_status("name", "Un mélange porte déjà ce nom."));
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
