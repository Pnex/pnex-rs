//! Inspector section of the `symbol` widget: colours, rotation, mirror,
//! stretch, and the opt-in live source (fill coloured by thresholds, value
//! caption). The source binding itself reuses the generic inspector block.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{DashboardLayout, SourceRef, SymbolOptions, Threshold, Widget};

use crate::components::icons;
use crate::components::symbols::{self, SymbolView};

use super::{EditorCx, Selection};

/// Default colour of a new state threshold (green = running).
const NEW_THRESHOLD_COLOR: &str = "#22c55e";

fn selected_id(cx: &EditorCx) -> Option<String> {
    match cx.selected.cloned() {
        Some(Selection::Widget(id)) => Some(id),
        _ => None,
    }
}

/// Mutates the selected widget (no-op when the selection vanished).
fn edit_widget(mut cx: EditorCx, f: impl FnOnce(&mut Widget)) {
    let Some(id) = selected_id(&cx) else { return };
    cx.layout.with_mut(|l: &mut DashboardLayout| {
        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
            f(w);
        }
    });
}

/// Mutates the symbol options of the selected widget.
fn edit_symbol(cx: EditorCx, f: impl FnOnce(&mut SymbolOptions)) {
    edit_widget(cx, |w| {
        f(w.options.symbol.get_or_insert_with(Default::default))
    });
}

#[component]
pub fn SymbolOptionsPanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let opts = widget.options.symbol.clone().unwrap_or_default();
    let name = symbols::find(&opts.shape)
        .map(symbols::symbol_name)
        .unwrap_or_else(|| opts.shape.clone());
    let stroke = opts
        .stroke
        .clone()
        .unwrap_or_else(|| symbols::DEFAULT_STROKE.to_string());
    let fill = opts
        .fill
        .clone()
        .unwrap_or_else(|| symbols::DEFAULT_FILL.to_string());
    let live = !widget.source.is_empty();
    let thresholds = widget.options.thresholds.clone();

    rsx! {
        div { class: "space-y-3",
            div { class: "flex items-center gap-3 rounded-lg border border-gray-100 bg-gray-50 p-2",
                div { class: "h-12 w-12 shrink-0",
                    SymbolView {
                        shape: opts.shape.clone(),
                        stroke: opts.stroke.clone(),
                        fill: opts.fill.clone(),
                        rotation: opts.rotation,
                        flip: opts.flip,
                    }
                }
                p { class: "min-w-0 text-xs font-medium text-gray-700", "{name}" }
            }
            div { class: "grid grid-cols-2 gap-2",
                div {
                    label {
                        r#for: "symbol-options-field-1",
                        class: "block text-[10px] font-medium uppercase text-gray-400",
                        {t!("sym-stroke")}
                    }
                    input {
                        id: "symbol-options-field-1",
                        class: "h-8 w-full cursor-pointer rounded border border-gray-300",
                        "type": "color",
                        disabled: !can_write,
                        value: "{stroke}",
                        oninput: move |e| {
                            let v = e.value();
                            edit_symbol(cx, |o| o.stroke = Some(v));
                        },
                    }
                }
                div {
                    label {
                        r#for: "symbol-options-field-2",
                        class: "block text-[10px] font-medium uppercase text-gray-400",
                        {t!("sym-fill")}
                    }
                    input {
                        id: "symbol-options-field-2",
                        class: "h-8 w-full cursor-pointer rounded border border-gray-300",
                        "type": "color",
                        disabled: !can_write,
                        value: "{fill}",
                        oninput: move |e| {
                            let v = e.value();
                            edit_symbol(cx, |o| o.fill = Some(v));
                        },
                    }
                }
                div {
                    label {
                        r#for: "symbol-options-field-3",
                        class: "block text-[10px] font-medium uppercase text-gray-400",
                        {t!("sym-rotation")}
                    }
                    select {
                        id: "symbol-options-field-3",
                        class: "w-full rounded border border-gray-300 bg-white px-2 py-1.5 text-sm",
                        disabled: !can_write,
                        onchange: move |e| {
                            let v = e.value().parse::<u16>().unwrap_or(0);
                            edit_symbol(cx, |o| o.rotation = v);
                        },
                        for deg in [0_u16, 90, 180, 270] {
                            option {
                                key: "{deg}",
                                value: "{deg}",
                                selected: opts.rotation == deg,
                                "{deg}°"
                            }
                        }
                    }
                }
                div { class: "flex flex-col justify-end gap-1 pb-1",
                    label { class: "flex items-center gap-2 text-xs text-gray-600",
                        input {
                            "type": "checkbox",
                            disabled: !can_write,
                            checked: opts.flip,
                            onchange: move |e| {
                                let v = e.checked();
                                edit_symbol(cx, |o| o.flip = v);
                            },
                        }
                        {t!("sym-flip")}
                    }
                    label { class: "flex items-center gap-2 text-xs text-gray-600",
                        input {
                            "type": "checkbox",
                            disabled: !can_write,
                            checked: opts.stretch,
                            onchange: move |e| {
                                let v = e.checked();
                                edit_symbol(cx, |o| o.stretch = v);
                            },
                        }
                        {t!("sym-stretch")}
                    }
                }
            }
            // Live source: opt-in (a static symbol has no source at all).
            div { class: "space-y-2 border-t border-gray-100 pt-2",
                label { class: "flex items-center gap-2 text-xs font-medium text-gray-700",
                    input {
                        "type": "checkbox",
                        disabled: !can_write,
                        checked: live,
                        onchange: move |e| {
                            let on = e.checked();
                            edit_widget(
                                cx,
                                |w| {
                                    if on && w.source.is_empty() {
                                        w.source
                                            .push(SourceRef {
                                                role: "primary".into(),
                                                metric: String::new(),
                                                device_id: String::new(),
                                                window: "5m".into(),
                                                memory: None,
                                                labels: Default::default(),
                                            });
                                    } else if !on {
                                        w.source.clear();
                                        if let Some(o) = w.options.symbol.as_mut() {
                                            o.show_value = false;
                                        }
                                    }
                                },
                            );
                        },
                    }
                    {t!("sym-live")}
                }
                p { class: "text-[10px] text-gray-400", {t!("sym-live-help")} }
                if live {
                    label { class: "flex items-center gap-2 text-xs text-gray-600",
                        input {
                            "type": "checkbox",
                            disabled: !can_write,
                            checked: opts.show_value,
                            onchange: move |e| {
                                let v = e.checked();
                                edit_symbol(cx, |o| o.show_value = v);
                            },
                        }
                        {t!("sym-show-value")}
                    }
                    p {
                        id: "symbol-options-states-label",
                        class: "block text-[10px] font-medium uppercase text-gray-400",
                        {t!("sym-states")}
                    }
                    for (i, th) in thresholds.iter().cloned().enumerate() {
                        div {
                            key: "th-{i}",
                            class: "flex items-center gap-2",
                            role: "group",
                            aria_labelledby: "symbol-options-states-label",
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
                                onclick: move |_| {
                                    edit_widget(
                                        cx,
                                        |w| {
                                            if i < w.options.thresholds.len() {
                                                w.options.thresholds.remove(i);
                                            }
                                        },
                                    );
                                },
                                icons::Trash { class: "h-3.5 w-3.5" }
                            }
                        }
                    }
                    button {
                        class: if can_write { "text-xs font-medium text-blue-600 hover:text-blue-800" } else { "hidden" },
                        onclick: move |_| {
                            edit_widget(
                                cx,
                                |w| {
                                    let next = w
                                        .options
                                        .thresholds
                                        .last()
                                        .map(|t| t.value + 1.0)
                                        .unwrap_or(1.0);
                                    w.options
                                        .thresholds
                                        .push(Threshold {
                                            value: next,
                                            color: NEW_THRESHOLD_COLOR.into(),
                                        });
                                },
                            );
                        },
                        {t!("sym-add-state")}
                    }
                    p { class: "text-[10px] text-gray-400", {t!("sym-states-help")} }
                }
            }
        }
    }
}
