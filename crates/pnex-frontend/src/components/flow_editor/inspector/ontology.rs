use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::ontology::flow::{
    LinkUpsertConfig, ObjectUpsertConfig, OntologyQueryConfig, QueryOutput,
};

// ─────────────── ontology nodes (partner-watch.md D312) ───────────────

const SELECT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";

/// Object types of the org, `(key, label, def)`.
fn type_choices(
    types: &[pnex_core::ontology::api::ObjectTypeView],
    writable: bool,
) -> Vec<(String, String)> {
    types
        .iter()
        .filter(|t| !writable || !t.def.native())
        .map(|t| {
            (
                t.def.key.clone(),
                crate::components::ontology::type_label(&t.def),
            )
        })
        .collect()
}

#[component]
pub(super) fn OntologyQueryForm(
    mut cx: EditorCx,
    initial: OntologyQueryConfig,
    can_write: bool,
) -> Element {
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let all = types.read().clone().unwrap_or_default();
    let choices = type_choices(&all, false);
    let current = initial.clone();
    let mut filters = use_signal(move || {
        serde_json::to_string_pretty(&current.filters).unwrap_or_else(|_| "[]".into())
    });
    let mut filters_bad = use_signal(|| false);
    let props: Vec<String> = all
        .iter()
        .find(|t| t.def.key == initial.object_type)
        .map(|t| {
            t.def
                .properties
                .iter()
                .filter(|p| !p.kind.is_temporal())
                .map(|p| p.key.clone())
                .collect()
        })
        .unwrap_or_default();
    let base = initial.clone();
    let mut patch = move |f: Box<dyn FnOnce(&mut OntologyQueryConfig)>| {
        let mut next = base.clone();
        f(&mut next);
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::OntologyQuery { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let mut p1 = patch.clone();
    let mut p2 = patch.clone();
    let mut p3 = patch.clone();
    let mut p4 = patch.clone();
    let mut p5 = patch.clone();
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-ontology-query-help")} }
            label { class: FIELD_LABEL, {t!("flows-ontology-type")} }
            select {
                class: SELECT,
                "data-testid": "ontology-query-type",
                disabled: !can_write,
                onchange: move |e| {
                    let v = e.value();
                    p1(Box::new(move |c| c.object_type = v));
                },
                option { value: "", selected: initial.object_type.is_empty(), "—" }
                for (k, label) in choices {
                    option {
                        key: "{k}",
                        value: "{k}",
                        selected: k == initial.object_type,
                        "{label}"
                    }
                }
            }
            label { class: FIELD_LABEL, {t!("flows-ontology-filters")} }
            textarea {
                class: "w-full h-32 px-2 py-1 border border-gray-300 rounded-lg text-xs font-mono",
                disabled: !can_write,
                value: "{filters}",
                oninput: move |e| filters.set(e.value()),
                onchange: move |e| match serde_json::from_str(&e.value()) {
                    Ok(list) => {
                        filters_bad.set(false);
                        p2(Box::new(move |c| c.filters = list));
                    }
                    Err(_) => filters_bad.set(true),
                },
            }
            if filters_bad() {
                p { class: "text-xs text-red-600", {t!("flows-ontology-filters-invalid")} }
            }
            p { class: "text-xs text-gray-500", {t!("flows-ontology-filters-help")} }
            div { class: "grid grid-cols-2 gap-2",
                div {
                    label { class: FIELD_LABEL, {t!("flows-ontology-order")} }
                    select {
                        class: SELECT,
                        disabled: !can_write,
                        onchange: move |e| {
                            let v = Some(e.value()).filter(|v| !v.is_empty());
                            p3(Box::new(move |c| c.order_by = v));
                        },
                        option { value: "", selected: initial.order_by.is_none(),
                            {t!("flows-ontology-order-title")}
                        }
                        for k in props {
                            option {
                                key: "{k}",
                                value: "{k}",
                                selected: initial.order_by.as_deref() == Some(k.as_str()),
                                "{k}"
                            }
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, {t!("flows-ontology-limit")} }
                    input {
                        class: SELECT,
                        r#type: "number",
                        min: "1",
                        max: "500",
                        disabled: !can_write,
                        value: "{initial.limit.unwrap_or(100)}",
                        onchange: move |e| {
                            let v = e.value().parse::<u64>().ok();
                            p4(Box::new(move |c| c.limit = v));
                        },
                    }
                }
            }
            label { class: FIELD_LABEL, {t!("flows-ontology-output")} }
            select {
                class: SELECT,
                disabled: !can_write,
                onchange: move |e| {
                    let v = if e.value() == "array" {
                        QueryOutput::Array
                    } else {
                        QueryOutput::Each
                    };
                    p5(Box::new(move |c| c.output = v));
                },
                option {
                    value: "each",
                    selected: initial.output == QueryOutput::Each,
                    {t!("flows-ontology-output-each")}
                }
                option {
                    value: "array",
                    selected: initial.output == QueryOutput::Array,
                    {t!("flows-ontology-output-array")}
                }
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: initial.descending,
                    onchange: move |e| {
                        let v = e.checked();
                        patch(Box::new(move |c| c.descending = v));
                    },
                }
                {t!("flows-ontology-descending")}
            }
        }
    }
}

#[component]
pub(super) fn ObjectUpsertForm(
    mut cx: EditorCx,
    initial: ObjectUpsertConfig,
    can_write: bool,
) -> Element {
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let all = types.read().clone().unwrap_or_default();
    let choices = type_choices(&all, true);
    let keys: Vec<String> = all
        .iter()
        .find(|t| t.def.key == initial.object_type)
        .map(|t| {
            t.def
                .properties
                .iter()
                .filter(|p| p.unique)
                .map(|p| p.key.clone())
                .collect()
        })
        .unwrap_or_default();
    let base = initial.clone();
    let mut patch = move |next: ObjectUpsertConfig| {
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::ObjectUpsert { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let mut p1 = patch;
    let mut p2 = patch;
    let mut p3 = patch;
    let (b1, b2, b3) = (base.clone(), base.clone(), base.clone());
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-object-upsert-help")} }
            label { class: FIELD_LABEL, {t!("flows-ontology-type")} }
            select {
                class: SELECT,
                "data-testid": "object-upsert-type",
                disabled: !can_write,
                onchange: move |e| {
                    p1(ObjectUpsertConfig {
                        object_type: e.value(),
                        key: String::new(),
                        ..b1.clone()
                    });
                },
                option { value: "", selected: initial.object_type.is_empty(), "—" }
                for (k, label) in choices {
                    option {
                        key: "{k}",
                        value: "{k}",
                        selected: k == initial.object_type,
                        "{label}"
                    }
                }
            }
            label { class: FIELD_LABEL, {t!("flows-object-upsert-key")} }
            select {
                class: SELECT,
                disabled: !can_write,
                onchange: move |e| {
                    p2(ObjectUpsertConfig {
                        key: e.value(),
                        ..b2.clone()
                    });
                },
                option { value: "", selected: initial.key.is_empty(), "—" }
                option { value: "id", selected: initial.key == "id",
                    {t!("flows-object-upsert-key-id")}
                }
                for k in keys {
                    option {
                        key: "{k}",
                        value: "{k}",
                        selected: k == initial.key,
                        "{k}"
                    }
                }
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: initial.skip_existing,
                    onchange: move |e| {
                        patch(ObjectUpsertConfig {
                            skip_existing: e.checked(),
                            ..base.clone()
                        });
                    },
                }
                {t!("flows-object-upsert-skip-existing")}
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: initial.propose,
                    onchange: move |e| {
                        p3(ObjectUpsertConfig {
                            propose: e.checked(),
                            ..b3.clone()
                        });
                    },
                }
                {t!("flows-object-upsert-propose")}
            }
        }
    }
}

#[component]
pub(super) fn LinkUpsertForm(
    mut cx: EditorCx,
    initial: LinkUpsertConfig,
    can_write: bool,
) -> Element {
    let links = use_resource(|| async { api::ontology::link_types().await.unwrap_or_default() });
    let all = links.read().clone().unwrap_or_default();
    let base = initial.clone();
    let mut patch = move |next: LinkUpsertConfig| {
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::LinkUpsert { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let mut p1 = patch;
    let b1 = base.clone();
    let mut p2 = patch;
    let b2 = base.clone();
    let mut p3 = patch;
    let b3 = base.clone();
    let mut p4 = patch;
    let b4 = base.clone();
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-link-upsert-help")} }
            label { class: FIELD_LABEL, {t!("flows-link-upsert-type")} }
            select {
                class: SELECT,
                "data-testid": "link-upsert-type",
                disabled: !can_write,
                onchange: move |e| {
                    p1(LinkUpsertConfig {
                        link_type: e.value(),
                        ..b1.clone()
                    });
                },
                option { value: "", selected: initial.link_type.is_empty(), "—" }
                for l in all {
                    option {
                        key: "{l.def.key}",
                        value: "{l.def.key}",
                        selected: l.def.key == initial.link_type,
                        {crate::components::ontology::link_label(Some(&l.def), &l.def.key, false)}
                    }
                }
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: initial.close,
                    onchange: move |e| {
                        patch(LinkUpsertConfig {
                            close: e.checked(),
                            ..base.clone()
                        });
                    },
                }
                {t!("flows-link-upsert-close")}
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write || initial.close,
                    checked: initial.skip_closed,
                    onchange: move |e| {
                        p2(LinkUpsertConfig {
                            skip_closed: e.checked(),
                            ..b2.clone()
                        });
                    },
                }
                {t!("flows-link-upsert-skip-closed")}
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write || initial.close,
                    checked: initial.propose,
                    onchange: move |e| {
                        p3(LinkUpsertConfig {
                            propose: e.checked(),
                            ..b3.clone()
                        });
                    },
                }
                {t!("flows-link-upsert-propose")}
            }
            label { class: FIELD_LABEL, {t!("flows-link-upsert-keep-best")} }
            input {
                class: SELECT,
                "data-testid": "link-upsert-keep-best",
                disabled: !can_write || initial.close,
                value: "{initial.keep_best}",
                onchange: move |e| {
                    p4(LinkUpsertConfig {
                        keep_best: e.value().trim().to_string(),
                        ..b4.clone()
                    });
                },
            }
        }
    }
}
