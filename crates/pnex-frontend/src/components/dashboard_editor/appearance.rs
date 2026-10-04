//! Inspector section "Appearance" (D135, D136): card icon, colour
//! thresholds (gauge, stat, indicator), value → colour / icon / label
//! rules and the staleness delay. Rules are plain text and colours, never
//! HTML (R11); the server validates the same bounds (`validate_states`).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{DashboardLayout, StateOp, StateRule, Threshold, Widget, STATE_RULES_MAX};

use crate::components::home_icons::HomeIconPicker;
use crate::components::icons;

use super::{EditorCx, Selection};

/// Colour of a new threshold / rule (green).
const NEW_COLOR: &str = "#22c55e";

/// Widget types whose value is coloured by thresholds.
pub fn has_thresholds(widget_type: &str) -> bool {
    matches!(widget_type, "gauge" | "stat" | "indicator")
}

/// Widget types that apply state rules and the card icon.
pub fn has_states(widget_type: &str) -> bool {
    matches!(widget_type, "stat" | "indicator")
}

/// Widget types reading a live value (staleness applies).
pub fn has_staleness(widget: &Widget) -> bool {
    !widget.source.is_empty() && widget.widget_type != "thermo_chart"
}

fn edit_widget(mut cx: EditorCx, f: impl FnOnce(&mut Widget)) {
    let Some(Selection::Widget(id)) = cx.selected.cloned() else {
        return;
    };
    cx.layout.with_mut(|l: &mut DashboardLayout| {
        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
            f(w);
        }
    });
}

fn edit_rule(cx: EditorCx, i: usize, f: impl FnOnce(&mut StateRule)) {
    edit_widget(cx, |w| {
        if let Some(r) = w.options.states.get_mut(i) {
            f(r);
        }
    });
}

fn op_key(op: StateOp) -> &'static str {
    match op {
        StateOp::Eq => "eq",
        StateOp::Gte => "gte",
        StateOp::Lte => "lte",
    }
}

#[component]
pub fn AppearancePanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let thresholds = widget.options.thresholds.clone();
    let rules = widget.options.states.clone();
    let show_thresholds = has_thresholds(&widget.widget_type);
    let show_states = has_states(&widget.widget_type);
    let show_stale = has_staleness(&widget);
    if !show_thresholds && !show_states && !show_stale {
        return rsx! {};
    }
    let stale = widget
        .options
        .stale_after_s
        .map(|s| s.to_string())
        .unwrap_or_default();
    rsx! {
        div { class: "space-y-2 border-t border-gray-100 pt-2",
            p { class: "text-[10px] font-medium uppercase text-gray-400", {t!("appear-title")} }
            if show_states {
                label { class: "block text-[10px] text-gray-500", {t!("appear-icon")} }
                HomeIconPicker {
                    value: widget.options.icon.clone(),
                    disabled: !can_write,
                    on_change: move |v: Option<String>| edit_widget(cx, |w| w.options.icon = v),
                }
            }
            if show_thresholds {
                label { class: "block text-[10px] text-gray-500", {t!("appear-thresholds")} }
                for (i, th) in thresholds.iter().cloned().enumerate() {
                    div { key: "th-{i}", class: "flex items-center gap-2",
                        span { class: "text-xs text-gray-500", "≥" }
                        input {
                            class: "w-24 rounded border border-gray-300 px-2 py-1 text-sm",
                            "type": "number",
                            step: "any",
                            disabled: !can_write,
                            value: "{th.value}",
                            onchange: move |e| {
                                let v = e.value().parse::<f64>().unwrap_or(0.0);
                                edit_widget(
                                    cx,
                                    |w| {
                                        if let Some(t) = w.options.thresholds.get_mut(i) {
                                            t.value = v;
                                        }
                                    },
                                );
                            },
                        }
                        input {
                            class: "h-7 w-10 cursor-pointer rounded border border-gray-300",
                            "type": "color",
                            disabled: !can_write,
                            value: "{th.color}",
                            oninput: move |e| {
                                let v = e.value();
                                edit_widget(
                                    cx,
                                    |w| {
                                        if let Some(t) = w.options.thresholds.get_mut(i) {
                                            t.color = v;
                                        }
                                    },
                                );
                            },
                        }
                        button {
                            class: if can_write { "text-gray-300 hover:text-red-500" } else { "hidden" },
                            onclick: move |_| edit_widget(
                                cx,
                                |w| {
                                    if i < w.options.thresholds.len() {
                                        w.options.thresholds.remove(i);
                                    }
                                },
                            ),
                            icons::Trash { class: "h-3.5 w-3.5" }
                        }
                    }
                }
                button {
                    class: if can_write { "text-xs font-medium text-blue-600 hover:text-blue-800" } else { "hidden" },
                    onclick: move |_| add_threshold(cx),
                    {t!("appear-add-threshold")}
                }
                p { class: "text-[10px] text-gray-400", {t!("appear-thresholds-help")} }
            }
            if show_states {
                label { class: "block text-[10px] text-gray-500", {t!("appear-states")} }
                for (i, rule) in rules.iter().cloned().enumerate() {
                    StateRuleRow {
                        key: "rule-{i}",
                        cx,
                        index: i,
                        rule,
                        can_write,
                    }
                }
                if rules.len() < STATE_RULES_MAX {
                    button {
                        class: if can_write { "text-xs font-medium text-blue-600 hover:text-blue-800" } else { "hidden" },
                        onclick: move |_| add_rule(cx),
                        {t!("appear-add-state")}
                    }
                }
                p { class: "text-[10px] text-gray-400", {t!("appear-states-help")} }
            }
            if show_stale {
                label { class: "block text-[10px] text-gray-500", {t!("appear-stale")} }
                input {
                    class: "w-full rounded border border-gray-300 px-2 py-1 text-sm",
                    "type": "number",
                    min: "{pnex_core::STALE_AFTER_MIN_S}",
                    max: "{pnex_core::STALE_AFTER_MAX_S}",
                    placeholder: t!("appear-stale-never").to_string(),
                    disabled: !can_write,
                    value: "{stale}",
                    onchange: move |e| {
                        let v = e.value().trim().parse::<u32>().ok();
                        edit_widget(cx, |w| w.options.stale_after_s = v);
                    },
                }
                p { class: "text-[10px] text-gray-400", {t!("appear-stale-help")} }
            }
        }
    }
}

fn add_threshold(cx: EditorCx) {
    edit_widget(cx, |w| {
        let next = w
            .options
            .thresholds
            .last()
            .map(|t| t.value + 1.0)
            .unwrap_or(1.0);
        w.options.thresholds.push(Threshold {
            value: next,
            color: NEW_COLOR.into(),
        });
    });
}

fn add_rule(cx: EditorCx) {
    edit_widget(cx, |w| {
        let next = w
            .options
            .states
            .last()
            .map(|r| r.value + 1.0)
            .unwrap_or(0.0);
        w.options.states.push(StateRule {
            value: next,
            color: Some(NEW_COLOR.into()),
            ..Default::default()
        });
    });
}

#[component]
fn StateRuleRow(cx: EditorCx, index: usize, rule: StateRule, can_write: bool) -> Element {
    let i = index;
    let color = rule.color.clone().unwrap_or_else(|| NEW_COLOR.to_string());
    let has_color = rule.color.is_some();
    let label = rule.label.clone().unwrap_or_default();
    rsx! {
        div { class: "space-y-1 rounded border border-gray-100 p-1.5",
            div { class: "flex items-center gap-1.5",
                select {
                    class: "rounded border border-gray-300 bg-white px-1 py-1 text-sm",
                    disabled: !can_write,
                    value: op_key(rule.op),
                    onchange: move |e| {
                        let op = match e.value().as_str() {
                            "gte" => StateOp::Gte,
                            "lte" => StateOp::Lte,
                            _ => StateOp::Eq,
                        };
                        edit_rule(cx, i, |r| r.op = op);
                    },
                    option { value: "eq", "=" }
                    option { value: "gte", "≥" }
                    option { value: "lte", "≤" }
                }
                input {
                    class: "w-16 rounded border border-gray-300 px-1.5 py-1 text-sm",
                    "type": "number",
                    step: "any",
                    disabled: !can_write,
                    value: "{rule.value}",
                    onchange: move |e| {
                        let v = e.value().parse::<f64>().unwrap_or(0.0);
                        edit_rule(cx, i, |r| r.value = v);
                    },
                }
                input {
                    class: if has_color { "h-7 w-8 cursor-pointer rounded border border-gray-300" } else { "h-7 w-8 cursor-pointer rounded border border-dashed border-gray-300 opacity-40" },
                    "type": "color",
                    disabled: !can_write,
                    value: "{color}",
                    oninput: move |e| {
                        let v = e.value();
                        edit_rule(cx, i, |r| r.color = Some(v));
                    },
                }
                button {
                    class: if can_write { "ml-auto text-gray-300 hover:text-red-500" } else { "hidden" },
                    onclick: move |_| edit_widget(
                        cx,
                        |w| {
                            if i < w.options.states.len() {
                                w.options.states.remove(i);
                            }
                        },
                    ),
                    icons::Trash { class: "h-3.5 w-3.5" }
                }
            }
            div { class: "flex items-start gap-1.5",
                input {
                    class: "min-w-0 flex-1 rounded border border-gray-300 px-1.5 py-1 text-sm",
                    maxlength: "{pnex_core::STATE_LABEL_MAX}",
                    placeholder: t!("appear-state-label").to_string(),
                    disabled: !can_write,
                    value: "{label}",
                    oninput: move |e| {
                        let v = e.value();
                        edit_rule(cx, i, |r| r.label = (!v.trim().is_empty()).then_some(v));
                    },
                }
                HomeIconPicker {
                    value: rule.icon.clone(),
                    disabled: !can_write,
                    on_change: move |v: Option<String>| edit_rule(cx, i, |r| r.icon = v),
                }
            }
        }
    }
}
