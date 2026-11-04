//! One property filter of a query, for the app builder and the dashboard
//! editor: a property, a comparison, a value as typed.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{FilterOp, PropertyFilter};
use serde_json::Value;

use crate::components::ui::{BTN_SECONDARY_SM, INPUT};

const OPS: [(FilterOp, &str); 10] = [
    (FilterOp::Eq, "eq"),
    (FilterOp::Ne, "ne"),
    (FilterOp::Lt, "lt"),
    (FilterOp::Lte, "lte"),
    (FilterOp::Gt, "gt"),
    (FilterOp::Gte, "gte"),
    (FilterOp::Contains, "contains"),
    (FilterOp::In, "in"),
    (FilterOp::Exists, "exists"),
    (FilterOp::Missing, "missing"),
];

fn op_label(op: FilterOp) -> String {
    match op {
        FilterOp::Eq => t!("apps-op-eq").to_string(),
        FilterOp::Ne => t!("apps-op-ne").to_string(),
        FilterOp::Lt => t!("apps-op-lt").to_string(),
        FilterOp::Lte => t!("apps-op-lte").to_string(),
        FilterOp::Gt => t!("apps-op-gt").to_string(),
        FilterOp::Gte => t!("apps-op-gte").to_string(),
        FilterOp::Contains => t!("apps-op-contains").to_string(),
        FilterOp::In => t!("apps-op-in").to_string(),
        FilterOp::Exists => t!("apps-op-exists").to_string(),
        FilterOp::Missing => t!("apps-op-missing").to_string(),
    }
}

/// A filter's value as typed: a list is comma-separated.
fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn value_of(op: FilterOp, text: &str) -> Value {
    match op {
        FilterOp::In => Value::Array(
            text.split(',')
                .map(|s| Value::String(s.trim().to_string()))
                .collect(),
        ),
        FilterOp::Exists | FilterOp::Missing => Value::Null,
        // The server compares a number as a number, a string as text, where
        // "900" comes after "1000".
        FilterOp::Lt | FilterOp::Lte | FilterOp::Gt | FilterOp::Gte => text
            .parse::<serde_json::Number>()
            .map_or_else(|_| Value::String(text.to_string()), Value::Number),
        _ => Value::String(text.to_string()),
    }
}

#[component]
pub fn FilterRow(
    i: usize,
    filter: PropertyFilter,
    props: Vec<(String, String)>,
    placeholder: String,
    #[props(default = false)] disabled: bool,
    on_change: EventHandler<PropertyFilter>,
    on_remove: EventHandler<()>,
) -> Element {
    let shows_value = !matches!(filter.op, FilterOp::Exists | FilterOp::Missing);
    let op = filter.op;
    let value = value_text(&filter.value);
    let on_key = filter.clone();
    let on_op = filter.clone();
    let on_value = filter.clone();
    rsx! {
        div { class: "flex flex-wrap items-center gap-2", "data-filter": "{i}",
            select {
                class: "{INPUT} max-w-[12rem]",
                disabled,
                onchange: move |e| {
                    on_change
                        .call(PropertyFilter {
                            key: e.value(),
                            ..on_key.clone()
                        });
                },
                for (k, l) in props.iter().cloned() {
                    option { key: "{k}", value: "{k}", selected: k == filter.key, "{l}" }
                }
            }
            select {
                class: "{INPUT} max-w-[10rem]",
                disabled,
                onchange: move |e| {
                    let picked = OPS
                        .iter()
                        .find(|(_, k)| *k == e.value())
                        .map(|(o, _)| *o)
                        .unwrap_or(FilterOp::Eq);
                    let text = value_text(&on_op.value);
                    on_change
                        .call(PropertyFilter {
                            op: picked,
                            value: value_of(picked, &text),
                            ..on_op.clone()
                        });
                },
                for (o, k) in OPS {
                    option { key: "{k}", value: k, selected: o == op, {op_label(o)} }
                }
            }
            if shows_value {
                input {
                    class: "{INPUT} max-w-[14rem]",
                    disabled,
                    placeholder: "{placeholder}",
                    value: "{value}",
                    oninput: move |e| {
                        on_change
                            .call(PropertyFilter {
                                value: value_of(op, &e.value()),
                                ..on_value.clone()
                            });
                    },
                }
            }
            button {
                r#type: "button",
                class: BTN_SECONDARY_SM,
                disabled,
                onclick: move |_| on_remove.call(()),
                {t!("common-delete")}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_comparison_sends_a_number_when_it_can() {
        assert_eq!(value_of(FilterOp::Gt, "1000"), json!(1000));
        // Half typed: left alone, or the field would rewrite it under the cursor.
        assert_eq!(value_of(FilterOp::Gt, "1."), json!("1."));
        assert_eq!(value_of(FilterOp::Lte, "2026-01-01"), json!("2026-01-01"));
        assert_eq!(value_of(FilterOp::Gte, "$today-30"), json!("$today-30"));
        // An exact match compares stored text: it stays as typed.
        assert_eq!(value_of(FilterOp::Eq, "1000"), json!("1000"));
    }

    #[test]
    fn a_value_reads_back_as_typed() {
        for (op, text) in [
            (FilterOp::Gt, "1000"),
            (FilterOp::In, "a, b"),
            (FilterOp::Eq, "x"),
        ] {
            assert_eq!(value_text(&value_of(op, text)), text);
        }
    }
}
