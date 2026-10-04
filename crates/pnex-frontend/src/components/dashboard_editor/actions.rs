//! Persisting actions of the dashboard editor: browser-side validation +
//! PATCH save, 409 conflict modal and the save-as-template modal.

use super::*;

use crate::components::modal::Modal;
use crate::state::toasts;

/// Save : validation navigateur puis PATCH (nouvelle version = live).
/// `name` : Some ⇒ renommage inline depuis le titre de la coquille (le
/// layout voyage avec — renommer vaut publication de l'état courant, D24).
pub(super) fn save(mut cx: EditorCx, name: Option<String>) {
    let mut saving = cx.saving;
    if saving() {
        return;
    }
    let violations = pnex_core::validate_layout(&cx.layout.read());
    if !violations.is_empty() {
        cx.violations.set(violations);
        return;
    }
    cx.violations.set(Vec::new());
    saving.set(true);
    let params = pnex_core::UpdateDashboard {
        name,
        expected_version_number: cx.saved_version.cloned(),
        layout: cx.layout.read().clone(),
    };
    let dashboard_id = cx.dashboard_id.cloned();
    spawn(async move {
        match api::dashboards::update(&dashboard_id, params).await {
            Ok(d) => {
                cx.name.set(d.name.clone());
                // D131: the server bound the control widgets to their
                // provisioned controls — adopt the ids (edits made while
                // saving stay local and dirty).
                let local = state::adopt_bound_controls(&cx.layout.read(), &d.layout);
                cx.layout.set(local);
                cx.saved_layout.set(d.layout.clone());
                cx.saved_version.set(d.current_version_number);
                toasts::success(t!("db-saved", version: d.current_version_number).to_string());
            }
            Err(e) if e.status == Some(409) => {
                cx.conflict.set(true);
            }
            Err(e) => {
                // 400 violations : mêmes messages que la validation locale.
                if let Some(v) = e.body.as_ref().and_then(|b| b.get("violations")).cloned() {
                    if let Ok(list) = serde_json::from_value::<Vec<VizViolation>>(v) {
                        cx.violations.set(list);
                    }
                }
                toasts::error(e);
            }
        }
        saving.set(false);
    });
}

/// Modale 409 : recharger depuis le serveur ou écraser avec ma version.
#[component]
pub(super) fn ConflictModal(mut cx: EditorCx, on_changed: EventHandler<()>) -> Element {
    rsx! {
        Modal {
            title: t!("db-conflict-title").to_string(),
            max_width: "max-w-lg".to_string(),
            on_close: move |_| cx.conflict.set(false),
            div { class: "space-y-4",
                p { class: "text-sm text-gray-600",
                    {t!("db-conflict-body", server : cx.saved_version.cloned())}
                }
                div { class: "flex justify-end gap-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| {
                            // Recharger : le document serveur écrase l'éditeur.
                            let id = cx.dashboard_id.cloned();
                            spawn(async move {
                                match api::dashboards::detail(&id).await {
                                    Ok(d) => {
                                        cx.layout.set(d.layout.clone());
                                        cx.saved_layout.set(d.layout);
                                        cx.saved_version.set(d.current_version_number);
                                        cx.violations.set(Vec::new());
                                    }
                                    Err(e) => toasts::error(e),
                                }
                                cx.conflict.set(false);
                                on_changed.call(());
                            });
                        },
                        {t!("db-conflict-reload")}
                    }
                    button {
                        class: "px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700",
                        onclick: move |_| {
                            // Écraser : re-lire le pointeur serveur puis
                            // re-PATCH avec MON layout (409 possible si un
                            // autre save passe entre-temps — toast).
                            let id = cx.dashboard_id.cloned();
                            let my_layout = cx.layout.read().clone();
                            spawn(async move {
                                match api::dashboards::detail(&id).await {
                                    Ok(d) => {
                                        let params = pnex_core::UpdateDashboard {
                                            name: None,
                                            expected_version_number: d.current_version_number,
                                            layout: my_layout,
                                        };
                                        match api::dashboards::update(&id, params).await {
                                            Ok(saved) => {
                                                cx.saved_layout.set(saved.layout.clone());
                                                cx.layout.set(saved.layout);
                                                cx.saved_version.set(saved.current_version_number);
                                                toasts::success(
                                                    t!("db-saved", version : saved.current_version_number)
                                                        .to_string(),
                                                );
                                            }
                                            Err(e) => toasts::error(e),
                                        }
                                    }
                                    Err(e) => toasts::error(e),
                                }
                                cx.conflict.set(false);
                                on_changed.call(());
                            });
                        },
                        {t!("db-conflict-overwrite")}
                    }
                }
            }
        }
    }
}

/// Modale « enregistrer comme modèle » (D41) : la bibliothèque reçoit
/// une **copie** du widget sans position.
#[component]
pub(super) fn SaveAsTemplate(mut cx: EditorCx, widget_id: String) -> Element {
    let mut name = use_signal(String::new);
    let mut busy = use_signal(|| false);
    rsx! {
        Modal {
            title: t!("lib-save-as").to_string(),
            max_width: "max-w-md".to_string(),
            on_close: move |_| cx.save_as.set(None),
            div { class: "space-y-4",
                div {
                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1",
                        {t!("lib-template-name")}
                    }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div { class: "flex justify-end gap-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| cx.save_as.set(None),
                        {t!("common-cancel")}
                    }
                    button {
                        class: if busy() || name().trim().is_empty() { "px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg opacity-40 cursor-not-allowed" } else { "px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700" },
                        disabled: busy() || name().trim().is_empty(),
                        onclick: move |_| {
                            let layout_now = cx.layout.read().clone();
                            let Some(w) = state::find_widget(&layout_now, &widget_id) else { return };
                            let template = pnex_core::WidgetTemplate::from_widget(&w);
                            let params = pnex_core::CreateVizWidget {
                                name: name().trim().to_string(),
                                kind: w.widget_type.clone(),
                                config: template,
                            };
                            spawn(async move {
                                busy.set(true);
                                match api::dashboards::create_widget(params).await {
                                    Ok(_) => {
                                        toasts::success(t!("lib-saved").to_string());
                                        name.set(String::new());
                                        cx.palette_reload.with_mut(|r| *r += 1);
                                        cx.save_as.set(None);
                                    }
                                    Err(e) => toasts::error(e),
                                }
                                busy.set(false);
                            });
                        },
                        {t!("lib-save-as")}
                    }
                }
            }
        }
    }
}
