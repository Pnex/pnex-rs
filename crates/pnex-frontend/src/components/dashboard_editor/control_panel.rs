//! Inspector of a control card (D125, D131): the widget **declares** its
//! own control — provisioned by the server when the dashboard is saved and
//! listed in the flow node catalog under this dashboard — or links an
//! existing control (shared state across surfaces, advanced). Plus the
//! optional state source and "Create the flow" (a draft `control-source` →
//! `device-write`, parcours §1.3).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{ControlRef, UiControl, ORIGIN_DASHBOARD};
use pnex_core::{SourceRef, Widget};
use uuid::Uuid;

use super::EditorCx;
use crate::api;
use crate::components::surface::control::kind_of_widget;
use crate::components::surface::spec_editor::SpecFields;
use crate::components::surface::{create_flow_draft, spec_summary};

/// Applies one undoable change to the widget `id`.
fn patch_widget(mut cx: EditorCx, id: &str, f: impl FnOnce(&mut Widget)) {
    let current = cx.layout.read().clone();
    cx.history.with_mut(|h| h.push(&current));
    cx.layout.with_mut(|l| {
        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
            f(w);
        }
    });
}

/// The control is the one this widget declared (origin = this dashboard,
/// this widget).
fn is_own(control: &UiControl, dashboard_id: &str, widget_id: &str) -> bool {
    control.origin.as_ref().is_some_and(|o| {
        o.surface == ORIGIN_DASHBOARD
            && o.surface_id.to_string() == dashboard_id
            && o.item_id == widget_id
    })
}

#[component]
pub(super) fn ControlPanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let kind = kind_of_widget(&widget.widget_type);
    let controls = use_resource(move || async move {
        // Refetched after each save: provisioned controls appear then.
        let _ = cx.saved_version.read();
        api::controls::list().await.unwrap_or_default()
    });
    let Some(kind) = kind else {
        return rsx! {};
    };
    let all: Vec<UiControl> = controls.read().clone().unwrap_or_default();
    let current = widget.options.control.as_ref().map(|c| c.control_id);
    let def = current.and_then(|id| all.iter().find(|c| c.id == id).cloned());
    let dashboard_id = cx.dashboard_id.read().clone();
    let own = def
        .as_ref()
        .is_some_and(|d| is_own(d, &dashboard_id, &widget.id));
    // Link candidates: same kind, never this widget's own control.
    let candidates: Vec<UiControl> = all
        .iter()
        .filter(|c| c.spec.kind == kind && !is_own(c, &dashboard_id, &widget.id))
        .cloned()
        .collect();
    let linked_id = if own { None } else { current };
    let wid_link = widget.id.clone();
    let wid_own = widget.id.clone();
    let wid_state = widget.id.clone();
    let wid_spec = widget.id.clone();
    let has_state = !widget.source.is_empty();
    // Domain declared by the widget (D137): its own choice, else what its
    // own control holds, else the default of the kind.
    let declared = widget
        .options
        .control_spec
        .clone()
        .or_else(|| def.as_ref().filter(|_| own).map(|d| d.spec.clone()))
        .unwrap_or_else(|| pnex_core::ui_control::ControlSpec::new(kind));
    let declared_error = declared
        .check()
        .map(|(code, _)| crate::components::surface::spec_editor::spec_error_text(code));
    let shared = current.is_some() && !own && def.is_some();
    let dirty = *cx.layout.read() != *cx.saved_layout.read();
    let reference = format!("{} › #{}", cx.name.read(), widget.id);

    rsx! {
        div { class: "space-y-2 rounded-lg border border-teal-100 bg-teal-50/40 p-2",
            p { class: "block text-[10px] font-medium text-gray-400 uppercase",
                {t!("insp-control")}
            }
            if current.is_none() {
                div { class: "rounded border border-dashed border-teal-300 bg-white px-2 py-1.5 text-xs",
                    p { class: "font-mono text-teal-800", "{reference}" }
                    p { class: "text-gray-500", {t!("insp-source-pending")} }
                }
            } else if let Some(d) = def.clone() {
                if own {
                    div { class: "rounded border border-teal-200 bg-white px-2 py-1.5 text-xs",
                        p { class: "font-mono text-teal-800", "{reference}" }
                    }
                } else {
                    div { class: "rounded border border-indigo-200 bg-white px-2 py-1.5 text-xs space-y-1",
                        p { class: "text-indigo-800",
                            {t!("insp-source-shared", label : d.label.clone())}
                        }
                        if can_write {
                            button {
                                class: "text-[11px] text-indigo-700 underline",
                                onclick: move |_| {
                                    patch_widget(cx, &wid_own, |w| w.options.control = None);
                                },
                                {t!("insp-source-own")}
                            }
                        }
                    }
                }
                ControlInfo { control: d }
            } else if controls.read().is_some() {
                p { class: "rounded bg-red-50 px-2 py-1 text-xs text-red-700",
                    {t!("controls-missing")}
                }
            }
            if can_write {
                details { class: "rounded border border-gray-200 bg-white p-2",
                    summary { class: "cursor-pointer text-xs font-medium text-gray-600",
                        {t!("insp-source-link")}
                    }
                    div { class: "mt-2 space-y-1",
                        p { class: "text-[10px] text-gray-500", {t!("insp-source-link-help")} }
                        select {
                            id: "insp-control",
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                            onchange: move |e| {
                                let Ok(id) = e.value().parse::<Uuid>() else { return };
                                patch_widget(
                                    cx,
                                    &wid_link,
                                    |w| w.options.control = Some(ControlRef { control_id: id }),
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
                                    {crate::components::surface::control_display_name(&c)}
                                }
                            }
                        }
                    }
                }
            }
            details {
                class: "rounded border border-gray-200 bg-white p-2",
                open: kind.has_options(),
                summary { class: "cursor-pointer text-xs font-medium text-gray-600",
                    {t!("insp-control-domain")}
                }
                div { class: "mt-2 space-y-1",
                    if shared {
                        p { class: "text-[10px] text-gray-500", {t!("insp-control-domain-shared")} }
                    } else {
                        SpecFields {
                            spec: declared,
                            disabled: !can_write,
                            on_change: move |next: pnex_core::ui_control::ControlSpec| {
                                patch_widget(cx, &wid_spec, |w| w.options.control_spec = Some(next));
                            },
                        }
                        if let Some(err) = declared_error {
                            p { class: "text-xs text-red-600", "{err}" }
                        }
                    }
                }
            }
            label { class: "flex items-center gap-2 text-xs text-gray-700",
                input {
                    r#type: "checkbox",
                    checked: has_state,
                    disabled: !can_write,
                    onchange: move |e| {
                        let on = e.checked();
                        patch_widget(
                            cx,
                            &wid_state,
                            |w| {
                                if on && w.source.is_empty() {
                                    w.source
                                        .push(SourceRef {
                                            role: "state".into(),
                                            metric: String::new(),
                                            device_id: String::new(),
                                            window: "1h".into(),
                                            memory: None,
                                        });
                                } else if !on {
                                    w.source.clear();
                                }
                            },
                        );
                    },
                }
                {t!("insp-control-state")}
            }
            if let Some(d) = def {
                if can_write {
                    FlowDraft { control: d, dirty }
                }
            }
        }
    }
}

/// Key, domain and listeners of the bound control.
#[component]
fn ControlInfo(control: UiControl) -> Element {
    let domain = spec_summary(&control.spec);
    let names: Vec<String> = control
        .listened_by
        .iter()
        .map(|l| l.flow_name.clone())
        .collect();
    rsx! {
        div { class: "space-y-1 text-xs",
            p { class: "text-gray-500",
                code { "{control.key}" }
                " · {domain}"
            }
            if names.is_empty() {
                p { class: "rounded bg-amber-50 px-2 py-1 text-amber-800",
                    {t!("controls-idle-help")}
                }
            } else {
                p { class: "text-gray-600", {t!("insp-control-listened", flows : names.join(", "))} }
            }
        }
    }
}

/// "Create the flow": pick the device and output pin, then a draft
/// `control-source` → `device-write` opens in the flow editor.
#[component]
pub(super) fn FlowDraft(control: UiControl, dirty: bool) -> Element {
    let devices = use_resource(|| async {
        api::devices::list(&api::devices::DeviceFilters {
            active: Some(true),
            limit: Some(200),
            ..Default::default()
        })
        .await
        .map(|p| p.results)
        .unwrap_or_default()
    });
    let mut device = use_signal(|| None::<(i64, String)>);
    let mut pin = use_signal(|| None::<String>);
    let pins = use_resource(move || {
        let pk = device().map(|d| d.0);
        async move {
            let Some(pk) = pk else {
                return Vec::new();
            };
            api::pins::pinout(pk)
                .await
                .map(|p| p.pins)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| matches!(p.mode.as_deref(), Some("digital_out" | "pwm_out")))
                .collect::<Vec<_>>()
        }
    });
    let list = devices.read().clone().unwrap_or_default();
    let pin_list = pins.read().clone().unwrap_or_default();
    let ready = device().is_some() && pin().is_some() && !dirty;

    rsx! {
        details { class: "rounded border border-gray-200 bg-white p-2",
            summary { class: "cursor-pointer text-xs font-medium text-teal-700",
                {t!("insp-control-flow")}
            }
            div { class: "mt-2 space-y-1",
                p { class: "text-[10px] text-gray-500", {t!("insp-control-flow-help")} }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                    onchange: move |e| {
                        let v = e.value();
                        let found = devices
                            .read()
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .find(|d| d.device_id == v)
                            .map(|d| (d.id, d.device_id));
                        device.set(found);
                        pin.set(None);
                    },
                    option {
                        value: "",
                        selected: device().is_none(),
                        disabled: true,
                        {t!("insp-control-flow-device")}
                    }
                    for d in list {
                        option { key: "{d.id}", value: "{d.device_id}", "{d.device_id}" }
                    }
                }
                if device().is_some() {
                    select {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                        onchange: move |e| pin.set(Some(e.value())),
                        option {
                            value: "",
                            selected: pin().is_none(),
                            disabled: true,
                            {t!("insp-control-flow-pin")}
                        }
                        for p in pin_list {
                            option { key: "{p.label}", value: "{p.label}",
                                "{p.label} ({p.mode.clone().unwrap_or_default()})"
                            }
                        }
                    }
                }
                if dirty {
                    p { class: "text-[10px] text-amber-700", {t!("insp-control-flow-save-first")} }
                }
                button {
                    class: "w-full rounded bg-teal-600 px-2 py-1 text-xs font-medium text-white disabled:opacity-40",
                    disabled: !ready,
                    onclick: move |_| {
                        if let (Some((_, slug)), Some(p)) = (device(), pin()) {
                            create_flow_draft(&control, &slug, &p);
                        }
                    },
                    {t!("insp-control-flow-create")}
                }
            }
        }
    }
}
