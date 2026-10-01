//! Debug screen choice at the wizard: precomputed options, the option
//! block with its wiring legend, and the catalog-based wiring resolver.

use super::*;

/// Option du picker écran du wizard (précalculée).
struct WizardScreenOption {
    label: String,
    kind: Option<String>,
    active: bool,
    locked: bool,
}

/// Choix de l'écran de debug au wizard — visible dès la step variante
/// (câblage fixé par la board, appliqué au device juste avant le 1er
/// build). « Aucun » verrouillé si la board porte un écran soudé.
#[component]
pub(super) fn ScreenPickBlock(
    board: api::boards::Board,
    mut screen_pick: Signal<Option<Option<String>>>,
) -> Element {
    let screens = board.screens();
    let pick = screen_pick();
    let builtin = screens
        .iter()
        .any(|s| s.get("builtin").and_then(|b| b.as_bool()) == Some(true));
    let mut options: Vec<WizardScreenOption> = vec![WizardScreenOption {
        label: t!("board-screen-none").to_string(),
        kind: None,
        // "No screen" stays lit while it is the effective choice: either the
        // user explicitly picked it (Some(None)) or nothing is picked yet and
        // the board carries no soldered screen. A locked builtin already
        // shows as active on its own, so both never light up together.
        active: matches!(pick, Some(None)) || (pick.is_none() && !builtin),
        locked: builtin,
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
        let is_builtin = s.get("builtin").and_then(|b| b.as_bool()) == Some(true);
        options.push(WizardScreenOption {
            label: name,
            active: is_builtin || pick.as_ref() == Some(&Some(kind.clone())),
            kind: Some(kind),
            locked: is_builtin,
        });
    }
    // Câblage du choix effectif (soudé forcé si rien de choisi).
    let effective_kind: Option<String> = match &pick {
        Some(k) => k.clone(),
        None => screens
            .iter()
            .find(|s| s.get("builtin").and_then(|b| b.as_bool()) == Some(true))
            .and_then(|s| s.get("kind"))
            .and_then(|k| k.as_str())
            .map(String::from),
    };
    let wiring = board_screen_wiring(&board, effective_kind.as_deref());

    rsx! {
        div { class: "mt-3 space-y-1",
            span { class: "text-xs text-gray-400", {t!("board-screen-toggle")} }
            div { class: "flex flex-wrap gap-1",
                for opt in options {
                    button {
                        key: "{opt.label}",
                        class: if opt.active {
                            "inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium bg-teal-600 text-white hover:bg-teal-700 disabled:opacity-50"
                        } else {
                            "inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium border border-teal-300 text-teal-700 hover:bg-teal-50 disabled:opacity-50"
                        },
                        disabled: opt.locked,
                        title: if opt.locked {
                            t!("board-screen-locked").to_string()
                        } else {
                            String::new()
                        },
                        onclick: move |_| screen_pick.set(Some(opt.kind.clone())),
                        {opt.label.clone()}
                    }
                }
            }
            if let Some((name, rows)) = wiring {
                div { class: "flex flex-wrap items-center gap-x-3 gap-y-1 rounded-lg border border-teal-200 bg-teal-50/60 px-3 py-2 text-xs text-teal-900",
                    span { class: "font-semibold", {name} }
                    for r in rows {
                        span { class: "font-mono", "{r.0}→{r.1}" }
                    }
                }
            }
        }
    }
}

/// Légende de câblage (rôle → label de broche) depuis le Board du
/// catalogue (le wizard n'a pas le pinout, les labels du profil suffisent).
fn board_screen_wiring(
    board: &api::boards::Board,
    kind: Option<&str>,
) -> Option<(String, Vec<(String, String)>)> {
    let kind = kind?;
    let screen = board
        .screens()
        .into_iter()
        .find(|s| s.get("kind").and_then(|k| k.as_str()) == Some(kind))?;
    let label_by_gpio: std::collections::HashMap<u16, String> = board
        .pins
        .iter()
        .filter_map(|p| p.gpio.map(|g| (g as u16, p.label.clone())))
        .collect();
    const ROLES: [&str; 7] = ["sda", "scl", "sck", "mosi", "cs", "dc", "rst"];
    let map = screen.get("pins")?.as_object()?;
    let mut rows: Vec<(String, String)> = Vec::new();
    for role in ROLES {
        if let Some(g) = map.get(role).and_then(|v| v.as_u64()) {
            rows.push((
                role.to_ascii_uppercase(),
                label_by_gpio
                    .get(&(g as u16))
                    .cloned()
                    .unwrap_or_else(|| format!("GPIO{g}")),
            ));
        }
    }
    let name = screen
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(kind)
        .to_string();
    (!rows.is_empty()).then_some((name, rows))
}
