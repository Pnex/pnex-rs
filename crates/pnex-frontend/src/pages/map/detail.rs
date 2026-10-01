use super::form::PoiFormModal;
use super::*;

// ─────────────────────────── détail POI (drawer) ───────────────────────────

#[component]
pub(super) fn PoiDetail(
    poi_id: String,
    /// Aperçu intégré : cible courante + état « afficher la carte ».
    mut preview: Signal<Option<PreviewTarget>>,
    mut preview_hidden: Signal<bool>,
    /// Épingle POI (★) : objet affiché en priorité à l'ouverture.
    mut pinned: Signal<Option<PreviewTarget>>,
    on_close: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let mut confirm_delete = use_signal(|| false);
    let mut editing = use_signal(|| false);
    // D43 : conflit d'attache (device déjà placé ailleurs) — le corps 409
    // alimente la boîte de confirmation de déplacement.
    let mut move_conflict = use_signal(|| None::<serde_json::Value>);
    // Version du détail : incrémentée après chaque mutation réussie (device,
    // liens, édition) → refetch du drawer. Sans dep réactive, la resource ne
    // tourne qu'une fois et affiche des lignes fantômes après un detach (le
    // clic suivant rejoue un DELETE sur un lien déjà supprimé → 404).
    let mut detail_version = use_signal(|| 0u64);
    // Picker d'objets à attacher (ouverture + onglet initial).
    let mut picker_open = use_signal(|| false);
    let mut picker_tab = use_signal(|| PickerTab::Media);
    // Navigation deep-link (dashboard attaché → /dashboards?id=…).

    let poi_id_for_res = poi_id.clone();
    let poi_resource = use_resource(move || {
        let _version = detail_version();
        let id = poi_id_for_res.clone();
        async move { viz::poi_detail(&id).await }
    });
    // Un clone par closure (poi_id n'est pas Copy et sert à plusieurs `move`).
    let poi_id_attach = poi_id.clone();
    let poi_id_delete = poi_id.clone();
    let poi_id_move = poi_id.clone();
    let devices_resource = use_resource(move || async move {
        api::devices::list(&api::devices::DeviceFilters {
            limit: Some(100),
            ..Default::default()
        })
        .await
        .ok()
    });

    let poi = poi_resource.read().clone().and_then(Result::ok);
    let devices = devices_resource
        .read()
        .clone()
        .flatten()
        .map(|p| p.results)
        .unwrap_or_default();

    // Section unique « objets attachés » : placements devices (D43) en tête,
    // puis les arêtes `placed_on` tous kinds confondus (D42).
    let attached_links: Vec<viz::VizLink> =
        poi.as_ref().map(|p| p.links.clone()).unwrap_or_default();
    let attached_devices: Vec<viz::DevicePlacement> =
        poi.as_ref().map(|p| p.devices.clone()).unwrap_or_default();
    let coord_text = poi
        .as_ref()
        .map(|p| {
            format!(
                "{:.5}°, {:.5}°",
                p.latitude.unwrap_or(0.0),
                p.longitude.unwrap_or(0.0)
            )
        })
        .unwrap_or_default();
    // Valeurs possédées pour les blocs conditionnels du rsx (pas de `let`
    // dans les corps d'éléments).
    let actions_poi = poi.clone();
    let confirm_poi = poi.clone();
    // D43 : message de la modale de suppression (avertissement « devices à
    // replacer ») et données de la boîte de confirmation de déplacement —
    // tout calculé hors rsx.
    let delete_message = confirm_poi.as_ref().map(|p| {
        let mut msg = t!("poi-delete-message", label: p.label.clone()).to_string();
        let device_count = p.devices.len();
        if device_count > 0 {
            msg.push(' ');
            msg.push_str(&t!("poi-delete-devices", count: device_count).to_string());
        }
        msg
    });
    let move_conflict_data = move_conflict().map(|body| {
        (
            body["device_id"].as_str().unwrap_or_default().to_string(),
            body["current_pin_label"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            body["current_placement_id"].as_i64().unwrap_or_default(),
        )
    });
    // Index devices (pk, slug, type) pour l'arbre — l'id containment device
    // = PK entière (registre D42), pas le slug.
    let device_index: Vec<(String, String, String)> = devices
        .iter()
        .map(|d| (d.id.to_string(), d.device_id.clone(), d.device_type.clone()))
        .collect();

    // ── Épingle + aperçu : seed depuis le POI, UNE FOIS par POI (garde
    // type `deep_seeded` — un effet qui re-consomme des données fixes boucle
    // sans fin, leçon « Page Unresponsive » 2026-09-13) ──
    let mut seeded_pin = use_signal(|| None::<String>);
    let poi_id_for_seed = poi_id.clone();
    use_effect(move || {
        if seeded_pin.cloned().as_deref() == Some(poi_id_for_seed.as_str()) {
            return;
        }
        let detail_now = poi_resource.value().read().clone().and_then(Result::ok);
        let Some(p) = detail_now else {
            return;
        };
        // Nom du pin résolu dans les arêtes (objet à la racine) ; un objet
        // rangé en dossier n'a plus d'arête → fallback sur l'id (cosmétique).
        let target = match (p.preview_kind.as_deref(), p.preview_id.as_deref()) {
            (Some(kind), Some(id)) if matches!(kind, "media_asset" | "dashboard" | "tour") => {
                let name = p
                    .links
                    .iter()
                    .find(|l| l.target_kind == kind && l.target_id == id)
                    .and_then(|l| l.target_label.clone())
                    .unwrap_or_else(|| id.to_string());
                Some(PreviewTarget {
                    kind: kind.to_string(),
                    id: id.to_string(),
                    name,
                })
            }
            _ => None,
        };
        pinned.set(target.clone());
        // Auto-open : l'aperçu épinglé s'affiche à l'ouverture du POI.
        preview.set(target);
        preview_hidden.set(false);
        seeded_pin.set(Some(poi_id_for_seed.clone()));
    });

    // Pose/retrait de l'épingle (★ des lignes de l'arbre) — toggle si déjà
    // épinglé sur cet objet ; PATCH poi → refetch drawer via on_changed.
    let pin_now = move |t: PreviewTarget| {
        let unpin = pinned
            .cloned()
            .is_some_and(|p| p.kind == t.kind && p.id == t.id);
        let poi = poi_id.clone();
        let name = t.name.clone();
        spawn(async move {
            let body = if unpin {
                serde_json::json!({ "preview_kind": null, "preview_id": null })
            } else {
                serde_json::json!({ "preview_kind": t.kind, "preview_id": t.id })
            };
            match viz::update_poi(&poi, body).await {
                Ok(p) => {
                    pinned.set(match (p.preview_kind, p.preview_id) {
                        (Some(k), Some(i)) => Some(PreviewTarget {
                            kind: k,
                            id: i,
                            name,
                        }),
                        _ => None,
                    });
                    if unpin {
                        toasts::success(t!("poi-unpin").to_string());
                    } else {
                        toasts::success(t!("poi-pin").to_string());
                    }
                }
                Err(err) => toasts::error(format!("{err}")),
            }
            detail_version += 1;
            on_changed.call(());
        });
    };

    // Navigation « Éditer » du panneau (l'édition vit dans l'onglet de l'app).
    let navigator = use_navigator();

    rsx! {
        // Overlay (école media.rs — backdrop + aside droite).
        div { class: "fixed inset-0 z-40",
            div {
                class: "absolute inset-0 bg-black/30",
                onclick: move |_| on_close.call(()),
            }
            // ── Aperçu intégré (read-only) : panneau plein-cadre à gauche du
            // drawer, au-dessus du backdrop — « Afficher la carte » → pilule
            // de rappel en bas à gauche.
            if let Some(target) = preview() {
                if !preview_hidden() {
                    div { class: "absolute inset-y-0 left-0 right-96 z-10 bg-white flex flex-col",
                        PoiPreviewPanel {
                            key: "preview-{target.kind}-{target.id}",
                            target,
                            on_close: move |_| {
                                preview.set(None);
                                preview_hidden.set(false);
                            },
                            on_edit: move |_| {
                                // Édition = onglet de l'app dédié (studio pour
                                // les tours via deep-link OPEN_TOUR).
                                let Some(t) = preview.cloned() else { return };
                                let nav = navigator;
                                spawn(async move {
                                    match t.kind.as_str() {
                                        "dashboard" => {
                                            nav.push(Route::Dashboards {
                                                id: t.id,
                                                mode: "edit".to_string(),
                                            });
                                        }
                                        "tour" => {
                                            let id = t.id.clone();
                                            OPEN_TOUR.with_mut(|t| *t = Some(id));
                                            nav.push(Route::Studio {});
                                        }
                                        _ => {
                                            nav.push(Route::Media {});
                                        }
                                    }
                                });
                            },
                            on_show_map: move |_| preview_hidden.set(true),
                        }
                    }
                } else {
                    // Pilule de rappel : rouvrir l'aperçu masqué.
                    button {
                        class: "absolute bottom-4 left-4 z-10 inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-gray-700 bg-white rounded-full shadow-lg border border-gray-200 hover:bg-gray-50 transition-colors",
                        onclick: move |_| preview_hidden.set(false),
                        icons::Map { class: "h-3.5 w-3.5 text-gray-400" }
                        {t!("poi-preview-recall", name: target.name.clone())}
                    }
                }
            }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h2 { class: "text-base font-semibold text-gray-900 truncate",
                        if let Some(p) = poi.as_ref() {
                            "{p.emoji} {p.label}"
                        } else {
                            {t!("poi-detail")}
                        }
                    }
                    button {
                        class: "p-1.5 text-gray-500 hover:text-gray-900 rounded-lg hover:bg-gray-100 transition-colors",
                        onclick: move |_| on_close.call(()),
                        icons::X { class: "h-5 w-5" }
                    }
                }
                div { class: "flex-1 overflow-y-auto p-4 space-y-4",
                    match poi.as_ref() {
                        None => rsx! { div { class: "text-sm text-gray-500", {t!("poi-loading")} } },
                        Some(p) => rsx! {
                            // Localisation libre (remplace la hiérarchie).
                            if let Some(detail) = p.location_detail.clone() {
                                div { class: "text-sm text-gray-600", "{detail}" }
                            }
                            if !coord_text.is_empty() {
                                div { class: "text-xs text-gray-400", "{coord_text}" }
                            }
                            // ── Objets attachés : arbre de dossiers ──
                            div { class: "space-y-1.5",
                                h3 { class: "text-xs font-semibold text-gray-500 uppercase tracking-wide",
                                    {t!("poi-attachments-title")}
                                }
                                PoiTreeSection {
                                    key: "{p.id}",
                                    poi_id: p.id.clone(),
                                    links: attached_links.clone(),
                                    placements: attached_devices.clone(),
                                    devices: device_index.clone(),
                                    version: detail_version,
                                    preview: preview,
                                    pinned: pinned,
                                    on_preview: move |t: PreviewTarget| {
                                        preview.set(Some(t));
                                        preview_hidden.set(false);
                                    },
                                    on_pin: pin_now,
                                    on_attach: move |_| {
                                        picker_tab.set(PickerTab::Media);
                                        picker_open.toggle();
                                    },
                                    on_mutated: move |_| {
                                        detail_version += 1;
                                        on_changed.call(());
                                    },
                                }
                            }
                        },
                    }
                }
                // Barre d'actions (édition + suppression).
                if actions_poi.is_some() {
                    div { class: "px-4 py-3 border-t border-gray-200 flex justify-between gap-2",
                        div { class: "flex gap-2",
                            button {
                                class: "px-3 py-1.5 text-sm text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                                onclick: move |_| editing.set(true),
                                {t!("poi-edit")}
                            }
                            button {
                                class: "px-3 py-1.5 text-sm text-gray-700 border border-gray-200 rounded-lg hover:bg-gray-50 transition-colors",
                                onclick: move |_| {
                                    if let Some(p) = actions_poi.as_ref() {
                                        if let (Some(lat), Some(lon)) = (p.latitude, p.longitude) {
                                            spawn(async move {
                                                map_viewer::flyTo(MAP_HOST, lon, lat, 17.0).await;
                                            });
                                        }
                                    }
                                },
                                {t!("poi-recenter")}
                            }
                        }
                        button {
                            class: "px-3 py-1.5 text-sm text-red-600 border border-red-200 rounded-lg hover:bg-red-50 transition-colors",
                            onclick: move |_| confirm_delete.set(true),
                            icons::Trash2 { class: "h-4 w-4 inline-block mr-1" }
                            {t!("viz-delete")}
                        }
                    }
                }
            }
        }
        // Formulaire d'édition (modale partagée création/édition).
        if let (true, Some(p)) = (editing(), poi.as_ref()) {
            PoiFormModal {
                key: "edit-{p.id}",
                initial: Some(p.clone()),
                coords: (p.latitude.unwrap_or(0.0), p.longitude.unwrap_or(0.0)),
                on_saved: move |_| {
                    editing.set(false);
                    detail_version += 1;
                    on_changed.call(());
                },
                on_close: move |_| editing.set(false),
            }
        }
        // Picker d'objets à attacher — le picker ne mute rien, le drawer
        // exécute la mutation (edge D42 ou placement D43).
        if picker_open() {
            ResourcePicker {
                key: "poi-picker-{detail_version}",
                initial_tab: Some(picker_tab()),
                on_picked: move |pick: ResourcePick| {
                    picker_open.set(false);
                    let id = poi_id_attach.clone();
                    // Auto-épingle : premier objet aperçuable attaché = pin
                    // par défaut (si le POI n'en a pas déjà une).
                    let pin_candidate: Option<(String, String, String)> = match &pick {
                        ResourcePick::MediaAsset { id, name } => {
                            Some(("media_asset".to_string(), id.clone(), name.clone()))
                        }
                        ResourcePick::Tour { id, name } => {
                            Some(("tour".to_string(), id.clone(), name.clone()))
                        }
                        ResourcePick::Dashboard { id, name } => {
                            Some(("dashboard".to_string(), id.clone(), name.clone()))
                        }
                        ResourcePick::Device { .. } => None,
                    };
                    spawn(async move {
                        let result = match pick {
                            ResourcePick::MediaAsset { id: target, name } => {
                                api::resources::create_edge(
                                    "placed_on",
                                    ("map_pin", &id),
                                    ("media_asset", &target),
                                    Some(serde_json::json!({ "label": name })),
                                )
                                .await
                                .map(|_| ())
                            }
                            ResourcePick::Tour { id: target, name } => {
                                api::resources::create_edge(
                                    "placed_on",
                                    ("map_pin", &id),
                                    ("tour", &target),
                                    Some(serde_json::json!({ "label": name })),
                                )
                                .await
                                .map(|_| ())
                            }
                            ResourcePick::Dashboard { id: target, name } => {
                                api::resources::create_edge(
                                    "placed_on",
                                    ("map_pin", &id),
                                    ("dashboard", &target),
                                    Some(serde_json::json!({ "label": name })),
                                )
                                .await
                                .map(|_| ())
                            }
                            ResourcePick::Device { slug } => {
                                // D43 : attache délibérée (plus jamais un
                                // remplacement silencieux — un device déjà
                                // placé répond 409 et ouvre la confirmation).
                                viz::attach_poi_device(&id, &slug, None)
                                    .await
                                    .map(|_| ())
                            }
                        };
                        match result {
                            Ok(_) => {
                                detail_version += 1;
                                on_changed.call(());
                                // Auto-épingle du premier objet aperçuable.
                                if let Some((kind, target, name)) = pin_candidate {
                                    let already = poi_resource
                                        .value()
                                        .read()
                                        .clone()
                                        .and_then(Result::ok)
                                        .map(|p| p.preview_kind.is_some())
                                        .unwrap_or(false);
                                    if !already {
                                        let body = serde_json::json!({
                                            "preview_kind": kind.clone(),
                                            "preview_id": target,
                                        });
                                        if let Err(err) = viz::update_poi(&id, body).await {
                                            toasts::error(format!("{err}"));
                                        } else {
                                            pinned.set(Some(PreviewTarget {
                                                kind,
                                                id: target,
                                                name,
                                            }));
                                        }
                                    }
                                }
                            }
                            Err(err) if err.status == Some(409) => {
                                // Device déjà placé : sur CE POI → simple
                                // refresh ; ailleurs → boîte de confirmation
                                // de déplacement (le corps porte le placement
                                // courant — D43, jamais silencieux).
                                if let Some(body) = err.body {
                                    if body["current_pin_id"].as_str() == Some(id.as_str()) {
                                        detail_version += 1;
                                        on_changed.call(());
                                    } else {
                                        move_conflict.set(Some(body));
                                    }
                                }
                            }
                            Err(err) => toasts::error(format!("{err}")),
                        }
                    });
                },
                on_close: move |_| picker_open.set(false),
            }
        }
        if confirm_delete() && delete_message.is_some() {
            // D43 : les devices placés perdent leur placement (cascade) —
            // avertissement « à replacer » dans le message.
            ConfirmDialog {
                title: t!("poi-delete-title").to_string(),
                message: delete_message.clone().unwrap_or_default(),
                confirm_label: t!("viz-delete").to_string(),
                on_confirm: move |_| {
                    confirm_delete.set(false);
                    let id = poi_id_delete.clone();
                    spawn(async move {
                        match viz::delete_poi(&id).await {
                            Ok(_) => {
                                toasts::success(t!("poi-deleted").to_string());
                                on_close.call(());
                                on_changed.call(());
                            }
                            Err(err) => toasts::error(format!("{err}")),
                        }
                    });
                },
                on_cancel: move |_| confirm_delete.set(false),
            }
        }
        // D43 : confirmation de déplacement d'un device déjà placé ailleurs
        // (le 409 a été silencieux — c'est ici que le déplacement se décide).
        if let Some((slug, label, placement_id)) = move_conflict_data {
            ConfirmDialog {
                title: t!("poi-device-move-title").to_string(),
                message: t!(
                    "poi-device-move-confirm",
                    slug: slug.clone(),
                    label: label.clone()
                )
                .to_string(),
                confirm_label: t!("poi-device-move-here").to_string(),
                on_confirm: move |_| {
                    move_conflict.set(None);
                    let target = poi_id_move.clone();
                    spawn(async move {
                        match viz::update_poi_placement(
                            placement_id,
                            serde_json::json!({ "pin_id": target }),
                        )
                        .await
                        {
                            Ok(_) => {
                                toasts::success(t!("poi-device-moved").to_string());
                                detail_version += 1;
                                on_changed.call(());
                            }
                            Err(err) => toasts::error(format!("{err}")),
                        }
                    });
                },
                on_cancel: move |_| move_conflict.set(None),
            }
        }
    }
}
