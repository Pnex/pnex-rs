use super::*;

/// Filtres courants de la sidebar (liste ET cluster — mêmes sélections).
pub(super) fn current_filters(
    search: Signal<String>,
    emoji: Signal<String>,
    has_device: Signal<bool>,
    has_position: Signal<bool>,
) -> viz::PoiFilters {
    viz::PoiFilters {
        search: Some(search()),
        emoji: Some(emoji()),
        has_device: has_device(),
        has_position: has_position(),
        ..Default::default()
    }
}

/// Refetch cluster (D37) + redessin des markers (items filtrés + couche GPS).
/// Les erreurs restent silencieuses (école « badge, jamais panic » — pas de
/// toast à chaque moveend si le réseau tombe).
pub(super) async fn redraw_with(
    positions: &Signal<Vec<viz::DevicePosition>>,
    filters: &viz::PoiFilters,
    west: f64,
    south: f64,
    east: f64,
    north: f64,
    zoom: i32,
) {
    let Ok(resp) = viz::cluster_pois(west, south, east, north, zoom, filters).await else {
        return;
    };
    let mut items: Vec<MapItem> = resp
        .items
        .iter()
        .map(|it| MapItem {
            kind: if it.count == 1 { "pin" } else { "cluster" },
            id: it.id.clone(),
            lon: it.lon,
            lat: it.lat,
            label: it.label.clone(),
            emoji: if it.count == 1 {
                it.emoji.clone()
            } else {
                String::new()
            },
            count: it.count,
        })
        .collect();
    for p in positions.read().iter() {
        items.push(MapItem {
            kind: "position",
            id: None,
            lon: p.longitude,
            lat: p.latitude,
            label: p.device_id.clone(),
            emoji: "🛰️".to_string(),
            count: 0,
        });
    }
    map_viewer::set_items(MAP_HOST, &items).await;
}

pub(super) async fn handle_click(
    mut selected: Signal<Option<String>>,
    mut pending_add: Signal<Option<(f64, f64)>>,
    current_zoom: Signal<f64>,
    add_mode: Signal<bool>,
    click: map_viewer::MapClick,
) {
    match click.kind.as_str() {
        // POI individuel → drawer détail.
        "pin" => {
            if let Some(id) = click.id {
                selected.set(Some(id));
            }
        }
        // Cluster numéroté → zoom +2 (le moveend refetch redessinera).
        "cluster" => {
            if let (Some(lon), Some(lat)) = (click.lng, click.lat) {
                let zoom = (current_zoom() + 2.0).min(22.0);
                map_viewer::flyTo(MAP_HOST, lon, lat, zoom).await;
            }
        }
        // Device GPS live → info toast.
        "position" => toasts::info(format!("🛰️ {}", click.label)),
        // Clic carte nue : en mode ajout (formulaire fermé), capture coords.
        "map" => {
            let coords = click.lng.zip(click.lat).map(|(lon, lat)| (lat, lon));
            if add_mode() && pending_add().is_none() {
                pending_add.set(coords);
            }
        }
        _ => {}
    }
}

// ─────────────────────────── ligne de liste ───────────────────────────

#[component]
pub(super) fn PoiRow(
    poi: viz::Poi,
    positions: Vec<viz::DevicePosition>,
    on_select: EventHandler<String>,
) -> Element {
    // GPS si l'un des devices placés transmet (D43 : plusieurs possibles).
    let has_gps = poi
        .devices
        .iter()
        .any(|d| positions.iter().any(|p| p.device_id == d.device_id));
    let device_count = poi.devices.len();
    let device_line = poi.devices.first().map(|d| {
        if device_count > 1 {
            format!("{} +{}", d.device_id, device_count - 1)
        } else {
            d.device_id.clone()
        }
    });
    rsx! {
        button {
            class: "w-full text-left px-4 py-2.5 border-b border-gray-100 hover:bg-gray-50 transition-colors",
            onclick: move |_| on_select.call(poi.id.clone()),
            div { class: "flex items-center gap-2",
                span { class: "text-xl leading-none", {poi.emoji.clone()} }
                span { class: "text-sm font-medium text-gray-900 truncate flex-1", {poi.label.clone()} }
                if device_count == 1 {
                    span { class: "px-1.5 py-0.5 text-[10px] font-semibold text-blue-700 bg-blue-50 rounded", {t!("poi-device-attached-badge")} }
                }
                if device_count > 1 {
                    span { class: "px-1.5 py-0.5 text-[10px] font-semibold text-blue-700 bg-blue-50 rounded", {t!("poi-device-count", count: device_count)} }
                }
                if has_gps {
                    span { class: "px-1.5 py-0.5 text-[10px] font-semibold text-emerald-700 bg-emerald-50 rounded", {t!("poi-gps-badge")} }
                }
            }
            if let Some(device) = device_line {
                div { class: "text-xs text-gray-500 mt-0.5 truncate", "{device}" }
            }
            if let Some(detail) = poi.location_detail.clone() {
                div { class: "text-xs text-gray-500 mt-0.5 truncate", "{detail}" }
            }
        }
    }
}
