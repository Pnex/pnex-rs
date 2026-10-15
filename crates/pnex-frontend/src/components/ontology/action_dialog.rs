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
use crate::state::toasts;

#[component]
pub fn ActionDialog(
    object_id: String,
    action_key: String,
    #[props(default)] initial: Map<String, Value>,
    on_close: Callback<()>,
    #[props(default)] on_done: Option<Callback<()>>,
) -> Element {
    let oid = object_id.clone();
    let loaded = use_resource(move || {
        let id = oid.clone();
        async move {
            let o = api::ontology::object(&id).await?;
            let t = api::ontology::types().await?;
            Ok::<_, api::error::ApiError>((o, t))
        }
    });
    let doc = use_signal(move || initial.clone());
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
                busy.set(true);
                spawn(async move {
                    let res = api::ontology::invoke_action(&id, &key, &body).await;
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
                // shortcut: `ref` parameters get no choice list, add one when a pack needs them.
                for p in action.params.iter().filter(|p| !p.kind.is_temporal()).cloned() {
                    PropertyInput {
                        key: "{p.key}",
                        error: {
                            let errs = errors.read();
                            errs.get(&format!("params.{}", p.key))
                                .or_else(|| errs.get(&p.key))
                                .and_then(|v| v.as_str())
                                .map(|t| crate::api::error_i18n::localize_field(&p.key, t))
                        },
                        def: p.clone(),
                        doc,
                    }
                }
            }
        }
    }
}
