//! Runners tab: where runs and builds go. An Admin creates a join token, shown once with
//! the commands that install a runner, and revokes a runner.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::error::ApiError;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::access;
use crate::state::toasts;
use pnex_core::runners::{JoinTokenCreate, JoinTokenCreated, RunnerView};

#[component]
pub(super) fn RunnersTab(reload: Signal<u32>) -> Element {
    let mut joining = use_signal(|| false);
    let mut confirm_revoke = use_signal(|| Option::<String>::None);
    let list = use_resource(move || async move {
        let _ = reload();
        api::hosted_agents::runners().await
    });
    let (state, is_empty, rows) = match &*list.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let can_revoke = access::can("runners:delete");
    let mut columns = vec![
        Column::new(t!("agents-col-name").to_string(), |r: &RunnerView| {
            rsx! {
                span { class: "text-sm font-medium text-ink", {r.name.clone()} }
            }
        }),
        Column::new(t!("agents-col-state").to_string(), |r: &RunnerView| {
            let label = if r.online {
                t!("agents-runner-online")
            } else {
                t!("agents-runner-offline")
            };
            rsx! {
                span { class: "text-sm text-slate", {label} }
            }
        }),
        Column::new(
            t!("agents-col-capabilities").to_string(),
            |r: &RunnerView| {
                let caps = r.capabilities.join(" · ");
                rsx! {
                    span { class: "font-mono text-xs text-slate", {caps} }
                }
            },
        )
        .secondary(),
    ];
    if can_revoke {
        columns.push(
            Column::new(t!("common-actions").to_string(), move |r: &RunnerView| {
                let id = r.id.clone();
                let confirm_id = id.clone();
                rsx! {
                    if confirm_revoke() == Some(confirm_id.clone()) {
                        button {
                            class: DANGER_BTN,
                            onclick: move |_| {
                                let id = id.clone();
                                spawn(async move {
                                    match api::hosted_agents::revoke_runner(&id).await {
                                        Ok(()) => {
                                            toasts::info(t!("agents-runner-revoked").to_string());
                                            reload.with_mut(|r| *r += 1);
                                        }
                                        Err(e) => toasts::error(e),
                                    }
                                    confirm_revoke.set(None);
                                });
                            },
                            {t!("agents-runner-confirm-revoke")}
                        }
                    } else {
                        button {
                            class: DANGER_BTN,
                            onclick: move |_| confirm_revoke.set(Some(confirm_id.clone())),
                            {t!("agents-runner-revoke")}
                        }
                    }
                }
            })
            .actions(),
        );
    }
    rsx! {
        if joining() {
            JoinTokenDialog { joining }
        }
        if access::can("runners:manage") {
            div { class: "flex justify-end mb-4",
                button { class: BTN_PRIMARY, onclick: move |_| joining.set(true),
                    {t!("agents-runner-join-new")}
                }
            }
        }
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-runners-empty").to_string(),
            empty_detail: rsx! {
                p { class: "text-sm text-slate mt-2", {t!("agents-runner-empty-hint")} }
            },
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|r: &RunnerView| r.id.clone()),
            }
        }
    }
}

/// Commands that install a runner with the token in a Secret of its namespace.
fn install_commands(origin: &str) -> String {
    format!(
        "kubectl create namespace pnex-runner\n\
         kubectl -n pnex-runner create secret generic pnex-runner-join --from-literal=token=<TOKEN>\n\
         helm install pnex-runner ./helm/pnex-runner -n pnex-runner \\\n  \
         --set serverUrl={origin} \\\n  \
         --set joinToken.existingSecret=pnex-runner-join"
    )
}

/// Names the token, then shows it once with the install commands.
#[component]
fn JoinTokenDialog(joining: Signal<bool>) -> Element {
    let mut joining = joining;
    let mut name = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut created = use_signal(|| Option::<JoinTokenCreated>::None);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    if let Some(token) = created() {
        let commands =
            install_commands(&crate::api::config::api_base()).replace("<TOKEN>", &token.token);
        let copy_token = token.token.clone();
        let copy_commands = commands.clone();
        return rsx! {
            crate::components::modal::Modal {
                title: t!("agents-runner-join-title").to_string(),
                max_width: "max-w-2xl".to_string(),
                on_close: move |_| joining.set(false),
                div { class: "space-y-3",
                    p { class: "text-sm text-ink",
                        {t!("agents-runner-join-once", expires : super::short_date(& token.expires_at))}
                    }
                    div { class: "flex items-center gap-2",
                        code { class: "flex-1 break-all rounded bg-gray-50 p-2 font-mono text-xs text-ink",
                            {token.token.clone()}
                        }
                        button {
                            class: BTN_SECONDARY_SM,
                            onclick: move |_| {
                                crate::util::copy_text(&copy_token);
                                toasts::info(t!("agents-runner-copied").to_string());
                            },
                            {t!("agents-runner-copy")}
                        }
                    }
                    p { class: "text-sm text-ink", {t!("agents-runner-join-install")} }
                    pre { class: "overflow-auto rounded bg-gray-50 p-3 font-mono text-xs leading-5 text-ink",
                        {commands}
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| {
                            crate::util::copy_text(&copy_commands);
                            toasts::info(t!("agents-runner-copied").to_string());
                        },
                        {t!("agents-runner-copy-commands")}
                    }
                    p { class: "text-xs text-slate", {t!("agents-runner-join-chart")} }
                }
            }
        };
    }
    rsx! {
        FormDialog {
            title: t!("agents-runner-join-new").to_string(),
            submit_label: t!("common-create").to_string(),
            on_close: move |_| joining.set(false),
            busy: busy(),
            valid: !name().trim().is_empty(),
            on_submit: move |_| {
                let input = JoinTokenCreate {
                    name: name().trim().to_string(),
                };
                busy.set(true);
                spawn(async move {
                    let result = api::hosted_agents::create_join_token(&input).await;
                    busy.set(false);
                    match result {
                        Ok(t) => created.set(Some(t)),
                        Err(e) => form_error.set(Some(e)),
                    }
                });
            },
            div { class: "space-y-4",
                div {
                    label { r#for: "runner-name", class: FIELD_LABEL,
                        {t!("agents-runner-field-name")}
                    }
                    input {
                        id: "runner-name",
                        class: "{INPUT} w-full",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                if let Some(err) = form_error() {
                    div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                        {crate::api::error_i18n::localize(&err)}
                    }
                }
            }
        }
    }
}
