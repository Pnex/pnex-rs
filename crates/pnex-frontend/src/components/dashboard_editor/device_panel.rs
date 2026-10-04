//! Guided addition "from a device" (parcours §1.2, D124/D125): pick a
//! device, then each output pin proposes its control card (digital output →
//! switch, PWM → slider) and each published metric its reading (value,
//! gauge, chart). A pin card creates an org control — never a pin binding
//! (D128) — and offers "Create the flow" (draft `control-source` →
//! `device-write`).

use std::collections::{BTreeMap, HashMap};

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{ControlKind, ControlRef, ControlSpec, CreateUiControl, UiControl};
use pnex_core::SourceRef;

use super::library;
use super::EditorCx;
use crate::api;
use crate::components::icons;
use crate::components::surface::{create_flow_draft, suggest_key};
use crate::state::toasts;

/// Kind proposed for an output pin mode.
fn kind_for_mode(mode: Option<&str>) -> Option<ControlKind> {
    match mode {
        Some("digital_out") => Some(ControlKind::Switch),
        Some("pwm_out") => Some(ControlKind::Slider),
        _ => None,
    }
}

/// Telemetry source of the device metric matching a pin label (the actual
/// state of the output), when the device publishes one.
fn state_source(device: &str, pin: &str, metrics: &[String]) -> Option<SourceRef> {
    metrics
        .iter()
        .find(|m| m.eq_ignore_ascii_case(pin))
        .map(|m| SourceRef {
            role: "state".into(),
            metric: m.clone(),
            device_id: device.to_owned(),
            window: "1h".into(),
            memory: None,
        })
}

/// Creates (or reuses, on a key already taken) the control of a pin, then
/// adds its card.
fn add_pin_card(
    cx: EditorCx,
    device: String,
    pin: String,
    kind: ControlKind,
    metrics: Vec<String>,
    mut created: Signal<HashMap<String, UiControl>>,
) {
    let key = suggest_key(&format!("{device}.{pin}"));
    let mut spec = ControlSpec::new(kind);
    if kind == ControlKind::Slider {
        spec.unit = Some("%".into());
    }
    let params = CreateUiControl {
        key: key.clone(),
        label: format!("{device} — {pin}"),
        spec,
    };
    spawn(async move {
        let control = match api::controls::create(params).await {
            Ok(c) => Some(c),
            // Same pin added again: reuse its control (one intention, many
            // surfaces).
            Err(e) if e.status == Some(409) => api::controls::list().await.ok().and_then(|all| {
                all.into_iter()
                    .find(|c| c.key == key && c.spec.kind == kind)
            }),
            Err(e) => {
                toasts::error(e);
                None
            }
        };
        let Some(control) = control else {
            return;
        };
        let id = control.id;
        let source = state_source(&device, &pin, &metrics);
        library::add_configured(cx, kind.widget_type(), move |w| {
            w.options.control = Some(ControlRef { control_id: id });
            w.source = source.into_iter().collect();
        });
        created.with_mut(|m| {
            m.insert(pin, control);
        });
    });
}

#[component]
pub fn DevicePanel(cx: EditorCx, by_source: BTreeMap<String, Vec<String>>) -> Element {
    let mut open = cx.devices_open;
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
    let created: Signal<HashMap<String, UiControl>> = use_signal(HashMap::new);
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
                .filter(|p| kind_for_mode(p.mode.as_deref()).is_some())
                .collect::<Vec<_>>()
        }
    });
    let list = devices.read().clone().unwrap_or_default();
    let slug = device().map(|d| d.1).unwrap_or_default();
    let metrics: Vec<String> = by_source.get(&slug).cloned().unwrap_or_default();
    let outputs: Vec<(String, ControlKind)> = pins
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| kind_for_mode(p.mode.as_deref()).map(|k| (p.label, k)))
        .collect();
    let dirty = *cx.layout.read() != *cx.saved_layout.read();
    let close_label = t!("eshell-close");

    rsx! {
        div { class: "absolute left-0 top-12 z-30 flex max-h-[75vh] w-80 flex-col overflow-hidden rounded-xl border border-gray-200 bg-white shadow-xl",
            div { class: "flex items-center justify-between px-3 pt-3",
                span { class: "text-sm font-semibold text-gray-900", {t!("db-from-device")} }
                button {
                    class: "text-gray-400 hover:text-gray-600",
                    title: "{close_label}",
                    onclick: move |_| open.set(false),
                    icons::X { class: "h-4 w-4" }
                }
            }
            div { class: "space-y-3 overflow-y-auto p-3",
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
                    p { class: "text-[10px] font-medium uppercase text-gray-400",
                        {t!("db-from-device-outputs")}
                    }
                    if outputs.is_empty() {
                        p { class: "text-xs text-gray-400", {t!("db-from-device-no-output")} }
                    }
                    for (pin, kind) in outputs {
                        PinRow {
                            key: "{pin}",
                            cx,
                            device: slug.clone(),
                            pin: pin.clone(),
                            kind,
                            metrics: metrics.clone(),
                            created,
                            dirty,
                        }
                    }
                    p { class: "text-[10px] font-medium uppercase text-gray-400",
                        {t!("db-from-device-metrics")}
                    }
                    if metrics.is_empty() {
                        p { class: "text-xs text-gray-400", {t!("db-from-device-no-metric")} }
                    }
                    for m in metrics.clone() {
                        MetricRow {
                            key: "{m}",
                            cx,
                            device: slug.clone(),
                            metric: m.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PinRow(
    cx: EditorCx,
    device: String,
    pin: String,
    kind: ControlKind,
    metrics: Vec<String>,
    created: Signal<HashMap<String, UiControl>>,
    dirty: bool,
) -> Element {
    let made = created.read().get(&pin).cloned();
    let action = match kind {
        ControlKind::Slider => t!("db-from-device-add-slider").to_string(),
        _ => t!("db-from-device-add-switch").to_string(),
    };
    let (d, p, m) = (device.clone(), pin.clone(), metrics.clone());
    rsx! {
        div { class: "rounded-lg border border-gray-200 p-2 space-y-1",
            div { class: "flex items-center justify-between gap-2",
                span { class: "truncate text-sm text-gray-800", "{pin}" }
                button {
                    class: "shrink-0 rounded bg-teal-600 px-2 py-0.5 text-xs font-medium text-white hover:bg-teal-700",
                    onclick: move |_| add_pin_card(cx, d.clone(), p.clone(), kind, m.clone(), created),
                    "{action}"
                }
            }
            if let Some(control) = made {
                if dirty {
                    p { class: "text-[10px] text-amber-700", {t!("insp-control-flow-save-first")} }
                } else {
                    button {
                        class: "w-full rounded border border-teal-200 px-2 py-0.5 text-xs text-teal-700 hover:bg-teal-50",
                        onclick: move |_| create_flow_draft(&control, &device, &pin),
                        {t!("insp-control-flow-create")}
                    }
                }
            }
        }
    }
}

#[component]
fn MetricRow(cx: EditorCx, device: String, metric: String) -> Element {
    let rows = [
        ("stat", t!("lib-kind-stat").to_string()),
        ("gauge", t!("lib-kind-gauge").to_string()),
        ("line", t!("lib-kind-line").to_string()),
    ];
    rsx! {
        div { class: "flex items-center justify-between gap-2 rounded-lg border border-gray-200 p-2",
            span { class: "truncate text-sm text-gray-800", "{metric}" }
            div { class: "flex shrink-0 gap-1",
                for (kind, label) in rows {
                    ReadingButton {
                        key: "{kind}",
                        cx,
                        kind: kind.to_string(),
                        label,
                        device: device.clone(),
                        metric: metric.clone(),
                    }
                }
            }
        }
    }
}

#[component]
fn ReadingButton(
    cx: EditorCx,
    kind: String,
    label: String,
    device: String,
    metric: String,
) -> Element {
    rsx! {
        button {
            class: "rounded border border-gray-300 px-1.5 py-0.5 text-[11px] text-gray-700 hover:bg-gray-50",
            onclick: move |_| {
                let (d, m) = (device.clone(), metric.clone());
                library::add_configured(
                    cx,
                    &kind,
                    move |w| {
                        w.title = m.clone();
                        w.source = vec![
                            SourceRef {
                                role: "primary".into(),
                                metric: m,
                                device_id: d,
                                window: "1h".into(),
                                memory: None,
                            },
                        ];
                    },
                );
            },
            "{label}"
        }
    }
}
