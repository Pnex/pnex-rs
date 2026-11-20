//! D829: what a rule proposes when it fires, an object of a type linked to what
//! fired, with a title and its required values. A member validates it in
//! `/proposals`. Shown to whoever may file proposals.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::ObjectTypeView;
use pnex_core::rules::RuleProposal;
use serde_json::{Map, Value};

use crate::api;
use crate::components::ontology::link_label;
use crate::components::ontology::property_input::PropertyInput;
use crate::components::ui::{FIELD_HELP, FIELD_LABEL, INPUT};

#[component]
pub(super) fn ProposeFields(
    propose: Signal<Option<RuleProposal>>,
    values: Signal<Map<String, Value>>,
    types: Vec<ObjectTypeView>,
) -> Element {
    let links = use_resource(|| async { api::ontology::link_types().await.unwrap_or_default() });
    let on = propose.read().is_some();
    let current = propose().unwrap_or_default();
    let chosen = current.type_key.clone();
    let link_choices: Vec<(String, String)> = links
        .read()
        .clone()
        .unwrap_or_default()
        .iter()
        .filter(|l| !chosen.is_empty() && l.def.from_types.contains(&chosen))
        .map(|l| {
            (
                l.def.key.clone(),
                link_label(Some(&l.def), &l.def.key, false),
            )
        })
        .collect();
    let no_links = link_choices.is_empty();
    let required: Vec<_> = types
        .iter()
        .find(|t| t.def.key == chosen)
        .map(|t| {
            t.def
                .properties
                .iter()
                .filter(|p| p.required)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut edit = move |f: &dyn Fn(&mut RuleProposal)| {
        let mut next = propose().unwrap_or_default();
        f(&mut next);
        propose.set(Some(next));
    };
    rsx! {
        fieldset { class: "space-y-3", "data-testid": "rule-propose",
            label { class: "flex items-start gap-2 text-sm text-ink",
                input {
                    r#type: "checkbox",
                    class: "mt-1",
                    checked: on,
                    onchange: move |e| propose.set(e.checked().then(RuleProposal::default)),
                }
                span {
                    span { class: "block font-medium", {t!("rules-propose")} }
                    span { class: "block text-gray-600", {t!("rules-propose-help")} }
                }
            }
            if on {
                div {
                    label { class: FIELD_LABEL, r#for: "rule-propose-type", {t!("rules-type")} }
                    select {
                        id: "rule-propose-type",
                        class: INPUT,
                        onchange: move |e| {
                            let key = e.value();
                            edit(
                                &|p| {
                                    p.type_key = key.clone();
                                    p.link_type.clear();
                                },
                            );
                            values.set(Map::new());
                        },
                        option { value: "", selected: chosen.is_empty(), {t!("gov-choose")} }
                        for t in types.iter() {
                            option {
                                key: "{t.def.key}",
                                value: "{t.def.key}",
                                selected: chosen == t.def.key,
                                {crate::components::ontology::type_label(&t.def)}
                            }
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "rule-propose-link",
                        {t!("rules-propose-link")}
                    }
                    select {
                        id: "rule-propose-link",
                        class: INPUT,
                        disabled: no_links,
                        onchange: move |e| {
                            let key = e.value();
                            edit(&|p| p.link_type = key.clone());
                        },
                        option { value: "", selected: current.link_type.is_empty(), {t!("gov-choose")} }
                        for (key, label) in link_choices {
                            option {
                                key: "{key}",
                                value: "{key}",
                                selected: current.link_type == key,
                                "{label}"
                            }
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "rule-propose-title",
                        {t!("rules-propose-title")}
                    }
                    input {
                        id: "rule-propose-title",
                        class: INPUT,
                        maxlength: 200,
                        value: "{current.title}",
                        oninput: move |e| {
                            let v = e.value();
                            edit(&|p| p.title = v.clone());
                        },
                    }
                    p { class: FIELD_HELP, {t!("rules-propose-title-help")} }
                }
                for def in required {
                    PropertyInput { key: "{def.key}", def: def.clone(), doc: values }
                }
            }
        }
    }
}
