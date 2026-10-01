//! Endpoints thermodynamiques (pnex-coolprop in-process) :
//!
//! - `POST /api/v1/thermo/diagram` : polylines d'un diagramme (p-h / T-s /
//!   psychrométrique) pour un fluide pur / prédéfini / mélange org
//!   (résolu par nom) — cache en mémoire clé = hash(body + **spec résolue**)
//!   ⇒ auto-invalidation à l'édition d'un mélange ;
//! - `POST /api/v1/thermo/cycle-points` : états `(input_pair, v1, v2)` →
//!   propriétés + coordonnées du diagramme cible.
//!
//! Les deux endpoints sont authentifiés org (header `X-Org-Id`) mais sans
//! `can_write` (lecture) ; les mélanges nommés sont résolus dans l'org.

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

/// Requête HTTP du diagramme : comme [`pnex_coolprop::DiagramRequest`] mais
/// `fluid` peut être un nom de mélange org (résolu en spec inline).
#[derive(Debug, Deserialize)]
pub struct ThermoDiagramBody {
    fluid: String,
    diagram: DiagramKind,
    x_range: Option<AxisRange>,
    y_range: Option<AxisRange>,
    isolines: Option<Vec<IsolineSpec>>,
    n_points: Option<usize>,
    pressure: Option<f64>,
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/thermo")
        .add("/diagram", post(diagram))
        .add("/cycle-points", post(cycle))
        .add("/fluids", get(fluids))
}

/// Réponse de `GET /api/v1/thermo/fluids` — deux listes pour le picker :
/// mélanges nommés de l'org + fluides CoolProp disponibles.
#[derive(Debug, serde::Serialize)]
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

/// Résout `fluid` : spec inline (`[`/`&`) → telle quelle ; sinon nom de
/// mélange de l'org → spec ; sinon nom de fluide CoolProp → telle quelle.
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

/// `POST /api/v1/thermo/diagram` — polylines du diagramme (cache).
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
    // Clé de cache : body canonique + spec résolue.
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
        .map_err(|e| {
            Error::CustomError(
                StatusCode::BAD_REQUEST,
                loco_rs::controller::ErrorDetail::new("coolprop_error", e.0),
            )
        })?;
    let result = Arc::new(result);
    cache_put(key, result.clone());
    Ok(format::json(&*result).into_response())
}

/// Corps de cycle-points.
#[derive(Debug, Deserialize)]
pub struct ThermoCycleBody {
    fluid: String,
    diagram: DiagramKind,
    points: Vec<pnex_coolprop::CyclePointInput>,
    pressure: Option<f64>,
}

/// `POST /api/v1/thermo/cycle-points` — états → propriétés + coordonnées.
async fn cycle(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<ThermoCycleBody>,
) -> Result<Response> {
    if body.points.len() > 64 {
        return Err(Error::CustomError(
            StatusCode::BAD_REQUEST,
            loco_rs::controller::ErrorDetail::new(
                "too_many_points",
                "au plus 64 points de cycle par requête".to_string(),
            ),
        ));
    }
    let spec = resolve_fluid(&ctx.db, &org, &body.fluid).await?;
    let (diagram, input, pressure) = (body.diagram, body.points, body.pressure);
    let points = crate::services::compute_limits::run_coolprop(move || {
        cycle_points(&spec, diagram, &input, pressure)
    })
    .await?
    .map_err(|e| {
        Error::CustomError(
            StatusCode::BAD_REQUEST,
            loco_rs::controller::ErrorDetail::new("coolprop_error", e.0),
        )
    })?;
    Ok(format::json(points).into_response())
}
