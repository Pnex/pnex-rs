//! Drawer d'historique des versions — liste paginée, chargement dans
//! l'éditeur, deploy d'une version antérieure (= rollback serveur).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::state::toasts;

/// Drawer latéral d'historique. `on_loaded` transmet la version à charger
/// dans l'éditeur (le graphe est appliqué par `FlowEditor`) ; `on_deployed`
/// est appelé après un deploy/rollback réussi.
#[component]
pub(crate) fn VersionsDrawer(
    flow_id: i64,
    can_write: bool,
    on_close: Callback<()>,
    on_loaded: Callback<pnex_core::FlowVersionDetail>,
    on_deployed: Callback<()>,
) -> Element {
    let reload = use_signal(|| 0u32);
    let versions = use_resource(move || async move {
        let _ = reload();
        api::flows::versions(flow_id, 50, 0).await
    });
    let mut confirm_deploy = use_signal(|| None::<i64>);
    let mut deploying = use_signal(|| false);

    let deploy_target = confirm_deploy();
    let close = move |_| on_close.call(());

    rsx! {
        div { class: "fixed inset-0 z-40",
            // Clic hors drawer → fermeture.
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("flows-versions-title")} }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: close,
                        { "✕" }
                    }
                }
                {match &*versions.value().read() {
                    Some(Ok(paged)) if paged.results.is_empty() => rsx! {
                        p { class: "p-4 text-sm text-gray-400", {t!("flows-versions-empty")} }
                    },
                    Some(Ok(paged)) => rsx! {
                        ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                            for version in paged.results.clone() {
                                li { key: "{version.id}", class: "px-4 py-3 space-y-2",
                                    div { class: "flex items-center gap-2",
                                        span { class: "text-sm font-semibold text-gray-900",
                                            {format!("v{}", version.version_number)}
                                        }
                                        if version.deployed {
                                            span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                                                {t!("flows-version-deployed-tag")}
                                            }
                                        }
                                        span { class: "text-xs text-gray-400 ml-auto",
                                            {date_label(&version.created_at)}
                                        }
                                    }
                                    div { class: "text-xs text-gray-500",
                                        if let Some(author) = &version.author {
                                            span { {author.clone()} }
                                        }
                                        if let Some(note) = &version.note {
                                            p { class: "mt-0.5 text-gray-600", {note.clone()} }
                                        }
                                    }
                                    div { class: "flex items-center gap-2",
                                        button {
                                            class: "px-2 py-1 text-xs text-blue-700 bg-blue-50 border border-blue-200 rounded-lg hover:bg-blue-100 transition-colors",
                                            onclick: move |_| {
                                                let flow_id = flow_id;
                                                let version_number = version.version_number;
                                                spawn(async move {
                                                    match api::flows::version(flow_id, version_number).await {
                                                        Ok(detail) => on_loaded.call(detail),
                                                        Err(err) => toasts::error(err.message),
                                                    }
                                                });
                                            },
                                            {t!("flows-versions-load")}
                                        }
                                        if can_write {
                                            button {
                                                class: "px-2 py-1 text-xs text-emerald-700 bg-emerald-50 border border-emerald-200 rounded-lg hover:bg-emerald-100 transition-colors",
                                                onclick: move |_| confirm_deploy.set(Some(version.version_number)),
                                                {t!("flows-versions-deploy")}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "m-4 bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                            {err.message.clone()}
                        }
                    },
                    None => rsx! {
                        div { class: "flex-1 flex items-center justify-center",
                            span { class: "animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600" }
                        }
                    },
                }}
            }
        }

        // Confirmation du deploy d'une version antérieure (= rollback).
        if let Some(version_number) = deploy_target {
            ConfirmDialog {
                key: "deploy-{version_number}",
                title: t!("flows-versions-deploy-confirm-title"),
                message: format!("v{version_number} — {}", t!("flows-versions-deploy-confirm-message")),
                confirm_label: t!("flows-versions-deploy"),
                on_confirm: move |_| {
                    confirm_deploy.set(None);
                    if deploying() {
                        return;
                    }
                    deploying.set(true);
                    spawn(async move {
                        match api::flows::rollback(flow_id, version_number).await {
                            Ok(_) => {
                                toasts::success("toast-flow-deployed");
                                on_deployed.call(());
                            }
                            Err(err) => toasts::error(err.message),
                        }
                        deploying.set(false);
                    });
                },
                on_cancel: move |_| confirm_deploy.set(None),
            }
        }
    }
}
