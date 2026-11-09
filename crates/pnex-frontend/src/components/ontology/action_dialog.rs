//! Invocation dialog of an ontology action (D183), shared by the object
//! page and the assistant's proposals: the parameters (pre-filled when the
//! assistant proposed them), then an explicit human click. The server
//! dispatches to the deployed flows; the dialog never acts by itself.

use dioxus::prelude::*;
use dioxus_i18n::t;
use serde_json::{Map, Value};

use crate::api;
use crate::components::crud::form::FormDialog;
use crate::components::ontology::property_input::PropertyInput;
use crate::components::ontology::ref_search::RefSearch;
use crate::state::toasts;
use pnex_core::ontology::api::LinkView;
use pnex_core::ontology::{PropertyDef, PropertyKind};

#[component]
pub fn ActionDialog(
    object_id: String,
    action_key: String,
    #[props(default)] initial: Map<String, Value>,
    /// The application whose screen invokes (app-builder.md D511).
    #[props(default)]
    app_id: Option<String>,
    on_close: Callback<()>,
    #[props(default)] on_done: Option<Callback<()>>,
) -> Element {
    let oid = object_id.clone();
    let ak = action_key.clone();
    let mut doc = use_signal(move || initial.clone());
    // Titles of the `ref` parameters prefilled from the object's links.
    let mut linked = use_signal(std::collections::BTreeMap::<String, (String, String)>::new);
    let loaded = use_resource(move || {
        let id = oid.clone();
        let ak = ak.clone();
        async move {
            let o = api::ontology::object(&id).await?;
            let t = api::ontology::types().await?;
            let params = t
                .iter()
                .find(|v| v.def.key == o.type_key)
                .and_then(|v| v.def.action(&ak))
                .map(|a| a.params.clone())
                .unwrap_or_default();
            if params
                .iter()
                .any(|p| matches!(p.kind, PropertyKind::Ref { .. }))
            {
                // Without its links the dialog still works, unfilled.
                let links = api::ontology::object_links(&id, None, None, false)
                    .await
                    .unwrap_or_default();
                let found = prefill(&id, &params, &links);
                doc.with_mut(|m| {
                    for (k, (v, _)) in &found {
                        m.entry(k.clone())
                            .or_insert_with(|| Value::String(v.clone()));
                    }
                });
                linked.set(found);
            }
            Ok::<_, api::error::ApiError>((o, t))
        }
    });
    let mut busy = use_signal(|| false);
    let mut errors = use_signal(Map::<String, Value>::new);

    let loaded_now = loaded.read().clone();
    let found = match loaded_now {
        Some(Ok((object, types))) => types
            .iter()
            .find(|t| t.def.key == object.type_key)
            .and_then(|t| t.def.action(&action_key))
            .cloned()
            .map(|a| (object, a)),
        Some(Err(_)) => None,
        None => return rsx! {},
    };
    let Some((object, action)) = found else {
        return rsx! {
            FormDialog {
                title: action_key.clone(),
                submit_label: t!("onto-action-run").to_string(),
                busy: false,
                valid: false,
                on_close: move |_| on_close.call(()),
                on_submit: move |_| {},
                p { class: "text-sm text-red-600", {t!("err-ontology-not-found")} }
            }
        };
    };
    let params = action.params.clone();
    let title = t!("onto-action-title", action : super::tr(&action.name, &action.names), object : object.title.clone());

    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("onto-action-run").to_string(),
            busy: busy(),
            valid: true,
            max_width: "max-w-xl".to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: move |_| {
                // Same validation as the server, before the round trip.
                if let Err((f, tok)) = pnex_core::ontology::schema::check_values(
                    &params,
                    &doc.read(),
                ) {
                    errors.set(Map::from_iter([(f, Value::String(tok))]));
                    return;
                }
                let id = object_id.clone();
                let key = action_key.clone();
                let body = doc.read().clone();
                let app = app_id.clone();
                busy.set(true);
                spawn(async move {
                    let res = api::ontology::invoke_action(&id, &key, &body, app.as_deref())
                        .await;
                    busy.set(false);
                    match res {
                        Ok(r) => {
                            toasts::success(
                                t!("onto-action-sent", count : r.flows.len()).to_string(),
                            );
                            if let Some(cb) = on_done {
                                cb.call(());
                            }
                            on_close.call(());
                        }
                        Err(e) => {
                            match crate::components::ontology::field_errors(&e) {
                                Some(fields) => errors.set(fields),
                                None => toasts::error(e),
                            }
                        }
                    }
                });
            },
            div { class: "space-y-3",
                p { class: "text-sm text-gray-600", {t!("onto-action-hint")} }
                if action.confirm {
                    p { class: "text-sm text-amber-700", {t!("onto-action-confirm-hint")} }
                }
                for p in action.params.iter().filter(|p| !p.kind.is_temporal()).cloned() {
                    if let PropertyKind::Ref { to_type } = &p.kind {
                        RefSearch {
                            key: "{p.key}",
                            field: p.key.clone(),
                            label: format!(
                                "{}{}",
                                super::property_input::prop_label(&p),
                                if p.required { " *" } else { "" },
                            ),
                            to_type: to_type.clone(),
                            doc,
                            chosen: prefilled(&linked.read(), &doc.read(), &p.key),
                            hint: prefilled(&linked.read(), &doc.read(), &p.key)
                                .map(|_| t!("onto-ref-linked").to_string()),
                            error: field_error(&errors.read(), &p.key),
                        }
                    } else {
                        PropertyInput {
                            key: "{p.key}",
                            error: field_error(&errors.read(), &p.key),
                            def: p.clone(),
                            doc,
                        }
                    }
                }
            }
        }
    }
}

/// Title of `key` when its value is still the one taken from the links (an
/// assistant's proposal wins over them).
fn prefilled(
    linked: &std::collections::BTreeMap<String, (String, String)>,
    doc: &Map<String, Value>,
    key: &str,
) -> Option<String> {
    let (id, title) = linked.get(key)?;
    (doc.get(key).and_then(Value::as_str) == Some(id.as_str())).then(|| title.clone())
}

fn field_error(errs: &Map<String, Value>, key: &str) -> Option<String> {
    errs.get(&format!("params.{key}"))
        .or_else(|| errs.get(key))
        .and_then(|v| v.as_str())
        .map(|t| crate::api::error_i18n::localize_field(key, t))
}

/// `ref` parameters with exactly one linked object of their type, open
/// links only: key -> (id, title). Two candidates leave the choice open.
fn prefill(
    object_id: &str,
    params: &[PropertyDef],
    links: &[LinkView],
) -> std::collections::BTreeMap<String, (String, String)> {
    let mut out = std::collections::BTreeMap::new();
    for p in params {
        let PropertyKind::Ref { to_type } = &p.kind else {
            continue;
        };
        let mut others: Vec<_> = links
            .iter()
            .filter(|l| l.valid_to.is_none())
            .map(|l| {
                if l.source.id == object_id {
                    &l.target
                } else {
                    &l.source
                }
            })
            .filter(|o| &o.type_key == to_type && o.id != object_id)
            .collect();
        others.sort_by(|a, b| a.id.cmp(&b.id));
        others.dedup_by(|a, b| a.id == b.id);
        if let [one] = others.as_slice() {
            out.insert(p.key.clone(), (one.id.clone(), one.title.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::api::ObjectRef;

    fn obj(id: &str, ty: &str) -> ObjectRef {
        ObjectRef {
            id: id.into(),
            type_key: ty.into(),
            title: format!("T{id}"),
            native_id: String::new(),
        }
    }

    fn link(from: ObjectRef, to: ObjectRef, closed: bool) -> LinkView {
        LinkView {
            id: 0,
            link_type: "l".into(),
            source: from,
            target: to,
            attributes: Map::new(),
            valid_from: String::new(),
            valid_to: closed.then(String::new),
            source_ref: None,
            history: None,
        }
    }

    fn param(key: &str, to: &str) -> PropertyDef {
        serde_json::from_value(serde_json::json!({"key": key, "kind": "ref", "to_type": to}))
            .unwrap()
    }

    #[test]
    fn only_a_single_open_link_prefills() {
        let lead = obj("L", "lead");
        let params = [
            param("organization", "organization"),
            param("person", "person"),
        ];
        let links = [
            link(lead.clone(), obj("A", "organization"), false),
            link(obj("A", "organization"), lead.clone(), false),
            link(lead.clone(), obj("B", "organization"), true),
            link(obj("P", "person"), lead.clone(), false),
            link(lead.clone(), obj("Q", "person"), false),
        ];
        let got = prefill("L", &params, &links);
        assert_eq!(got.get("organization"), Some(&("A".into(), "TA".into())));
        assert!(!got.contains_key("person"), "two people: the human picks");
    }
}
