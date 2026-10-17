//! Thermodynamic endpoints (pnex-coolprop in-process):
//!
//! - `POST /api/v1/thermo/diagram/query`: polylines of a diagram (p-h, T-s,
//!   psychrometric) for a pure / predefined fluid or an org mixture
//!   (resolved by name); in-memory cache keyed by body + **resolved spec**,
//!   so editing a mixture invalidates its entries;
//! - `POST /api/v1/thermo/cycle-points/query`: states `(input_pair, v1,
//!   v2)` to properties + coordinates on the target diagram.
//!
//! Both are reads with a complex body (D242: `…/query`), org-authenticated
//! without `can_write`; named mixtures are resolved in the caller's org.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::auth::OrgContext;
use crate::models::_entities::fluid_mixtures;
use crate::models::fluid_mixtures::FluidMixtures;
use pnex_coolprop::{
    compute_diagram, cycle_points, AxisRange, DiagramKind, DiagramRequest, DiagramResult,
    IsolineSpec,
};

/// Cache en mémoire (D3) : clé = hash du body canonique **+ spec résolue**
/// ⇒ éditer un mélange invalide automatiquement les entrées périmées.
/// Cap 32, éviction FIFO (le mutex global CoolProp sérialise de toute façon
/// les calculs, le cache protège surtout les ticks de polling 15 s).
type CacheState = (
    HashMap<String, Arc<DiagramResult>>,
    std::collections::VecDeque<String>,
);

static DIAGRAM_CACHE: OnceLock<Mutex<CacheState>> = OnceLock::new();

fn cache() -> &'static Mutex<CacheState> {
    DIAGRAM_CACHE.get_or_init(|| Mutex::new((HashMap::new(), Default::default())))
}

const CACHE_CAP: usize = 32;

fn cache_get(key: &str) -> Option<Arc<DiagramResult>> {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0
        .get(key)
        .cloned()
}

fn cache_put(key: String, v: Arc<DiagramResult>) {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if c.0.len() >= CACHE_CAP && !c.0.contains_key(&key) {
        if let Some(old) = c.1.pop_front() {
            c.0.remove(&old);
        }
        c.1.push_back(key.clone());
    }
    c.0.insert(key, v);
}

/// Diagram request: like [`pnex_coolprop::DiagramRequest`] but `fluid` may
/// be the name of an org mixture (resolved to its inline spec).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ThermoDiagramBody {
    /// CoolProp fluid name, inline CoolProp spec (`HEOS::R32[0.5]&R125[0.5]`)
    /// or the name of an org fluid mixture.
    fluid: String,
    #[schema(value_type = DiagramKindSchema)]
    diagram: DiagramKind,
    /// Axis range `{min, max}` in the diagram's SI units.
    #[schema(value_type = Option<Object>)]
    x_range: Option<AxisRange>,
    #[schema(value_type = Option<Object>)]
    y_range: Option<AxisRange>,
    /// Isolines `[{param, values: [f64]}]` (CoolProp parameter name).
    #[schema(value_type = Option<Vec<Object>>)]
    isolines: Option<Vec<IsolineSpec>>,
    /// Samples per curve.
    n_points: Option<usize>,
    /// Atmospheric pressure (Pa) of the psychrometric diagram.
    pressure: Option<f64>,
}

/// Diagram kind (schema mirror of `pnex_coolprop::DiagramKind`, which has
/// no OpenAPI derive).
#[derive(utoipa::ToSchema)]
#[schema(as = ThermoDiagramKind)]
#[allow(dead_code)]
enum DiagramKindSchema {
    /// log(p)-h: x = Hmass (J/kg), y = P (Pa).
    #[schema(rename = "ph")]
    Ph,
    /// T-s: x = Smass (J/kg·K), y = T (K).
    #[schema(rename = "ts")]
    Ts,
    /// Psychrometric: x = T (°C), y = W (kg water / kg dry air).
    #[schema(rename = "psychro")]
    Psychro,
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/thermo")
        .add("/diagram/query", post(diagram))
        .add("/cycle-points/query", post(cycle))
        .add("/fluids", get(fluids))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(diagram, cycle, fluids))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

/// Answer of `GET /api/v1/thermo/fluids`: the two picker lists, named
/// mixtures of the org + fluids known to CoolProp.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ThermoFluids {
    pub mixtures: Vec<String>,
    pub fluids: Vec<String>,
    /// Mixture name → resolved CoolProp spec (the flow node freezes the
    /// spec at pick time — D6). Unconvertible mixtures are left out.
    pub mixture_specs: std::collections::BTreeMap<String, String>,
}

/// `GET /api/v1/thermo/fluids` — plus de saisie libre du fluide : l'org
/// fournit ses mélanges, CoolProp ses fluides (retour utilisateur
/// 2026-09-18 : « l'utilisateur doit deviner les fluides »).
#[utoipa::path(
    get,
    path = "/api/v1/thermo/fluids",
    tag = "thermo",
    operation_id = "thermo_fluids",
    summary = "List the fluids usable in thermodynamic diagrams",
    description = "Returns the org's named fluid mixtures (with their resolved CoolProp spec) and the pure fluids known to CoolProp. Read-only, any org member.",
    responses(
        (status = 200, body = ThermoFluids),
        (status = 503, description = "CoolProp compute pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn fluids(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let rows = FluidMixtures::find()
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .order_by_asc(fluid_mixtures::Column::Name)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mixture_specs = rows
        .iter()
        .filter_map(|m| {
            let comp: pnex_core::FluidMixtureComposition =
                serde_json::from_value(m.composition.clone()).ok()?;
            Some((m.name.clone(), comp.to_coolprop_spec().ok()?))
        })
        .collect();
    let mixtures = rows.into_iter().map(|m| m.name).collect();
    let mut fluids = crate::services::compute_limits::run_coolprop(|| pnex_coolprop::fluids_list())
        .await?
        .unwrap_or_default();
    fluids.sort();
    Ok(format::json(ThermoFluids {
        mixtures,
        fluids,
        mixture_specs,
    })
    .into_response())
}

/// Most cycle points in one query.
const MAX_CYCLE_POINTS: usize = 64;

/// 400 `coolprop-error`: CoolProp's message travels verbatim in
/// `args.detail` (and the description).
fn coolprop_error(detail: &str) -> Error {
    crate::controllers::coded_error(
        StatusCode::BAD_REQUEST,
        pnex_core::err_codes::COOLPROP_ERROR,
        format!("CoolProp error: {detail}"),
        Some(serde_json::json!({ "detail": detail })),
    )
}

/// Resolves `fluid`: inline spec (`[`/`&`) as is; else an org mixture name
/// to its spec; else a CoolProp fluid name as is.
async fn resolve_fluid(db: &DatabaseConnection, org: &OrgContext, fluid: &str) -> Result<String> {
    if fluid.contains('[') || fluid.contains('&') {
        return Ok(fluid.to_string());
    }
    let row = FluidMixtures::find()
        .filter(fluid_mixtures::Column::OrgId.eq(org.org.id))
        .filter(fluid_mixtures::Column::Name.eq(fluid))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if let Some(m) = row {
        let comp: pnex_core::FluidMixtureComposition =
            serde_json::from_value(m.composition).map_err(|_| Error::InternalServerError)?;
        return comp
            .to_coolprop_spec()
            .map_err(|_| Error::InternalServerError);
    }
    Ok(fluid.to_string())
}

/// `POST /api/v1/thermo/diagram/query`: diagram polylines (cached).
#[utoipa::path(
    post,
    path = "/api/v1/thermo/diagram/query",
    tag = "thermo",
    operation_id = "thermo_diagram",
    summary = "Compute a thermodynamic diagram",
    description = "Computes the polylines of a p-h, T-s or psychrometric diagram (saturation dome + requested isolines) for a pure fluid, an inline CoolProp spec or an org mixture name. Read-only, no side effect; results are cached in memory keyed by request + resolved spec.",
    request_body = ThermoDiagramBody,
    responses(
        (status = 200, description = "Diagram result: `polylines` (curves in SI units) plus `x`/`y` axis metadata", body = serde_json::Value),
        (status = 400, description = "`coolprop-error`: CoolProp rejected the fluid or the ranges (`args.detail`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "CoolProp compute pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn diagram(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<ThermoDiagramBody>,
) -> Result<Response> {
    let spec = resolve_fluid(&ctx.db, &org, &body.fluid).await?;
    let req = DiagramRequest {
        fluid: spec.clone(),
        diagram: body.diagram,
        x_range: body.x_range,
        y_range: body.y_range,
        isolines: body.isolines.unwrap_or_default(),
        n_points: body.n_points,
        pressure: body.pressure,
    };
    // Cache key: canonical body + resolved spec.
    let key = format!(
        "{}|{}",
        serde_json::to_string(&req).unwrap_or_default(),
        spec
    );
    if let Some(hit) = cache_get(&key) {
        return Ok(format::json(&*hit).into_response());
    }
    // Blocking FFI behind a global mutex: run it off the async workers,
    // under the per-pod CoolProp cap (503 server-busy when saturated).
    let result = crate::services::compute_limits::run_coolprop(move || compute_diagram(&req))
        .await?
        .map_err(|e| coolprop_error(&e.0))?;
    let result = Arc::new(result);
    cache_put(key, result.clone());
    Ok(format::json(&*result).into_response())
}

/// Body of the cycle-points query.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ThermoCycleBody {
    /// Same resolution as the diagram `fluid`.
    fluid: String,
    #[schema(value_type = DiagramKindSchema)]
    diagram: DiagramKind,
    /// At most 64 points `{label, input_pair, v1, v2}`; `input_pair`
    /// is a CoolProp pair name such as `PT_INPUTS`.
    #[schema(value_type = Vec<Object>)]
    points: Vec<pnex_coolprop::CyclePointInput>,
    pressure: Option<f64>,
}

/// Computed cycle states, one per input point.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct ThermoCyclePoints {
    /// Properties + diagram coordinates of each input state, input order.
    #[schema(value_type = Vec<Object>)]
    points: Vec<pnex_coolprop::CyclePointOutput>,
}

/// `POST /api/v1/thermo/cycle-points/query`: states to properties +
/// coordinates.
#[utoipa::path(
    post,
    path = "/api/v1/thermo/cycle-points/query",
    tag = "thermo",
    operation_id = "thermo_cycle_points",
    summary = "Compute the states of a thermodynamic cycle",
    description = "For each input state (CoolProp input pair + two values) returns its properties and its coordinates on the target diagram. At most 64 points. Read-only, no side effect.",
    request_body = ThermoCycleBody,
    responses(
        (status = 200, description = "One computed point per input (properties + diagram coordinates)", body = ThermoCyclePoints),
        (status = 400, description = "`thermo-too-many-points` (`args.max`) or `coolprop-error` (`args.detail`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "CoolProp compute pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn cycle(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<ThermoCycleBody>,
) -> Result<Response> {
    if body.points.len() > MAX_CYCLE_POINTS {
        return Err(crate::controllers::coded_error(
            StatusCode::BAD_REQUEST,
            pnex_core::err_codes::THERMO_TOO_MANY_POINTS,
            format!("at most {MAX_CYCLE_POINTS} cycle points per request"),
            Some(serde_json::json!({ "max": MAX_CYCLE_POINTS.to_string() })),
        ));
    }
    let spec = resolve_fluid(&ctx.db, &org, &body.fluid).await?;
    let (diagram, input, pressure) = (body.diagram, body.points, body.pressure);
    let points = crate::services::compute_limits::run_coolprop(move || {
        cycle_points(&spec, diagram, &input, pressure)
    })
    .await?
    .map_err(|e| coolprop_error(&e.0))?;
    Ok(format::json(ThermoCyclePoints { points }).into_response())
}
