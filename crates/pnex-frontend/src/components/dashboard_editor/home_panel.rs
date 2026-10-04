//! Inspector of a home card (D138): variant, one source per role (device
//! metric or org memory, picked in one list), optional control roles to
//! toggle, and the value domain of each active role (`SpecFields`). The
//! server provisions one control per active role at save.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::home::{HomeCardOptions, BINARY_VARIANTS};
use pnex_core::memory::MemoryRef;
use pnex_core::ui_control::ControlSpec;
use pnex_core::{SourceRef, Widget, VIZ_WINDOW_PRESETS};

use super::inspector::SourceCatalog;
use super::{EditorCx, Selection};
use crate::components::surface::home_card::{card_label, role_label};
use crate::components::surface::spec_editor::{spec_error_text, SpecFields};

fn edit_widget(mut cx: EditorCx, f: impl FnOnce(&mut Widget)) {
    let Some(Selection::Widget(id)) = cx.selected.cloned() else {
        return;
    };
    let current = cx.layout.read().clone();
    cx.history.with_mut(|h| h.push(&current));
    cx.layout.with_mut(|l| {
        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
            f(w);
        }
    });
}

fn edit_home(cx: EditorCx, f: impl FnOnce(&mut HomeCardOptions)) {
    edit_widget(cx, |w| {
        if let Some(h) = w.options.home.as_mut() {
            f(h);
        }
    });
}

/// Encoded choice of the source list: `t|device|metric` or `m|key|field`.
fn encode(s: &SourceRef) -> String {
    match &s.memory {
        Some(m) => format!("m|{}|{}", m.key, m.field),
        None if s.device_id.is_empty() => String::new(),
        None => format!("t|{}|{}", s.device_id, s.metric),
    }
}

/// Applies an encoded choice to a source (window kept).
fn decode_into(v: &str, s: &mut SourceRef) {
    let mut parts = v.splitn(3, '|');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("t"), Some(device), Some(metric)) => {
            s.memory = None;
            s.device_id = device.to_string();
            s.metric = metric.to_string();
        }
        (Some("m"), Some(key), Some(field)) => {
            s.memory = Some(MemoryRef {
                key: key.to_string(),
                field: field.to_string(),
            });
            s.device_id.clear();
            s.metric.clear();
        }
        _ => {}
    }
}

fn variant_label(v: &str) -> String {
    match v {
        "door" => t!("hvar-door").to_string(),
        "window" => t!("hvar-window").to_string(),
        "motion" => t!("hvar-motion").to_string(),
        "presence" => t!("hvar-presence").to_string(),
        "smoke" => t!("hvar-smoke").to_string(),
        "leak" => t!("hvar-leak").to_string(),
        "co" => t!("hvar-co").to_string(),
        _ => t!("hvar-generic").to_string(),
    }
}

#[component]
pub fn HomePanel(cx: EditorCx, widget: Widget, can_write: bool, catalog: SourceCatalog) -> Element {
    let Some(home) = widget.options.home.clone() else {
        return rsx! {};
    };
    let card = home.card;
    let source_roles = card.source_roles();
    let control_roles = card.control_roles();
    let on_above = home.on_above.map(|v| v.to_string()).unwrap_or_default();
    rsx! {
        div { class: "space-y-3 rounded-lg border border-amber-100 bg-amber-50/40 p-2",
            p { class: "text-[10px] font-medium uppercase text-gray-400",
                {t!("hcard-inspector", card : card_label(card))}
            }
            if card == pnex_core::home::HomeCard::Binary {
                label { class: "block text-[10px] text-gray-500", {t!("hcard-variant")} }
                select {
                    class: "w-full rounded border border-gray-300 bg-white px-2 py-1.5 text-sm",
                    disabled: !can_write,
                    value: home.variant.clone().unwrap_or_default(),
                    onchange: move |e| {
                        let v = e.value();
                        edit_home(cx, |h| h.variant = Some(v));
                    },
                    for v in BINARY_VARIANTS {
                        option { key: "{v}", value: "{v}", {variant_label(v)} }
                    }
                }
                label { class: "block text-[10px] text-gray-500", {t!("hcard-on-above")} }
                input {
                    class: "w-full rounded border border-gray-300 px-2 py-1 text-sm",
                    inputmode: "decimal",
                    placeholder: "0.5",
                    disabled: !can_write,
                    value: "{on_above}",
                    onchange: move |e| {
                        let v = e.value().trim().replace(',', ".").parse::<f64>().ok();
                        edit_home(cx, |h| h.on_above = v);
                    },
                }
            }
            if !source_roles.is_empty() {
                p { class: "text-[10px] font-medium uppercase text-gray-400", {t!("hcard-sources")} }
                for r in source_roles.iter() {
                    RoleSource {
                        key: "src-{r.role}",
                        cx,
                        role: r.role,
                        required: r.required,
                        current: widget.source_of(r.role).cloned(),
                        catalog: catalog.clone(),
                        can_write,
                    }
                }
            }
            if !control_roles.is_empty() {
                p { class: "text-[10px] font-medium uppercase text-gray-400",
                    {t!("hcard-controls")}
                }
                for r in control_roles.iter() {
                    RoleControl {
                        key: "ctl-{r.role}",
                        cx,
                        widget_id: widget.id.clone(),
                        home: home.clone(),
                        role: r.role,
                        required: r.required,
                        can_write,
                    }
                }
                p { class: "text-[10px] text-gray-400", {t!("hcard-controls-help")} }
            }
        }
    }
}

#[component]
fn RoleSource(
    cx: EditorCx,
    role: &'static str,
    required: bool,
    current: Option<SourceRef>,
    catalog: SourceCatalog,
    can_write: bool,
) -> Element {
    let selected = current.as_ref().map(encode).unwrap_or_default();
    let window = current
        .as_ref()
        .map(|s| s.window.clone())
        .unwrap_or_else(|| "1h".into());
    let present = current.is_some();
    let devices: Vec<(String, Vec<String>)> = catalog
        .by_source
        .iter()
        .map(|(d, ms)| (d.clone(), ms.clone()))
        .collect();
    let memory: Vec<(String, Vec<String>)> = catalog
        .memory
        .iter()
        .filter(|(_, f)| !f.is_empty())
        .map(|(k, f)| (k.clone(), f.clone()))
        .collect();
    let telemetry = current.as_ref().is_some_and(|s| s.memory.is_none());
    rsx! {
        div { class: "space-y-1",
            div { class: "flex items-center justify-between",
                label { class: "text-xs text-gray-700", {role_label(role)} }
                if !required && present && can_write {
                    button {
                        class: "text-[10px] text-gray-400 hover:text-red-500",
                        onclick: move |_| edit_widget(cx, |w| w.source.retain(|s| s.role != role)),
                        {t!("common-delete")}
                    }
                }
            }
            select {
                class: "w-full rounded border border-gray-300 bg-white px-2 py-1.5 text-sm",
                disabled: !can_write,
                value: "{selected}",
                onchange: move |e| {
                    let v = e.value();
                    edit_widget(cx, |w| set_role_source(w, role, &v));
                },
                option {
                    value: "",
                    disabled: true,
                    selected: selected.is_empty(),
                    {t!("insp-pick-source")}
                }
                for (device, metrics) in devices {
                    optgroup { key: "{device}", label: "{device}",
                        for m in metrics {
                            option { key: "{m}", value: "t|{device}|{m}", "{m}" }
                        }
                    }
                }
                if !memory.is_empty() {
                    optgroup { label: t!("insp-memory").to_string(),
                        for (key, fields) in memory {
                            for f in fields {
                                option { key: "{key}#{f}", value: "m|{key}|{f}",
                                    if f.is_empty() {
                                        "{key}"
                                    } else {
                                        "{key} · {f}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if telemetry {
                select {
                    class: "w-full rounded border border-gray-300 bg-white px-2 py-1 text-xs",
                    disabled: !can_write,
                    value: "{window}",
                    onchange: move |e| {
                        let v = e.value();
                        edit_widget(
                            cx,
                            |w| {
                                if let Some(s) = w.source.iter_mut().find(|s| s.role == role) {
                                    s.window = v;
                                }
                            },
                        );
                    },
                    for (key, _) in VIZ_WINDOW_PRESETS {
                        option { key: "{key}", value: "{key}", "{key}" }
                    }
                }
            }
        }
    }
}

/// Points the source of `role` at an encoded choice (creates it if absent).
fn set_role_source(w: &mut Widget, role: &str, v: &str) {
    if !w.source.iter().any(|s| s.role == role) {
        w.source.push(SourceRef {
            role: role.to_string(),
            metric: String::new(),
            device_id: String::new(),
            window: "1h".into(),
            memory: None,
        });
    }
    if let Some(s) = w.source.iter_mut().find(|s| s.role == role) {
        decode_into(v, s);
    }
}

#[component]
fn RoleControl(
    cx: EditorCx,
    widget_id: String,
    home: HomeCardOptions,
    role: &'static str,
    required: bool,
    can_write: bool,
) -> Element {
    let active = required || home.specs.contains_key(role);
    let spec = home.spec_of(role).unwrap_or_else(|| {
        ControlSpec::new(
            home.card
                .control_role(role)
                .map(|r| r.kind)
                .unwrap_or(pnex_core::ui_control::ControlKind::Switch),
        )
    });
    let error = spec.check().map(|(code, _)| spec_error_text(code));
    let reference = format!("#{widget_id}.{role}");
    let card = home.card;
    rsx! {
        div { class: "space-y-1 rounded border border-gray-200 bg-white p-2",
            label { class: "flex items-center gap-2 text-xs text-gray-700",
                if !required {
                    input {
                        r#type: "checkbox",
                        checked: active,
                        disabled: !can_write,
                        onchange: move |e| {
                            let on = e.checked();
                            edit_home(
                                cx,
                                |h| {
                                    if on {
                                        if let Some(s) = card.default_spec(role) {
                                            h.specs.insert(role.to_string(), s);
                                        }
                                    } else {
                                        h.specs.remove(role);
                                        h.controls.remove(role);
                                    }
                                },
                            );
                        },
                    }
                }
                span { class: "font-medium", {role_label(role)} }
                span { class: "ml-auto font-mono text-[10px] text-gray-400", "{reference}" }
            }
            if active {
                details {
                    summary { class: "cursor-pointer text-[11px] text-gray-500",
                        {t!("insp-control-domain")}
                    }
                    div { class: "mt-1",
                        SpecFields {
                            spec,
                            disabled: !can_write,
                            on_change: move |next: ControlSpec| {
                                edit_home(
                                    cx,
                                    |h| {
                                        h.specs.insert(role.to_string(), next);
                                    },
                                );
                            },
                        }
                    }
                }
                if let Some(err) = error {
                    p { class: "text-xs text-red-600", "{err}" }
                }
            }
        }
    }
}
