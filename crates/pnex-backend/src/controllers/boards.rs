//! Board profiles: catalogue of the v2 board profiles (`mcu_boards.details`)
//! for the wizard variant picker and the SVG editor.
//!
//! `GET /api/v1/boards`: read by any org member. Mux functions, flags,
//! offered modes and warnings are **computed server-side** (chip caps in
//! code, B0.6): the frontend never re-implements the validation.

use axum::extract::{Query, State};
use axum::response::Response;
use loco_rs::prelude::*;
use sea_orm::{EntityTrait, QueryOrder};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::mcu_boards;

pub fn routes() -> Routes {
    Routes::new().prefix("/api/v1").add("/boards", get(list))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// SoC family filter (e.g. `esp32`, `esp8266`); unknown value = empty list.
    #[serde(default)]
    soc: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// One pin of a board profile, with its server-computed display enrichment.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[schema(
    description = "One pin of a board profile, with server-computed mux functions, flags, modes and warnings."
)]
struct BoardPinDto {
    label: String,
    gpio: Option<u16>,
    kind: pnex_core::BoardPinKind,
    pos: pnex_core::PinPos,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(rename = "fn", skip_serializing_if = "Vec::is_empty")]
    fns: Vec<pnex_core::PinFn>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    flags: Vec<pnex_core::PinFlag>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    available_modes: Vec<pnex_core::Mode>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

/// One board profile of `GET /api/v1/boards`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct BoardDto {
    id: i64,
    name: String,
    soc: String,
    pretty_name: Option<String>,
    chip_label: Option<String>,
    profile_id: String,
    pio_board: Option<String>,
    /// Header layout of the profile (`BoardLayout`).
    layout: serde_json::Value,
    /// Peripherals the board can host (`BoardPeripherals`).
    peripherals: serde_json::Value,
    pins: Vec<BoardPinDto>,
}

fn pin_dto(soc: pnex_core::Soc, pp: &pnex_core::BoardProfilePin) -> BoardPinDto {
    let fns: Vec<pnex_core::PinFn> = pp
        .gpio
        .map(|g| pnex_core::caps::pin_fns(soc, g).to_vec())
        .unwrap_or_default();
    let flags: Vec<pnex_core::PinFlag> = pp
        .gpio
        .map(|g| pnex_core::caps::pin_flags(soc, g).to_vec())
        .unwrap_or_default();
    let available_modes: Vec<pnex_core::Mode> = pp
        .gpio
        .map(|g| pnex_core::caps::available_modes(soc, g))
        .unwrap_or_default();
    let warnings: Vec<String> = pp
        .gpio
        .map(|g| pnex_core::caps::pin_warnings(soc, g))
        .unwrap_or_default();
    BoardPinDto {
        label: pp.label.clone(),
        gpio: pp.gpio,
        kind: pp.kind,
        pos: pp.pos,
        note: pp.note.clone(),
        fns,
        flags,
        available_modes,
        warnings,
    }
}

/// `GET /api/v1/boards`: boards whose `details` parse as v2, enriched with
/// the chip caps. `?soc=esp32` filters by family (wizard variant picker).
/// Bounded catalogue: filtered in Rust, then the page is cut.
#[utoipa::path(
    get,
    path = "/api/v1/boards",
    tag = "boards",
    operation_id = "boards_list",
    summary = "List the board profiles",
    description = "Paginated global catalogue of v2 board profiles (pinout editor, provisioning wizard). Pin mux functions, flags, available modes and warnings are computed server-side from the chip capabilities. Boards whose profile does not parse as v2 are skipped. Read-only, any org member.",
    params(ListQuery),
    responses((status = 200, body = pnex_core::Paginated<BoardDto>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let _ = org; // read: any authenticated member (catalogue convention)
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = mcu_boards::Entity::find()
        .order_by_asc(mcu_boards::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mut boards = Vec::new();
    for b in rows {
        let Some(parsed) = b
            .details
            .as_ref()
            .and_then(|d| serde_json::from_value::<pnex_core::BoardDetails>(d.clone()).ok())
        else {
            continue;
        };
        let p = &parsed;
        let Some(soc) = pnex_core::Soc::from_board_soc(&b.soc) else {
            continue;
        };
        if let Some(f) = &q.soc {
            if pnex_core::Soc::from_board_soc(f) != Some(soc) {
                continue;
            }
        }
        let pins: Vec<BoardPinDto> = p.pins.iter().map(|pp| pin_dto(soc, pp)).collect();
        boards.push(BoardDto {
            id: b.id,
            name: b.name.clone(),
            soc: b.soc.clone(),
            pretty_name: b.pretty_name.clone().or_else(|| p.name.clone()),
            chip_label: p.chip_label.clone(),
            profile_id: p.board.clone(),
            pio_board: b.pio_board.clone(),
            layout: serde_json::to_value(p.layout).unwrap_or_default(),
            peripherals: serde_json::to_value(&p.peripherals).unwrap_or_default(),
            pins,
        });
    }
    let count = boards.len() as i64;
    let (skip, take) = page.slice(boards.len());
    let filters: Vec<(String, String)> = q
        .soc
        .iter()
        .map(|s| ("soc".to_string(), s.clone()))
        .collect();
    format::json(pagination::envelope(
        "/api/v1/boards",
        &filters,
        page,
        count,
        boards.into_iter().skip(skip).take(take).collect(),
    ))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
