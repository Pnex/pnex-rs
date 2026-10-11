//! Case at an instant (cases.md K14): a cursor stepping through the
//! timeline instants replays the members present then, the links valid
//! then among them and where each member stood, as a graph and a map side
//! by side. Clicking a graph node focuses its timeline lane.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CasePlace, LinkTypeView};

use crate::api;
use crate::components::ontology::graph_view::GraphView;
use crate::components::ontology::short_time;
use crate::components::ui::BTN_SECONDARY_SM;
use crate::map_viewer::{self, MapItem, MapOpts};

const MAP_HOST: &str = "pnex-case-map";

#[component]
pub fn SnapshotPanel(
    case_id: String,
    /// Timeline instants, oldest first, deduplicated.
    steps: Vec<String>,
    /// `None` = now.
    cursor: Signal<Option<String>>,
    focus: Signal<Option<String>>,
    link_types: Vec<LinkTypeView>,
    reload: Signal<u32>,
) -> Element {
    let mut cursor = cursor;
    let mut focus = focus;
    let snap = use_resource(move || {
        let id = case_id.clone();
        let at = cursor();
        let _ = reload();
        async move { api::ontology::case_snapshot(&id, at.as_deref()).await }
    });
    // Position on the steps; one past the last step is "now".
    let idx = cursor()
        .and_then(|c| steps.iter().position(|s| *s == c))
        .unwrap_or(steps.len());
    let last = steps.len();
    let step_to = {
        let steps = steps.clone();
        move |i: usize| steps.get(i).cloned()
    };
    let (prev, next, slide) = (step_to.clone(), step_to.clone(), step_to);
    let label = match cursor() {
        Some(at) => format!("{} UTC", short_time(&at)),
        None => t!("case-snapshot-now").to_string(),
    };
    let data = snap.read().as_ref().and_then(|r| r.as_ref().ok()).cloned();

    rsx! {
        div {
            class: "bg-white rounded-lg border border-gray-200 p-4 space-y-3",
            "data-testid": "case-snapshot",
            div { class: "flex flex-wrap items-center gap-2",
                h2 { class: "font-semibold text-gray-900 mr-auto", {t!("case-snapshot")} }
                span { class: "text-sm font-medium text-gray-900 tabular-nums", "{label}" }
                if cursor().is_some() {
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| cursor.set(None),
                        {t!("case-snapshot-now")}
                    }
                }
            }
            div { class: "flex items-center gap-2",
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    disabled: idx == 0,
                    "aria-label": t!("case-snapshot-prev"),
                    title: t!("case-snapshot-prev"),
                    onclick: move |_| cursor.set(prev(idx.saturating_sub(1))),
                    "‹"
                }
                input {
                    r#type: "range",
                    class: "flex-1 accent-signal",
                    min: "0",
                    max: "{last}",
                    step: "1",
                    value: "{idx}",
                    "aria-label": t!("case-snapshot"),
                    "aria-valuetext": "{label}",
                    oninput: move |e| {
                        let i = e.value().parse().unwrap_or(last);
                        cursor.set(slide(i));
                    },
                }
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    disabled: idx >= last,
                    "aria-label": t!("case-snapshot-next"),
                    title: t!("case-snapshot-next"),
                    onclick: move |_| cursor.set(next(idx + 1)),
                    "›"
                }
            }
            p { class: "text-xs text-gray-500", {t!("case-snapshot-help")} }
            match data {
                None => rsx! {
                    p { class: "text-sm text-gray-500", {t!("common-loading")} }
                },
                Some(s) if s.graph.nodes.is_empty() => rsx! {
                    p { class: "text-sm text-gray-500 py-4 text-center", {t!("case-snapshot-empty")} }
                },
                Some(s) => rsx! {
                    div { class: "grid gap-3 xl:grid-cols-2",
                        GraphView {
                            neighborhood: s.graph.clone(),
                            center: focus().unwrap_or_default(),
                            link_types: link_types.clone(),
                            on_select: move |id: String| {
                                let next = if focus().as_deref() == Some(id.as_str()) { None } else { Some(id) };
                                focus.set(next);
                            },
                        }
                        if s.places.is_empty() {
                            p { class: "text-sm text-gray-500 rounded-lg border border-dashed border-gray-300 p-4",
                                {t!("case-snapshot-no-places")}
                            }
                        } else {
                            CaseMap { places: s.places.clone() }
                        }
                    }
                },
            }
        }
    }
}

fn item(p: &CasePlace) -> MapItem {
    let label = match &p.via {
        Some(v) => format!("{} ({})", p.object.title, v.title),
        None => p.object.title.clone(),
    };
    MapItem {
        kind: "pin",
        id: Some(p.object.id.clone()),
        lon: p.lon,
        lat: p.lat,
        label,
        emoji: "📍".into(),
        count: 0,
    }
}

/// Centre and zoom that frame every place (rough: from the widest span).
fn frame(places: &[CasePlace]) -> ((f64, f64), f64) {
    let (mut w, mut s, mut e, mut n) = (180.0f64, 90.0f64, -180.0f64, -90.0f64);
    for p in places {
        (w, e) = (w.min(p.lon), e.max(p.lon));
        (s, n) = (s.min(p.lat), n.max(p.lat));
    }
    let span = (e - w).max(n - s).max(0.01);
    (
        ((w + e) / 2.0, (s + n) / 2.0),
        (8.0 - span.log2()).clamp(2.0, 14.0),
    )
}

/// Places on the org basemap, framed once when the map mounts; later
/// snapshots replace the markers and keep the user's view.
#[component]
fn CaseMap(places: Vec<CasePlace>) -> Element {
    let mut mounted = use_signal(|| false);
    let mut failed = use_signal(|| false);
    // Markers of the latest snapshot: the mount task shows them once the
    // map exists, whatever arrived while it was loading.
    let mut latest = use_signal(Vec::<MapItem>::new);
    use_effect(use_reactive!(|places| {
        latest.set(places.iter().map(item).collect());
        if mounted() {
            spawn(async move {
                map_viewer::set_items(MAP_HOST, &latest.peek().clone()).await;
            });
            return;
        }
        mounted.set(true);
        let (center, zoom) = frame(&places);
        spawn(async move {
            let maps = api::geo::basemaps().await.unwrap_or_default();
            let style_url = crate::pages::map::pick_basemap(&maps)
                .map(|b| b.style_url.clone())
                .unwrap_or_default();
            let opts = MapOpts {
                style_url,
                center,
                zoom,
                items: Vec::new(),
            };
            let ok = map_viewer::mount(MAP_HOST, &opts).await;
            failed.set(!ok);
            if ok {
                map_viewer::set_items(MAP_HOST, &latest.peek().clone()).await;
            }
        });
    }));
    use_drop(|| map_viewer::unmount(MAP_HOST));
    rsx! {
        div { class: "relative",
            div {
                id: MAP_HOST,
                class: "h-80 w-full rounded-lg border border-gray-200 overflow-hidden",
                "data-testid": "case-map",
            }
            if failed() {
                p { class: "absolute inset-x-2 top-2 rounded-md bg-white/90 px-2 py-1 text-xs text-gray-700",
                    {t!("viz-map-unavailable")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::api::ObjectRef;

    fn at(lat: f64, lon: f64) -> CasePlace {
        CasePlace {
            object: ObjectRef {
                id: "a".into(),
                type_key: "person".into(),
                title: "Ann".into(),
                native_id: "a".into(),
            },
            lat,
            lon,
            via: None,
            link_type: String::new(),
        }
    }

    #[test]
    fn frames_every_place() {
        let ((lon, lat), zoom) = frame(&[at(43.0, 5.0), at(49.0, 2.0)]);
        assert_eq!((lon, lat), (3.5, 46.0));
        assert!(zoom > 4.0 && zoom < 6.0, "{zoom}");
        // A single place gets a close zoom, not an infinite one.
        assert_eq!(frame(&[at(43.0, 5.0)]).1, 14.0);
    }
}
