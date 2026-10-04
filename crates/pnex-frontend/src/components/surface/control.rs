//! Control card body (`switch` / `slider` / `button` / `number`, D125):
//! shows the last commanded value of an org control, writes a new one, and
//! the optional state source (the real value reported by a device).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{ControlKind, ControlSpec};
use pnex_core::{TelemetryPoint, Widget};
use uuid::Uuid;

use super::SurfaceControls;
use crate::api;
use crate::components::charts::format_value;
use crate::components::confirm::ConfirmDialog;
use crate::state::toasts;

/// Kind driven by a control widget type (`None`: not a control widget).
pub fn kind_of_widget(widget_type: &str) -> Option<ControlKind> {
    ControlKind::ALL
        .into_iter()
        .find(|k| k.widget_type() == widget_type)
}

/// Decimals shown for a value of `spec` (widget option first, else from the
/// step: integer steps show integers).
fn decimals_of(widget: &Widget, spec: &ControlSpec) -> u8 {
    widget
        .options
        .decimals
        .unwrap_or_else(|| match spec.step_value() {
            Some(step) if step.fract() != 0.0 => 2,
            Some(_) => 0,
            None if spec.kind == ControlKind::Number => 2,
            None => 0,
        })
}

/// Writes `v` (already accepted by the spec) and records it on success.
fn fire(
    surface: SurfaceControls,
    id: Uuid,
    v: f64,
    mut busy: Signal<bool>,
    mut drag: Signal<Option<f64>>,
) {
    busy.set(true);
    let via = surface.via.cloned();
    spawn(async move {
        match api::controls::write_value(id, v, Some(via)).await {
            Ok(value) => surface.record(id, value),
            Err(e) => toasts::error(e),
        }
        drag.set(None);
        busy.set(false);
    });
}

#[component]
pub fn ControlBody(widget: Widget, state: Option<TelemetryPoint>) -> Element {
    let surface = try_use_context::<SurfaceControls>();
    let busy = use_signal(|| false);
    // Slider position while dragged / awaiting the write.
    let mut drag: Signal<Option<f64>> = use_signal(|| None);
    // Value awaiting the confirmation dialog (`spec.confirm`).
    let mut pending: Signal<Option<f64>> = use_signal(|| None);
    let mut draft = use_signal(String::new);

    let kind = kind_of_widget(&widget.widget_type).unwrap_or(ControlKind::Switch);
    let control_id = widget.options.control.as_ref().map(|c| c.control_id);
    let def = control_id.and_then(|id| surface.and_then(|s| s.def(&id)));
    let value = control_id.and_then(|id| surface.and_then(|s| s.value(&id)));
    let missing = control_id.is_some() && def.is_none() && surface.is_some_and(|s| s.loaded());
    let idle = def.as_ref().is_some_and(|d| d.listened_by.is_empty());
    let interactive = surface.is_some_and(|s| s.interactive) && def.is_some() && !busy();
    let spec = def
        .as_ref()
        .map(|d| d.spec.clone())
        .unwrap_or_else(|| ControlSpec::new(kind));
    let decimals = decimals_of(&widget, &spec);
    let unit = spec.unit.clone().unwrap_or_default();
    let title = if !widget.title.trim().is_empty() {
        widget.title.clone()
    } else {
        def.as_ref().map(|d| d.label.clone()).unwrap_or_default()
    };
    let label = def.as_ref().map(|d| d.label.clone()).unwrap_or_default();
    let last_by = value
        .as_ref()
        .and_then(|v| v.by.clone())
        .map(|by| t!("controls-last-by", by : by).to_string())
        .unwrap_or_default();
    let shown = value.as_ref().map(|v| v.v);

    // Gate then confirm (or write): the value is checked against the spec
    // in the browser first, the server checks it again.
    let confirm_needed = spec.confirm;
    let spec_send = spec.clone();
    let mut send = move |v: f64| {
        let (Some(surface), Some(id)) = (surface, control_id) else {
            return;
        };
        let Ok(v) = spec_send.accepts(v) else {
            toasts::error(t!("controls-value-refused").to_string());
            drag.set(None);
            return;
        };
        if confirm_needed {
            pending.set(Some(v));
        } else {
            fire(surface, id, v, busy, drag);
        }
    };

    let body = match kind {
        ControlKind::Switch => {
            let on = shown == Some(spec.on_value());
            let next = if on {
                spec.off_value()
            } else {
                spec.on_value()
            };
            let track = if on { "bg-teal-600" } else { "bg-gray-300" };
            let knob = if on { "translate-x-6" } else { "translate-x-0" };
            let state_text = match shown {
                None => "—".to_string(),
                Some(_) if on => t!("controls-on").to_string(),
                Some(_) => t!("controls-off").to_string(),
            };
            rsx! {
                div { class: "flex flex-1 items-center justify-center gap-3",
                    button {
                        r#type: "button",
                        role: "switch",
                        aria_checked: "{on}",
                        aria_label: "{title}",
                        class: "relative inline-flex h-8 w-14 shrink-0 items-center rounded-full p-1 transition-colors disabled:opacity-50 {track}",
                        disabled: !interactive,
                        onclick: move |_| send(next),
                        span { class: "inline-block h-6 w-6 rounded-full bg-white shadow transition-transform {knob}" }
                    }
                    span { class: "text-sm font-medium text-gray-700", "{state_text}" }
                }
            }
        }
        ControlKind::Slider => {
            let min = spec.min_value().unwrap_or(0.0);
            let max = spec.max_value().unwrap_or(100.0);
            let step = spec.step_value().unwrap_or(1.0);
            let position = drag().or(shown).unwrap_or(min);
            let text = match drag().or(shown) {
                Some(v) => format_value(v, decimals),
                None => "—".into(),
            };
            rsx! {
                div { class: "flex flex-1 flex-col justify-center gap-1 px-3",
                    div { class: "flex items-baseline justify-center gap-1",
                        span { class: "text-2xl font-semibold text-gray-900", "{text}" }
                        span { class: "text-xs text-gray-400", "{unit}" }
                    }
                    input {
                        r#type: "range",
                        aria_label: "{title}",
                        class: "w-full accent-teal-600 disabled:opacity-50",
                        min: "{min}",
                        max: "{max}",
                        step: "{step}",
                        value: "{position}",
                        disabled: !interactive,
                        oninput: move |e| drag.set(e.value().parse::<f64>().ok()),
                        onchange: move |e| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                send(v);
                            }
                        },
                    }
                }
            }
        }
        ControlKind::Button => {
            let press = spec.press_value();
            let caption = if label.is_empty() {
                t!("controls-press").to_string()
            } else {
                label.clone()
            };
            rsx! {
                div { class: "flex flex-1 items-center justify-center px-3",
                    button {
                        r#type: "button",
                        class: "w-full rounded-lg bg-teal-600 px-3 py-2 text-sm font-medium text-white hover:bg-teal-700 active:bg-teal-800 disabled:opacity-50",
                        disabled: !interactive,
                        onclick: move |_| send(press),
                        "{caption}"
                    }
                }
            }
        }
        ControlKind::Number => {
            let text = match shown {
                Some(v) => format_value(v, decimals),
                None => "—".into(),
            };
            let min = spec.min_value().map(|v| v.to_string()).unwrap_or_default();
            let max = spec.max_value().map(|v| v.to_string()).unwrap_or_default();
            let step = spec
                .step_value()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "any".into());
            rsx! {
                div { class: "flex flex-1 flex-col justify-center gap-1 px-3",
                    div { class: "flex items-baseline justify-center gap-1",
                        span { class: "text-2xl font-semibold text-gray-900", "{text}" }
                        span { class: "text-xs text-gray-400", "{unit}" }
                    }
                    form {
                        class: "flex gap-1",
                        onsubmit: move |e| {
                            e.prevent_default();
                            let parsed = draft.peek().trim().replace(',', ".").parse::<f64>();
                            if let Ok(v) = parsed {
                                send(v);
                                draft.set(String::new());
                            }
                        },
                        input {
                            r#type: "number",
                            aria_label: "{title}",
                            class: "min-w-0 flex-1 rounded border border-gray-300 px-2 py-1 text-sm",
                            min: "{min}",
                            max: "{max}",
                            step: "{step}",
                            value: "{draft}",
                            disabled: !interactive,
                            oninput: move |e| draft.set(e.value()),
                        }
                        button {
                            r#type: "submit",
                            class: "rounded bg-teal-600 px-2 py-1 text-xs font-medium text-white disabled:opacity-50",
                            disabled: !interactive || draft().trim().is_empty(),
                            {t!("controls-send")}
                        }
                    }
                }
            }
        }
    };

    let state_line = state.as_ref().map(|p| {
        t!(
            "controls-state", value : format!("{} {}", format_value(p.value, decimals), unit)
            .trim().to_string()
        )
        .to_string()
    });
    let confirm_value = pending().map(|v| format!("{} {}", format_value(v, decimals), unit));

    rsx! {
        div {
            class: "flex h-full w-full flex-col overflow-hidden rounded-lg",
            title: "{last_by}",
            div { class: "flex items-center justify-between gap-2 px-3 pt-2",
                span { class: "truncate text-xs font-medium text-gray-500", "{title}" }
                if missing {
                    span { class: "shrink-0 rounded bg-red-50 px-1.5 text-[10px] font-medium text-red-700",
                        {t!("controls-missing")}
                    }
                } else if idle {
                    span {
                        class: "shrink-0 rounded bg-amber-50 px-1.5 text-[10px] font-medium text-amber-700",
                        title: t!("controls-idle-help").to_string(),
                        {t!("controls-idle")}
                    }
                }
            }
            {body}
            if let Some(line) = state_line {
                p { class: "px-3 pb-2 text-center text-[11px] text-gray-500", "{line}" }
            }
        }
        if let Some(text) = confirm_value {
            ConfirmDialog {
                title: t!("controls-confirm-title").to_string(),
                message: t!("controls-confirm-body", label : title.clone(), value : text).to_string(),
                confirm_label: t!("controls-confirm-send").to_string(),
                on_confirm: move |_| {
                    let v = pending.peek().to_owned();
                    pending.set(None);
                    if let (Some(v), Some(surface), Some(id)) = (v, surface, control_id) {
                        fire(surface, id, v, busy, drag);
                    }
                },
                on_cancel: move |_| {
                    pending.set(None);
                    drag.set(None);
                },
            }
        }
    }
}
