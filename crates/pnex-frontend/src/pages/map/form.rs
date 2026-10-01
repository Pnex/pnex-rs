use super::*;

// ───────────────────── formulaire POI (création/édition) ───────────────────

/// D43 : plus de `<select>` device dans ce formulaire — les devices
/// s'attachent **délibérément** depuis le drawer (« Objets attachés »,
/// picker onglet Device, déplacement confirmé s'ils sont déjà placés).
#[component]
pub(super) fn PoiFormModal(
    #[props(default)] initial: Option<viz::Poi>,
    coords: (f64, f64),
    on_saved: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    let mut label = use_signal(|| {
        initial
            .as_ref()
            .map(|p| p.label.clone())
            .unwrap_or_default()
    });
    let mut emoji = use_signal(|| {
        initial
            .as_ref()
            .map(|p| p.emoji.clone())
            .unwrap_or_else(|| "📍".to_string())
    });
    let mut detail = use_signal(|| {
        initial
            .as_ref()
            .and_then(|p| p.location_detail.clone())
            .unwrap_or_default()
    });
    let mut lat = use_signal(|| {
        initial
            .as_ref()
            .and_then(|p| p.latitude)
            .unwrap_or(coords.0)
            .to_string()
    });
    let mut lon = use_signal(|| {
        initial
            .as_ref()
            .and_then(|p| p.longitude)
            .unwrap_or(coords.1)
            .to_string()
    });
    let mut saving = use_signal(|| false);

    // Copie légère (pas de double move de `initial` dans les closures).
    let is_edit = initial.is_some();
    let edit_id = initial.as_ref().map(|p| p.id.clone());

    rsx! {
        Modal {
            title: if is_edit { t!("poi-edit-title").to_string() } else { t!("poi-add-title").to_string() },
            max_width: "max-w-md".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("poi-field-label")} }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        value: "{label}",
                        oninput: move |e| label.set(e.value()),
                    }
                }
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("poi-field-emoji")} }
                    div { class: "flex flex-wrap gap-1 mb-1.5",
                        for e in PALETTE {
                            button {
                                class: if emoji() == e { "w-9 h-9 text-xl rounded-lg border-2 border-blue-500 bg-blue-50" } else { "w-9 h-9 text-xl rounded-lg border border-gray-200 hover:border-blue-300" },
                                onclick: move |_| emoji.set(e.to_string()),
                                "{e}"
                            }
                        }
                    }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        placeholder: t!("poi-field-emoji"),
                        value: "{emoji}",
                        oninput: move |e| emoji.set(e.value()),
                    }
                }
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("poi-field-location")} }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        placeholder: t!("poi-field-location-hint"),
                        value: "{detail}",
                        oninput: move |e| detail.set(e.value()),
                    }
                }
                div { class: "grid grid-cols-2 gap-3",
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("poi-field-lat")} }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                            value: "{lat}",
                            oninput: move |e| lat.set(e.value()),
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("poi-field-lon")} }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                            value: "{lon}",
                            oninput: move |e| lon.set(e.value()),
                        }
                    }
                }
                div { class: "flex justify-end gap-2 pt-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm font-semibold text-white bg-blue-600 rounded-lg hover:bg-blue-700 transition-colors disabled:opacity-50",
                        disabled: saving(),
                        onclick: move |_| {
                            // Garde client : coords numériques obligatoires.
                            let (Some(lat_v), Some(lon_v)) =
                                (lat().trim().parse::<f64>().ok(), lon().trim().parse::<f64>().ok())
                            else {
                                toasts::error(t!("poi-coords-invalid").to_string());
                                return;
                            };
                            let body = serde_json::json!({
                                "label": label(),
                                "emoji": emoji(),
                                "location_detail": detail(),
                                "latitude": lat_v,
                                "longitude": lon_v,
                            });
                            saving.set(true);
                            let edit_id = edit_id.clone();
                            spawn(async move {
                                let result = if is_edit {
                                    viz::update_poi(edit_id.as_deref().unwrap_or(""), body).await.map(|_| ())
                                } else {
                                    viz::create_poi(body).await.map(|_| ())
                                };
                                saving.set(false);
                                match result {
                                    Ok(_) => {
                                        toasts::success(if is_edit {
                                            t!("poi-updated").to_string()
                                        } else {
                                            t!("poi-created").to_string()
                                        });
                                        on_saved.call(());
                                    }
                                    Err(err) => toasts::error(format!("{err}")),
                                }
                            });
                        },
                        if saving() { {t!("viz-saving")} } else { {t!("viz-save")} }
                    }
                }
            }
        }
    }
}
