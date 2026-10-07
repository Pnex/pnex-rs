//! Value domain editor of a control (D125, D137), shared by the Controls
//! page form and the dashboard inspector: on/off, bounds and step, press
//! value, select / command options, colour mode, unit, confirmation.
//! Numbers are parsed on change (empty = default of the kind); the caller
//! shows [`spec_error_text`] of `ControlSpec::check`.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{
    valid_option_key, ColorMode, ControlKind, ControlOption, ControlSpec, CONTROL_OPTIONS_MAX,
    CONTROL_OPTION_LABEL_MAX,
};

use crate::components::home_icons::HomeIconPicker;
use crate::components::icons;

/// Localized text of a spec check code (`ControlSpec::check`).
pub fn spec_error_text(code: &str) -> String {
    match code {
        "control_spec_switch_same" => t!("controls-spec-switch-same").to_string(),
        "control_spec_range" => t!("controls-spec-range").to_string(),
        "control_spec_step" => t!("controls-spec-step").to_string(),
        "control_spec_unit" => t!("controls-spec-unit").to_string(),
        "control_spec_options_count" => t!("controls-spec-options-count").to_string(),
        "control_spec_option_duplicate" => t!("controls-spec-option-duplicate").to_string(),
        "control_spec_option_key" => t!("controls-spec-option-key").to_string(),
        "control_spec_option_label" => t!("controls-spec-option-label").to_string(),
        _ => t!("controls-form-number-invalid").to_string(),
    }
}

fn num_text(v: Option<f64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

/// Empty = `None`; unparsable = unchanged (`Err`).
fn parse_opt(s: &str) -> Result<Option<f64>, ()> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Some)
        .ok_or(())
}

/// First free option key `option-N` and value (max + 1).
fn next_option(spec: &ControlSpec) -> ControlOption {
    let value = spec
        .options
        .iter()
        .map(|o| o.value)
        .fold(f64::NEG_INFINITY, f64::max);
    let value = if value.is_finite() { value + 1.0 } else { 0.0 };
    let mut n = spec.options.len() + 1;
    let mut key = format!("option-{n}");
    while spec.options.iter().any(|o| o.key == key) {
        n += 1;
        key = format!("option-{n}");
    }
    ControlOption {
        value,
        key,
        label: None,
        icon: None,
    }
}

#[component]
pub fn SpecFields(
    spec: ControlSpec,
    disabled: bool,
    on_change: EventHandler<ControlSpec>,
) -> Element {
    let k = spec.kind;
    let ranged = matches!(
        k,
        ControlKind::Slider | ControlKind::Number | ControlKind::Stepper
    ) || (k == ControlKind::Color && spec.color_mode() == ColorMode::Kelvin);
    let (pmin, pmax, pstep) = match k {
        ControlKind::Slider | ControlKind::Stepper => ("0", "100", "1"),
        ControlKind::Color => ("2200", "6500", "50"),
        _ => ("", "", ""),
    };
    let show_unit =
        !(k == ControlKind::Color && spec.color_mode() == ColorMode::Rgb) && !k.has_options();
    let rgb = spec.color_mode() == ColorMode::Rgb;
    let unit_text = spec.unit.clone().unwrap_or_default();
    // Copy callbacks built outside the markup (dx fmt rule 6): each edits
    // one field of a copy of the current spec.
    let field = |f: fn(&mut ControlSpec, Option<f64>)| {
        let base = spec.clone();
        Callback::new(move |v: Option<f64>| {
            let mut next = base.clone();
            f(&mut next, v);
            on_change.call(next);
        })
    };
    let set_on = field(|s, v| s.on = v);
    let set_off = field(|s, v| s.off = v);
    let set_min = field(|s, v| s.min = v);
    let set_max = field(|s, v| s.max = v);
    let set_step = field(|s, v| s.step = v);
    let set_press = field(|s, v| s.press = v);
    let base_unit = spec.clone();
    let set_unit = Callback::new(move |u: String| {
        let mut next = base_unit.clone();
        let u = u.trim().to_string();
        next.unit = (!u.is_empty()).then_some(u);
        on_change.call(next);
    });
    let base_confirm = spec.clone();
    let set_confirm = Callback::new(move |on: bool| {
        let mut next = base_confirm.clone();
        next.confirm = on;
        on_change.call(next);
    });
    let base_mode = spec.clone();
    let set_mode = Callback::new(move |mode: ColorMode| {
        let mut next = ControlSpec::new(ControlKind::Color);
        next.color = Some(mode);
        next.unit = (mode == ColorMode::Kelvin).then(|| "K".to_string());
        next.confirm = base_mode.confirm;
        on_change.call(next);
    });
    let rgb_label = t!("controls-color-rgb").to_string();
    let kelvin_label = t!("controls-color-kelvin").to_string();
    rsx! {
        div { class: "space-y-2",
            if k == ControlKind::Color {
                div { class: "flex gap-1 rounded border border-gray-200 p-0.5",
                    ModeButton {
                        label: rgb_label,
                        active: rgb,
                        disabled,
                        on_press: move |_| set_mode.call(ColorMode::Rgb),
                    }
                    ModeButton {
                        label: kelvin_label,
                        active: !rgb,
                        disabled,
                        on_press: move |_| set_mode.call(ColorMode::Kelvin),
                    }
                }
            }
            div { class: "grid grid-cols-3 gap-2",
                if k == ControlKind::Switch {
                    NumInput {
                        label: t!("controls-form-on").to_string(),
                        value: num_text(spec.on),
                        placeholder: "1",
                        disabled,
                        on_value: set_on,
                    }
                    NumInput {
                        label: t!("controls-form-off").to_string(),
                        value: num_text(spec.off),
                        placeholder: "0",
                        disabled,
                        on_value: set_off,
                    }
                }
                if ranged {
                    NumInput {
                        label: t!("controls-form-min").to_string(),
                        value: num_text(spec.min),
                        placeholder: pmin,
                        disabled,
                        on_value: set_min,
                    }
                    NumInput {
                        label: t!("controls-form-max").to_string(),
                        value: num_text(spec.max),
                        placeholder: pmax,
                        disabled,
                        on_value: set_max,
                    }
                    NumInput {
                        label: t!("controls-form-step").to_string(),
                        value: num_text(spec.step),
                        placeholder: pstep,
                        disabled,
                        on_value: set_step,
                    }
                }
                if k == ControlKind::Button {
                    NumInput {
                        label: t!("controls-form-press").to_string(),
                        value: num_text(spec.press),
                        placeholder: "1",
                        disabled,
                        on_value: set_press,
                    }
                }
                if show_unit {
                    label { class: "block",
                        span { class: "mb-1 block text-xs font-medium text-gray-500",
                            {t!("controls-form-unit")}
                        }
                        input {
                            class: "w-full rounded border border-gray-300 px-2 py-1.5 text-sm",
                            value: "{unit_text}",
                            placeholder: "%",
                            disabled,
                            oninput: move |e| set_unit.call(e.value()),
                        }
                    }
                }
            }
            if k.has_options() {
                OptionsEditor { spec: spec.clone(), disabled, on_change }
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    checked: spec.confirm,
                    disabled,
                    onchange: move |e| set_confirm.call(e.checked()),
                }
                {t!("controls-form-confirm")}
            }
        }
    }
}

#[component]
fn ModeButton(label: String, active: bool, disabled: bool, on_press: EventHandler<()>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if active { "flex-1 rounded bg-blue-600 px-2 py-1 text-xs font-medium text-white" } else { "flex-1 rounded px-2 py-1 text-xs font-medium text-gray-600 hover:bg-gray-100" },
            disabled,
            onclick: move |_| on_press.call(()),
            "{label}"
        }
    }
}

/// Number input committing on change; an unparsable entry is ignored.
#[component]
fn NumInput(
    label: String,
    value: String,
    placeholder: String,
    disabled: bool,
    on_value: EventHandler<Option<f64>>,
) -> Element {
    // Applied on every keystroke (O28: on blur only, the error of an
    // intermediate state stayed shown while the next field was typed). The
    // local draft keeps partial input ("-", "0.") the spec cannot hold; the
    // outer value wins as soon as it means something else.
    let mut draft = use_signal(|| value.clone());
    let text = draft();
    let shown = match parse_opt(&text) {
        Err(()) => text,
        Ok(v) if Ok(v) == parse_opt(&value) => text,
        Ok(_) => value.clone(),
    };
    rsx! {
        // The label wraps its input: named for assistive tech (O31).
        label { class: "block",
            span { class: "mb-1 block text-xs font-medium text-gray-500", "{label}" }
            input {
                class: "w-full rounded border border-gray-300 px-2 py-1.5 text-sm",
                r#type: "text",
                inputmode: "decimal",
                value: "{shown}",
                placeholder: "{placeholder}",
                disabled,
                oninput: move |e| {
                    let raw = e.value();
                    let parsed = parse_opt(&raw);
                    draft.set(raw);
                    if let Ok(v) = parsed {
                        on_value.call(v);
                    }
                },
            }
        }
    }
}

/// Options of a select / command: key, label, value, icon; add / remove.
#[component]
fn OptionsEditor(
    spec: ControlSpec,
    disabled: bool,
    on_change: EventHandler<ControlSpec>,
) -> Element {
    let rows = spec.options.clone();
    let can_add = !disabled && rows.len() < CONTROL_OPTIONS_MAX;
    let add_base = spec.clone();
    let add = Callback::new(move |_: ()| {
        let mut next = add_base.clone();
        let o = next_option(&next);
        next.options.push(o);
        on_change.call(next);
    });
    rsx! {
        div { class: "space-y-1",
            label { class: "block text-xs font-medium text-gray-500", {t!("controls-form-options")} }
            for (i, o) in rows.into_iter().enumerate() {
                OptionRow {
                    key: "opt-{i}",
                    spec: spec.clone(),
                    index: i,
                    option: o,
                    disabled,
                    on_change,
                }
            }
            if can_add {
                button {
                    r#type: "button",
                    class: "text-xs font-medium text-blue-600 hover:text-blue-800",
                    onclick: move |_| add.call(()),
                    {t!("controls-form-add-option")}
                }
            }
            p { class: "text-[10px] text-gray-400", {t!("controls-form-options-help")} }
        }
    }
}

#[component]
fn OptionRow(
    spec: ControlSpec,
    index: usize,
    option: ControlOption,
    disabled: bool,
    on_change: EventHandler<ControlSpec>,
) -> Element {
    let i = index;
    // Copy callbacks built outside the markup: each edits one field of
    // option `i` in a copy of the spec.
    let field = |f: fn(&mut ControlOption, String)| {
        let base = spec.clone();
        Callback::new(move |v: String| {
            let mut next = base.clone();
            if let Some(o) = next.options.get_mut(i) {
                f(o, v);
            }
            on_change.call(next);
        })
    };
    let set_key = field(|o, v| o.key = v.trim().to_string());
    let set_label = field(|o, v| o.label = (!v.trim().is_empty()).then_some(v));
    let set_value = field(|o, v| {
        if let Ok(Some(n)) = parse_opt(&v) {
            o.value = n;
        }
    });
    let base_icon = spec.clone();
    let set_icon = Callback::new(move |icon: Option<String>| {
        let mut next = base_icon.clone();
        if let Some(o) = next.options.get_mut(i) {
            o.icon = icon;
        }
        on_change.call(next);
    });
    let base_remove = spec.clone();
    let remove = Callback::new(move |_: ()| {
        let mut next = base_remove.clone();
        if i < next.options.len() {
            next.options.remove(i);
        }
        on_change.call(next);
    });
    let key_class = if valid_option_key(&option.key) {
        "w-24 rounded border border-gray-300 px-1.5 py-1 font-mono text-xs"
    } else {
        "w-24 rounded border border-red-400 px-1.5 py-1 font-mono text-xs"
    };
    let hint = crate::components::surface::option_label(&ControlOption {
        label: None,
        ..option.clone()
    });
    let label_text = option.label.clone().unwrap_or_default();
    rsx! {
        div { class: "flex items-center gap-1",
            input {
                class: key_class,
                title: t!("controls-form-option-key").to_string(),
                value: "{option.key}",
                disabled,
                oninput: move |e| set_key.call(e.value()),
            }
            input {
                class: "min-w-0 flex-1 rounded border border-gray-300 px-1.5 py-1 text-xs",
                maxlength: "{CONTROL_OPTION_LABEL_MAX}",
                placeholder: "{hint}",
                value: "{label_text}",
                disabled,
                oninput: move |e| set_label.call(e.value()),
            }
            input {
                class: "w-14 rounded border border-gray-300 px-1.5 py-1 text-xs",
                title: t!("controls-form-option-value").to_string(),
                inputmode: "decimal",
                value: "{option.value}",
                disabled,
                onchange: move |e| set_value.call(e.value()),
            }
            HomeIconPicker {
                value: option.icon.clone(),
                disabled,
                on_change: set_icon,
            }
            button {
                r#type: "button",
                class: if disabled { "hidden" } else { "text-gray-300 hover:text-red-500" },
                onclick: move |_| remove.call(()),
                icons::Trash { class: "h-3.5 w-3.5" }
            }
        }
    }
}
