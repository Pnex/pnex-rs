//! The form of a rule: what it watches first, then only the fields of that kind.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{FilterOp, PropertyFilter};
use pnex_core::rules::{Cmp, Rule, RuleCondition, RuleInput, RuleKind};
use serde_json::Value;

use super::cmp_label;
use crate::api;
use crate::api::error_i18n::localize;
use crate::components::crud::form::FormDialog;
use crate::components::ontology::filter_row::FilterRow;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_ERROR, FIELD_LABEL, INPUT};

const CMPS: [Cmp; 4] = [Cmp::Gt, Cmp::Gte, Cmp::Lt, Cmp::Lte];
const RADIO_ROW: &str = "flex items-start gap-3 rounded-md border border-line px-3 py-2 cursor-pointer has-[:checked]:border-signal has-[:checked]:bg-signal-50";

#[component]
pub(super) fn RuleForm(
    initial: Option<Rule>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let start = initial.clone().map(|r| r.condition).unwrap_or_default();
    let mut name = use_signal(|| initial.as_ref().map(|r| r.name.clone()).unwrap_or_default());
    let mut kind = use_signal(|| start.kind);
    let mut type_key = use_signal(|| start.type_key.clone());
    let mut filters = use_signal(|| start.filters.clone());
    let mut device = use_signal(|| start.device_id.clone());
    let mut metric = use_signal(|| start.metric.clone());
    let mut cmp = use_signal(|| start.cmp);
    let mut threshold = use_signal(|| start.threshold.to_string());
    let mut hold = use_signal(|| start.hold_minutes.to_string());
    let mut channel = use_signal(|| {
        initial
            .as_ref()
            .and_then(|r| r.channel_id.clone())
            .unwrap_or_default()
    });
    let mut enabled = use_signal(|| initial.as_ref().is_none_or(|r| r.enabled));
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let id = initial.as_ref().map(|r| r.id.clone());
    let version = initial.as_ref().map(|r| r.version);
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let channels =
        use_resource(|| async { api::notify::list_channels().await.unwrap_or_default() });

    let input = move || RuleInput {
        name: name().trim().to_string(),
        condition: RuleCondition {
            kind: kind(),
            type_key: type_key(),
            filters: filters(),
            device_id: device().trim().to_string(),
            metric: metric().trim().to_string(),
            cmp: cmp(),
            threshold: threshold()
                .trim()
                .replace(',', ".")
                .parse()
                .unwrap_or(f64::NAN),
            hold_minutes: hold().trim().parse().unwrap_or(0),
        },
        channel_id: Some(channel()).filter(|c| !c.is_empty()),
        enabled: enabled(),
        expected_version: version,
    };
    let submit = move |_| {
        let id = id.clone();
        let body = input();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::rules::save(id.as_deref(), &body).await {
                Ok(_) => on_saved.call(()),
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    let all_types = types.read().clone().unwrap_or_default();
    let chosen_type = type_key();
    let props: Vec<(String, String)> = all_types
        .iter()
        .find(|t| t.def.key == chosen_type)
        .map(|t| {
            t.def
                .properties
                .iter()
                .map(|p| (p.key.clone(), p.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let type_choices: Vec<(String, String)> = all_types
        .iter()
        .map(|t| {
            (
                t.def.key.clone(),
                crate::components::ontology::type_label(&t.def),
            )
        })
        .collect();
    let all_channels = channels.read().clone().unwrap_or_default();
    let chosen_channel = channel();
    let chosen_cmp = cmp();
    let k = kind();
    let title = if initial.is_some() {
        t!("rules-edit")
    } else {
        t!("rules-add")
    };
    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("pipe-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: busy(),
            valid: input().violation().is_none(),
            max_width: "max-w-xl".to_string(),
            div { class: "space-y-4",
                div {
                    label { class: FIELD_LABEL, r#for: "rule-name", {t!("pipe-col-name")} }
                    input {
                        id: "rule-name",
                        class: INPUT,
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                fieldset { class: "space-y-2",
                    legend { class: FIELD_LABEL, {t!("rules-kind")} }
                    for (value, label, help) in [
                        (RuleKind::Objects, t!("rules-kind-objects"), t!("rules-kind-objects-help")),
                        (RuleKind::Series, t!("rules-kind-series"), t!("rules-kind-series-help")),
                    ]
                    {
                        label { key: "{label}", class: RADIO_ROW,
                            input {
                                r#type: "radio",
                                name: "rule-kind",
                                class: "mt-1",
                                checked: k == value,
                                onchange: move |_| kind.set(value),
                            }
                            span {
                                span { class: "block text-sm font-medium text-ink", "{label}" }
                                span { class: "block text-sm text-gray-600", "{help}" }
                            }
                        }
                    }
                }
                if k == RuleKind::Objects {
                    div {
                        label { class: FIELD_LABEL, r#for: "rule-type", {t!("rules-type")} }
                        select {
                            id: "rule-type",
                            class: INPUT,
                            onchange: move |e| {
                                type_key.set(e.value());
                                filters.set(Vec::new());
                            },
                            option { value: "", selected: chosen_type.is_empty(), {t!("gov-choose")} }
                            for (key, label) in type_choices {
                                option {
                                    key: "{key}",
                                    value: "{key}",
                                    selected: chosen_type == key,
                                    "{label}"
                                }
                            }
                        }
                    }
                    if !chosen_type.is_empty() {
                        div { class: "space-y-2",
                            p { class: FIELD_LABEL, {t!("rules-filters")} }
                            for (i, f) in filters().into_iter().enumerate() {
                                FilterRow {
                                    key: "{i}",
                                    i,
                                    filter: f,
                                    props: props.clone(),
                                    placeholder: String::new(),
                                    on_change: move |next: PropertyFilter| filters.with_mut(|v| v[i] = next),
                                    on_remove: move |_| {
                                        filters
                                            .with_mut(|v| {
                                                v.remove(i);
                                            })
                                    },
                                }
                            }
                            button {
                                r#type: "button",
                                class: BTN_SECONDARY_SM,
                                disabled: props.is_empty(),
                                onclick: move |_| {
                                    let first = props.first().map(|p| p.0.clone()).unwrap_or_default();
                                    filters
                                        .with_mut(|v| {
                                            v.push(PropertyFilter {
                                                key: first,
                                                op: FilterOp::Eq,
                                                value: Value::Null,
                                            })
                                        });
                                },
                                {t!("rules-add-filter")}
                            }
                        }
                    }
                } else {
                    div { class: "grid grid-cols-1 gap-3 sm:grid-cols-2",
                        div {
                            label { class: FIELD_LABEL, r#for: "rule-device", {t!("rules-device")} }
                            input {
                                id: "rule-device",
                                class: INPUT,
                                value: "{device}",
                                oninput: move |e| device.set(e.value()),
                            }
                        }
                        div {
                            label { class: FIELD_LABEL, r#for: "rule-metric", {t!("rules-metric")} }
                            input {
                                id: "rule-metric",
                                class: INPUT,
                                value: "{metric}",
                                oninput: move |e| metric.set(e.value()),
                            }
                        }
                    }
                    div { class: "grid grid-cols-1 gap-3 sm:grid-cols-3",
                        div {
                            label { class: FIELD_LABEL, r#for: "rule-cmp", {t!("rules-cmp")} }
                            select {
                                id: "rule-cmp",
                                class: INPUT,
                                onchange: move |e| {
                                    let picked = CMPS.into_iter().nth(e.value().parse().unwrap_or(0));
                                    cmp.set(picked.unwrap_or_default());
                                },
                                for (i, c) in CMPS.into_iter().enumerate() {
                                    option {
                                        key: "{i}",
                                        value: "{i}",
                                        selected: chosen_cmp == c,
                                        "{cmp_label(c)}"
                                    }
                                }
                            }
                        }
                        div {
                            label { class: FIELD_LABEL, r#for: "rule-threshold",
                                {t!("rules-threshold")}
                            }
                            input {
                                id: "rule-threshold",
                                class: "{INPUT} tabular-nums",
                                inputmode: "decimal",
                                value: "{threshold}",
                                oninput: move |e| threshold.set(e.value()),
                            }
                        }
                        div {
                            label { class: FIELD_LABEL, r#for: "rule-hold", {t!("rules-hold")} }
                            input {
                                id: "rule-hold",
                                class: "{INPUT} tabular-nums",
                                r#type: "number",
                                min: "0",
                                value: "{hold}",
                                oninput: move |e| hold.set(e.value()),
                            }
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "rule-channel", {t!("rules-channel")} }
                    select {
                        id: "rule-channel",
                        class: INPUT,
                        onchange: move |e| channel.set(e.value()),
                        option { value: "", selected: chosen_channel.is_empty(),
                            {t!("rules-channel-in-app")}
                        }
                        for c in all_channels {
                            option {
                                key: "{c.id}",
                                value: "{c.id}",
                                selected: chosen_channel == c.id.to_string(),
                                "{c.name}"
                            }
                        }
                    }
                }
                label { class: "flex items-center gap-2 text-sm text-ink",
                    input {
                        r#type: "checkbox",
                        checked: enabled(),
                        onchange: move |e| enabled.set(e.checked()),
                    }
                    {t!("rules-enabled")}
                }
                if let Some(e) = error() {
                    p { class: FIELD_ERROR, role: "alert", "{e}" }
                }
            }
        }
    }
}
