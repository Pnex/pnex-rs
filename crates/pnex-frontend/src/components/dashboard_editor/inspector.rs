//! Inspecteur — réglages du widget/trait sélectionné : titre, **binding
//! télémétrie** (métrique × appareil × fenêtre via le catalogue
//! `/telemetry/catalog` hissé dans `DashboardEditor`), unité/bornes/
//! décimales/texte. Le corps seul est rendu ici : la coquille
//! (`InspectorPanel`) fournit le panneau, l'en-tête et la fermeture.
//! Les modifications mutent le document via les reducers purs ; le
//! save (et sa validation) reste au bouton de la barre.

use std::collections::{BTreeMap, BTreeSet};

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{SourceRef, VIZ_WINDOW_PRESETS};

use super::appearance::AppearancePanel;
use super::control_panel::ControlPanel;
use super::home_panel::HomePanel;
use super::symbol_options::SymbolOptionsPanel;
use super::thermo_panel::ThermoPanel;
use crate::components::icons;

use super::state;
use super::{EditorCx, Selection};

/// Catalogue télémétrie **remodelé par source** — l'inspecteur fait
/// choisir la source (device réel ou device virtuel d'un flow) d'abord,
/// la métrique ensuite (liste filtrée à la source choisie). `flows`
/// porte les device_id virtuels (`pred_dev="virtual_device"`, slug
/// `flow_{id}`) pour séparer les deux groupes dans le select.
#[derive(Clone, Default, PartialEq)]
pub struct SourceCatalog {
    /// device_id → métriques publiées sur cette source.
    pub by_source: BTreeMap<String, Vec<String>>,
    /// Devices virtuels de flows (`flow_{id}`).
    pub flows: BTreeSet<String>,
    /// Catalogue chargé (ressource résolue) — faux pendant le fetch.
    pub ready: bool,
    /// Org shared memory live keys → numeric fields (`""` = scalar value),
    /// written by the flow `memory-write` nodes.
    pub memory: BTreeMap<String, Vec<String>>,
}

/// Corps de l'inspecteur — monté seulement sur sélection (principe 4).
/// `metrics` (métrique → devices) reste pour `ThermoPanel` ; `catalog`
/// est la vue par source pour le binding des widgets classiques.
#[component]
pub fn InspectorBody(
    cx: EditorCx,
    can_write: bool,
    metrics: BTreeMap<String, Vec<String>>,
    catalog: SourceCatalog,
) -> Element {
    let selected = cx.selected.cloned();
    rsx! {
        {
            match selected {
                None => rsx! {},
                Some(Selection::Wire(id)) => rsx! {
                    wire_panel { cx, wire_id: id }
                },
                Some(Selection::Widget(id)) => rsx! {
                    widget_panel {
                        cx,
                        widget_id: id,
                        can_write,
                        metrics,
                        catalog,
                    }
                },
            }
        }
    }
}

#[component]
fn wire_panel(mut cx: EditorCx, wire_id: String) -> Element {
    let action = format!("{} · {}", t!("db-tool-wire"), t!("common-delete"));
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", "{wire_id}" }
            button {
                class: "inline-flex items-center px-3 py-1.5 text-sm text-red-600 border border-red-200 rounded-lg hover:bg-red-50",
                onclick: move |_| {
                    let current = cx.layout.read().clone();
                    cx.history.with_mut(|h| h.push(&current));
                    cx.layout.with_mut(|l| state::delete_wire(l, &wire_id));
                    cx.selected.set(None);
                    crate::state::toasts::info(t!("db-wire-deleted").to_string());
                },
                "{action}"
            }
        }
    }
}

#[component]
fn widget_panel(
    mut cx: EditorCx,
    widget_id: String,
    can_write: bool,
    metrics: BTreeMap<String, Vec<String>>,
    catalog: SourceCatalog,
) -> Element {
    let w = state::find_widget(&cx.layout.read(), &widget_id);
    let Some(w) = w else {
        return rsx! {
            p { class: "text-xs text-gray-400", {t!("insp-no-selection")} }
        };
    };

    let primary = w.source.first().cloned().unwrap_or(SourceRef {
        role: "primary".into(),
        metric: String::new(),
        device_id: String::new(),
        window: "1h".into(),
        memory: None,
    });
    // Memory source: the "metric" select lists the numeric fields of the
    // key (the stored field stays visible even when not live anymore).
    let memory_fields: Vec<String> = primary
        .memory
        .as_ref()
        .map(|m| {
            let mut fields = catalog.memory.get(&m.key).cloned().unwrap_or_default();
            if !fields.contains(&m.field) {
                fields.push(m.field.clone());
            }
            fields
        })
        .unwrap_or_default();
    let memory_key = primary.memory.as_ref().map(|m| m.key.clone());
    let memory_field = primary
        .memory
        .as_ref()
        .map(|m| m.field.clone())
        .unwrap_or_default();
    // Métriques de la source choisie ; la métrique enregistrée reste
    // visible même si le catalogue ne la connaît plus (série disparue)
    // — l'affichage reflète l'état, jamais l'inverse.
    let source_metrics: Vec<String> = catalog
        .by_source
        .get(&primary.device_id)
        .cloned()
        .unwrap_or_default();
    let metric_in_catalog = source_metrics.contains(&primary.metric);
    // Metric select is inert without write access, a device or a catalog.
    let metric_locked = !can_write || primary.device_id.is_empty() || source_metrics.is_empty();
    let is_control = pnex_core::CONTROL_WIDGET_TYPES.contains(&w.widget_type.as_str());
    let is_mobile = cx.layout.read().format == pnex_core::DashboardFormat::Mobile;
    // Copie pour la closure du select source (piège FnMut/Fn captures :
    // `catalog` sert aussi au rendu, jamais de move partagé).
    let cat_for_source = catalog.clone();

    rsx! {
        div { class: "space-y-3",
            // Titre
            field_label {
                label_key: "title",
                label: t!("insp-widget-title").to_string(),
            }
            input {
                id: "insp-title",
                class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                disabled: !can_write,
                value: "{w.title}",
                oninput: move |e| {
                    let v = e.value();
                    let Some(widget_id) = selected_widget_id(&cx) else { return };
                    cx.layout
                        .with_mut(|l| {
                            if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
                                w.title = v.clone();
                            }
                        });
                },
            }
            if is_mobile {
                SpanPicker { cx, widget: w.clone(), can_write }
            }
            if is_control {
                ControlPanel {
                    key: "{w.id}",
                    cx,
                    widget: w.clone(),
                    can_write,
                }
            }
            if w.widget_type == "symbol" {
                SymbolOptionsPanel { cx, widget: w.clone(), can_write }
            }
            if w.widget_type == pnex_core::home::HOME_WIDGET_TYPE {
                HomePanel {
                    cx,
                    widget: w.clone(),
                    can_write,
                    catalog: catalog.clone(),
                }
            }
            // thermo_chart : panneau dédié, pas de binding générique.
            if w.widget_type == "thermo_chart" {
                ThermoPanel {
                    cx,
                    widget_id: widget_id.clone(),
                    can_write,
                    metrics: metrics.clone(),
                    memory: catalog.memory.clone(),
                }
            }
            // Binding (sauf widget texte et thermo_chart) — la source
            // (device ou flow) se choisit d'abord ; la métrique est
            // filtrée à la source et verrouillée tant qu'elle n'est
            // pas choisie. Chaque changement **s'écrit** dans la
            // source : plus de valeur affichée qui ne serait pas dans
            // l'état (l'ancien fallback `eff_*` produisait des saves
            // en « device_id invalide : « » »).
            // A static symbol (no source) has no binding either.
            if w.widget_type != "text" && w.widget_type != "thermo_chart"
                && w.widget_type != pnex_core::home::HOME_WIDGET_TYPE
                && !(w.widget_type == "symbol" && w.source.is_empty())
                && !(is_control && w.source.is_empty())
            {
                field_label { label_key: "source", label: t!("insp-source").to_string() }
                select {
                    id: "insp-source",
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                    disabled: !can_write,
                    onchange: move |e| {
                        let v = e.value();
                        let Some(widget_id) = selected_widget_id(&cx) else { return };
                        if let Some(key) = v.strip_prefix("mem:") {
                            let field = cat_for_source
                                .memory
                                .get(key)
                                .and_then(|fs| fs.first().cloned())
                                .unwrap_or_default();
                            let key = key.to_string();
                            cx.layout
                                .with_mut(|l| set_source(
                                    l,
                                    &widget_id,
                                    |s| {
                                        s.memory = Some(pnex_core::memory::MemoryRef {
                                            key,
                                            field,
                                        });
                                        s.device_id.clear();
                                        s.metric.clear();
                                    },
                                ));
                            return;
                        }
                        let first_metric = cat_for_source
                            .by_source
                            .get(&v)
                            .and_then(|ms| ms.first().cloned())
                            .unwrap_or_default();
                        cx.layout
                            .with_mut(|l| set_source(
                                l,
                                &widget_id,
                                |s| {
                                    s.memory = None;
                                    s.device_id = v.clone();
                                    s.metric = first_metric.clone();
                                },
                            ));
                    },
                    if primary.device_id.is_empty() && memory_key.is_none() {
                        option { value: "", selected: true, disabled: true, {t!("insp-pick-source")} }
                    }
                    if !devices_of(&catalog, &primary.device_id).is_empty() {
                        optgroup { label: t!("insp-devices").to_string(),
                            for d in devices_of(&catalog, &primary.device_id) {
                                option {
                                    key: "{d}",
                                    value: "{d}",
                                    selected: primary.device_id == d,
                                    "{d}"
                                }
                            }
                        }
                    }
                    if !flows_of(&catalog).is_empty() {
                        optgroup { label: t!("insp-flows").to_string(),
                            for d in flows_of(&catalog) {
                                option {
                                    key: "flow-{d}",
                                    value: "{d}",
                                    selected: primary.device_id == d,
                                    "{d}"
                                }
                            }
                        }
                    }
                    if !catalog.memory.is_empty() || memory_key.is_some() {
                        optgroup { label: t!("insp-memory").to_string(),
                            for k in memory_keys_of(&catalog, memory_key.as_deref()) {
                                option {
                                    key: "mem-{k}",
                                    value: "mem:{k}",
                                    selected: memory_key.as_deref() == Some(k.as_str()),
                                    "{k}"
                                }
                            }
                        }
                    }
                }
                if memory_key.is_some() {
                    field_label {
                        label_key: "field",
                        label: t!("insp-memory-field").to_string(),
                    }
                    select {
                        id: "insp-field",
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                        disabled: !can_write,
                        onchange: move |e| {
                            let v = e.value();
                            let Some(widget_id) = selected_widget_id(&cx) else { return };
                            cx.layout
                                .with_mut(|l| set_source(
                                    l,
                                    &widget_id,
                                    |s| {
                                        if let Some(m) = s.memory.as_mut() {
                                            m.field = v.clone();
                                        }
                                    },
                                ));
                        },
                        for f in memory_fields.clone() {
                            option {
                                key: "{f}",
                                value: "{f}",
                                selected: memory_field == f,
                                if f.is_empty() {
                                    {t!("insp-memory-whole-value")}
                                } else {
                                    "{f}"
                                }
                            }
                        }
                    }
                    p { class: "text-[10px] text-gray-400 mt-1", {t!("insp-memory-help")} }
                } else {
                    field_label {
                        label_key: "metric",
                        label: t!("insp-metric").to_string(),
                    }
                    select {
                        id: "insp-metric",
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                        disabled: metric_locked,
                        onchange: move |e| {
                            let v = e.value();
                            let Some(widget_id) = selected_widget_id(&cx) else { return };
                            cx.layout.with_mut(|l| set_source(l, &widget_id, |s| s.metric = v.clone()));
                        },
                        if primary.device_id.is_empty() {
                            option { value: "", selected: true, disabled: true,
                                {t!("insp-pick-source-metric")}
                            }
                        }
                        for m in &source_metrics {
                            option {
                                key: "{m}",
                                value: "{m}",
                                selected: metric_in_catalog && primary.metric == *m,
                                "{m}"
                            }
                        }
                        if !primary.metric.is_empty() && !metric_in_catalog {
                            option {
                                key: "stale-{primary.metric}",
                                value: "{primary.metric}",
                                selected: true,
                                "{primary.metric}"
                            }
                        }
                    }
                    // Série enregistrée absente du catalogue (source
                    // disparue depuis le save) : affichée telle quelle.
                    if catalog.ready && catalog.by_source.is_empty() {
                        p { class: "text-[10px] text-gray-400 mt-1", {t!("insp-no-series")} }
                    }
                    field_label {
                        label_key: "window",
                        label: t!("insp-window").to_string(),
                    }
                    select {
                        id: "insp-window",
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                        disabled: !can_write,
                        value: "{primary.window}",
                        onchange: move |e| {
                            let v = e.value();
                            let Some(widget_id) = selected_widget_id(&cx) else { return };
                            cx.layout.with_mut(|l| set_source(l, &widget_id, |s| s.window = v.clone()));
                        },
                        for (key, _) in VIZ_WINDOW_PRESETS {
                            option { key: "{key}", value: "{key}", "{key}" }
                        }
                    }
                }
            }
            // Options par type
            // Empty branches below: thermo_chart options live in ThermoPanel
            // (above), a static symbol's in SymbolOptionsPanel. Comments stay
            // out of empty rsx branches: dx fmt drops them.
            if w.widget_type == "text" {
                field_label { label_key: "text", label: t!("insp-text").to_string() }
                textarea {
                    id: "insp-text",
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                    rows: "3",
                    disabled: !can_write,
                    oninput: move |e| {
                        let v = e.value();
                        let Some(widget_id) = selected_widget_id(&cx) else { return };
                        cx.layout
                            .with_mut(|l| {
                                if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
                                    w.options.text = Some(v.clone());
                                }
                            });
                    },
                    "{w.options.text.clone().unwrap_or_default()}"
                }
            } else if w.widget_type == "thermo_chart" {

            } else if w.widget_type == "symbol" && w.source.is_empty() {

            } else if is_control {

            } else {
                div { class: "grid grid-cols-2 gap-2",
                    div {
                        field_label {
                            label_key: "unit",
                            label: t!("insp-unit").to_string(),
                        }
                        input {
                            id: "insp-unit",
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                            disabled: !can_write,
                            value: "{w.options.unit.clone().unwrap_or_default()}",
                            oninput: move |e| {
                                let v = e.value();
                                let Some(widget_id) = selected_widget_id(&cx) else { return };
                                cx.layout
                                    .with_mut(|l| {
                                        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
                                            w.options.unit = Some(v.clone());
                                        }
                                    });
                            },
                        }
                    }
                    div {
                        field_label {
                            label_key: "decimals",
                            label: t!("insp-decimals").to_string(),
                        }
                        input {
                            id: "insp-decimals",
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                            disabled: !can_write,
                            "type": "number",
                            min: "0",
                            max: "6",
                            value: "{w.options.decimals.unwrap_or(1)}",
                            onchange: move |e| {
                                let v = e.value().parse::<u8>().unwrap_or(1).min(6);
                                let Some(widget_id) = selected_widget_id(&cx) else { return };
                                cx.layout
                                    .with_mut(|l| {
                                        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
                                            w.options.decimals = Some(v);
                                        }
                                    });
                            },
                        }
                    }
                    div {
                        field_label {
                            label_key: "min",
                            label: t!("insp-min").to_string(),
                        }
                        input {
                            id: "insp-min",
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                            disabled: !can_write,
                            "type": "number",
                            step: "any",
                            value: "{w.options.min.map(|v| v.to_string()).unwrap_or_default()}",
                            onchange: move |e| {
                                let v = e.value().parse::<f64>().ok();
                                let Some(widget_id) = selected_widget_id(&cx) else { return };
                                cx.layout
                                    .with_mut(|l| {
                                        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
                                            w.options.min = v;
                                        }
                                    });
                            },
                        }
                    }
                    div {
                        field_label {
                            label_key: "max",
                            label: t!("insp-max").to_string(),
                        }
                        input {
                            id: "insp-max",
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                            disabled: !can_write,
                            "type": "number",
                            step: "any",
                            value: "{w.options.max.map(|v| v.to_string()).unwrap_or_default()}",
                            onchange: move |e| {
                                let v = e.value().parse::<f64>().ok();
                                let Some(widget_id) = selected_widget_id(&cx) else { return };
                                cx.layout
                                    .with_mut(|l| {
                                        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
                                            w.options.max = v;
                                        }
                                    });
                            },
                        }
                    }
                }
            }

            AppearancePanel { cx, widget: w.clone(), can_write }
            div { class: "pt-2 flex flex-col gap-2 border-t border-gray-100",
                button {
                    class: if can_write { "inline-flex items-center justify-center px-3 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50" } else { "hidden" },
                    onclick: move |_| {
                        let Some(widget_id) = selected_widget_id(&cx) else { return };
                        cx.save_as.set(Some(widget_id));
                    },
                    icons::Plus { class: "h-4 w-4 mr-1" }
                    {t!("insp-to-library")}
                }
                button {
                    class: if can_write { "inline-flex items-center justify-center px-3 py-1.5 text-sm text-red-600 border border-red-200 rounded-lg hover:bg-red-50" } else { "hidden" },
                    onclick: move |_| {
                        let Some(widget_id) = selected_widget_id(&cx) else { return };
                        let current = cx.layout.read().clone();
                        cx.history.with_mut(|h| h.push(&current));
                        cx.layout.with_mut(|l| state::delete_widget(l, &widget_id));
                        cx.selected.set(None);
                    },
                    icons::Trash { class: "h-4 w-4 mr-1" }
                    {t!("insp-delete")}
                }
            }
        }
    }
}

/// Mobile card width (D124): half or full row.
#[component]
fn SpanPicker(mut cx: EditorCx, widget: pnex_core::Widget, can_write: bool) -> Element {
    let half = widget.options.span == Some(1);
    let (id_half, id_full) = (widget.id.clone(), widget.id.clone());
    let on = "flex-1 rounded px-2 py-1 text-xs font-medium bg-blue-600 text-white";
    let off = "flex-1 rounded px-2 py-1 text-xs font-medium text-gray-600 hover:bg-gray-100";
    rsx! {
        field_label { label_key: "span", label: t!("insp-span").to_string() }
        div {
            id: "insp-span",
            class: "flex gap-1 rounded border border-gray-200 p-0.5",
            button {
                r#type: "button",
                class: if half { on } else { off },
                disabled: !can_write,
                onclick: move |_| {
                    let current = cx.layout.read().clone();
                    cx.history.with_mut(|h| h.push(&current));
                    cx.layout.with_mut(|l| state::set_span(l, &id_half, 1));
                },
                {t!("db-card-half")}
            }
            button {
                r#type: "button",
                class: if half { off } else { on },
                disabled: !can_write,
                onclick: move |_| {
                    let current = cx.layout.read().clone();
                    cx.history.with_mut(|h| h.push(&current));
                    cx.layout.with_mut(|l| state::set_span(l, &id_full, 2));
                },
                {t!("db-card-full")}
            }
        }
    }
}

/// Label de champ (petite aide au-dessus des inputs).
#[component]
fn field_label(label_key: String, label: String) -> Element {
    rsx! {
        label {
            key: "{label_key}",
            // Pairs with the control's `insp-<key>` id (accessible name).
            r#for: "insp-{label_key}",
            class: "block text-[10px] font-medium text-gray-400 uppercase mt-1",
            "{label}"
        }
    }
}

/// Sources réelles (hors devices virtuels de flows), triées — plus the
/// stored device when it published nothing in the catalog window (kept
/// visible and selected, like a stale memory key).
fn devices_of(catalog: &SourceCatalog, current: &str) -> Vec<String> {
    let mut devices: Vec<String> = catalog
        .by_source
        .keys()
        .filter(|d| !catalog.flows.contains(*d))
        .cloned()
        .collect();
    if !current.is_empty() && !current.starts_with("flow_") && !devices.iter().any(|d| d == current)
    {
        devices.push(current.to_string());
    }
    devices
}

/// Live memory keys that expose at least one numeric field, plus the
/// stored key when it is not live anymore (kept visible and selected).
fn memory_keys_of(catalog: &SourceCatalog, current: Option<&str>) -> Vec<String> {
    let mut keys: Vec<String> = catalog
        .memory
        .iter()
        .filter(|(_, fields)| !fields.is_empty())
        .map(|(k, _)| k.clone())
        .collect();
    if let Some(k) = current {
        if !keys.iter().any(|x| x == k) {
            keys.push(k.to_string());
        }
    }
    keys
}

/// Sources virtuelles de flows (`flow_{id}`), triées.
fn flows_of(catalog: &SourceCatalog) -> Vec<String> {
    catalog
        .by_source
        .keys()
        .filter(|d| catalog.flows.contains(*d))
        .cloned()
        .collect()
}

/// L'id du widget sélectionné **relu du signal** : les closures des
/// handlers ne capturent jamais le String (piège FnMut/fncaptures,
/// école flow editor).
fn selected_widget_id(cx: &EditorCx) -> Option<String> {
    match cx.selected.cloned() {
        Some(Selection::Widget(id)) => Some(id),
        _ => None,
    }
}

/// Met à jour la source primaire du widget (crée la source si absente).
fn set_source(l: &mut pnex_core::DashboardLayout, widget_id: &str, f: impl FnOnce(&mut SourceRef)) {
    if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
        if w.source.is_empty() {
            w.source.push(SourceRef {
                role: "primary".into(),
                metric: String::new(),
                device_id: String::new(),
                window: "1h".into(),
                memory: None,
            });
        }
        f(&mut w.source[0]);
    }
}
