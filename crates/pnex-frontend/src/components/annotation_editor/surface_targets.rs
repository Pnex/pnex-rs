//! Inspector editors of the surface targets (D128/D129, D131): the control
//! of a `control` item (its own source, or a link to an existing control), the source of a `reading` item
//! (device or flow telemetry, org memory) with its mini chart (value,
//! sparkline, gauge, indicator).

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::memory::MemoryRef;
use pnex_core::ui_control::{ControlKind, UiControl, ORIGIN_ANNOTATION};
use pnex_core::{
    reading_display, AnnotationTarget, SourceRef, READING_DISPLAYS, VIZ_WINDOW_PRESETS,
};
use uuid::Uuid;

use crate::api;
use crate::components::annotation_editor::state::AnnotationEditorCx;
use crate::components::surface::{control_display_name, kind_text, spec_summary};

/// Replaces the target of item `id`.
fn set_target(cx: AnnotationEditorCx, id: &str, target: AnnotationTarget) {
    let id = id.to_owned();
    cx.update_doc(move |doc| {
        if let Some(it) = doc.items.iter_mut().find(|it| it.id == id) {
            it.target = target;
        }
    });
}

/// Reading target with mini chart `shape` (`READING_DISPLAYS`); `spark`
/// mirrors `line` for older readers of the document.
fn reading_target(
    source: SourceRef,
    shape: &str,
    min: Option<f64>,
    max: Option<f64>,
) -> AnnotationTarget {
    // A memory value has no history: never a sparkline.
    let shape = if source.memory.is_some() && shape == "line" {
        "stat"
    } else {
        shape
    };
    let gauge = shape == "gauge";
    AnnotationTarget::Reading {
        source,
        spark: shape == "line",
        display: Some(shape.to_string()),
        min: if gauge { min } else { None },
        max: if gauge { max } else { None },
    }
}

/// Fluent key of a mini chart choice.
fn display_label_key(shape: &str) -> &'static str {
    match shape {
        "line" => "annot-display-line",
        "gauge" => "annot-display-gauge",
        "indicator" => "annot-display-indicator",
        _ => "annot-display-stat",
    }
}

/// The control is the one this item declared (origin = this layer, this
/// item).
fn is_own(control: &UiControl, layer_id: &str, item_id: &str) -> bool {
    control.origin.as_ref().is_some_and(|o| {
        o.surface == ORIGIN_ANNOTATION
            && o.surface_id.to_string() == layer_id
            && o.item_id == item_id
    })
}

/// Control of a `control` item (D131): the item declares its own source
/// (kind picked here, provisioned by the server at save, listed in the flow
/// catalog under this layer), or links an existing control (advanced).
#[component]
pub fn ControlTargetEditor(
    cx: AnnotationEditorCx,
    item_id: String,
    current: Uuid,
    kind: Option<ControlKind>,
) -> Element {
    let controls = use_resource(move || async move {
        // Refetched after each save: provisioned controls appear then.
        let _ = cx.saved_version.read();
        api::controls::list().await.unwrap_or_default()
    });
    let all: Vec<UiControl> = controls.read().clone().unwrap_or_default();
    let loaded = controls.read().is_some();
    let layer_id = cx.layer_id.cloned().unwrap_or_default();
    let def = all.iter().find(|c| c.id == current).cloned();
    let own = def.as_ref().is_some_and(|d| is_own(d, &layer_id, &item_id));
    let pending = current.is_nil();
    let candidates: Vec<UiControl> = all
        .iter()
        .filter(|c| !is_own(c, &layer_id, &item_id))
        .cloned()
        .collect();
    let linked_id = if own || pending { None } else { Some(current) };
    let fallback_kind = kind
        .or(def.as_ref().map(|d| d.spec.kind))
        .unwrap_or(ControlKind::Switch);
    let (id_kind, id_link, id_own) = (item_id.clone(), item_id.clone(), item_id.clone());
    let reference = format!("#{item_id}");

    rsx! {
        div { class: "space-y-2 rounded-lg border border-teal-100 bg-teal-50/40 p-2",
            p { class: "text-[10px] font-medium uppercase text-gray-400", {t!("insp-control")} }
            if pending {
                div { class: "rounded border border-dashed border-teal-300 bg-white px-2 py-1.5 text-xs",
                    p { class: "font-mono text-teal-800", "{reference}" }
                    p { class: "text-gray-500", {t!("annot-source-pending")} }
                }
                label { class: "block text-xs font-medium text-gray-600",
                    {t!("annot-source-kind")}
                    select {
                        class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                        onchange: move |e| {
                            let picked = ControlKind::ALL.into_iter().find(|k| k.as_str() == e.value());
                            if let Some(k) = picked {
                                set_target(
                                    cx,
                                    &id_kind,
                                    AnnotationTarget::Control {
                                        control_id: Uuid::nil(),
                                        kind: Some(k),
                                    },
                                );
                            }
                        },
                        for k in ControlKind::ALL {
                            option {
                                key: "{k.as_str()}",
                                value: "{k.as_str()}",
                                selected: k == fallback_kind,
                                {kind_text(k)}
                            }
                        }
                    }
                }
            } else if let Some(d) = def.clone() {
                if own {
                    div { class: "rounded border border-teal-200 bg-white px-2 py-1.5 text-xs",
                        p { class: "font-mono text-teal-800", "{reference}" }
                        p { class: "text-gray-500", {kind_text(d.spec.kind)} }
                    }
                } else {
                    div { class: "rounded border border-indigo-200 bg-white px-2 py-1.5 text-xs space-y-1",
                        p { class: "text-indigo-800",
                            {t!("insp-source-shared", label : d.label.clone())}
                        }
                        button {
                            class: "text-[11px] text-indigo-700 underline",
                            onclick: move |_| {
                                set_target(
                                    cx,
                                    &id_own,
                                    AnnotationTarget::Control {
                                        control_id: Uuid::nil(),
                                        kind: Some(fallback_kind),
                                    },
                                );
                            },
                            {t!("insp-source-own")}
                        }
                    }
                }
                p { class: "text-[11px] text-gray-500",
                    code { "{d.key}" }
                    " · {spec_summary(&d.spec)}"
                }
                if d.listened_by.is_empty() {
                    p { class: "rounded bg-amber-50 px-2 py-1 text-[11px] text-amber-800",
                        {t!("controls-idle-help")}
                    }
                }
            } else if loaded {
                div { class: "rounded bg-red-50 px-2 py-1 text-xs text-red-700 space-y-1",
                    p { {t!("controls-missing")} }
                    button {
                        class: "text-[11px] underline",
                        onclick: move |_| {
                            set_target(
                                cx,
                                &id_own,
                                AnnotationTarget::Control {
                                    control_id: Uuid::nil(),
                                    kind: Some(fallback_kind),
                                },
                            );
                        },
                        {t!("insp-source-own")}
                    }
                }
            }
            details { class: "rounded border border-gray-200 bg-white p-2",
                summary { class: "cursor-pointer text-xs font-medium text-gray-600",
                    {t!("insp-source-link")}
                }
                div { class: "mt-2 space-y-1",
                    p { class: "text-[10px] text-gray-500", {t!("insp-source-link-help")} }
                    select {
                        class: "w-full rounded-lg border-gray-300 text-sm",
                        onchange: move |e| {
                            let Ok(id) = e.value().parse::<Uuid>() else { return };
                            set_target(
                                cx,
                                &id_link,
                                AnnotationTarget::Control {
                                    control_id: id,
                                    kind: None,
                                },
                            );
                        },
                        option {
                            value: "",
                            selected: linked_id.is_none(),
                            disabled: true,
                            {t!("insp-control-pick")}
                        }
                        for c in candidates {
                            option {
                                key: "{c.id}",
                                value: "{c.id}",
                                selected: linked_id == Some(c.id),
                                "{control_display_name(&c)} · {kind_text(c.spec.kind)}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn ReadingTargetEditor(
    cx: AnnotationEditorCx,
    item_id: String,
    source: SourceRef,
    spark: bool,
    display: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
) -> Element {
    let shape = reading_display(display.as_deref(), spark).to_string();
    let catalog = use_resource(|| async { api::telemetry::catalog().await.ok() });
    let memory = use_resource(|| async { api::memory::keys().await.unwrap_or_default() });
    // device id → metrics (devices and flow virtual devices alike).
    let by_source: BTreeMap<String, Vec<String>> = catalog
        .read()
        .clone()
        .flatten()
        .filter(|c| c.available)
        .map(|c| {
            let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for s in c.series {
                map.entry(s.device_id).or_default().push(s.metric);
            }
            map
        })
        .unwrap_or_default();
    let mem: BTreeMap<String, Vec<String>> = memory
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|k| !k.fields.is_empty())
        .map(|k| (k.key, k.fields))
        .collect();
    let is_memory = source.memory.is_some();
    let mem_key = source.memory.as_ref().map(|m| m.key.clone());
    let mem_field = source
        .memory
        .as_ref()
        .map(|m| m.field.clone())
        .unwrap_or_default();
    let metrics = by_source
        .get(&source.device_id)
        .cloned()
        .unwrap_or_default();
    let fields = mem_key
        .as_ref()
        .and_then(|k| mem.get(k).cloned())
        .unwrap_or_default();
    let (id_src, id_metric, id_field, id_spark, id_win, id_min, id_max) = (
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
    );
    let (src_m, src_f, src_s, src_w, src_lo, src_hi) = (
        source.clone(),
        source.clone(),
        source.clone(),
        source.clone(),
        source.clone(),
        source.clone(),
    );
    let (sh_src, sh_field, sh_metric, sh_win) =
        (shape.clone(), shape.clone(), shape.clone(), shape.clone());
    // Mini charts offered: a memory value has no history (no sparkline).
    let displays: Vec<&'static str> = READING_DISPLAYS
        .iter()
        .copied()
        .filter(|d| !(is_memory && *d == "line"))
        .collect();
    let min_text = min.map(|v| v.to_string()).unwrap_or_default();
    let max_text = max.map(|v| v.to_string()).unwrap_or_default();
    let (cat_src, mem_src) = (by_source.clone(), mem.clone());

    rsx! {
        label { class: "block text-xs font-medium text-gray-600",
            {t!("insp-source")}
            select {
                class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                onchange: move |e| {
                    let v = e.value();
                    let next = if let Some(key) = v.strip_prefix("mem:") {
                        SourceRef {
                            role: "primary".into(),
                            metric: String::new(),
                            device_id: String::new(),
                            window: "1h".into(),
                            memory: Some(MemoryRef {
                                key: key.to_string(),
                                field: mem_src
                                    .get(key)
                                    .and_then(|f| f.first().cloned())
                                    .unwrap_or_default(),
                            }),
                        }
                    } else { // A memory value has no history: no sparkline.
                        SourceRef {
                            role: "primary".into(),
                            metric: cat_src
                                .get(&v)
                                .and_then(|m| m.first().cloned())
                                .unwrap_or_default(),
                            device_id: v,
                            window: "1h".into(),
                            memory: None,
                        }
                    };
                    set_target(cx, &id_src, reading_target(next, &sh_src, min, max));
                },
                if source.device_id.is_empty() && !is_memory {
                    option { value: "", selected: true, disabled: true, {t!("insp-pick-source")} }
                }
                optgroup { label: t!("insp-devices").to_string(),
                    for d in by_source.keys().cloned() {
                        option {
                            key: "{d}",
                            value: "{d}",
                            selected: !is_memory && source.device_id == d,
                            "{d}"
                        }
                    }
                }
                if !mem.is_empty() {
                    optgroup { label: t!("insp-memory").to_string(),
                        for k in mem.keys().cloned() {
                            option {
                                key: "mem-{k}",
                                value: "mem:{k}",
                                selected: mem_key.as_deref() == Some(k.as_str()),
                                "{k}"
                            }
                        }
                    }
                }
            }
        }
        if is_memory {
            label { class: "block text-xs font-medium text-gray-600",
                {t!("insp-memory-field")}
                select {
                    class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                    onchange: move |e| {
                        let mut next = src_f.clone();
                        if let Some(m) = next.memory.as_mut() {
                            m.field = e.value();
                        }
                        set_target(cx, &id_field, reading_target(next, &sh_field, min, max));
                    },
                    for f in fields {
                        option {
                            key: "{f}",
                            value: "{f}",
                            selected: mem_field == f,
                            if f.is_empty() {
                                {t!("insp-memory-whole-value")}
                            } else {
                                "{f}"
                            }
                        }
                    }
                }
            }
        } else if !source.device_id.is_empty() {
            label { class: "block text-xs font-medium text-gray-600",
                {t!("insp-metric")}
                select {
                    class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                    onchange: move |e| {
                        let mut next = src_m.clone();
                        next.metric = e.value();
                        set_target(cx, &id_metric, reading_target(next, &sh_metric, min, max));
                    },
                    for m in metrics {
                        option {
                            key: "{m}",
                            value: "{m}",
                            selected: source.metric == m,
                            "{m}"
                        }
                    }
                }
            }
            if shape == "line" {
                label { class: "block text-xs font-medium text-gray-600",
                    {t!("insp-window")}
                    select {
                        class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                        onchange: move |e| {
                            let mut next = src_w.clone();
                            next.window = e.value();
                            set_target(cx, &id_win, reading_target(next, &sh_win, min, max));
                        },
                        for (key, _) in VIZ_WINDOW_PRESETS {
                            option {
                                key: "{key}",
                                value: "{key}",
                                selected: source.window == *key,
                                "{key}"
                            }
                        }
                    }
                }
            }
        }
        // Mini chart drawn on the media card.
        label { class: "block text-xs font-medium text-gray-600",
            {t!("annot-display")}
            select {
                class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                onchange: move |e| {
                    set_target(cx, &id_spark, reading_target(src_s.clone(), &e.value(), min, max));
                },
                for d in displays {
                    option { key: "{d}", value: "{d}", selected: shape == d,
                        {t!(display_label_key(d))}
                    }
                }
            }
        }
        if shape == "gauge" {
            div { class: "grid grid-cols-2 gap-2",
                label { class: "block text-xs font-medium text-gray-600",
                    {t!("annot-gauge-min")}
                    input {
                        r#type: "number",
                        class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                        placeholder: "0",
                        value: "{min_text}",
                        onchange: move |e| {
                            let lo = e.value().trim().parse::<f64>().ok();
                            set_target(cx, &id_min, reading_target(src_lo.clone(), "gauge", lo, max));
                        },
                    }
                }
                label { class: "block text-xs font-medium text-gray-600",
                    {t!("annot-gauge-max")}
                    input {
                        r#type: "number",
                        class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                        placeholder: "100",
                        value: "{max_text}",
                        onchange: move |e| {
                            let hi = e.value().trim().parse::<f64>().ok();
                            set_target(cx, &id_max, reading_target(src_hi.clone(), "gauge", min, hi));
                        },
                    }
                }
            }
        }
    }
}
