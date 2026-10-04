//! Bibliothèque de widgets (palette à la demande, D41) — templates de
//! l'org. **Armer-glisser** : pointerdown arme le modèle, le pointerup sur
//! le canvas pose un **snapshot** (D41 : éditer le modèle n'affecte jamais
//! les instances). Les « + Nouveau » du panneau historique sont devenus les
//! items de la palette coquille (clic = widget vide du type, source à
//! compléter dans l'inspecteur) ; ce fichier porte le pied de popover
//! (modèles serveur) et le registre types↔icônes.

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::editor_shell::{PaletteIcon, PaletteItem};
use crate::components::icons;
use crate::components::surface::home_card::card_label;
use crate::state::toasts;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::home::HomeCard;

use super::state;
use super::EditorCx;

/// Palette key opening the symbol library (not a widget kind: a symbol
/// needs a shape, picked in the library panel).
pub const SYMBOLS_KEY: &str = "symbol";

/// Palette key opening the guided "from a device" panel (parcours §1.2).
pub const FROM_DEVICE_KEY: &str = "from_device";

pub fn kind_label(kind: &str) -> String {
    // t! needs literal keys: explicit match (PALETTE_GROUPS lists the
    // palette entries).
    match kind {
        "gauge" => t!("lib-kind-gauge").to_string(),
        "stat" => t!("lib-kind-stat").to_string(),
        "line" => t!("lib-kind-line").to_string(),
        "indicator" => t!("lib-kind-indicator").to_string(),
        "text" => t!("lib-kind-text").to_string(),
        "thermo_chart" => t!("lib-kind-thermo_chart").to_string(),
        "symbol" => t!("lib-kind-symbol").to_string(),
        "switch" => t!("lib-kind-switch").to_string(),
        "slider" => t!("lib-kind-slider").to_string(),
        "button" => t!("lib-kind-button").to_string(),
        "number" => t!("lib-kind-number").to_string(),
        "select" => t!("lib-kind-select").to_string(),
        "stepper" => t!("lib-kind-stepper").to_string(),
        "command" => t!("lib-kind-command").to_string(),
        "color" => t!("lib-kind-color").to_string(),
        other => other.to_string(),
    }
}

/// Icône + tuile d'un type de widget (palette + en-tête d'inspecteur).
pub fn kind_icon(kind: &str) -> (PaletteIcon, &'static str) {
    match kind {
        "gauge" => (PaletteIcon::Gauge, "bg-blue-50 text-blue-600"),
        "stat" => (PaletteIcon::Hash, "bg-blue-50 text-blue-600"),
        "line" => (PaletteIcon::LineChart, "bg-green-50 text-green-600"),
        "indicator" => (PaletteIcon::CheckCircle, "bg-emerald-50 text-emerald-600"),
        "text" => (PaletteIcon::Info, "bg-gray-100 text-gray-600"),
        "thermo_chart" => (PaletteIcon::Thermometer, "bg-amber-50 text-amber-600"),
        "symbol" => (PaletteIcon::Shapes, "bg-violet-50 text-violet-600"),
        "switch" => (PaletteIcon::ToggleRight, "bg-teal-50 text-teal-600"),
        "slider" => (PaletteIcon::SlidersHorizontal, "bg-teal-50 text-teal-600"),
        "button" => (PaletteIcon::Pointer, "bg-teal-50 text-teal-600"),
        "number" => (PaletteIcon::Hash, "bg-teal-50 text-teal-600"),
        "select" => (PaletteIcon::Layers, "bg-teal-50 text-teal-600"),
        "stepper" => (PaletteIcon::Thermometer, "bg-teal-50 text-teal-600"),
        "command" => (PaletteIcon::Pointer, "bg-teal-50 text-teal-600"),
        "color" => (PaletteIcon::Image, "bg-teal-50 text-teal-600"),
        _ => (PaletteIcon::Puzzle, "bg-gray-100 text-gray-600"),
    }
}

/// Palette key prefix of a home card (`home:thermostat`, D138).
pub const HOME_KEY_PREFIX: &str = "home:";

/// Home card palette groups (D134, D138), after "Start".
const HOME_GROUPS: [(&str, &[HomeCard]); 3] = [
    (
        "home",
        &[
            HomeCard::Light,
            HomeCard::Thermostat,
            HomeCard::Cover,
            HomeCard::Fan,
            HomeCard::Gate,
            HomeCard::Lock,
            HomeCard::Alarm,
            HomeCard::Scene,
            HomeCard::Irrigation,
            HomeCard::Appliance,
        ],
    ),
    (
        "sensors",
        &[
            HomeCard::Binary,
            HomeCard::ThermoHygro,
            HomeCard::AirQuality,
            HomeCard::Weather,
            HomeCard::Clock,
        ],
    ),
    (
        "energy",
        &[HomeCard::Power, HomeCard::Meter, HomeCard::EnergyFlow],
    ),
];

/// Palette groups in display order (D134). Each widget kind belongs to
/// exactly one group; the shell starts a section whenever it changes.
const PALETTE_GROUPS: [(&str, &[&str]); 4] = [
    (
        "controls",
        &[
            "switch", "slider", "button", "number", "select", "stepper", "command", "color",
        ],
    ),
    ("display", &["stat", "gauge", "indicator", "text"]),
    ("charts", &["line"]),
    ("industrial", &["thermo_chart"]),
];

fn group_label(group: &str) -> String {
    match group {
        "start" => t!("lib-group-start").to_string(),
        "home" => t!("lib-group-home").to_string(),
        "sensors" => t!("lib-group-sensors").to_string(),
        "energy" => t!("lib-group-energy").to_string(),
        "controls" => t!("lib-group-controls").to_string(),
        "display" => t!("lib-group-display").to_string(),
        "charts" => t!("lib-group-charts").to_string(),
        "industrial" => t!("lib-group-industrial").to_string(),
        other => other.to_string(),
    }
}

fn kind_item(kind: &str, group: &str) -> PaletteItem {
    let (icon, tile) = kind_icon(kind);
    let item = PaletteItem::new(kind.to_string(), kind_label(kind))
        .with_icon(icon, tile)
        .with_group(group_label(group));
    // Control cards act through a flow (D128): say so in the palette.
    if pnex_core::CONTROL_WIDGET_TYPES.contains(&kind) {
        item.with_description(t!("lib-control-desc").to_string())
    } else {
        item
    }
}

/// Items de la palette coquille — un par type de widget (clic = ajout),
/// grouped by category (D134).
pub fn palette_items() -> Vec<PaletteItem> {
    let mut items = vec![
        PaletteItem::new(FROM_DEVICE_KEY, t!("db-from-device").to_string())
            .with_description(t!("db-from-device-desc").to_string())
            .with_icon(PaletteIcon::Cpu, "bg-teal-50 text-teal-700")
            .with_group(group_label("start")),
    ];
    for (group, cards) in HOME_GROUPS {
        items.extend(cards.iter().map(|card| {
            PaletteItem::new(
                format!("{HOME_KEY_PREFIX}{}", card.as_str()),
                card_label(*card),
            )
            .with_icon(
                PaletteIcon::Home(card.default_icon(None)),
                "bg-amber-50 text-amber-700",
            )
            .with_group(group_label(group))
        }));
    }
    for (group, kinds) in PALETTE_GROUPS {
        items.extend(kinds.iter().map(|k| kind_item(k, group)));
    }
    let (icon, tile) = kind_icon(SYMBOLS_KEY);
    items.push(
        PaletteItem::new(SYMBOLS_KEY, t!("sym-library").to_string())
            .with_description(t!("sym-library-desc").to_string())
            .with_icon(icon, tile)
            .with_group(group_label("industrial")),
    );
    items
}

/// Ajout d'un widget vide du type — ex-boutons « + Nouveau » de la
/// bibliothèque, désormais déclenchés par le pick de la palette.
pub fn add_new_widget(mut cx: EditorCx, kind: &str) {
    if let Some(card) = kind.strip_prefix(HOME_KEY_PREFIX).and_then(HomeCard::parse) {
        add_home_card(cx, card);
        return;
    }
    let current = cx.layout.read().clone();
    cx.history.with_mut(|h| h.push(&current));
    cx.counter.with_mut(|c| *c += 1);
    let id = state::next_id("w", cx.counter.cloned());
    let x = 80 + (cx.counter.cloned() % 8) as i64 * 24;
    let y = 80 + (cx.counter.cloned() % 8) as i64 * 24;
    let section = cx.section.cloned();
    cx.layout.with_mut(|l| {
        state::new_widget(l, id.clone(), kind, x, y);
        if l.format == pnex_core::DashboardFormat::Mobile {
            state::place_in_section(l, &id, section.as_deref());
            // Compact cards side by side, ESPHome style; charts take the row.
            if matches!(kind, "switch" | "button" | "stat" | "indicator" | "color") {
                state::set_span(l, &id, 1);
            }
        }
    });
    cx.selected.set(Some(super::Selection::Widget(id)));
}

/// Adds a home card (D138): required roles declared at their default
/// domain, required sources to pick in the inspector.
pub fn add_home_card(mut cx: EditorCx, card: HomeCard) {
    let current = cx.layout.read().clone();
    cx.history.with_mut(|h| h.push(&current));
    cx.counter.with_mut(|c| *c += 1);
    let id = state::next_id("w", cx.counter.cloned());
    let x = 80 + (cx.counter.cloned() % 8) as i64 * 24;
    let y = 80 + (cx.counter.cloned() % 8) as i64 * 24;
    let section = cx.section.cloned();
    cx.layout.with_mut(|l| {
        state::new_home_card(l, id.clone(), card, x, y);
        if l.format == pnex_core::DashboardFormat::Mobile {
            state::place_in_section(l, &id, section.as_deref());
            state::set_span(l, &id, card.default_span());
        }
    });
    cx.selected.set(Some(super::Selection::Widget(id)));
}

/// Adds a widget of `kind` like a palette pick, then lets `f` complete it
/// (control, source…) in the same undo step. Returns the widget id.
pub fn add_configured(
    mut cx: EditorCx,
    kind: &str,
    f: impl FnOnce(&mut pnex_core::Widget),
) -> String {
    add_new_widget(cx, kind);
    let id = state::next_id("w", cx.counter.cloned());
    cx.layout.with_mut(|l| {
        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
            f(w);
        }
    });
    id
}

/// Adds a symbol widget of the catalog shape (library panel pick).
pub fn add_symbol(mut cx: EditorCx, shape: &str) {
    let current = cx.layout.read().clone();
    cx.history.with_mut(|h| h.push(&current));
    cx.counter.with_mut(|c| *c += 1);
    let n = cx.counter.cloned() as i64;
    let id = state::next_id("w", n as u32);
    // Symbols are small: spread them on a 90 px grid instead of the
    // 24 px cascade of the card widgets (which would stack them).
    let x = 80 + (n % 8) * 90;
    let y = 80 + (n / 8 % 6) * 90;
    let section = cx.section.cloned();
    cx.layout.with_mut(|l| {
        state::new_symbol(l, id.clone(), shape, x, y);
        if l.format == pnex_core::DashboardFormat::Mobile {
            state::place_in_section(l, &id, section.as_deref());
            state::set_span(l, &id, 1);
        }
    });
    cx.selected.set(Some(super::Selection::Widget(id)));
}

/// Une ligne de la bibliothèque : drag (armer le modèle) + suppression
/// confirmée. Deux closures `move` par ligne → copie d'id par closure.
fn library_rows(
    loaded: &[pnex_core::VizWidget],
    mut cx: EditorCx,
    mut deleting: Signal<Option<String>>,
) -> Vec<Element> {
    loaded
        .iter()
        .map(|w| {
            let widget = w.clone();
            let wid_delete = w.id.clone();
            let caption = kind_label(&w.kind);
            rsx! {
                div {
                    key: "{widget.id}",
                    class: "group flex items-center justify-between px-2 py-1.5 rounded border border-gray-100 hover:border-blue-200 hover:bg-blue-50 cursor-grab select-none",
                    onpointerdown: move |_| {
                        // Arme le drag : la pose se fait au pointerup
                        // sur le canvas (snapshot au drop, D41).
                        cx.drag_template.set(Some(widget.clone()));
                    },
                    div { class: "min-w-0",
                        p { class: "truncate text-xs font-medium text-gray-800", "{widget.name}" }
                        p { class: "text-[10px] text-gray-400", "{caption}" }
                    }
                    button {
                        class: "shrink-0 text-gray-300 hover:text-red-500 opacity-0 group-hover:opacity-100",
                        onclick: move |event| {
                            event.stop_propagation();
                            deleting.set(Some(wid_delete.clone()));
                        },
                        icons::Trash { class: "h-3.5 w-3.5" }
                    }
                }
            }
        })
        .collect()
}

#[component]
pub fn TemplateList(mut cx: EditorCx) -> Element {
    let reload = cx.palette_reload;
    let widgets = use_resource(move || async move {
        let _ = reload();
        api::dashboards::widgets(&crate::api::dashboards::WidgetLibraryFilters {
            search: None,
            kind: None,
        })
        .await
        .ok()
        .map(|p| p.results)
    });
    let loaded = widgets
        .read()
        .as_ref()
        .cloned()
        .flatten()
        .unwrap_or_default();
    let mut deleting: Signal<Option<String>> = use_signal(|| None);

    rsx! {
        div { class: "space-y-1",
            if loaded.is_empty() {
                p { class: "p-2 text-xs text-gray-400", {t!("lib-empty")} }
            }
            // Lignes précalculées en Vec<Element> : chaque ligne a DEUX
            // closures `move` qui consomment des copies différentes de
            // l'id — impossible dans une itération rsx directe.
            for row in library_rows(&loaded, cx, deleting) {
                {row}
            }
        }
        if let Some(id) = deleting.cloned() {
            ConfirmDialog {
                title: t!("lib-title").to_string(),
                message: t!("lib-delete-confirm").to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_confirm: move |_| {
                    let id = id.clone();
                    spawn(async move {
                        match api::dashboards::delete_widget(&id).await {
                            Ok(()) => {
                                toasts::success(t!("lib-deleted").to_string());
                                cx.palette_reload.with_mut(|r| *r += 1);
                            }
                            Err(e) => toasts::error(e),
                        }
                        deleting.set(None);
                    });
                },
                on_cancel: move |_| deleting.set(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_widget_kind_has_one_palette_group() {
        for kind in pnex_core::VIZ_WIDGET_TYPES {
            let n = PALETTE_GROUPS
                .iter()
                .filter(|(_, kinds)| kinds.contains(kind))
                .count();
            // `symbol` and `home_card` are reached through their own entries.
            let expected = usize::from(*kind != SYMBOLS_KEY && *kind != "home_card");
            assert_eq!(n, expected, "{kind}");
        }
    }

    #[test]
    fn every_home_card_is_in_one_palette_group() {
        for card in HomeCard::ALL {
            let n = HOME_GROUPS
                .iter()
                .filter(|(_, cards)| cards.contains(&card))
                .count();
            assert_eq!(n, 1, "{card:?}");
        }
    }
}
