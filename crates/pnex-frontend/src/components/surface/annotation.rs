//! Control and reading items of the annotations (D128/D129) rendered with
//! the dashboard cards: the popover of one item and the side panel of a
//! viewer share [`AnnotationSurface`].

use std::collections::HashMap;
use std::time::Duration;

use dioxus::prelude::*;
use pnex_core::{AnnotationTarget, ResolvedAnnotationItem, TelemetryPoint, Widget, WidgetOptions};

use super::control::ControlBody;
use super::use_surface_controls;
use crate::components::dashboard_widget::WidgetBody;
use crate::util::sleep;

/// Surface poll cadence (D31, same as the dashboards).
const POLL_SECS: u64 = 15;

/// Synthetic dashboard widget of a control / reading item (`None` for the
/// device, pin, status and note kinds).
pub fn item_widget(item: &ResolvedAnnotationItem) -> Option<Widget> {
    let mut w = Widget {
        id: item.id.clone(),
        widget_type: String::new(),
        title: item.label.clone(),
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        source: vec![],
        options: WidgetOptions::default(),
    };
    match &item.target {
        AnnotationTarget::Control { control_id, kind } => {
            // Placeholder type: the card follows the control kind. A nil id
            // is an own control not provisioned yet (unsaved layer).
            w.widget_type = kind.map_or("switch", |k| k.widget_type()).into();
            w.options.control =
                (!control_id.is_nil()).then_some(pnex_core::ui_control::ControlRef {
                    control_id: *control_id,
                });
        }
        AnnotationTarget::Reading {
            source,
            spark,
            display,
            min,
            max,
        } => {
            w.widget_type = pnex_core::reading_display(display.as_deref(), *spark).into();
            w.source = vec![source.clone()];
            w.options.min = *min;
            w.options.max = *max;
        }
        _ => return None,
    }
    Some(w)
}

/// `true` for the kinds rendered as surface cards.
pub fn is_surface_item(item: &ResolvedAnnotationItem) -> bool {
    matches!(
        item.target,
        AnnotationTarget::Control { .. } | AnnotationTarget::Reading { .. }
    )
}

type LiveValues = HashMap<String, Option<Vec<TelemetryPoint>>>;

/// Live data of the control / reading items: their own control context and
/// a self-rearming 15 s poll. The item set is captured at mount: key the
/// calling component by the item ids.
fn use_surface_live(
    items: &[ResolvedAnnotationItem],
) -> (Vec<(ResolvedAnnotationItem, Widget)>, LiveValues) {
    let mut reload = use_signal(|| 0u32);
    let mut polling = use_signal(|| false);
    let widgets: Vec<(ResolvedAnnotationItem, Widget)> = items
        .iter()
        .filter_map(|i| item_widget(i).map(|w| (i.clone(), w)))
        .collect();
    let ids: Vec<uuid::Uuid> = widgets
        .iter()
        .filter_map(|(_, w)| w.options.control.as_ref().map(|c| c.control_id))
        .collect();
    let sources: Vec<pnex_core::SourceRef> = widgets
        .iter()
        .flat_map(|(_, w)| w.source.iter().cloned())
        .collect();
    let via = items
        .first()
        .map(|i| format!("annotation:{}", i.layer_id))
        .unwrap_or_default();
    use_surface_controls(
        move || ids.clone(),
        reload,
        via,
        crate::state::org::current_can_write(),
    );
    let batch = use_resource(move || {
        let sources = sources.clone();
        let _ = reload();
        async move {
            if sources.is_empty() {
                return HashMap::new();
            }
            crate::components::dashboard_live::fetch_live_values(sources).await
        }
    });
    let values: LiveValues = batch.read().clone().unwrap_or_default();
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }
    (widgets, values)
}

/// Cards of the control / reading items, stacked (marker popover). Key the
/// component by the item ids (see [`use_surface_live`]).
#[component]
pub fn AnnotationSurface(items: Vec<ResolvedAnnotationItem>) -> Element {
    let (widgets, values) = use_surface_live(&items);
    rsx! {
        div { class: "space-y-2",
            for (item, w) in widgets {
                SurfaceCard {
                    key: "{item.id}",
                    via: format!("annotation:{}", item.layer_id),
                    w,
                    values: values.clone(),
                }
            }
        }
    }
}

#[component]
fn SurfaceCard(
    w: Widget,
    via: String,
    values: HashMap<String, Option<Vec<TelemetryPoint>>>,
) -> Element {
    let points = w
        .source
        .first()
        .and_then(|s| values.get(&s.series_key()))
        .cloned()
        .flatten();
    let height = match w.widget_type.as_str() {
        "line" => "h-32",
        "stat" | "indicator" => "h-24",
        _ => "h-32",
    };
    let is_control = w.options.control.is_some();
    rsx! {
        div { class: "{height} overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm",
            if is_control {
                ControlBody { widget: w.clone(), state: None, via: Some(via.clone()) }
            } else {
                WidgetBody {
                    widget: w.clone(),
                    points,
                    values: Some(values.clone()),
                }
            }
        }
    }
}

/// How a viewer shows the annotations of its media (D147).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnnotMode {
    Hidden,
    /// Markers only; a click opens the popover.
    Dots,
    /// Markers + the control / reading cards anchored next to them.
    Cards,
}

/// Effective mode: the viewer's choice, else cards when the media carries
/// control / reading items, dots otherwise.
pub fn effective_mode(choice: Option<AnnotMode>, has_cards: bool) -> AnnotMode {
    choice.unwrap_or(if has_cards {
        AnnotMode::Cards
    } else {
        AnnotMode::Dots
    })
}

/// Three-state switch of a viewer (hidden / dots / cards), top-left over
/// the media. The cards state is offered only when there are cards.
#[component]
pub fn AnnotModeToggle(
    mode: AnnotMode,
    has_cards: bool,
    on_change: EventHandler<AnnotMode>,
) -> Element {
    let mut options = vec![
        (AnnotMode::Hidden, "annot-mode-hidden"),
        (AnnotMode::Dots, "annot-mode-dots"),
    ];
    if has_cards {
        options.push((AnnotMode::Cards, "annot-mode-cards"));
    }
    rsx! {
        div {
            // Right of the pannellum zoom / fullscreen controls (top-left).
            class: "absolute top-2 left-12 z-30 inline-flex rounded-lg border border-gray-200 bg-white/90 shadow-sm overflow-hidden",
            role: "group",
            aria_label: dioxus_i18n::t!("annot-toggle"),
            for (m, key) in options {
                button {
                    key: "{key}",
                    class: if m == mode { "px-2.5 py-1.5 text-xs font-medium bg-blue-600 text-white" } else { "px-2.5 py-1.5 text-xs font-medium text-gray-700 hover:bg-white" },
                    aria_pressed: if m == mode { "true" } else { "false" },
                    onclick: move |_| on_change.call(m),
                    {dioxus_i18n::t!(key)}
                }
            }
        }
    }
}

/// Overlay of the control / reading cards over a viewer — a SIBLING of the
/// viewer host (an unmount wipes the host's children). Panorama and splat
/// cards are positioned by the JS glue (`follow_cards`, it owns their
/// `transform`/`visibility`); flat images pass `flat` positions (fractions
/// of the image box) and are placed in CSS. Key by the item ids.
#[component]
pub fn AnnotationCardsOverlay(
    items: Vec<ResolvedAnnotationItem>,
    overlay_id: String,
    #[props(default)] flat: bool,
) -> Element {
    let (widgets, values) = use_surface_live(&items);
    rsx! {
        div {
            id: "{overlay_id}",
            // Flat cards may stand above the image top edge: no clipping.
            class: if flat { "absolute inset-0 z-20 pointer-events-none" } else { "absolute inset-0 z-20 pointer-events-none overflow-hidden" },
            for (item, w) in widgets {
                if flat {
                    if let pnex_core::AnnotationGeometry::Flat { x, y } = item.geometry {
                        div {
                            key: "{item.id}",
                            "data-annot-card": "{item.id}",
                            "data-pos": "1",
                            class: "pnex-annot-card absolute w-52 pointer-events-auto",
                            style: "left: calc({x} * 100%); top: calc({y} * 100%); transform: translate(-50%, calc(-100% - 14px));",
                            SurfaceCard {
                                via: format!("annotation:{}", item.layer_id),
                                w,
                                values: values.clone(),
                            }
                        }
                    }
                } else {
                    div {
                        key: "{item.id}",
                        "data-annot-card": "{item.id}",
                        class: "pnex-annot-card absolute left-0 top-0 w-52 pointer-events-auto",
                        SurfaceCard {
                            via: format!("annotation:{}", item.layer_id),
                            w,
                            values: values.clone(),
                        }
                    }
                }
            }
        }
    }
}
