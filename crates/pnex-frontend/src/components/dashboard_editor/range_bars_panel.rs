//! Inspector section of the `range_bars` widget (D182): aggregation op,
//! window relative to now, and bucket mode — the time ranges of a stream or
//! time-of-day slices (bounds + timezone). The series itself is the widget
//! source, picked in the generic binding block.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::aggregate::{
    AggregateOp, AggregateWidgetOptions, WidgetBuckets, DEFAULT_TIMEZONE, WIDGET_WINDOWS,
};
use pnex_core::{DashboardLayout, Widget};

use crate::api;

use super::{EditorCx, Selection};

const SELECT: &str = "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white";
const INPUT: &str = "w-full px-2 py-1.5 border border-gray-300 rounded text-sm";
const LABEL: &str = "block text-[10px] font-medium uppercase text-gray-400";

/// Mutates the aggregation options of the selected widget.
fn edit(mut cx: EditorCx, f: impl FnOnce(&mut AggregateWidgetOptions)) {
    let Some(Selection::Widget(id)) = cx.selected.cloned() else {
        return;
    };
    cx.layout.with_mut(|l: &mut DashboardLayout| {
        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
            f(w.options.aggregate.get_or_insert_with(Default::default));
        }
    });
}

fn op_label(op: AggregateOp) -> String {
    match op {
        AggregateOp::Sum => t!("rb-op-sum").to_string(),
        AggregateOp::Avg => t!("rb-op-avg").to_string(),
        AggregateOp::Max => t!("rb-op-max").to_string(),
        AggregateOp::Min => t!("rb-op-min").to_string(),
        AggregateOp::Increase => t!("rb-op-increase").to_string(),
    }
}

fn window_label(key: &str) -> String {
    match key {
        "24h" => t!("rb-window-24h").to_string(),
        _ => t!("rb-window-7d").to_string(),
    }
}

/// `"00:00, 06:00"` → bounds (validated at save, shown as typed).
fn parse_bounds(raw: &str) -> Vec<String> {
    raw.split([',', ';', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[component]
pub fn RangeBarsPanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let opts = widget.options.aggregate.clone().unwrap_or_default();
    let streams = use_resource(|| async {
        api::media_streams::list(Some(100), Some(0))
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });
    let streams = streams.read().clone().unwrap_or_default();
    let is_ranges = matches!(opts.buckets, WidgetBuckets::Ranges { .. });
    let (stream_id, bounds, timezone) = match &opts.buckets {
        WidgetBuckets::Ranges { stream_id } => (stream_id.clone(), String::new(), String::new()),
        WidgetBuckets::Slices { bounds, timezone } => (
            String::new(),
            bounds.join(", "),
            timezone.clone().unwrap_or_default(),
        ),
    };
    let current_op = opts.op;
    let current_window = opts.window.clone();

    let on_mode = move |e: FormEvent| {
        let buckets = if e.value() == "ranges" {
            WidgetBuckets::Ranges {
                stream_id: String::new(),
            }
        } else {
            AggregateWidgetOptions::default().buckets
        };
        edit(cx, |o| o.buckets = buckets);
    };
    let on_bounds = move |e: FormEvent| {
        let parsed = parse_bounds(&e.value());
        edit(cx, |o| {
            if let WidgetBuckets::Slices { bounds, .. } = &mut o.buckets {
                *bounds = parsed;
            }
        });
    };
    let on_stream = move |e: FormEvent| {
        let stream_id = e.value();
        edit(cx, |o| o.buckets = WidgetBuckets::Ranges { stream_id });
    };
    let on_timezone = move |e: FormEvent| {
        let v = e.value().trim().to_string();
        edit(cx, |o| {
            if let WidgetBuckets::Slices { timezone, .. } = &mut o.buckets {
                *timezone = (!v.is_empty()).then_some(v);
            }
        });
    };

    rsx! {
        div { class: "space-y-3",
            div { class: "grid grid-cols-2 gap-2",
                div {
                    label { r#for: "rb-op", class: LABEL, {t!("rb-op")} }
                    select {
                        id: "rb-op",
                        class: SELECT,
                        disabled: !can_write,
                        onchange: move |e| {
                            if let Some(op) = AggregateOp::from_wire(&e.value()) {
                                edit(cx, |o| o.op = op);
                            }
                        },
                        for op in AggregateOp::ALL {
                            option {
                                key: "{op.wire()}",
                                value: "{op.wire()}",
                                selected: op == current_op,
                                {op_label(op)}
                            }
                        }
                    }
                }
                div {
                    label { r#for: "rb-window", class: LABEL, {t!("rb-window")} }
                    select {
                        id: "rb-window",
                        class: SELECT,
                        disabled: !can_write,
                        onchange: move |e| {
                            let v = e.value();
                            edit(cx, |o| o.window = v);
                        },
                        for (key, _) in WIDGET_WINDOWS {
                            option {
                                key: "{key}",
                                value: "{key}",
                                selected: current_window == *key,
                                {window_label(key)}
                            }
                        }
                    }
                }
            }
            div {
                label { r#for: "rb-mode", class: LABEL, {t!("rb-mode")} }
                select {
                    id: "rb-mode",
                    class: SELECT,
                    disabled: !can_write,
                    onchange: on_mode,
                    option { value: "slices", selected: !is_ranges, {t!("rb-mode-slices")} }
                    option { value: "ranges", selected: is_ranges, {t!("rb-mode-ranges")} }
                }
            }
            if is_ranges {
                div {
                    label { r#for: "rb-stream", class: LABEL, {t!("rb-stream")} }
                    select {
                        id: "rb-stream",
                        class: SELECT,
                        disabled: !can_write,
                        onchange: on_stream,
                        if stream_id.is_empty() {
                            option { value: "", selected: true, disabled: true,
                                {t!("rb-pick-stream")}
                            }
                        }
                        for s in streams.iter() {
                            option {
                                key: "{s.id}",
                                value: "{s.id}",
                                selected: s.id == stream_id,
                                "{s.name}"
                            }
                        }
                    }
                }
            } else {
                div {
                    label { r#for: "rb-bounds", class: LABEL, {t!("rb-bounds")} }
                    input {
                        id: "rb-bounds",
                        class: INPUT,
                        disabled: !can_write,
                        value: "{bounds}",
                        placeholder: "00:00, 06:00, 09:00",
                        onchange: on_bounds,
                    }
                    p { class: "text-[10px] text-gray-400 mt-1", {t!("rb-bounds-help")} }
                }
                div {
                    label { r#for: "rb-timezone", class: LABEL, {t!("rb-timezone")} }
                    input {
                        id: "rb-timezone",
                        class: INPUT,
                        disabled: !can_write,
                        value: "{timezone}",
                        placeholder: DEFAULT_TIMEZONE,
                        onchange: on_timezone,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_text_splits_on_separators() {
        assert_eq!(
            parse_bounds(" 00:00, 06:00;09:00  12:00,"),
            vec!["00:00", "06:00", "09:00", "12:00"]
        );
        assert!(parse_bounds("  ").is_empty());
    }
}
