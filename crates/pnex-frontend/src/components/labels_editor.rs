//! Éditeur de labels D42 (chips + input, validation charset serveur
//! re-vérifiée client) — générique sur (kind, id) : réutilisable tel quel
//! pour devices, dashboards, tours, POI, flows. École MetadataEditor
//! (pages/devices.rs) : état local, écriture explicite, jamais de panic.

use dioxus::prelude::*;
use std::collections::BTreeMap;

use crate::api;
use crate::state::toasts;
use dioxus_i18n::t;

/// Éditeur de labels transverse (D42) — chips nom[/valeur] + saisie.
#[component]
pub fn LabelsEditor(
    /// Kind de la ressource ("media_asset", "device", …).
    kind: String,
    /// PK stringifiée.
    id: String,
    can_write: bool,
    /// Prévenir le parent après écriture (refresh liste).
    on_changed: Option<Callback<()>>,
) -> Element {
    let mut labels: Signal<BTreeMap<String, Option<String>>> = use_signal(BTreeMap::new);
    let mut input = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut dirty = use_signal(|| false);

    // Charge les labels à l'ouverture (et quand (kind, id) change).
    let kind_for_load = kind.clone();
    let id_for_load = id.clone();
    use_resource(move || {
        let (kind, id) = (kind_for_load.clone(), id_for_load.clone());
        async move {
            match api::resources::get_labels(&kind, &id).await {
                Ok(set) => {
                    labels.set(set);
                    error.set(String::new());
                }
                Err(err) => error.set(err.message),
            }
        }
    });
    // Entrées pré-calculées (display "name:valeur" | "name") — le rsx
    // reste plat, école pages/media.rs.
    let entries: Vec<(String, String)> = labels
        .read()
        .iter()
        .map(|(k, v)| {
            let display = match v {
                Some(v) => format!("{k}:{v}"),
                None => k.clone(),
            };
            (k.clone(), display)
        })
        .collect();

    rsx! {
        div { class: "space-y-2",
            h3 { class: "text-sm font-semibold text-gray-700", {t!("resources-labels-title")} }
            div { class: "flex flex-wrap gap-1.5",
                for (name, display) in entries {
                    span {
                        key: "{name}",
                        class: "inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs bg-blue-100 text-blue-800",
                        "{display}",
                        if can_write {
                            button {
                                class: "text-blue-500 hover:text-red-600 font-bold ml-1",
                                onclick: move |_| {
                                    labels.with_mut(|m| { m.remove(&name); });
                                    dirty.set(true);
                                },
                                {"×"}
                            }
                        }
                    }
                }
            }
            if can_write {
                div { class: "flex gap-2",
                    input {
                        class: "flex-1 border border-gray-300 rounded-lg px-3 py-1.5 text-sm",
                        placeholder: t!("resources-label-input-placeholder"),
                        value: "{input}",
                        oninput: move |evt| input.set(evt.value()),
                        onkeydown: move |evt: KeyboardEvent| {
                            if evt.key() == Key::Enter {
                                let raw = input.take();
                                match parse_entry(&raw) {
                                    Ok((name, value)) => {
                                        labels.with_mut(|m| { m.insert(name, value); });
                                        dirty.set(true);
                                        error.set(String::new());
                                    }
                                    Err(msg) => toasts::error(msg),
                                }
                            }
                        },
                    }
                    button {
                        class: "px-3 py-1.5 rounded-lg text-sm font-medium bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50",
                        disabled: !dirty(),
                        onclick: move |_| {
                            let snapshot = labels.read().clone();
                            let kind = kind.clone();
                            let id = id.clone();
                            spawn(async move {
                                match api::resources::put_labels(&kind, &id, &snapshot).await {
                                    Ok(_) => {
                                        dirty.set(false);
                                        error.set(String::new());
                                        toasts::success(t!("resources-label-saved"));
                                        if let Some(cb) = on_changed { cb.call(()); }
                                    }
                                    Err(err) => toasts::error(format!("{err}")),
                                }
                            });
                        },
                        {t!("resources-label-save")}
                    }
                }
            }
            if !error.read().is_empty() {
                p { class: "text-xs text-red-600", "{error}" }
            }
        }
    }
}

/// Normalise `name:valeur` ou `name` (tag nu) — charset serveur re-vérifié.
fn parse_entry(raw: &str) -> Result<(String, Option<String>), String> {
    let raw = raw.trim();
    let (name, value) = match raw.split_once(':') {
        Some((n, v)) => (n.trim(), Some(v.trim().to_string())),
        None => (raw, None),
    };
    if name.is_empty() {
        return Err(t!("resources-label-name-required").to_string());
    }
    let name = name.to_lowercase();
    for c in name.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return Err(t!("resources-label-name-invalid").to_string());
        }
    }
    if let Some(v) = &value {
        if v.is_empty() || v.chars().count() > 255 {
            return Err(t!("resources-label-value-invalid").to_string());
        }
    }
    Ok((name, value))
}
