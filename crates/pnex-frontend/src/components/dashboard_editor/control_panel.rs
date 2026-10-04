//! Inspector of a control card (D125): the org control it drives (pick or
//! create inline), the optional state source, and "Create the flow" (a
//! draft `control-source` → `device-write`, parcours §1.3).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{ControlRef, ControlSpec, CreateUiControl, UiControl};
use pnex_core::{SourceRef, Widget};
use uuid::Uuid;

use super::EditorCx;
use crate::api;
use crate::components::surface::control::kind_of_widget;
use crate::components::surface::{create_flow_draft, spec_summary, suggest_key};
use crate::state::toasts;

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

#[component]
pub(super) fn ControlPanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let kind = kind_of_widget(&widget.widget_type);
    let mut reload = use_signal(|| 0u32);
    let controls = use_resource(move || async move {
        let _ = reload();
        api::controls::list().await.unwrap_or_default()
    });
    let Some(kind) = kind else {
        return rsx! {};
    };
    let all: Vec<UiControl> = controls.read().clone().unwrap_or_default();
    let candidates: Vec<UiControl> = all
        .iter()
        .filter(|c| c.spec.kind == kind)
        .cloned()
        .collect();
    let current = widget.options.control.as_ref().map(|c| c.control_id);
    let def = current.and_then(|id| all.iter().find(|c| c.id == id).cloned());
    let loaded = controls.read().is_some();
    let wid = widget.id.clone();
    let wid_state = widget.id.clone();
    let has_state = !widget.source.is_empty();
    let dirty = *cx.layout.read() != *cx.saved_layout.read();

    rsx! {
        div { class: "space-y-2 rounded-lg border border-teal-100 bg-teal-50/40 p-2",
            label {
                r#for: "insp-control",
                class: "block text-[10px] font-medium text-gray-400 uppercase",
                {t!("insp-control")}
            }
            select {
                id: "insp-control",
                class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                disabled: !can_write,
                onchange: move |e| {
                    let Ok(id) = e.value().parse::<Uuid>() else { return };
                    patch_widget(
                        cx,
                        &wid,
                        |w| w.options.control = Some(ControlRef { control_id: id }),
                    );
                },
                if current.is_none() {
                    option { value: "", selected: true, disabled: true, {t!("insp-control-pick")} }
                }
                for c in candidates {
                    option {
                        key: "{c.id}",
                        value: "{c.id}",
                        selected: current == Some(c.id),
                        "{c.label} ({c.key})"
                    }
                }
                if current.is_some() && def.is_none() && loaded {
                    option { value: "", selected: true, disabled: true, {t!("controls-missing")} }
                }
            }
            if let Some(d) = def.clone() {
                ControlInfo { control: d }
            }
            if can_write {
                QuickCreate {
                    kind_spec: ControlSpec::new(kind),
                    on_created: move |id: Uuid| {
                        reload += 1;
                        let wid = widget.id.clone();
                        patch_widget(
                            cx,
                            &wid,
                            |w| w.options.control = Some(ControlRef { control_id: id }),
                        );
                    },
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

/// Domain and listeners of the picked control.
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

/// Inline creation of a control of the card kind (default spec; tune it on
/// the Controls page).
#[component]
fn QuickCreate(kind_spec: ControlSpec, on_created: EventHandler<Uuid>) -> Element {
    let mut label = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let key = suggest_key(&label());
    let ok = !label().trim().is_empty() && !key.is_empty() && !busy();
    let submit = move |_| {
        let params = CreateUiControl {
            key: suggest_key(&label.peek()),
            label: label.peek().trim().to_string(),
            spec: kind_spec.clone(),
        };
        busy.set(true);
        spawn(async move {
            match api::controls::create(params).await {
                Ok(c) => {
                    label.set(String::new());
                    on_created.call(c.id);
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        details { class: "rounded border border-gray-200 bg-white p-2",
            summary { class: "cursor-pointer text-xs font-medium text-gray-600",
                {t!("insp-control-create")}
            }
            div { class: "mt-2 space-y-1",
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                    placeholder: t!("controls-label-placeholder").to_string(),
                    value: "{label}",
                    oninput: move |e| label.set(e.value()),
                }
                if !key.is_empty() {
                    p { class: "text-[10px] text-gray-400",
                        {t!("insp-control-key", key : key.clone())}
                    }
                }
                button {
                    class: "w-full rounded bg-gray-900 px-2 py-1 text-xs text-white disabled:opacity-40",
                    disabled: !ok,
                    onclick: submit,
                    {t!("common-create")}
                }
            }
        }
    }
}

/// "Create the flow": pick the device and output pin, then a draft
/// `control-source` → `device-write` opens in the flow editor.
#[component]
fn FlowDraft(control: UiControl, dirty: bool) -> Element {
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
