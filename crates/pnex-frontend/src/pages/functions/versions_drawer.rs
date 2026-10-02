use super::*;

/// Version history drawer — list (version_number DESC, note, date). Every
/// row carries a "Load" action: the version code ships in the summary, so
/// ANY past version can be restored into the editor as a local draft
/// (saving then creates a new version — append-only).
#[component]
pub(super) fn FunctionVersionsDrawer(
    fn_id: i64,
    current_version: i64,
    on_close: Callback<()>,
    on_load: Callback<String>,
) -> Element {
    let versions =
        use_resource(move || async move { api::functions::versions(fn_id, 50, 0).await });
    let close = move |_| on_close.call(());

    rsx! {
        div { class: "fixed inset-0 z-40",
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900",
                        {t!("functions-versions-title")}
                    }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: close,
                        {"✕"}
                    }
                }
                {
                    match &*versions.value().read() {
                        Some(Ok(paged)) if paged.results.is_empty() => rsx! {
                            p { class: "p-4 text-sm text-gray-400", {t!("functions-versions-empty")} }
                        },
                        Some(Ok(paged)) => rsx! {
                            ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                                for version in paged.results.clone() {
                                    li { key: "{version.id}", class: "px-4 py-3 space-y-1.5",
                                        div { class: "flex items-center gap-2",
                                            span { class: "text-sm font-semibold text-gray-900",
                                                {format!("v{}", version.version_number)}
                                            }
                                            if version.version_number == current_version {
                                                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                                                    {t!("functions-version-current-tag")}
                                                }
                                            }
                                            span { class: "text-xs text-gray-400 ml-auto", {date_label(&version.created_at)} }
                                        }
                                        if let Some(note) = &version.note {
                                            p { class: "text-xs text-gray-600", {note.clone()} }
                                        }
                                        button {
                                            class: "px-2 py-1 text-xs text-blue-700 bg-blue-50 border border-blue-200 rounded-lg hover:bg-blue-100 transition-colors",
                                            onclick: move |_| {
                                                let code = version.code.clone();
                                                on_load.call(code);
                                            },
                                            {t!("functions-versions-load")}
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
                    }
                }
            }
        }
    }
}
