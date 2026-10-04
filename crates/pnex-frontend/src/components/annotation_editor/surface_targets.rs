//! Inspector editors of the surface targets (D128/D129): the org control of
//! a `control` item (pick or create inline), the source of a `reading` item
//! (device or flow telemetry, org memory) with its optional sparkline.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::memory::MemoryRef;
use pnex_core::ui_control::{ControlKind, ControlSpec, CreateUiControl, UiControl};
use pnex_core::{AnnotationTarget, SourceRef, VIZ_WINDOW_PRESETS};
use uuid::Uuid;

use crate::api;
use crate::components::annotation_editor::state::AnnotationEditorCx;
use crate::components::surface::{kind_text, spec_summary, suggest_key};
use crate::state::toasts;

/// Replaces the target of item `id`.
fn set_target(cx: AnnotationEditorCx, id: &str, target: AnnotationTarget) {
    let id = id.to_owned();
    cx.update_doc(move |doc| {
        if let Some(it) = doc.items.iter_mut().find(|it| it.id == id) {
            it.target = target;
        }
    });
}

#[component]
pub fn ControlTargetEditor(cx: AnnotationEditorCx, item_id: String, current: Uuid) -> Element {
    let mut reload = use_signal(|| 0u32);
    let controls = use_resource(move || async move {
        let _ = reload();
        api::controls::list().await.unwrap_or_default()
    });
    let all: Vec<UiControl> = controls.read().clone().unwrap_or_default();
    let def = all.iter().find(|c| c.id == current).cloned();
    let mut label = use_signal(String::new);
    let mut kind = use_signal(|| ControlKind::Switch);
    let mut busy = use_signal(|| false);
    let (id_pick, id_new) = (item_id.clone(), item_id.clone());
    let key = suggest_key(&label());

    rsx! {
        label { class: "block text-xs font-medium text-gray-600",
            {t!("insp-control")}
            select {
                class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                onchange: move |e| {
                    if let Ok(id) = e.value().parse::<Uuid>() {
                        set_target(
                            cx,
                            &id_pick,
                            AnnotationTarget::Control {
                                control_id: id,
                            },
                        );
                    }
                },
                if def.is_none() {
                    option { value: "", selected: true, disabled: true, {t!("insp-control-pick")} }
                }
                for c in all.clone() {
                    option {
                        key: "{c.id}",
                        value: "{c.id}",
                        selected: c.id == current,
                        "{c.label} · {kind_text(c.spec.kind)}"
                    }
                }
            }
        }
        if let Some(d) = def {
            p { class: "text-[11px] text-gray-500",
                code { "{d.key}" }
                " · {spec_summary(&d.spec)}"
            }
            if d.listened_by.is_empty() {
                p { class: "rounded bg-amber-50 px-2 py-1 text-[11px] text-amber-800",
                    {t!("controls-idle-help")}
                }
            }
        }
        details { class: "rounded border border-gray-200 bg-white p-2",
            summary { class: "cursor-pointer text-xs font-medium text-gray-600",
                {t!("annot-control-create")}
            }
            div { class: "mt-2 space-y-1",
                select {
                    class: "w-full rounded-lg border-gray-300 text-sm",
                    onchange: move |e| {
                        if let Some(k) = ControlKind::ALL.into_iter().find(|k| k.as_str() == e.value()) {
                            kind.set(k);
                        }
                    },
                    for k in ControlKind::ALL {
                        option {
                            key: "{k.as_str()}",
                            value: "{k.as_str()}",
                            selected: k == kind(),
                            {kind_text(k)}
                        }
                    }
                }
                input {
                    class: "w-full rounded-lg border-gray-300 text-sm",
                    placeholder: t!("controls-label-placeholder").to_string(),
                    value: "{label}",
                    oninput: move |e| label.set(e.value()),
                }
                button {
                    class: "w-full rounded bg-gray-900 px-2 py-1 text-xs text-white disabled:opacity-40",
                    disabled: key.is_empty() || busy(),
                    onclick: move |_| {
                        let params = CreateUiControl {
                            key: suggest_key(&label.peek()),
                            label: label.peek().trim().to_string(),
                            spec: ControlSpec::new(*kind.peek()),
                        };
                        let id = id_new.clone();
                        busy.set(true);
                        spawn(async move {
                            match api::controls::create(params).await {
                                Ok(c) => {
                                    label.set(String::new());
                                    reload += 1;
                                    set_target(
                                        cx,
                                        &id,
                                        AnnotationTarget::Control {
                                            control_id: c.id,
                                        },
                                    );
                                }
                                Err(e) => toasts::error(e),
                            }
                            busy.set(false);
                        });
                    },
                    {t!("common-create")}
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
) -> Element {
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
    let (id_src, id_metric, id_field, id_spark, id_win) = (
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
        item_id.clone(),
    );
    let (src_m, src_f, src_s, src_w) = (
        source.clone(),
        source.clone(),
        source.clone(),
        source.clone(),
    );
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
                    let spark = spark && next.memory.is_none();
                    set_target(
                        cx,
                        &id_src,
                        AnnotationTarget::Reading {
                            source: next,
                            spark,
                        },
                    );
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
                        set_target(
                            cx,
                            &id_field,
                            AnnotationTarget::Reading {
                                source: next,
                                spark: false,
                            },
                        );
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
                        set_target(
                            cx,
                            &id_metric,
                            AnnotationTarget::Reading {
                                source: next,
                                spark,
                            },
                        );
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
            label { class: "flex items-center gap-2 text-xs text-gray-600",
                input {
                    r#type: "checkbox",
                    checked: spark,
                    onchange: move |e| {
                        let on = e.checked();
                        set_target(
                            cx,
                            &id_spark,
                            AnnotationTarget::Reading {
                                source: src_s.clone(),
                                spark: on,
                            },
                        );
                    },
                }
                {t!("annot-reading-spark")}
            }
            if spark {
                label { class: "block text-xs font-medium text-gray-600",
                    {t!("insp-window")}
                    select {
                        class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                        onchange: move |e| {
                            let mut next = src_w.clone();
                            next.window = e.value();
                            set_target(
                                cx,
                                &id_win,
                                AnnotationTarget::Reading {
                                    source: next,
                                    spark: true,
                                },
                            );
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
    }
}
