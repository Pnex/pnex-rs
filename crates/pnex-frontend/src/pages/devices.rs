//! Gestion des appareils — portée du `Devices.tsx` React sur l'API Phase 4 :
//! liste filtrable (type, statut, device_id), enregistrement via l'assistant
//! modal (`components/device_wizard.rs` : build auto suivi en modale, snippet
//! Python pour les customs), détail (token de provisioning, métadonnées
//! JSON), suppression. Le scoping org vient du client (`X-Org-Id`), l'écriture
//! est réservée owner/admin (le serveur force, l'UI masque).
//!
//! Le détail est piloté par un signal local `selected` + `key` (cf. orgs.rs).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::icons;
use crate::components::pager::Pager;
use crate::state::{org, session, toasts};

/// Rôle de l'utilisateur dans l'org courante (« owner »/« admin »/« viewer »).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

/// Taille de page de la liste (alignée sur le défaut serveur, D14 :
/// `PAGINATION_DEFAULT_LIMIT`, 10).
const PAGE_SIZE: i64 = 10;

#[component]
pub fn Devices() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| None::<i64>);
    let mut filter_type = use_signal(|| "all".to_string());
    let mut filter_status = use_signal(|| "all".to_string());
    let mut filter_capability = use_signal(String::new);
    let mut search = use_signal(String::new);
    // Page courante (0-based) — remise à 0 à chaque changement de filtre.
    let mut page = use_signal(|| 0i64);
    // Assistant d'enregistrement (mont/démont = état propre à chaque ouverture).
    let mut wizard_open = use_signal(|| false);

    let can_write = current_role()
        .is_some_and(|role| matches!(role.as_str(), "owner" | "admin"));

    let list = use_resource(move || {
        let filters = api::devices::DeviceFilters {
            device_type: match filter_type().as_str() {
                "all" => None,
                other => Some(other.to_string()),
            },
            capability: {
                let value = filter_capability().trim().to_string();
                if value.is_empty() { None } else { Some(value) }
            },
            device_id: None,
            search: {
                let value = search().trim().to_string();
                if value.is_empty() { None } else { Some(value) }
            },
            active: match filter_status().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::devices::list(&filters).await
        }
    });

    // Capacités pour le filtre (le wizard charge son propre catalogue).
    let capabilities = use_resource(|| async move { api::devices::capabilities().await });

    rsx! {
        div { class: "p-6",
            div { class: "mb-8 flex items-center justify-between flex-wrap gap-3",
                div {
                    h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-devices")} }
                    p { class: "text-gray-600 mt-2", {t!("devices-subtitle")} }
                }
                if can_write {
                    button {
                        class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium",
                        onclick: move |_| wizard_open.set(true),
                        icons::Plus { class: "h-4 w-4 inline mr-1" }
                        {t!("devices-register")}
                    }
                }
            }

            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                match selected() {
                    Some(device_pk) => rsx! {
                        DeviceDetail {
                            key: "{device_pk}",
                            device_pk,
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
                                    filter_type.set(event.value());
                                    page.set(0);
                                    reload.with_mut(|r| *r += 1);
                                },
                                option { value: "all", selected: filter_type() == "all", {t!("devices-type-all")} }
                                option { value: "sensor", selected: filter_type() == "sensor", {t!("devices-type-sensor")} }
                                option { value: "actuator", selected: filter_type() == "actuator", {t!("devices-type-actuator")} }
                                option { value: "mixed", selected: filter_type() == "mixed", {t!("devices-type-mixed")} }
                            }
                            select {
                                class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                onchange: move |event| {
                                    filter_status.set(event.value());
                                    page.set(0);
                                    reload.with_mut(|r| *r += 1);
                                },
                                option { value: "all", selected: filter_status() == "all", {t!("devices-status-all")} }
                                option { value: "true", selected: filter_status() == "true", {t!("devices-status-active")} }
                                option { value: "false", selected: filter_status() == "false", {t!("devices-status-inactive")} }
                            }
                            {match &*capabilities.read() {
                                Some(Ok(caps)) if !caps.is_empty() => rsx! {
                                    select {
                                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                        onchange: move |event| {
                                            filter_capability.set(event.value());
                                            page.set(0);
                                            reload.with_mut(|r| *r += 1);
                                        },
                                        option { value: "", selected: filter_capability().is_empty(), {t!("devices-capability-all")} }
                                        for cap in caps {
                                            option {
                                                value: "{cap.name}",
                                                selected: filter_capability() == cap.name,
                                                {cap.name.clone()}
                                            }
                                        }
                                    }
                                },
                                _ => rsx! {},
                            }}
                            input {
                                class: "flex-1 min-w-48 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                r#type: "search",
                                placeholder: t!("devices-search-placeholder"),
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
                                p { class: "text-gray-500 text-center py-12", {t!("devices-empty")} }
                            },
                            Some(Ok(paged)) => rsx! {
                                div { class: "bg-white rounded-lg shadow-sm overflow-hidden",
                                    table { class: "min-w-full divide-y divide-gray-200",
                                        thead { class: "bg-gray-50",
                                            tr {
                                                th { class: "th", {t!("devices-col-id")} }
                                                th { class: "th", {t!("devices-col-type")} }
                                                th { class: "th", {t!("devices-col-model")} }
                                                th { class: "th", {t!("devices-col-status")} }
                                                th { class: "th", {t!("common-actions")} }
                                            }
                                        }
                                        tbody { class: "bg-white divide-y divide-gray-200",
                                            for device in paged.results.clone() {
                                                {device_row(device, selected)}
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

                        // Assistant d'enregistrement (monté à la demande :
                        // l'état interne se réinitialise à chaque ouverture).
                        if wizard_open() {
                            crate::components::device_wizard::DeviceWizard {
                                on_close: move |_| wizard_open.set(false),
                                on_changed: move |_| reload.with_mut(|r| *r += 1),
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Ligne device : identifiant firmware, type, modèle, statut, action détail.
fn device_row(device: pnex_core::Device, mut selected: Signal<Option<i64>>) -> Element {
    let pk = device.id;
    let (type_badge, type_label) = type_badge(&device.device_type);
    rsx! {
        tr { key: "{pk}", class: "hover:bg-gray-50",
            td { class: "td font-medium text-gray-900",
                code { class: "text-sm", {device.device_id.clone()} }
            }
            td { class: "td",
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {type_badge}",
                    {type_label}
                }
            }
            td { class: "td text-gray-600", {device.predefined_device_name.clone()} }
            td { class: "td",
                div { class: "flex flex-col gap-0.5",
                    if device.active {
                        span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800 w-fit",
                            {t!("devices-status-active")}
                        }
                    } else {
                        span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-600 w-fit",
                            {t!("devices-status-inactive")}
                        }
                    }
                    {last_seen_label(&device.last_seen)}
                }
            }
            td { class: "td",
                button {
                    class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors",
                    onclick: move |_| selected.set(Some(pk)),
                    {t!("devices-detail")}
                }
            }
        }
    }
}

/// Badge de type — classes littérales complètes (scan Tailwind).
fn type_badge(device_type: &str) -> (&'static str, String) {
    let label = match device_type {
        "sensor" => t!("devices-type-sensor"),
        "actuator" => t!("devices-type-actuator"),
        "mixed" => t!("devices-type-mixed"),
        other => other.to_string(),
    };
    let badge = match device_type {
        "sensor" => "bg-blue-100 text-blue-800",
        "actuator" => "bg-amber-100 text-amber-800",
        "mixed" => "bg-purple-100 text-purple-800",
        _ => "bg-gray-100 text-gray-800",
    };
    (badge, label)
}

/// Libellé « vu à HH:MM:SS » (heure locale) sous le badge de statut —
/// le bail de vie Phase 5 rend cette information vivante.
fn last_seen_label(last_seen: &Option<String>) -> Element {
    let label = match last_seen
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    {
        Some(ts) => {
            let local = ts.with_timezone(&chrono::Local);
            format!("{} {}", t!("devices-last-seen-at"), local.format("%H:%M:%S"))
        }
        None => t!("devices-last-seen-never"),
    };
    rsx! {
        span { class: "text-[11px] text-gray-400", {label} }
    }
}

#[component]
fn DeviceDetail(
    device_pk: i64,
    can_write: bool,
    on_back: Callback<()>,
    on_changed: Callback<()>,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut confirm_delete = use_signal(|| false);
    let mut show_token = use_signal(|| false);

    let detail = use_resource(move || async move {
        let _ = reload();
        api::devices::detail(device_pk).await
    });

    let refresh = Callback::new(move |_: ()| {
        reload.with_mut(|r| *r += 1);
        on_changed.call(());
    });

    match &*detail.value().read() {
        Some(Ok(device)) => {
            let capabilities = device.capabilities.clone();
            let token = device.device_token.clone();
            let device_id = device.device_id.clone();
            let metadata_text = metadata_to_text(&device.metadata);
            rsx! {
                div { class: "bg-white rounded-lg shadow-sm",
                    div { class: "p-6 border-b border-gray-200 flex items-center justify-between gap-4",
                        div {
                            button {
                                class: "text-sm text-blue-600 hover:text-blue-700 mb-1",
                                onclick: move |_| on_back.call(()),
                                { "← " }
                                {t!("devices-back")}
                            }
                            h2 { class: "text-xl font-semibold text-gray-900",
                                code { {device_id.clone()} }
                            }
                        }
                        if can_write {
                            button {
                                class: "px-3 py-1.5 text-sm font-medium text-red-700 bg-red-50 border border-red-200 rounded-lg hover:bg-red-100 transition-colors",
                                onclick: move |_| confirm_delete.set(true),
                                icons::Trash2 { class: "h-4 w-4 inline mr-1" }
                                {t!("devices-delete")}
                            }
                        }
                    }

                    // Capacités du modèle
                    if !capabilities.is_empty() {
                        div { class: "p-6 border-b border-gray-200",
                            h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider mb-3", {t!("devices-capabilities")} }
                            div { class: "flex flex-wrap gap-2",
                                for cap in capabilities {
                                    span {
                                        key: "{cap.id}",
                                        class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-700",
                                        title: "{cap.mode}",
                                        {cap.name.clone()}
                                    }
                                }
                            }
                        }
                    }

                    // Token de provisioning (affiché sur demande)
                    div { class: "p-6 border-b border-gray-200",
                        div { class: "flex items-center justify-between mb-3",
                            h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider", {t!("devices-token")} }
                            if token.as_ref().is_some_and(|t| t.is_active) {
                                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800",
                                    {t!("devices-token-active")}
                                }
                            }
                        }
                        match (&token, show_token()) {
                            (Some(token), true) => rsx! {
                                div { class: "space-y-2",
                                    div { class: "text-xs text-gray-500", {t!("devices-token-value")} }
                                    code { class: "block p-3 bg-gray-50 rounded-lg text-sm break-all", {token.token.clone()} }
                                    div { class: "text-xs text-gray-500", {t!("devices-encryption-key")} }
                                    code { class: "block p-3 bg-gray-50 rounded-lg text-sm break-all",
                                        {token.encryption_key.clone().unwrap_or_else(|| "—".into())}
                                    }
                                    button {
                                        class: "text-sm text-blue-600 hover:text-blue-700",
                                        onclick: move |_| show_token.set(false),
                                        {t!("devices-token-hide")}
                                    }
                                }
                            },
                            _ => rsx! {
                                button {
                                    class: "text-sm text-blue-600 hover:text-blue-700",
                                    onclick: move |_| show_token.set(true),
                                    {t!("devices-token-show")}
                                }
                            },
                        }
                    }

                    // Métadonnées (metadata-only, contrat Django)
                    div { class: "p-6",
                        h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider mb-3", {t!("devices-metadata")} }
                        if can_write {
                            MetadataEditor {
                                key: "{device_pk}-{reload}",
                                device_pk,
                                initial: metadata_text,
                                on_saved: refresh,
                            }
                        } else {
                            code { class: "block p-3 bg-gray-50 rounded-lg text-sm break-all whitespace-pre-wrap",
                                {metadata_text}
                            }
                        }
                    }
                }

                if confirm_delete() {
                    crate::components::confirm::ConfirmDialog {
                        title: t!("devices-confirm-delete-title"),
                        message: t!("devices-confirm-delete-message"),
                        confirm_label: t!("devices-delete"),
                        on_confirm: move |_| {
                            confirm_delete.set(false);
                            let device_pk = device_pk;
                            spawn(async move {
                                match api::devices::delete(device_pk).await {
                                    Ok(()) => toasts::success("toast-saved"),
                                    Err(err) => toasts::error(err.message),
                                }
                            });
                            on_back.call(());
                            on_changed.call(());
                        },
                        on_cancel: move |_| confirm_delete.set(false),
                    }
                }
            }
        }
        Some(Err(err)) => rsx! {
            div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700", {err.message.clone()} }
        },
        None => rsx! {
            div { class: "text-center py-12",
                span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
            }
        },
    }
}

/// Métadonnées en texte éditable (pretty-printé, `null` si absent).
fn metadata_to_text(metadata: &Option<serde_json::Value>) -> String {
    match metadata {
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into()),
        None => "null".into(),
    }
}

/// Éditeur JSON des métadonnées — seul champ modifiable d'un device.
/// Remonté par `key` à chaque rechargement ressource (état local propre).
#[component]
fn MetadataEditor(device_pk: i64, initial: String, on_saved: Callback<()>) -> Element {
    let mut text = use_signal(move || initial);
    let mut invalid = use_signal(|| false);
    let border = if invalid() {
        "border-red-400 bg-red-50"
    } else {
        "border-gray-300"
    };

    rsx! {
        div { class: "space-y-2",
            textarea {
                class: "w-full h-40 px-3 py-2 border rounded-lg text-sm font-mono {border}",
                value: "{text}",
                oninput: move |event| {
                    text.set(event.value());
                    invalid.set(false);
                },
            }
            div { class: "flex items-center gap-3",
                button {
                    class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm",
                    onclick: move |_| {
                        let raw = text.cloned();
                        let parsed = if raw.trim().is_empty() {
                            Ok(serde_json::Value::Null)
                        } else {
                            serde_json::from_str(&raw)
                        };
                        match parsed {
                            Ok(metadata) => {
                                let device_pk = device_pk;
                                spawn(async move {
                                    match api::devices::update_metadata(device_pk, metadata).await {
                                        Ok(_) => toasts::success("toast-saved"),
                                        Err(err) => toasts::error(err.message),
                                    }
                                    on_saved.call(());
                                });
                            }
                            Err(_) => invalid.set(true),
                        }
                    },
                    {t!("devices-metadata-save")}
                }
                if invalid() {
                    span { class: "text-sm text-red-600", {t!("devices-metadata-invalid")} }
                }
            }
        }
    }
}
