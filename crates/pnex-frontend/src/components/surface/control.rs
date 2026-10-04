//! Control card body (`switch` / `slider` / `button` / `number`, D125):
//! shows the last commanded value of an org control, writes a new one, and
//! the optional state source (the real value reported by a device).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{hex_to_rgb, rgb_to_hex, ColorMode, ControlKind, ControlSpec};
use pnex_core::{TelemetryPoint, Widget};
use uuid::Uuid;

use super::{option_label, SurfaceControls};
use crate::api;
use crate::components::charts::format_value;
use crate::components::confirm::ConfirmDialog;
use crate::components::home_icons::HomeIconView;
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

/// Text of a value of `spec`: option label, `#rrggbb`, kelvin or number.
fn value_text(spec: &ControlSpec, v: f64, decimals: u8, unit: &str) -> String {
    match spec.kind {
        ControlKind::Select | ControlKind::Command => spec
            .option_of(v)
            .map(option_label)
            .unwrap_or_else(|| format_value(v, decimals)),
        ControlKind::Color if spec.color_mode() == ColorMode::Rgb => rgb_to_hex(v),
        _ => format!("{} {unit}", format_value(v, decimals))
            .trim()
            .to_string(),
    }
}

/// Writes `v` (already accepted by the spec) and records it on success.
fn fire(
    surface: SurfaceControls,
    id: Uuid,
    v: f64,
    via: Option<String>,
    mut busy: Signal<bool>,
    mut drag: Signal<Option<f64>>,
) {
    busy.set(true);
    let via = via.unwrap_or_else(|| surface.via.cloned());
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
pub fn ControlBody(
    widget: Widget,
    state: Option<TelemetryPoint>,
    /// Originating surface of the writes when it differs from the context
    /// one (an annotation panel mixes items of several layers).
    #[props(default)]
    via: Option<String>,
) -> Element {
    let surface = try_use_context::<SurfaceControls>();
    let busy = use_signal(|| false);
    // Slider position while dragged / awaiting the write.
    let mut drag: Signal<Option<f64>> = use_signal(|| None);
    // Value awaiting the confirmation dialog (`spec.confirm`).
    let mut pending: Signal<Option<f64>> = use_signal(|| None);
    let mut draft = use_signal(String::new);

    let control_id = widget.options.control.as_ref().map(|c| c.control_id);
    let def = control_id.and_then(|id| surface.and_then(|s| s.def(&id)));
    // The control decides the card shape (an annotation only knows the
    // control id); the widget type is the fallback while loading.
    let kind = def
        .as_ref()
        .map(|d| d.spec.kind)
        .or_else(|| kind_of_widget(&widget.widget_type))
        .unwrap_or(ControlKind::Switch);
    let via_override = via.clone();
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
    // A Copy callback: the option buttons of a select / command each hold
    // one inside a loop.
    let send = Callback::new(move |v: f64| {
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
            fire(surface, id, v, via_override.clone(), busy, drag);
        }
    });

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
                        onclick: move |_| send.call(next),
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
                                send.call(v);
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
                        onclick: move |_| send.call(press),
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
                                send.call(v);
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
        ControlKind::Select => {
            let options = spec.options.clone();
            let segmented = options.len() <= 4;
            let current = shown.unwrap_or(f64::NAN);
            if segmented {
                rsx! {
                    div { class: "flex flex-1 items-center px-3",
                        div {
                            role: "radiogroup",
                            aria_label: "{title}",
                            class: "flex w-full gap-1 rounded-lg bg-gray-100 p-1",
                            for o in options {
                                OptionButton {
                                    key: "{o.key}",
                                    label: option_label(&o),
                                    icon: o.icon.clone(),
                                    active: o.value == current,
                                    disabled: !interactive,
                                    on_press: move |_| send.call(o.value),
                                }
                            }
                        }
                    }
                }
            } else {
                let selected = spec
                    .option_of(current)
                    .map(|o| o.value.to_string())
                    .unwrap_or_default();
                rsx! {
                    div { class: "flex flex-1 items-center px-3",
                        select {
                            aria_label: "{title}",
                            class: "w-full rounded border border-gray-300 bg-white px-2 py-1.5 text-sm disabled:opacity-50",
                            disabled: !interactive,
                            value: "{selected}",
                            onchange: move |e| {
                                if let Ok(v) = e.value().parse::<f64>() {
                                    send.call(v);
                                }
                            },
                            if selected.is_empty() {
                                option {
                                    value: "",
                                    disabled: true,
                                    selected: true,
                                    "—"
                                }
                            }
                            for o in options {
                                option { key: "{o.key}", value: "{o.value}", {option_label(&o)} }
                            }
                        }
                    }
                }
            }
        }
        ControlKind::Stepper => {
            let min = spec.min_value().unwrap_or(0.0);
            let max = spec.max_value().unwrap_or(100.0);
            let step = spec.step_value().unwrap_or(1.0);
            let base = shown.unwrap_or(min);
            let down = (base - step).max(min);
            let up = (base + step).min(max);
            let text = match shown {
                Some(v) => format_value(v, decimals),
                None => "—".into(),
            };
            let btn = "flex h-10 w-10 shrink-0 items-center justify-center rounded-full border border-gray-300 text-xl font-medium text-gray-700 hover:bg-gray-50 active:bg-gray-100 disabled:opacity-40";
            rsx! {
                div { class: "flex flex-1 items-center justify-center gap-4 px-3",
                    button {
                        r#type: "button",
                        class: btn,
                        aria_label: t!("controls-step-down").to_string(),
                        disabled: !interactive || shown.is_some_and(|v| v <= min),
                        onclick: move |_| send.call(down),
                        "−"
                    }
                    div { class: "flex items-baseline gap-1",
                        span { class: "text-3xl font-semibold text-gray-900", "{text}" }
                        span { class: "text-xs text-gray-400", "{unit}" }
                    }
                    button {
                        r#type: "button",
                        class: btn,
                        aria_label: t!("controls-step-up").to_string(),
                        disabled: !interactive || shown.is_some_and(|v| v >= max),
                        onclick: move |_| send.call(up),
                        "+"
                    }
                }
            }
        }
        ControlKind::Command => {
            let options = spec.options.clone();
            let current = shown.unwrap_or(f64::NAN);
            rsx! {
                div { class: "flex flex-1 items-center gap-1 px-3",
                    for o in options {
                        OptionButton {
                            key: "{o.key}",
                            label: option_label(&o),
                            icon: o.icon.clone(),
                            active: o.value == current,
                            disabled: !interactive,
                            boxed: true,
                            on_press: move |_| send.call(o.value),
                        }
                    }
                }
            }
        }
        ControlKind::Color if spec.color_mode() == ColorMode::Rgb => {
            let hex = shown.map(rgb_to_hex).unwrap_or_else(|| "#ffffff".into());
            rsx! {
                div { class: "flex flex-1 items-center justify-center gap-3 px-3",
                    input {
                        r#type: "color",
                        aria_label: "{title}",
                        class: "h-12 w-16 cursor-pointer rounded-lg border border-gray-300 disabled:opacity-50",
                        value: "{hex}",
                        disabled: !interactive,
                        onchange: move |e| {
                            if let Some(v) = hex_to_rgb(&e.value()) {
                                send.call(v);
                            }
                        },
                    }
                    span { class: "font-mono text-sm text-gray-600",
                        if shown.is_some() {
                            "{hex}"
                        } else {
                            "—"
                        }
                    }
                }
            }
        }
        ControlKind::Color => {
            let min = spec.min_value().unwrap_or(2_200.0);
            let max = spec.max_value().unwrap_or(6_500.0);
            let step = spec.step_value().unwrap_or(50.0);
            let position = drag().or(shown).unwrap_or(min);
            let text = match drag().or(shown) {
                Some(v) => format!("{} K", format_value(v, 0)),
                None => "—".into(),
            };
            rsx! {
                div { class: "flex flex-1 flex-col justify-center gap-1 px-3",
                    span { class: "text-center text-2xl font-semibold text-gray-900",
                        "{text}"
                    }
                    input {
                        r#type: "range",
                        aria_label: "{title}",
                        class: "h-3 w-full cursor-pointer appearance-none rounded-full disabled:opacity-50",
                        style: "background: linear-gradient(to right, #ffa94d, #fff4e0, #cfe3ff)",
                        min: "{min}",
                        max: "{max}",
                        step: "{step}",
                        value: "{position}",
                        disabled: !interactive,
                        oninput: move |e| drag.set(e.value().parse::<f64>().ok()),
                        onchange: move |e| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                send.call(v);
                            }
                        },
                    }
                }
            }
        }
    };

    let spec_text = spec.clone();
    let state_line = state.as_ref().map(|p| {
        t!("controls-state", value : value_text(&spec_text, p.value, decimals, &unit)).to_string()
    });
    let confirm_value = pending().map(|v| value_text(&spec_text, v, decimals, &unit));

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
                        fire(surface, id, v, via.clone(), busy, drag);
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

/// One option of a select (segment) or command (boxed button): icon above
/// the label, highlighted when it holds the current value.
#[component]
fn OptionButton(
    label: String,
    icon: Option<String>,
    active: bool,
    disabled: bool,
    #[props(default)] boxed: bool,
    on_press: EventHandler<()>,
) -> Element {
    let class = match (boxed, active) {
        (false, true) => "flex min-w-0 flex-1 flex-col items-center gap-0.5 rounded-md bg-white px-1 py-1.5 text-xs font-medium text-teal-700 shadow disabled:opacity-50",
        (false, false) => "flex min-w-0 flex-1 flex-col items-center gap-0.5 rounded-md px-1 py-1.5 text-xs font-medium text-gray-600 hover:bg-white/60 disabled:opacity-50",
        (true, true) => "flex min-w-0 flex-1 flex-col items-center gap-0.5 rounded-lg border border-teal-600 bg-teal-50 px-1 py-2 text-xs font-medium text-teal-700 disabled:opacity-50",
        (true, false) => "flex min-w-0 flex-1 flex-col items-center gap-0.5 rounded-lg border border-gray-300 px-1 py-2 text-xs font-medium text-gray-700 hover:bg-gray-50 active:bg-gray-100 disabled:opacity-50",
    };
    rsx! {
        button {
            r#type: "button",
            role: if boxed { "button" } else { "radio" },
            aria_checked: if boxed { None } else { Some(active.to_string()) },
            class,
            disabled,
            onclick: move |_| on_press.call(()),
            if let Some(id) = icon {
                HomeIconView { id, class: "h-5 w-5" }
            }
            span { class: "max-w-full truncate", "{label}" }
        }
    }
}
