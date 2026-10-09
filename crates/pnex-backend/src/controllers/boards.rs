//! Board profiles — catalogue des profils de board v2 (`mcu_boards.details`,
//! schéma v2) pour le picker de variantes du wizard et l'éditeur SVG.
//!
//! `GET /api/v1/boards` — lecture pour tout membre d'org. Les fonctions de
//! mux, flags, modes offerts et avertissements sont **calculés serveur**
//! (chip-caps en code, B0.6) : le front ne réimplémente jamais la validation.

use axum::extract::{Query, State};
use axum::response::Response;
use loco_rs::prelude::*;
use sea_orm::{EntityTrait, QueryOrder};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::models::_entities::mcu_boards;

pub fn routes() -> Routes {
    Routes::new().prefix("/api/v1").add("/boards", get(list))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    soc: Option<String>,
}

/// Un pin d'un profil board — enrichissement affichage calculé serveur.
#[derive(serde::Serialize)]
struct BoardPinDto {
    label: String,
    gpio: Option<u16>,
    kind: &'static str,
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

fn kind_str(k: pnex_core::BoardPinKind) -> &'static str {
    match k {
        pnex_core::BoardPinKind::Gpio => "gpio",
        pnex_core::BoardPinKind::Power => "power",
        pnex_core::BoardPinKind::Gnd => "gnd",
        pnex_core::BoardPinKind::En => "en",
    }
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
        kind: kind_str(pp.kind),
        pos: pp.pos,
        note: pp.note.clone(),
        fns,
        flags,
        available_modes,
        warnings,
    }
}

/// `GET /api/v1/boards` — boards dont `details` parse en v2, enrichies
/// chip-caps (fn/flags/modes/warnings dérivés côté serveur). `?soc=esp32`
/// filtre par famille (picker de variantes du wizard).
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let _ = org; // lecture : tout membre authentifié (convention catalogue)
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
        boards.push(serde_json::json!({
            "id": b.id,
            "name": b.name.clone(),
            "soc": b.soc.clone(),
            "pretty_name": b.pretty_name.clone().or_else(|| p.name.clone()),
            "chip_label": p.chip_label.clone(),
            "profile_id": p.board.clone(),
            "pio_board": b.pio_board.clone(),
            "layout": serde_json::to_value(p.layout).unwrap_or_default(),
            "peripherals": serde_json::to_value(&p.peripherals).unwrap_or_default(),
            "pins": pins,
        }));
    }
    format::json(serde_json::json!({ "boards": boards }))
}
