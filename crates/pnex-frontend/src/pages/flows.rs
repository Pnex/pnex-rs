//! Flux ETL (D18) — liste des flows de l'org (filtres + pagination D14) et
//! éditeur drag & drop (`components/flow_editor/`, sous-vue pilotée par le
//! signal local `selected` — pattern `devices.rs`). La page compose le
//! socle CRUD (`components/crud/` — README du module pour la doctrine) :
//! ListLayout / FilterBar / ListStates / DataTable / ListPager.
//!
//! Le scoping org vient du client (`X-Org-Id`), l'écriture est réservée
//! owner/admin (le serveur force, l'UI masque). La création démarre le flow
//! avec un nœud inject par défaut : l'API refuse un graphe vide
//! (`empty_graph`).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{FlowGraph, FlowNode, FlowNodeKind, FlowSummary, InjectConfig, Position};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::loading_overlay::LoadingOverlay;
use crate::state::{org, session, toasts};

/// Rôle de l'utilisateur dans l'org courante (« owner »/« admin »/« viewer ») —
/// même helper que `devices.rs` (privé par page, convention projet).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

#[component]
pub fn Flows(id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    // The open flow mirrors the `?id=` of the URL (O31): deep links (search,
    // assistant, events, "Create and open the flow") and a reload land on
    // it, the nav link (no id) goes back to the list.
    let mut selected = use_signal(|| id.parse::<i64>().ok());
    use_effect(use_reactive((&id,), move |(id,)| {
        let want = id.parse::<i64>().ok();
        if *selected.peek() != want {
            selected.set(want);
        }
    }));
    use_effect(move || {
        let want = selected().map(|f| f.to_string()).unwrap_or_default();
        if let crate::app::Route::Flows { id } = router().current::<crate::app::Route>() {
            if id != want {
                navigator().replace(crate::app::Route::Flows { id: want });
            }
        }
    });
    let mut filter_status = use_signal(|| "all".to_string());
    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    // Cible de suppression (id, nom) — confirmation à la demande.
    let mut delete_target = use_signal(|| None::<(i64, String)>);
    // Suppression en cours (id) : le delete attend l'acquittement runtime —
    // la ligne montre un spinner pour ne pas donner l'impression d'un clic perdu.
    let mut deleting = use_signal(|| None::<i64>);

    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    let list = use_resource(move || {
        let filters = api::flows::FlowFilters {
            search: {
                let value = search().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            },
            status: match filter_status().as_str() {
                "all" => None,
                other => Some(other.to_string()),
            },
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::flows::list(&filters).await
        }
    });

    // Lecture synchrone de la ressource (doctrine socle CRUD : le
    // discriminant d'état et les lignes sortent du match avant le rsx).
    let (list_state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.results.is_empty() && paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    // Colonnes de la table — les closures d'action capturent les signaux
    // (Copy) de la page ; les classes td historiques via with_td_class.
    let columns = vec![
        Column::new(t!("flows-col-name").to_string(), |flow: &FlowSummary| {
            rsx! { {flow.name.clone()} }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("flows-col-status").to_string(), |flow: &FlowSummary| {
            let (badge, label) = status_badge(&flow.status);
            rsx! {
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {badge} w-fit",
                    {label}
                }
            }
        }),
        Column::new(t!("flows-col-versions").to_string(), |flow: &FlowSummary| {
            rsx! {
                div { class: "flex items-center gap-2",
                    span { {format!("v{}", flow.latest_version_number)} }
                    if let Some(deployed) = flow.deployed_version_number {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-50 text-green-700 border border-green-200",
                            {format!("v{deployed} {}", t!("flows-version-deployed-tag"))}
                        }
                    }
                }
            }
        })
        .with_td_class("text-sm text-gray-600").secondary(),
        Column::new(t!("flows-col-updated").to_string(), |flow: &FlowSummary| {
            rsx! { {date_label(&flow.updated_at)} }
        })
        .with_td_class("text-gray-500 text-sm").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |flow: &FlowSummary| {
                let pk = flow.id;
                // Cloné AVANT la closure onclick : la référence `flow` ne
                // sort pas du corps de la cellule (E0521).
                let name_delete = flow.name.clone();
                let is_deleting = deleting() == Some(pk);
                rsx! {
                    div { class: "flex items-center gap-1.5",
                        button {
                            class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: is_deleting,
                            onclick: move |_| selected.set(Some(pk)),
                            {t!("flows-open")}
                        }
                        if can_write {
                            button {
                                class: format!("{DANGER_BTN} disabled:opacity-40 disabled:cursor-not-allowed"),
                                disabled: is_deleting,
                                onclick: move |_| delete_target.set(Some((pk, name_delete.clone()))),
                                if is_deleting {
                                    span { class: "animate-spin inline-block rounded-full h-3 w-3 border-2 border-red-400 border-t-transparent align-middle mr-0.5" }
                                } else {
                                    icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                }
                                {if is_deleting { t!("flows-deleting") } else { t!("flows-delete") }}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        if let Some(flow_id) = selected() {
            // Éditeur plein écran, HORS ListLayout — plus de bouton
            // « + Nouveau flow » en contexte d'édition (principe 1 de la
            // coquille d'éditeur : l'ajout vit dans le canvas).
            crate::components::flow_editor::FlowEditor {
                key: "{flow_id}",
                flow_id,
                can_write,
                on_back: move |_| {
                    selected.set(None);
                    // Retour à la liste : refetch systématique —
                    // la vérité serveur (flow créé, renommage,
                    // deploy faits dans l'éditeur) s'affiche sans
                    // passer par le bouton refresh.
                    reload.with_mut(|r| *r += 1);
                },
                on_changed: move |_| reload.with_mut(|r| *r += 1),
            }
        } else {
            ListLayout {
                title: t!("nav-flows").to_string(),
                subtitle: Some(t!("flows-subtitle").to_string()),
                on_refresh: move |_| reload.with_mut(|r| *r += 1),
                can_write,
                add_label: Some(t!("flows-new").to_string()),
                on_add: move |_| {
                    // Création immédiate, sans modale : nom daté + graphe
                    // de départ, puis ouverture directe de l'éditeur
                    // (fluidité — retour user 2026-09-18). Le renommage
                    // reste possible après coup.
                    let params = pnex_core::CreateFlow {
                        name: t!("flows-default-name", date : crate ::util::now_label()).to_string(),
                        device_id: None,
                        graph: starter_graph(),
                        author: session::user().map(|user| user.username),
                        note: None,
                    };
                    spawn(async move {
                        match api::flows::create(params).await {
                            Ok(flow) => {
                                toasts::success("toast-flow-created");
                                selected.set(Some(flow.id));
                                reload.with_mut(|r| *r += 1);
                            }
                            Err(err) => toasts::error(err),
                        }
                    });
                },
                if org::current().is_none() {
                    p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
                } else {
                    FilterBar {
                        select {
                            aria_label: t!("flows-col-status"),
                            class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                            onchange: move |event| {
                                filter_status.set(event.value());
                                page.set(0);
                                reload.with_mut(|r| *r += 1);
                            },
                            option {
                                value: "all",
                                selected: filter_status() == "all",
                                {t!("flows-filter-status-all")}
                            }
                            option {
                                value: "draft",
                                selected: filter_status() == "draft",
                                {t!("flows-status-draft")}
                            }
                            option {
                                value: "deployed",
                                selected: filter_status() == "deployed",
                                {t!("flows-status-deployed")}
                            }
                            option {
                                value: "stopped",
                                selected: filter_status() == "stopped",
                                {t!("flows-status-stopped")}
                            }
                            option {
                                value: "error",
                                selected: filter_status() == "error",
                                {t!("flows-status-error")}
                            }
                        }
                        SearchInput {
                            placeholder: t!("flows-search-placeholder").to_string(),
                            value: search,
                            on_submit: move |_| {
                                page.set(0);
                                reload.with_mut(|r| *r += 1);
                            },
                        }
                    }

                    ListStates {
                        state: list_state,
                        is_empty,
                        empty_message: t!("flows-empty").to_string(),
                        div { class: "relative",
                            DataTable {
                                columns,
                                rows,
                                row_key: RowKey::new(|flow: &FlowSummary| flow.id.to_string()),
                            }
                            // Opération serveur en cours (delete attend
                            // l'acquittement runtime) — feedback global.
                            if deleting().is_some() {
                                LoadingOverlay { message: t!("flows-deleting").to_string() }
                            }
                        }
                        ListPager { count, page }
                    }
                    // Modal de création supprimée : « + Nouveau » crée
                    // directement (nom daté) et ouvre l'éditeur.

                    // Suppression confirmée d'une ligne.
                    if let Some((flow_id, flow_name)) = delete_target() {
                        ConfirmDialog {
                            title: t!("flows-confirm-delete-title"),
                            message: t!(
                                "common-quoted-message", name : flow_name.clone(), message :
                                t!("flows-confirm-delete-message")
                            ),
                            confirm_label: t!("flows-delete"),
                            on_confirm: move |_| {
                                delete_target.set(None);
                                deleting.set(Some(flow_id));
                                spawn(async move {
                                    // La liste est rafraîchie dans les deux
                                    // cas : elle reflète la vérité serveur
                                    // (un échec = le flow existe encore).
                                    let outcome = api::flows::delete(flow_id).await;
                                    deleting.set(None);
                                    match outcome {
                                        Ok(()) => toasts::success("toast-flow-deleted"),
                                        Err(err) => toasts::error(err),
                                    }
                                    reload.with_mut(|r| *r += 1);
                                });
                            },
                            on_cancel: move |_| delete_target.set(None),
                        }
                    }
                }
            }
        }
    }
}

/// Badge de statut — classes littérales complètes (scan Tailwind).
fn status_badge(status: &str) -> (&'static str, String) {
    let label = match status {
        "deployed" => t!("flows-status-deployed"),
        "stopped" => t!("flows-status-stopped"),
        "error" => t!("flows-status-error"),
        _ => t!("flows-status-draft"),
    };
    let badge = match status {
        "deployed" => "bg-green-100 text-green-800",
        "stopped" => "bg-amber-100 text-amber-800",
        "error" => "bg-red-100 text-red-800",
        _ => "bg-gray-100 text-gray-800",
    };
    (badge, label)
}

/// Graphe de départ d'un nouveau flow : un nœud inject en **répétition
/// continue** (l'API refuse un graphe vide — violation `empty_graph`).
pub(crate) fn starter_graph() -> FlowGraph {
    FlowGraph {
        nodes: vec![FlowNode {
            id: "n1".into(),
            name: None,
            position: Some(Position { x: 100.0, y: 100.0 }),
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Inject {
                config: InjectConfig {
                    repeat_secs: Some(30.0),
                    ..Default::default()
                },
            },
        }],
    }
}
