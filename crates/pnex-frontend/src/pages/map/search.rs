use super::*;

// ───────────────────── address search (geo-layers.md §8) ─────────────────────

/// Address search over the map, through the org's default geocoder (server
/// proxy). A result recentres the map; « + POI » opens the creation form
/// prefilled with the address and its coordinates.
#[component]
pub(super) fn AddressSearch(
    pending_add: Signal<Option<(f64, f64)>>,
    suggested: Signal<Option<pnex_core::geo::GeocodeResult>>,
) -> Element {
    let mut query = use_signal(String::new);
    let mut hits = use_signal(Vec::<pnex_core::geo::GeocodeResult>::new);
    let mut searched = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let mut run = move || {
        let q = query().trim().to_string();
        if q.is_empty() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::geo::geocode(&q).await {
                Ok(found) => {
                    hits.set(found);
                    error.set(None);
                }
                Err(err) => {
                    hits.set(Vec::new());
                    error.set(Some(api::error_i18n::localize(&err)));
                }
            }
            searched.set(true);
            busy.set(false);
        });
    };

    rsx! {
        div { class: "relative w-72",
            input {
                r#type: "search",
                class: "w-full px-3 py-2.5 text-sm bg-white rounded-lg shadow border border-gray-200",
                placeholder: t!("geo-search-placeholder"),
                aria_label: t!("geo-search-placeholder"),
                value: "{query}",
                oninput: move |e| {
                    query.set(e.value());
                    searched.set(false);
                },
                onkeydown: move |e| {
                    if e.key() == Key::Enter {
                        run();
                    }
                },
            }
            if busy() {
                span { class: "absolute right-3 top-3 animate-spin rounded-full h-4 w-4 border-b-2 border-blue-600" }
            }
            if searched() {
                div { class: "absolute mt-1 w-full bg-white rounded-lg shadow border border-gray-200 text-sm max-h-72 overflow-y-auto",
                    if let Some(msg) = error() {
                        p { class: "px-3 py-2 text-red-700", {msg} }
                    } else if hits().is_empty() {
                        p { class: "px-3 py-2 text-gray-500", {t!("geo-search-none")} }
                    }
                    for (i, hit) in hits().into_iter().enumerate() {
                        AddressHit {
                            key: "{i}",
                            hit,
                            on_pick: move |h: pnex_core::geo::GeocodeResult| {
                                searched.set(false);
                                fly_to_hit(&h);
                            },
                            on_add: move |h: pnex_core::geo::GeocodeResult| {
                                searched.set(false);
                                fly_to_hit(&h);
                                suggested.set(Some(h.clone()));
                                pending_add.set(Some((h.lat, h.lon)));
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AddressHit(
    hit: pnex_core::geo::GeocodeResult,
    on_pick: EventHandler<pnex_core::geo::GeocodeResult>,
    on_add: EventHandler<pnex_core::geo::GeocodeResult>,
) -> Element {
    let for_add = hit.clone();
    let for_pick = hit.clone();
    rsx! {
        div { class: "flex items-center gap-2 px-3 py-2 hover:bg-gray-50 border-b border-gray-100 last:border-0",
            button {
                r#type: "button",
                class: "flex-1 text-left text-gray-800",
                onclick: move |_| on_pick.call(for_pick.clone()),
                {hit.label.clone()}
            }
            button {
                r#type: "button",
                class: "shrink-0 px-2 py-1 text-xs text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50",
                onclick: move |_| on_add.call(for_add.clone()),
                {t!("geo-search-add-poi")}
            }
        }
    }
}

fn fly_to_hit(hit: &pnex_core::geo::GeocodeResult) {
    let (lat, lon) = (hit.lat, hit.lon);
    spawn(async move {
        map_viewer::flyTo(MAP_HOST, lon, lat, 16.0).await;
    });
}
