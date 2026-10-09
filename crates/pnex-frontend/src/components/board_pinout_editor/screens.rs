//! Screen peripheral layer: resolved screen state kind, wiring legend rows,
//! picker options and the device-level screen setter (`PUT /peripherals`).

use super::*;

/// Option du picker écran (précalculée — pas de `let` dans les rsx).
pub(crate) struct ScreenOptionView {
    pub(crate) label: String,
    /// Kind du driver (`None` = aucun écran).
    pub(crate) kind: Option<String>,
    pub(crate) active: bool,
    /// Écran soudé (builtin) — verrouillé actif.
    pub(crate) locked: bool,
}

/// Resolved screen state of a board (kind or `None`).
pub(crate) fn screen_state_kind(board: &api::pins::BoardSummary) -> Option<String> {
    match board.peripherals_state.get("screen") {
        Some(serde_json::Value::String(k)) => Some(k.clone()),
        _ => None,
    }
}

/// Une ligne de la légende de câblage : rôle écran → broche de la carte.
#[derive(Clone, PartialEq)]
pub(crate) struct ScreenWiringRow {
    pub(crate) role: String,
    pub(crate) label: String,
}

/// Légende de câblage pour le kind d'écran donné, lue depuis le profil
/// board (rôles) + les labels du pinout (broche). Affichée au-dessus du
/// SVG et dans le modal build (pour le choix qui va être flashé).
pub(crate) fn screen_wiring(
    board: &api::pins::BoardSummary,
    pins: &[api::pins::PinoutPin],
    kind: Option<&str>,
) -> Option<(String, Vec<ScreenWiringRow>)> {
    let screens = board.peripherals.get("screens")?.as_array()?.clone();
    let kind = kind?;
    let screen = screens
        .iter()
        .find(|s| s.get("kind").and_then(|k| k.as_str()) == Some(kind))?;
    let label_by_gpio: std::collections::HashMap<u16, String> = pins
        .iter()
        .filter_map(|p| p.gpio.map(|g| (g as u16, p.label.clone())))
        .collect();
    const ROLE_ORDER: [&str; 7] = ["sda", "scl", "sck", "mosi", "cs", "dc", "rst"];
    let pins_map = screen.get("pins")?.as_object()?;
    let mut rows = Vec::new();
    for role in ROLE_ORDER {
        if let Some(g) = pins_map.get(role).and_then(|v| v.as_u64()) {
            rows.push(ScreenWiringRow {
                role: role.to_ascii_uppercase(),
                label: label_by_gpio
                    .get(&(g as u16))
                    .cloned()
                    .unwrap_or_else(|| format!("GPIO{g}")),
            });
        }
    }
    let name = screen
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(kind)
        .to_string();
    (!rows.is_empty()).then_some((name, rows))
}

/// Bandeau de câblage affiché au-dessus du SVG quand un écran est choisi.
#[component]
pub(crate) fn ScreenWiring(name: String, rows: Vec<ScreenWiringRow>) -> Element {
    rsx! {
        div { class: "mb-3 flex flex-wrap items-center gap-x-3 gap-y-1 rounded-lg border border-teal-200 bg-teal-50/60 px-3 py-2 text-xs text-teal-900",
            span { class: "font-semibold", {t!("board-screen-toggle")} }
            span { class: "font-semibold", "{name}" }
            span { class: "text-teal-700", "·" }
            for r in rows {
                span { class: "font-mono", "{r.role}→{r.label}" }
            }
        }
    }
}

/// Options du picker depuis le profil board + l'état résolu du device.
/// The server state (`peripherals_state.screen`) is a kind.
pub(crate) fn screen_options(board: &api::pins::BoardSummary) -> Vec<ScreenOptionView> {
    let screens = board
        .peripherals
        .get("screens")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let current: Option<String> = match board.peripherals_state.get("screen") {
        Some(serde_json::Value::String(k)) => Some(k.clone()),
        _ => None,
    };
    let mut options = vec![ScreenOptionView {
        label: t!("board-screen-none").to_string(),
        kind: None,
        active: current.is_none(),
        locked: false,
    }];
    for s in &screens {
        let kind = s
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        let name = s
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(&kind)
            .to_string();
        let builtin = s.get("builtin").is_some_and(|b| b.as_bool() == Some(true));
        options.push(ScreenOptionView {
            label: name,
            active: builtin || current.as_deref() == Some(kind.as_str()),
            kind: Some(kind),
            locked: builtin,
        });
    }
    options
}

/// Écran choisi (génériques à écran) — PUT /peripherals `{"screen": kind|null}`.
/// Choix écran d'un device — `PUT /peripherals` `{"screen": kind|null}`,
/// partagé avec le modal build (appliqué juste avant le build).
pub(crate) async fn set_screen(device_pk: i64, screen: Option<String>) -> Result<(), String> {
    let body = serde_json::json!({ "screen": screen });
    crate::api::client::request::<serde_json::Value>(
        reqwest::Method::PUT,
        &format!("/api/v1/devices/{device_pk}/peripherals"),
        Some(body),
    )
    .await
    .map(|_| ())
    .map_err(|e| e.message)
}
