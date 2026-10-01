//! Drawer d'historique des versions + bloc publication (publier une version,
//! dépublier, lien public copiable) — école `flow_editor/versions.rs`.
//!
//! Prop `tour_id` String : chaque closure reçoit **son propre clone local
//! nommé** (école CanvasNode « un clone local par closure ») — une closure
//! `move` ne peut pas partager un String avec ses sœurs.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::state::toasts;

/// Drawer latéral : historique append-only + publication.
/// `on_loaded` : version à charger dans l'éditeur (restauration par édition) ;
/// `on_changed` : le tour a changé côté serveur (publish/unpublish/share).
#[component]
pub(crate) fn VersionsDrawer(
    tour_id: String,
    can_write: bool,
    share_token: Option<String>,
    published_version: Option<i64>,
    on_close: Callback<()>,
    on_loaded: Callback<pnex_core::TourVersionDetail>,
    on_changed: Callback<()>,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let tour_id_versions = tour_id.clone();
    let versions = use_resource(move || {
        let _ = reload();
        let tour_id = tour_id_versions.clone();
        async move { api::tours::versions(&tour_id, 50, 0).await }
    });
    let mut confirm_publish = use_signal(|| None::<i64>);
    let mut busy = use_signal(|| false);

    let close = move |_| on_close.call(());

    // Un clone local nommé par closure (l'historique est rendu via des
    // tuples possédant leur propre clone — voir `items` plus bas).
    let tour_id_publish_latest = tour_id.clone();
    let tour_id_unpublish = tour_id.clone();
    let tour_id_share = tour_id.clone();
    let tour_id_revoke = tour_id.clone();

    rsx! {
        div { class: "fixed inset-0 z-40",
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("studio-versions-title")} }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: close,
                        { "✕" }
                    }
                }

                // ─── Publication (writers) ───
                if can_write {
                    div { class: "px-4 py-3 border-b border-gray-200 space-y-2 bg-gray-50",
                        div { class: "flex items-center gap-2 text-xs",
                            if let Some(version) = published_version {
                                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                                    {t!("studio-published-tag", version: version)}
                                }
                            } else {
                                span { class: "text-gray-500", {t!("studio-publish-none")} }
                            }
                        }
                        div { class: "flex flex-wrap gap-2",
                            button {
                                class: "px-3 py-1.5 text-xs bg-emerald-600 text-white rounded-lg hover:bg-emerald-700 transition-colors font-medium disabled:opacity-40",
                                disabled: busy(),
                                onclick: move |_| {
                                    busy.set(true);
                                    let id = tour_id_publish_latest.clone();
                                    spawn(async move {
                                        match api::tours::publish(&id, None).await {
                                            Ok(_) => {
                                                toasts::success("toast-tour-published");
                                                on_changed.call(());
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(err) => toasts::error(err),
                                        }
                                        busy.set(false);
                                    });
                                },
                                {t!("studio-publish-latest")}
                            }
                            if published_version.is_some() {
                                button {
                                    class: "px-3 py-1.5 text-xs text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors disabled:opacity-40",
                                    disabled: busy(),
                                    onclick: move |_| {
                                        busy.set(true);
                                        let id = tour_id_unpublish.clone();
                                        spawn(async move {
                                            match api::tours::unpublish(&id).await {
                                                Ok(_) => {
                                                    toasts::success("toast-tour-unpublished");
                                                    on_changed.call(());
                                                    reload.with_mut(|r| *r += 1);
                                                }
                                                Err(err) => toasts::error(err),
                                            }
                                            busy.set(false);
                                        });
                                    },
                                    {t!("studio-unpublish")}
                                }
                            }
                            {match share_token.clone() {
                                Some(token) => rsx! {
                                    button {
                                        class: "px-3 py-1.5 text-xs text-blue-700 bg-white border border-blue-300 rounded-lg hover:bg-blue-50 transition-colors",
                                        onclick: move |_| {
                                            let url = api::tours::share_url(&token);
                                            crate::util::copy_text(&url);
                                            toasts::success("toast-tour-share-copied");
                                        },
                                        {t!("studio-share-copy")}
                                    }
                                    button {
                                        class: "px-3 py-1.5 text-xs text-red-700 bg-white border border-red-200 rounded-lg hover:bg-red-50 transition-colors",
                                        onclick: move |_| {
                                            busy.set(true);
                                            let id = tour_id_revoke.clone();
                                            spawn(async move {
                                                match api::tours::revoke_share(&id).await {
                                                    Ok(_) => {
                                                        toasts::success("toast-tour-share-revoked");
                                                        on_changed.call(());
                                                        reload.with_mut(|r| *r += 1);
                                                    }
                                                    Err(err) => toasts::error(err),
                                                }
                                                busy.set(false);
                                            });
                                        },
                                        {t!("studio-share-revoke")}
                                    }
                                },
                                None => rsx! {
                                    button {
                                        class: "px-3 py-1.5 text-xs text-blue-700 bg-white border border-blue-300 rounded-lg hover:bg-blue-50 transition-colors disabled:opacity-40",
                                        disabled: busy() || published_version.is_none(),
                                        title: if published_version.is_none() { t!("studio-share-needs-publish") } else { "".to_string() },
                                        onclick: move |_| {
                                            busy.set(true);
                                            let id = tour_id_share.clone();
                                            spawn(async move {
                                                match api::tours::share(&id).await {
                                                    Ok(_) => {
                                                        toasts::success("toast-tour-shared");
                                                        on_changed.call(());
                                                        reload.with_mut(|r| *r += 1);
                                                    }
                                                    Err(err) => toasts::error(err),
                                                }
                                                busy.set(false);
                                            });
                                        },
                                        {t!("studio-share")}
                                    }
                                },
                            }}
                        }
                    }
                }

                // ─── Historique ───
                {match &*versions.value().read() {
                    Some(Ok(paged)) if paged.results.is_empty() => rsx! {
                        p { class: "p-4 text-sm text-gray-400", {t!("studio-versions-empty")} }
                    },
                    Some(Ok(paged)) => rsx! {
                        ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                            // Ligne dédiée (école `flow_row`) : valeurs
                            // possédées par ligne → closures sans conflit.
                            for version in paged.results.clone() {
                                {
                                    version_row(
                                        version,
                                        tour_id.clone(),
                                        can_write,
                                        published_version,
                                        confirm_publish,
                                        on_loaded,
                                    )
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

        // Confirmation : publier une version antérieure (= repointage).
        if let Some(version_number) = confirm_publish() {
            ConfirmDialog {
                key: "publish-{version_number}",
                title: t!("studio-versions-publish-confirm-title"),
                message: format!("v{version_number} — {}", t!("studio-versions-publish-confirm-message")),
                confirm_label: t!("studio-versions-publish"),
                on_confirm: move |_| {
                    confirm_publish.set(None);
                    if busy() {
                        return;
                    }
                    busy.set(true);
                    let id = tour_id.clone();
                    spawn(async move {
                        match api::tours::publish(&id, Some(version_number)).await {
                            Ok(_) => {
                                toasts::success("toast-tour-published");
                                on_changed.call(());
                                reload.with_mut(|r| *r += 1);
                            }
                            Err(err) => toasts::error(err),
                        }
                        busy.set(false);
                    });
                },
                on_cancel: move |_| confirm_publish.set(None),
            }
        }
    }
}

/// Ligne d'historique : valeurs possédées (école `flow_row`) — les closures
/// capturent leurs propres copies sans conflit entre lignes.
#[allow(clippy::too_many_arguments)]
fn version_row(
    version: pnex_core::TourVersionSummary,
    tour_id: String,
    can_write: bool,
    published_version: Option<i64>,
    mut confirm_publish: Signal<Option<i64>>,
    on_loaded: Callback<pnex_core::TourVersionDetail>,
) -> Element {
    let tour_id_load = tour_id.clone();
    rsx! {
        li { key: "{version.id}", class: "px-4 py-3 space-y-2",
            div { class: "flex items-center gap-2",
                span { class: "text-sm font-semibold text-gray-900",
                    {format!("v{}", version.version_number)}
                }
                if version.published {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                        {t!("studio-version-published-tag")}
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
                        let id = tour_id_load.clone();
                        let version_number = version.version_number;
                        spawn(async move {
                            match api::tours::version(&id, version_number).await {
                                Ok(detail) => on_loaded.call(detail),
                                Err(err) => toasts::error(err),
                            }
                        });
                    },
                    {t!("studio-versions-load")}
                }
                if can_write && published_version != Some(version.version_number) {
                    button {
                        class: "px-2 py-1 text-xs text-emerald-700 bg-emerald-50 border border-emerald-200 rounded-lg hover:bg-emerald-100 transition-colors",
                        onclick: move |_| confirm_publish.set(Some(version.version_number)),
                        {t!("studio-versions-publish")}
                    }
                }
            }
        }
    }
}
