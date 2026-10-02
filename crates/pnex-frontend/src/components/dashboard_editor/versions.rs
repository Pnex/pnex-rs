//! Version history drawer — same right-side drawer as every other editor
//! (`flow_editor/versions.rs`):
//! - **Load** a version → the editor takes that layout (saving creates
//!   v(n+1));
//! - **Restore** → moves the **live** pointer back (media school, no new
//!   version is created).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::icons;
use crate::state::toasts;

use super::EditorCx;

#[component]
pub fn VersionsDrawer(
    mut cx: EditorCx,
    on_close: Callback<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let versions = use_resource(move || {
        let _ = reload();
        let id = cx.dashboard_id.cloned();
        async move { api::dashboards::versions(&id).await }
    });
    let mut restoring: Signal<Option<i64>> = use_signal(|| None);
    let close = move |_| on_close.call(());

    rsx! {
        div { class: "fixed inset-0 z-40",
            // Click outside the drawer closes it.
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("ver-drawer-title")} }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: close,
                        icons::X { class: "h-4 w-4" }
                    }
                }
                {
                    match &*versions.value().read() {
                        Some(Ok(rows)) if rows.is_empty() => rsx! {
                            p { class: "p-4 text-sm text-gray-400", {t!("ver-empty")} }
                        },
                        Some(Ok(rows)) => rsx! {
                            ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                                for v in rows.clone() {
                                    li { key: "{v.version_number}", class: "px-4 py-3 space-y-2",
                                        div { class: "flex items-center gap-2",
                                            span { class: "text-sm font-semibold text-gray-900",
                                                {format!("v{}", v.version_number)}
                                            }
                                            if v.current {
                                                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                                                    {t!("ver-current")}
                                                }
                                            }
                                            span { class: "text-xs text-gray-400 ml-auto", {date_label(&v.created_at)} }
                                        }
                                        if let Some(author) = &v.author {
                                            div { class: "text-xs text-gray-500", {author.clone()} }
                                        }
                                        div { class: "flex items-center gap-2",
                                            button {
                                                class: "px-2 py-1 text-xs text-blue-700 bg-blue-50 border border-blue-200 rounded-lg hover:bg-blue-100 transition-colors",
                                                onclick: move |_| {
                                                    let id = cx.dashboard_id.cloned();
                                                    let n = v.version_number;
                                                    spawn(async move {
                                                        match api::dashboards::version_detail(&id, n).await {
                                                            Ok(detail) => {
                                                                let previous = cx.layout.read().clone();
                                                                cx.history.with_mut(|h| h.push(&previous));
                                                                cx.layout.set(detail.layout);
                                                                cx.selected.set(None);
                                                                cx.violations.set(Vec::new());
                                                                on_close.call(());
                                                            }
                                                            Err(e) => toasts::error(e),
                                                        }
                                                    });
                                                },
                                                {t!("ver-load")}
                                            }
                                            // Load: the editor takes this layout —
                                            // the next save creates v(n+1).
                                            if !v.current {
                                                button {
                                                    class: "px-2 py-1 text-xs text-emerald-700 bg-emerald-50 border border-emerald-200 rounded-lg hover:bg-emerald-100 transition-colors",
                                                    onclick: move |_| restoring.set(Some(v.version_number)),
                                                    {t!("ver-restore")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        Some(Err(err)) => rsx! {
                            div { class: "m-4 bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                {crate::api::error_i18n::localize(err)}
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

        if let Some(n) = restoring() {
            ConfirmDialog {
                key: "restore-{n}",
                title: t!("ver-restore").to_string(),
                message: t!("ver-restore-confirm", n : n).to_string(),
                confirm_label: t!("ver-restore").to_string(),
                on_confirm: move |_| {
                    restoring.set(None);
                    let id = cx.dashboard_id.cloned();
                    let restored_msg = t!("ver-restored", n : n).to_string();
                    spawn(async move {
                        match api::dashboards::restore(&id, n).await {
                            Ok(d) => {
                                cx.saved_version.set(d.current_version_number);
                                cx.saved_layout.set(d.layout.clone());
                                cx.layout.set(d.layout);
                                toasts::success(restored_msg);
                                on_changed.call(());
                                let next = reload() + 1;
                                reload.set(next);
                            }
                            Err(e) => toasts::error(e),
                        }
                    });
                },
                on_cancel: move |_| restoring.set(None),
            }
        }
    }
}
