//! Drawer d'historique des versions d'une couche d'annotations + bloc
//! publication (école `tour_editor/versions.rs`) : publier (défaut =
//! dernière), publier une version antérieure (= repointage, D56),
//! dépublier, charger une version dans l'éditeur.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::state::toasts;

/// Drawer latéral : historique append-only + publication d'une couche.
/// `on_loaded` : version à charger dans l'éditeur (restauration par
/// édition, D56) ; `on_changed` : la couche a changé côté serveur.
#[component]
pub fn AnnotationVersionsDrawer(
    layer_id: String,
    can_write: bool,
    published_version: Option<i64>,
    on_close: Callback<()>,
    on_loaded: Callback<pnex_core::AnnotationLayerVersionDetail>,
    on_changed: Callback<()>,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut busy = use_signal(|| false);
    let mut confirm_publish = use_signal(|| None::<i64>);
    let layer_versions = layer_id.clone();
    let versions = use_resource(move || {
        let _ = reload();
        let layer_id = layer_versions.clone();
        async move { api::annotation_layers::versions(&layer_id, 50, 0).await }
    });
    let layer_latest = layer_id.clone();
    let layer_unpub = layer_id.clone();

    rsx! {
        div { class: "fixed inset-0 z-40",
            div {
                class: "absolute inset-0",
                onclick: move |_| on_close.call(()),
            }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("annot-versions-title")} }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: move |_| on_close.call(()),
                        {"✕"}
                    }
                }
                if can_write {
                    div { class: "px-4 py-3 border-b border-gray-200 space-y-2 bg-gray-50",
                        div { class: "flex items-center gap-2 text-xs",
                            if let Some(version) = published_version {
                                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                                    {t!("annot-published-tag", version : version)}
                                }
                            } else {
                                span { class: "text-gray-500", {t!("annot-publish-none")} }
                            }
                        }
                        div { class: "flex flex-wrap gap-2",
                            button {
                                class: "px-3 py-1.5 text-xs bg-emerald-600 text-white rounded-lg hover:bg-emerald-700 transition-colors font-medium disabled:opacity-40",
                                disabled: busy(),
                                onclick: move |_| {
                                    busy.set(true);
                                    let id = layer_latest.clone();
                                    spawn(async move {
                                        match api::annotation_layers::publish(&id, None).await {
                                            Ok(_) => {
                                                toasts::success(t!("toast-annot-published"));
                                                on_changed.call(());
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(err) => toasts::error(err),
                                        }
                                        busy.set(false);
                                    });
                                },
                                {t!("annot-publish-latest")}
                            }
                            if published_version.is_some() {
                                button {
                                    class: "px-3 py-1.5 text-xs text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors disabled:opacity-40",
                                    disabled: busy(),
                                    onclick: move |_| {
                                        busy.set(true);
                                        let id = layer_unpub.clone();
                                        spawn(async move {
                                            match api::annotation_layers::unpublish(&id).await {
                                                Ok(_) => {
                                                    toasts::success(t!("toast-annot-unpublished"));
                                                    on_changed.call(());
                                                    reload.with_mut(|r| *r += 1);
                                                }
                                                Err(err) => toasts::error(err),
                                            }
                                            busy.set(false);
                                        });
                                    },
                                    {t!("annot-unpublish")}
                                }
                            }
                        }
                    }
                }

                // ─── Historique ───
                {
                    match &*versions.value().read() {
                        Some(Ok(paged)) if paged.results.is_empty() => rsx! {
                            p { class: "p-4 text-sm text-gray-400", {t!("annot-versions-empty")} }
                        },
                        Some(Ok(paged)) => rsx! {
                            ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                                for version in paged.results.clone() {
                                    {
                                        version_row(
                                            version,
                                            layer_id.clone(),
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
                    }
                }
            }
        }

        // Confirmation : publier une version antérieure (= repointage, D56).
        if let Some(version_number) = confirm_publish() {
            ConfirmDialog {
                key: "annot-publish-{version_number}",
                title: t!("annot-versions-publish-confirm-title"),
                message: format!("v{version_number} — {}", t!("annot-versions-publish-confirm-message")),
                confirm_label: t!("annot-publish-latest"),
                on_confirm: move |_| {
                    confirm_publish.set(None);
                    if busy() {
                        return;
                    }
                    busy.set(true);
                    let id = layer_id.clone();
                    spawn(async move {
                        match api::annotation_layers::publish(&id, Some(version_number)).await {
                            Ok(_) => {
                                toasts::success(t!("toast-annot-published"));
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

/// Ligne d'historique : valeurs possédées (école tour_editor/versions) —
/// closures sans conflit entre lignes.
fn version_row(
    version: pnex_core::AnnotationLayerVersionSummary,
    layer_id: String,
    can_write: bool,
    published_version: Option<i64>,
    mut confirm_publish: Signal<Option<i64>>,
    on_loaded: Callback<pnex_core::AnnotationLayerVersionDetail>,
) -> Element {
    let layer_load = layer_id.clone();
    rsx! {
        li { key: "{version.id}", class: "px-4 py-3 space-y-2",
            div { class: "flex items-center gap-2",
                span { class: "text-sm font-semibold text-gray-900",
                    {format!("v{}", version.version_number)}
                }
                if version.published {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                        {t!("annot-version-published-tag")}
                    }
                }
                span { class: "text-xs text-gray-400 ml-auto", {date_label(&version.created_at)} }
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
                        let id = layer_load.clone();
                        let n = version.version_number;
                        spawn(async move {
                            match api::annotation_layers::version(&id, n).await {
                                Ok(detail) => on_loaded.call(detail),
                                Err(err) => toasts::error(err),
                            }
                        });
                    },
                    {t!("annot-versions-load")}
                }
                if can_write && published_version != Some(version.version_number) {
                    button {
                        class: "px-2 py-1 text-xs text-emerald-700 bg-emerald-50 border border-emerald-200 rounded-lg hover:bg-emerald-100 transition-colors",
                        onclick: move |_| confirm_publish.set(Some(version.version_number)),
                        {t!("annot-versions-publish")}
                    }
                }
            }
        }
    }
}
