//! Éditeur de labels D42 (chips + input, validation charset serveur
//! re-vérifiée client) — générique sur (kind, id) : réutilisable tel quel
//! pour devices, dashboards, tours, POI, flows. École MetadataEditor
//! (pages/devices.rs) : état local, écriture explicite, jamais de panic.

use dioxus::prelude::*;
use std::collections::{BTreeMap, HashMap};

use crate::api::resources::LabelSet;

use crate::api;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::ui::BTN_SECONDARY_SM;
use crate::components::ui::{LabelTag, BTN_PRIMARY, INPUT};
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
    rsx! {
        div { class: "space-y-2",
            h3 { class: "text-sm font-semibold text-gray-700", {t!("resources-labels-title")} }
            LabelChipsInput {
                labels,
                can_write,
                on_edit: move |_| {
                    dirty.set(true);
                    error.set(String::new());
                },
            }
            if can_write {
                div { class: "flex justify-end",
                    button {
                        class: BTN_PRIMARY,
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
                                        if let Some(cb) = on_changed {
                                            cb.call(());
                                        }
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

/// Shared D42 label chips + input (no persistence): the single label
/// design used by [`LabelsEditor`] and by forms that collect labels before
/// the resource exists (device wizard). The caller owns the label set and
/// decides when to write it.
#[component]
pub fn LabelChipsInput(
    /// Label set edited in place (`name -> Some(value)` or bare tag).
    labels: Signal<BTreeMap<String, Option<String>>>,
    can_write: bool,
    /// Called after every local edit (add or remove).
    on_edit: Option<Callback<()>>,
) -> Element {
    let mut labels = labels;
    let mut input = use_signal(String::new);
    let entries: Vec<(String, Option<String>)> = labels
        .read()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    rsx! {
        div { class: "space-y-2",
            div { class: "flex flex-wrap gap-1.5",
                for (name, value) in entries {
                    LabelTag { key: "{name}", name: name.clone(), value,
                        if can_write {
                            button {
                                class: "border-l border-line-strong px-1.5 text-slate hover:bg-red-50 hover:text-alarm",
                                r#type: "button",
                                aria_label: t!("resources-label-remove", name : name.clone()),
                                onclick: move |_| {
                                    labels.write().remove(&name);
                                    if let Some(cb) = on_edit {
                                        cb.call(());
                                    }
                                },
                                {"×"}
                            }
                        }
                    }
                }
            }
            if can_write {
                LabelSuggestions { id: "pnex-labels-edit" }
                input {
                    class: INPUT,
                    list: "pnex-labels-edit",
                    placeholder: t!("resources-label-input-placeholder"),
                    value: "{input}",
                    oninput: move |evt| input.set(evt.value()),
                    onkeydown: move |evt: KeyboardEvent| {
                        if evt.key() == Key::Enter {
                            evt.prevent_default();
                            let raw = input.take();
                            match parse_entry(&raw) {
                                Ok((name, value)) => {
                                    labels.write().insert(name, value);
                                    if let Some(cb) = on_edit {
                                        cb.call(());
                                    }
                                }
                                Err(msg) => toasts::error(msg),
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Read-only label chips (list rows, cards). Renders nothing for an
/// empty set; chips wrap so they never widen their container.
#[component]
pub fn LabelChips(labels: LabelSet) -> Element {
    if labels.is_empty() {
        return rsx! {};
    }
    let entries: Vec<(String, Option<String>)> =
        labels.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    rsx! {
        div { class: "flex flex-wrap gap-1 mt-1 font-normal whitespace-normal",
            for (name, value) in entries {
                LabelTag { key: "{name}", name, value }
            }
        }
    }
}

/// Own labels of a page of list rows, fetched in one
/// `POST /resources/labels/batch` call (`kind` + stringified PKs).
/// `ids` runs in the synchronous part of the resource, so reading the list
/// resource there re-fetches the labels on every list (re)load. Any
/// failure yields an empty map: no chips, the list is never blocked.
pub fn use_row_labels(
    kind: &'static str,
    mut ids: impl FnMut() -> Vec<String> + 'static,
) -> HashMap<String, LabelSet> {
    let batch = use_resource(move || {
        let ids = ids();
        async move {
            // Server batches are capped: a long client-paged list is
            // fetched in successive chunks, never truncated.
            let mut all = HashMap::new();
            for chunk in ids.chunks(api::resources::LABELS_BATCH_MAX) {
                all.extend(
                    api::resources::labels_batch(kind, chunk.to_vec())
                        .await
                        .unwrap_or_default(),
                );
            }
            all
        }
    });
    let snapshot = batch.value().read().clone().unwrap_or_default();
    snapshot
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

/// Editor toolbar button opening the [`LabelsEditor`] in a modal: the one
/// "Labels" entry point of the canvas editors (flow, dashboard, tour),
/// styled like their "Versions" button.
#[component]
pub fn LabelsButton(
    /// Resource kind (`pnex_core::resources::KIND_*`).
    kind: String,
    /// Stringified PK.
    id: String,
    can_write: bool,
    /// Called after a save, so a list can reload its chips.
    on_changed: Option<Callback<()>>,
) -> Element {
    let mut open = use_signal(|| false);
    let title = t!("resources-labels-title").to_string();
    rsx! {
        button {
            class: BTN_SECONDARY_SM,
            r#type: "button",
            title: "{title}",
            aria_label: "{title}",
            onclick: move |_| open.set(true),
            icons::Tag { class: "h-4 w-4 sm:mr-1" }
            span { class: "hidden sm:inline", "{title}" }
        }
        if open() {
            Modal {
                title: title.clone(),
                max_width: "max-w-md".to_string(),
                on_close: move |_| open.set(false),
                LabelsEditor {
                    key: "{kind}-{id}",
                    kind: kind.clone(),
                    id: id.clone(),
                    can_write,
                    on_changed,
                }
            }
        }
    }
}

/// `<datalist>` of the org's labels (`name` and `name:value`), the
/// suggestions of every label input. Fetch failure = no suggestions.
#[component]
pub fn LabelSuggestions(id: String) -> Element {
    let catalog =
        use_resource(|| async { api::resources::labels_catalog().await.unwrap_or_default() });
    let options: Vec<String> = catalog
        .value()
        .read()
        .iter()
        .flatten()
        .flat_map(|e| {
            std::iter::once(e.name.clone())
                .chain(e.values.iter().map(|v| format!("{}:{v}", e.name)))
        })
        .collect();
    rsx! {
        datalist { id,
            for option in options {
                option { key: "{option}", value: "{option}" }
            }
        }
    }
}
