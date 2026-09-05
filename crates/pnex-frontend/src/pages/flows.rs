//! Flux ETL (D18) — liste des flows de l'org (filtres + pagination D14) et
//! éditeur drag & drop (`components/flow_editor/`, sous-vue pilotée par le
//! signal local `selected` — pattern `devices.rs`).
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
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::pager::Pager;
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

/// Taille de page de la liste (D14 : `PAGINATION_DEFAULT_LIMIT`, 10).
const PAGE_SIZE: i64 = 10;

#[component]
pub fn Flows() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| None::<i64>);
    let mut filter_status = use_signal(|| "all".to_string());
    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut create_open = use_signal(|| false);
    // Cible de suppression (id, nom) — confirmation à la demande.
    let mut delete_target = use_signal(|| None::<(i64, String)>);

    let can_write = current_role().is_some_and(|role| matches!(role.as_str(), "owner" | "admin"));

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

    rsx! {
        div { class: "p-6",
            div { class: "mb-8 flex items-center justify-between flex-wrap gap-3",
                div {
                    h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-flows")} }
                    p { class: "text-gray-600 mt-2", {t!("flows-subtitle")} }
                }
                if can_write {
                    button {
                        class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium",
                        onclick: move |_| create_open.set(true),
                        icons::Plus { class: "h-4 w-4 inline mr-1" }
                        {t!("flows-new")}
                    }
                }
            }

            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                match selected() {
                    Some(flow_id) => rsx! {
                        crate::components::flow_editor::FlowEditor {
                            key: "{flow_id}",
                            flow_id,
                            can_write,
                            on_back: move |_| selected.set(None),
                            on_changed: move |_| reload.with_mut(|r| *r += 1),
                        }
                    },
                    None => rsx! {
                        // Filtres
                        div { class: "mb-6 flex flex-wrap items-center gap-2",
                            select {
                                class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                onchange: move |event| {
                                    filter_status.set(event.value());
                                    page.set(0);
                                    reload.with_mut(|r| *r += 1);
                                },
                                option { value: "all", selected: filter_status() == "all", {t!("flows-filter-status-all")} }
                                option { value: "draft", selected: filter_status() == "draft", {t!("flows-status-draft")} }
                                option { value: "deployed", selected: filter_status() == "deployed", {t!("flows-status-deployed")} }
                                option { value: "error", selected: filter_status() == "error", {t!("flows-status-error")} }
                            }
                            input {
                                class: "flex-1 min-w-48 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                r#type: "search",
                                placeholder: t!("flows-search-placeholder"),
                                value: "{search}",
                                oninput: move |event| search.set(event.value()),
                                onkeydown: move |event| {
                                    if event.key() == Key::Enter {
                                        event.prevent_default();
                                        page.set(0);
                                        reload.with_mut(|r| *r += 1);
                                    }
                                },
                            }
                            button {
                                class: "px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                                onclick: move |_| reload.with_mut(|r| *r += 1),
                                icons::RefreshCw { class: "h-4 w-4" }
                            }
                        }

                        match &*list.value().read() {
                            Some(Ok(paged)) if paged.results.is_empty() && paged.count == 0 => rsx! {
                                p { class: "text-gray-500 text-center py-12", {t!("flows-empty")} }
                            },
                            Some(Ok(paged)) => rsx! {
                                div { class: "bg-white rounded-lg shadow-sm overflow-hidden",
                                    table { class: "min-w-full divide-y divide-gray-200",
                                        thead { class: "bg-gray-50",
                                            tr {
                                                th { class: "th", {t!("flows-col-name")} }
                                                th { class: "th", {t!("flows-col-status")} }
                                                th { class: "th", {t!("flows-col-versions")} }
                                                th { class: "th", {t!("flows-col-device")} }
                                                th { class: "th", {t!("flows-col-updated")} }
                                                th { class: "th", {t!("common-actions")} }
                                            }
                                        }
                                        tbody { class: "bg-white divide-y divide-gray-200",
                                            for flow in paged.results.clone() {
                                                {flow_row(flow, selected, can_write, delete_target)}
                                            }
                                        }
                                    }
                                }
                                Pager {
                                    count: paged.count,
                                    page_size: PAGE_SIZE,
                                    page: page,
                                    on_navigate: move |new_page| {
                                        page.set(new_page);
                                    },
                                }
                            },
                            Some(Err(err)) => rsx! {
                                div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700", {err.message.clone()} }
                            },
                            None => rsx! {
                                div { class: "text-center py-12",
                                    span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
                                }
                            },
                        }

                        // Suppression confirmée d'une ligne.
                        if let Some((flow_id, flow_name)) = delete_target() {
                            crate::components::confirm::ConfirmDialog {
                                title: t!("flows-confirm-delete-title"),
                                message: format!("« {flow_name} » — {}", t!("flows-confirm-delete-message")),
                                confirm_label: t!("flows-delete"),
                                on_confirm: move |_| {
                                    delete_target.set(None);
                                    spawn(async move {
                                        match api::flows::delete(flow_id).await {
                                            Ok(()) => {
                                                toasts::success("toast-flow-deleted");
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(err) => toasts::error(err.message),
                                        }
                                    });
                                },
                                on_cancel: move |_| delete_target.set(None),
                            }
                        }

                        // Modal de création (monté à la demande).
                        if create_open() {
                            CreateFlowModal {
                                on_close: move |_| create_open.set(false),
                                on_created: move |flow_id| {
                                    create_open.set(false);
                                    selected.set(Some(flow_id));
                                },
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Ligne flow : nom, statut, versions (dernière + déployée), appareil,
/// mise à jour, action.
fn flow_row(
    flow: FlowSummary,
    mut selected: Signal<Option<i64>>,
    can_write: bool,
    mut delete_target: Signal<Option<(i64, String)>>,
) -> Element {
    let pk = flow.id;
    let (status_badge, status_label) = status_badge(&flow.status);
    let device_cell = flow.device_id.map(|id| format!("#{id}")).unwrap_or_else(|| "—".into());

    rsx! {
        tr { key: "{pk}", class: "hover:bg-gray-50",
            td { class: "td font-medium text-gray-900", {flow.name.clone()} }
            td { class: "td",
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {status_badge} w-fit",
                    {status_label}
                }
            }
            td { class: "td text-sm text-gray-600",
                div { class: "flex items-center gap-2",
                    span { {format!("v{}", flow.latest_version_number)} }
                    if let Some(deployed) = flow.deployed_version_number {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-50 text-green-700 border border-green-200",
                            {format!("v{deployed} {}", t!("flows-version-deployed-tag"))}
                        }
                    }
                }
            }
            td { class: "td text-gray-600", {device_cell} }
            td { class: "td text-gray-500 text-sm", {date_label(&flow.updated_at)} }
            td { class: "td",
                div { class: "flex items-center gap-1.5",
                    button {
                        class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors",
                        onclick: move |_| selected.set(Some(pk)),
                        {t!("flows-open")}
                    }
                    if can_write {
                        button {
                            class: "px-3 py-1 text-sm text-red-700 bg-red-50 border border-red-200 rounded-lg hover:bg-red-100 transition-colors",
                            onclick: move |_| delete_target.set(Some((pk, flow.name.clone()))),
                            icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                            {t!("flows-delete")}
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
        "error" => t!("flows-status-error"),
        _ => t!("flows-status-draft"),
    };
    let badge = match status {
        "deployed" => "bg-green-100 text-green-800",
        "error" => "bg-red-100 text-red-800",
        _ => "bg-gray-100 text-gray-800",
    };
    (badge, label)
}

/// Graphe de départ d'un nouveau flow : un nœud inject avec un déclencheur
/// initial (l'API refuse un graphe vide — violation `empty_graph`).
pub(crate) fn starter_graph() -> FlowGraph {
    FlowGraph {
        nodes: vec![FlowNode {
            id: "n1".into(),
            name: None,
            position: Some(Position { x: 100.0, y: 100.0 }),
            outputs: vec![],
            kind: FlowNodeKind::Inject {
                config: InjectConfig { once_delay_secs: Some(1.0), ..Default::default() },
            },
        }],
    }
}

/// Modal de création : nom + appareil optionnel + note. À la création,
/// l'éditeur s'ouvre directement sur le nouveau flow.
#[component]
fn CreateFlowModal(on_close: Callback<()>, on_created: Callback<i64>) -> Element {
    let mut name = use_signal(String::new);
    let mut device_id = use_signal(String::new);
    let mut note = use_signal(String::new);
    let mut name_error = use_signal(|| false);
    let mut creating = use_signal(|| false);

    // Catalogue borné pour le `<select>` (page max, parité `predefined_devices`).
    let devices = use_resource(|| async move {
        api::devices::list(&api::devices::DeviceFilters {
            limit: Some(100),
            ..Default::default()
        })
        .await
    });

    let submit = move |_| {
        let trimmed = name().trim().to_string();
        if trimmed.is_empty() {
            name_error.set(true);
            return;
        }
        creating.set(true);
        let params = pnex_core::CreateFlow {
            name: trimmed,
            device_id: device_id().parse::<i64>().ok(),
            graph: starter_graph(),
            author: session::user().map(|user| user.username),
            note: {
                let value = note().trim().to_string();
                if value.is_empty() { None } else { Some(value) }
            },
        };
        spawn(async move {
            match api::flows::create(params).await {
                Ok(flow) => {
                    toasts::success("toast-flow-created");
                    on_close.call(());
                    on_created.call(flow.id);
                }
                Err(err) => {
                    creating.set(false);
                    toasts::error(err.message);
                }
            }
        });
    };

    rsx! {
        Modal {
            title: t!("flows-create-title"),
            max_width: "max-w-md".to_string(),
            on_close,
            div { class: "space-y-4",
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-field-name")} }
                    input {
                        class: if name_error() {
                            "w-full px-3 py-2 border border-red-400 bg-red-50 rounded-lg text-sm"
                        } else {
                            "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm"
                        },
                        r#type: "text",
                        value: "{name}",
                        oninput: move |event| {
                            name.set(event.value());
                            name_error.set(false);
                        },
                    }
                    if name_error() {
                        span { class: "text-xs text-red-600 mt-1 block", {t!("flows-field-name-required")} }
                    }
                }
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-field-device")} }
                    select {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                        value: "{device_id}",
                        onchange: move |event| device_id.set(event.value()),
                        option { value: "", {t!("flows-field-device-none")} }
                        {match &*devices.value().read() {
                            Some(Ok(paged)) => rsx! {
                                for device in paged.results.clone() {
                                    option { value: "{device.id}", {format!("#{} · {}", device.id, device.device_id)} }
                                }
                            },
                            _ => rsx! {},
                        }}
                    }
                }
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-field-note")} }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        r#type: "text",
                        value: "{note}",
                        oninput: move |event| note.set(event.value()),
                    }
                }
                div { class: "flex justify-end gap-2 pt-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                        r#type: "button",
                        disabled: creating(),
                        onclick: submit,
                        icons::Plus { class: "h-4 w-4 inline mr-1" }
                        {t!("flows-new")}
                    }
                }
            }
        }
    }
}

