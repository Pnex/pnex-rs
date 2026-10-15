//! Map of the case (cases-workspace.md K28): each tracked member's route,
//! travelled up to the cursor, and where it stands at the cursor; the
//! located members (places, areas, POIs) as markers; on the org basemap,
//! framed once on all of it. A list of what is located centres the map on
//! an item and opens it in the inspector, as a click on its marker does.
//! The tracks go to map.js once per window; each frame only sends the
//! cursor.

use dioxus::prelude::*;
use dioxus_i18n::t;

use pnex_core::geo_area::GeoArea;
use pnex_core::ontology::api::CaseTrack;
use serde_json::json;

use super::{model, timeline::actor_color, Sel, Ws};
use crate::api;
use crate::components::map_panel::{MapPanel, MapPanelRow, MapPanelToggle};
use crate::map_viewer::{self, MapItem, MapOpts};

const MAP_HOST: &str = "pnex-case-map";
/// Share of the visible window drawn as the recent, stronger trail.
const RECENT_SHARE: i64 = 10;

/// map.js `setTracks` payload: absolute instants, colours of the lanes.
fn tracks_payload(
    tracks: &[CaseTrack],
    follow: Option<&str>,
    window: (i64, i64),
) -> serde_json::Value {
    let tracks: Vec<_> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| follow.is_none_or(|f| t.object.id == f))
        .map(|(i, t)| {
            let segments: Vec<Vec<(f64, f64, i64)>> = model::segments(t)
                .iter()
                .map(|seg| seg.iter().map(|p| (p.lon, p.lat, p.t_ms)).collect())
                .collect();
            json!({ "id": t.object.id, "color": actor_color(i), "segments": segments })
        })
        .collect();
    json!({ "recentMs": (window.1 - window.0) / RECENT_SHARE, "tracks": tracks })
}

/// Centre and zoom framing `(lon, lat)` points.
fn frame(points: &[(f64, f64)]) -> ((f64, f64), f64) {
    let (mut w, mut s, mut e, mut n) = (180.0f64, 90.0f64, -180.0f64, -90.0f64);
    for (lon, lat) in points {
        (w, e) = (w.min(*lon), e.max(*lon));
        (s, n) = (s.min(*lat), n.max(*lat));
    }
    let span = (e - w).max(n - s).max(0.005);
    (
        ((w + e) / 2.0, (s + n) / 2.0),
        (8.0 - span.log2()).clamp(2.0, 16.0),
    )
}

#[component]
pub(super) fn CaseMap() -> Element {
    let ws = use_context::<Ws>();
    let mut mounted = use_signal(|| false);
    let mut ready = use_signal(|| false);
    let mut failed = use_signal(|| false);
    let mut no_basemap = use_signal(|| false);
    let tracks = (ws.tracks)().map(|t| t.tracks).unwrap_or_default();
    let cursor = (ws.cursor)();
    let follow = (ws.follow)();
    // The selected event's place, pinned.
    let pin = match (ws.sel)() {
        Sel::Event(id) => (ws.data)().and_then(|w| {
            let e = w.events.into_iter().find(|e| e.event.id == id)?;
            let at = e
                .event
                .properties
                .get(pnex_core::ontology::EVENT_LOCATION)?;
            Some(MapItem {
                kind: "pin",
                id: None,
                lon: at.get("lon")?.as_f64()?,
                lat: at.get("lat")?.as_f64()?,
                label: e.event.title,
                emoji: "📌".into(),
                count: 0,
            })
        }),
        _ => None,
    };
    let places = (ws.data)().map(|w| w.places).unwrap_or_default();
    let place_items = places.iter().map(|p| MapItem {
        kind: "pin",
        id: Some(p.object.id.clone()),
        lon: p.lon,
        lat: p.lat,
        label: p.object.title.clone(),
        emoji: "📍".into(),
        count: 0,
    });
    let items: Vec<MapItem> = tracks
        .iter()
        .filter(|t| follow.as_ref().is_none_or(|f| &t.object.id == f))
        .filter_map(|t| {
            let (lon, lat) = model::position(t, cursor)?;
            Some(MapItem {
                kind: "position",
                id: Some(t.object.id.clone()),
                lon,
                lat,
                label: t.object.title.clone(),
                emoji: String::new(),
                count: 0,
            })
        })
        .chain(pin)
        .chain(place_items)
        .collect();
    // The list: tracked members where they stand, then the located members.
    let located: Vec<(String, String, f64, f64, bool)> = items
        .iter()
        .filter(|i| i.kind != "pin" || i.id.is_some())
        .filter_map(|i| {
            Some((
                i.id.clone()?,
                i.label.clone(),
                i.lon,
                i.lat,
                i.kind == "position",
            ))
        })
        .collect();
    let has_points = tracks
        .iter()
        .any(|t| t.segments.iter().any(|s| !s.is_empty()))
        || (ws.data)().is_some_and(|w| !w.zones.is_empty() || !w.places.is_empty());
    // Mount once there is something to frame.
    use_effect(move || {
        let tracks = (ws.tracks)().map(|t| t.tracks).unwrap_or_default();
        let zones = (ws.data)().map(|w| w.zones).unwrap_or_default();
        let places = (ws.data)().map(|w| w.places).unwrap_or_default();
        let points: Vec<(f64, f64)> = tracks
            .iter()
            .flat_map(|t| t.segments.iter().flatten().map(|p| (p.0, p.1)))
            .chain(
                zones
                    .iter()
                    .filter_map(|z| Some(GeoArea::parse(&z.area)?.center())),
            )
            .chain(places.iter().map(|p| (p.lon, p.lat)))
            .collect();
        // The basemap shows as soon as the case is loaded, even with nothing
        // located yet (the hint sits on the map, never on a grey void).
        if *mounted.peek() || (ws.data)().is_none() {
            return;
        }
        mounted.set(true);
        let (center, zoom) = if points.is_empty() {
            (
                crate::pages::map::DEFAULT_CENTER,
                crate::pages::map::DEFAULT_ZOOM,
            )
        } else {
            frame(&points)
        };
        spawn(async move {
            let maps = api::geo::basemaps().await.unwrap_or_default();
            let chosen = crate::pages::map::pick_basemap(&maps);
            no_basemap.set(chosen.is_none());
            let opts = MapOpts {
                style_url: chosen.map(|b| b.style_url.clone()).unwrap_or_default(),
                style_url_dark: chosen.and_then(|b| b.style_url_dark.clone()),
                center,
                zoom,
                // The located members at once (positions follow the cursor).
                items: places
                    .iter()
                    .map(|p| MapItem {
                        kind: "pin",
                        id: Some(p.object.id.clone()),
                        lon: p.lon,
                        lat: p.lat,
                        label: p.object.title.clone(),
                        emoji: "📍".into(),
                        count: 0,
                    })
                    .collect(),
            };
            let ok = map_viewer::mount(MAP_HOST, &opts).await;
            failed.set(!ok);
            ready.set(ok);
        });
    });
    use_effect(use_reactive!(|items| {
        if ready() {
            spawn(async move {
                map_viewer::set_items(MAP_HOST, &items).await;
            });
        }
    }));
    // Areas of the members, as polygons.
    use_effect(move || {
        let zones = (ws.data)().map(|w| w.zones).unwrap_or_default();
        let features: Vec<_> = zones
            .iter()
            .filter_map(|z| {
                let g = GeoArea::parse(&z.area)?.to_geojson(64);
                Some(json!({ "type": "Feature", "properties": { "title": z.object.title }, "geometry": g }))
            })
            .collect();
        let payload = json!({ "type": "FeatureCollection", "features": features });
        if ready() {
            spawn(async move {
                map_viewer::set_zones(MAP_HOST, &payload).await;
            });
        }
    });
    // Tracks once per window or followed member; then only the cursor.
    use_effect(move || {
        let payload = tracks_payload(
            &(ws.tracks)().map(|t| t.tracks).unwrap_or_default(),
            (ws.follow)().as_deref(),
            (ws.window)(),
        );
        if ready() {
            spawn(async move {
                map_viewer::set_tracks(MAP_HOST, &payload).await;
            });
        }
    });
    use_effect(move || {
        let t = (ws.cursor)();
        if ready() {
            spawn(async move {
                map_viewer::set_time(MAP_HOST, t).await;
            });
        }
    });
    // A click on a marker opens what it stands for.
    use_future(move || async move {
        let mut seen = 0u64;
        loop {
            crate::util::sleep(std::time::Duration::from_millis(300)).await;
            if let Some(click) = map_viewer::take_click(seen).await {
                seen = click.seq;
                if let Some(id) = click.id {
                    let mut sel = ws.sel;
                    sel.set(Sel::Object(id));
                }
            }
        }
    });
    use_drop(|| map_viewer::unmount(MAP_HOST));
    let selected = match (ws.sel)() {
        Sel::Object(id) => Some(id),
        _ => None,
    };
    // The list panel of every map (components/map_panel.rs): search and a
    // tracked-only filter over the located members.
    let mut panel_open = use_signal(|| true);
    let search = use_signal(String::new);
    let mut tracked_only = use_signal(|| false);
    let total = located.len();
    let needle = search().trim().to_lowercase();
    let shown: Vec<(String, String, f64, f64, bool)> = located
        .into_iter()
        .filter(|(_, title, _, _, tracked)| {
            (needle.is_empty() || title.to_lowercase().contains(&needle))
                && (!tracked_only() || *tracked)
        })
        .collect();
    rsx! {
        section { class: "absolute inset-0", "aria-label": t!("cw-map"),
            // MapLibre makes its container `position: relative`: size it
            // by height, not by `inset`.
            div {
                id: MAP_HOST,
                class: "h-full w-full overflow-hidden",
                "data-testid": "case-map",
            }
            if no_basemap() {
                p { class: "absolute inset-x-2 bottom-2 z-10 rounded-md border border-line bg-surface px-2 py-1 text-xs text-slate",
                    {t!("geo-basemap-none")}
                }
            }
            if !has_points {
                p { class: "absolute left-1/2 top-4 z-10 -translate-x-1/2 rounded-full border border-line bg-surface px-3 py-1.5 text-xs text-slate shadow",
                    {t!("cw-map-empty")}
                }
            }
            if failed() {
                p { class: "absolute inset-x-2 bottom-2 rounded-md bg-surface px-2 py-1 text-xs text-slate",
                    {t!("viz-map-unavailable")}
                }
            }
            if total > 0 && panel_open() {
                MapPanel {
                    title: t!("cw-located").to_string(),
                    docked: true,
                    on_close: move |_| panel_open.set(false),
                    search,
                    search_placeholder: t!("cw-located-search").to_string(),
                    filters: rsx! {
                        label { class: "flex items-center gap-2 text-sm text-gray-700",
                            input {
                                r#type: "checkbox",
                                checked: tracked_only(),
                                onchange: move |_| tracked_only.toggle(),
                            }
                            {t!("cw-located-tracked-only")}
                        }
                    },
                    footer: rsx! {
                        {t!("cw-located-count", count : shown.len())}
                    },
                    for (id, title, lon, lat, tracked) in shown {
                        MapPanelRow {
                            key: "{id}",
                            title,
                            detail: tracked.then(|| t!("cw-located-tracked").to_string()),
                            selected: selected.as_deref() == Some(id.as_str()),
                            lead: rsx! {
                                span {
                                    class: "h-2.5 w-2.5 shrink-0 rounded-full",
                                    style: if tracked { "background: {super::timeline::ACTOR_COLORS[1]}" } else { "background: var(--color-signal)" },
                                }
                            },
                            onclick: {
                                let id = id.clone();
                                move |_| {
                                    let mut sel = ws.sel;
                                    sel.set(Sel::Object(id.clone()));
                                    spawn(async move {
                                        map_viewer::flyTo(MAP_HOST, lon, lat, 15.0).await;
                                    });
                                }
                            },
                        }
                    }
                }
            }
            if total > 0 && !panel_open() {
                div { class: "absolute right-3 top-3 z-20",
                    MapPanelToggle { onclick: move |_| panel_open.set(true) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_every_point() {
        let ((lon, lat), zoom) = frame(&[(5.0, 43.0), (2.0, 49.0)]);
        assert_eq!((lon, lat), (3.5, 46.0));
        assert!(zoom > 4.0 && zoom < 6.0, "{zoom}");
        let close = frame(&[(5.0, 43.0)]).1;
        assert!(close > 15.0 && close <= 16.0, "{close}");
    }

    #[test]
    fn payload_keeps_lane_colours_and_absolute_instants() {
        let track = |id: &str| CaseTrack {
            object: pnex_core::ontology::api::ObjectRef {
                id: id.into(),
                type_key: "device".into(),
                title: id.into(),
                native_id: "1".into(),
            },
            t0_ms: 1_000,
            segments: vec![vec![(2.0, 48.0, None, 0), (2.1, 48.1, None, 500)]],
            raw_points: 2,
        };
        let p = tracks_payload(&[track("a"), track("b")], Some("b"), (0, 10_000));
        assert_eq!(p["recentMs"], 1_000);
        assert_eq!(p["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(p["tracks"][0]["color"], actor_color(1));
        assert_eq!(p["tracks"][0]["segments"][0][1], json!([2.1, 48.1, 1_500]));
    }
}
