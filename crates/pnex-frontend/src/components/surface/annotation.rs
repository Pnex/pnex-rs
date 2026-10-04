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
        AnnotationTarget::Reading { source, spark } => {
            w.widget_type = if *spark { "line" } else { "stat" }.into();
            w.source = vec![source.clone()];
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

/// Cards of the control / reading items, with their own control context
/// and a self-rearming 15 s poll. Key the component by the item ids: the
/// polled set is captured at mount.
#[component]
pub fn AnnotationSurface(items: Vec<ResolvedAnnotationItem>) -> Element {
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
    let values: HashMap<String, Option<Vec<TelemetryPoint>>> =
        batch.read().clone().unwrap_or_default();
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }

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
        "stat" => "h-24",
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
