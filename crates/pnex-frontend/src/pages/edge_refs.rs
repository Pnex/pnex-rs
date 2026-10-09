//! Page `/edges/refs` — référentiels Edge : identifiants WiFi
//! (`wifi_credentials`) et serveurs PNeX (`pnex_hosts`), les entrées
//! consommées par le wizard device et la file de build firmware.
//!
//! École `pages/notifications.rs` : onglets pill (WiFi / serveurs) au-dessus
//! du socle CRUD (ListLayout + DataTable + ConfirmDialog). Création = upsert
//! POST (clé (org, ssid) / (org, host)) ; édition = PUT in-place (id
//! conservé, renommage possible, 409 si la clé visée existe ailleurs).
//! Droits owner/admin comme partout (le serveur force, l'UI masque).

use crate::components::secret_field::{can_manage_secrets, SecretDraft, SecretField};
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{PnexHost, WifiCredential};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::state::{org, session, toasts};

fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Wifi,
    Hosts,
}

/// Host display (same rendering as the wizard picker) — devices always
/// connect over TLS (D70: wss only).
fn host_display(host: &str) -> String {
    format!("wss://{host}")
}

#[component]
pub fn EdgeRefs() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut tab = use_signal(|| Tab::Wifi);
    // Formulaires : None = fermé, Some(None) = création, Some(Some(row)) =
    // édition (upsert POST sur la même clé).
    let mut edit_wifi = use_signal(|| None::<Option<WifiCredential>>);
    let mut edit_host = use_signal(|| None::<Option<PnexHost>>);
    let mut delete_target = use_signal(|| None::<(&'static str, i64, String)>);

    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    // Valeurs des signaux-modals lues AVANT le rsx (pas de `let` dans rsx).
    let wifi_modal = edit_wifi.read().clone();
    let host_modal = edit_host.read().clone();
    let delete_modal = delete_target.read().clone();

    let wifis = use_resource(move || {
        let _ = reload();
        async move { api::wifi::list().await }
    });
    let hosts = use_resource(move || {
        let _ = reload();
        async move { api::hosts::list().await }
    });
    // Host imposed by the deployment (`PNEX_PROD_HOST`): the hosts tab turns
    // read-only. An error (older server) means no lock.
    let locked = use_resource(|| async { api::hosts::locked().await.ok().flatten() });
    let locked_host = locked.cloned().flatten();
    let hosts_locked = locked_host.is_some();

    rsx! {
        ListLayout {
            title: t!("edgerefs-title").to_string(),
            subtitle: Some(t!("edgerefs-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: can_write && !(tab() == Tab::Hosts && hosts_locked),
            // Le libellé « ajouter » suit l'onglet actif (WiFi vs serveurs).
            add_label: Some(
                if tab() == Tab::Wifi {
                    t!("edgerefs-new-wifi").to_string()
                } else {
                    t!("edgerefs-new-host").to_string()
                },
            ),
            on_add: move |_| match tab() {
                Tab::Wifi => edit_wifi.set(Some(None)),
                Tab::Hosts => edit_host.set(Some(None)),
            },
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                div { class: "mb-6 flex flex-wrap gap-1 border-b border-gray-200 pb-2",
                    for tab_def in [(Tab::Wifi, "edgerefs-tab-wifi"), (Tab::Hosts, "edgerefs-tab-hosts")] {
                        button {
                            key: "{tab_def.1}",
                            class: if tab() == tab_def.0 { "px-4 py-2 rounded-lg text-sm font-medium bg-blue-600 text-white" } else { "px-4 py-2 rounded-lg text-sm font-medium text-gray-600 hover:bg-gray-100" },
                            onclick: move |_| tab.set(tab_def.0),
                            span { class: "inline-flex items-center gap-1.5",
                                match tab_def.0 {
                                    Tab::Wifi => rsx! {
                                        icons::Wifi { class: "h-4 w-4" }
                                    },
                                    Tab::Hosts => rsx! {
                                        icons::Server { class: "h-4 w-4" }
                                    },
                                }
                                {t!(tab_def.1)}
                            }
                        }
                    }
                }

                match tab() {
                    Tab::Wifi => rsx! {
                        WifiTab {
                            wifis,
                            can_write,
                            edit_wifi,
                            delete_target,
                        }
                    },
                    Tab::Hosts => {
                        match locked_host.clone() {
                            Some(host) => rsx! {
                                div { class: "rounded-lg border border-gray-200 bg-gray-50 p-4 text-sm text-gray-700 space-y-1",
                                    p { class: "font-mono", "wss://{host}" }
                                    p { class: "text-xs text-gray-500", {t!("wizard-host-locked-hint")} }
                                }
                            },
                            None => rsx! {
                                HostsTab {
                                    hosts,
                                    can_write,
                                    edit_host,
                                    delete_target,
                                }
                            },
                        }
                    }
                }
            }

            // ── Modals ──────────────────────────────────────────────
            if let Some(existing) = wifi_modal {
                WifiForm {
                    key: "{existing.as_ref().map(|w| w.id).unwrap_or_default()}",
                    existing,
                    on_close: move |_| edit_wifi.set(None),
                    on_saved: move |_| {
                        edit_wifi.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some(existing) = host_modal {
                HostForm {
                    key: "{existing.as_ref().map(|h| h.id).unwrap_or_default()}",
                    existing,
                    on_close: move |_| edit_host.set(None),
                    on_saved: move |_| {
                        edit_host.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some((what, id, name)) = delete_modal {
                ConfirmDialog {
                    title: t!("edgerefs-confirm-delete-title"),
                    message: t!(
                        "common-quoted-message", name : name.clone(), message :
                        t!("edgerefs-confirm-delete-message")
                    ),
                    confirm_label: t!("common-delete"),
                    on_confirm: move |_| {
                        let (what, id, _) = (what, id, name.clone());
                        delete_target.set(None);
                        spawn(async move {
                            let result = match what {
                                "wifi" => api::wifi::delete(id).await,
                                _ => api::hosts::delete(id).await,
                            };
                            match result {
                                Ok(()) => {
                                    toasts::success("toast-edgerefs-deleted");
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(err) => toasts::error(err),
                            }
                        });
                    },
                    on_cancel: move |_| delete_target.set(None),
                }
            }
        }
    }
}

/// Résultats typés pour les props de tabs (alias lisibles).
type WifiListResult = Result<Vec<WifiCredential>, crate::api::error::ApiError>;
type HostListResult = Result<Vec<PnexHost>, crate::api::error::ApiError>;

#[component]
fn WifiTab(
    wifis: Resource<WifiListResult>,
    can_write: bool,
    mut edit_wifi: Signal<Option<Option<WifiCredential>>>,
    mut delete_target: Signal<Option<(&'static str, i64, String)>>,
) -> Element {
    // Lecture synchrone de la ressource (doctrine socle CRUD).
    let (list_state, is_empty, rows) = match &*wifis.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(rows)) => (Some(Ok(())), rows.is_empty(), rows.clone()),
        Some(Err(err)) => (Some(Err(err.clone())), false, Vec::new()),
    };

    let columns = vec![
        Column::new(
            t!("builds-field-ssid").to_string(),
            |wifi: &WifiCredential| {
                rsx! { {wifi.ssid.clone()} }
            },
        )
        .with_td_class("font-medium text-gray-900"),
        Column::new(
            t!("edgerefs-col-created").to_string(),
            |wifi: &WifiCredential| {
                rsx! { {date_label(&wifi.created_at)} }
            },
        )
        .with_td_class("text-gray-600").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |wifi: &WifiCredential| {
                // Cloné avant les closures onclick (référence &T ne sort pas
                // du corps de cellule).
                let wifi_edit = wifi.clone();
                let id_delete = wifi.id;
                let name_delete = wifi.ssid.clone();
                rsx! {
                    if can_write {
                        div { class: "flex gap-2",
                            button {
                                class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                                onclick: move |_| edit_wifi.set(Some(Some(wifi_edit.clone()))),
                                {t!("edgerefs-edit")}
                            }
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| delete_target.set(Some(("wifi", id_delete, name_delete.clone()))),
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("common-delete")}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        ListStates {
            state: list_state,
            is_empty,
            empty_message: t!("edgerefs-empty-wifi").to_string(),
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|wifi: &WifiCredential| wifi.id.to_string()),
            }
        }
    }
}

#[component]
fn HostsTab(
    hosts: Resource<HostListResult>,
    can_write: bool,
    mut edit_host: Signal<Option<Option<PnexHost>>>,
    mut delete_target: Signal<Option<(&'static str, i64, String)>>,
) -> Element {
    // Lecture synchrone de la ressource (doctrine socle CRUD).
    let (list_state, is_empty, rows) = match &*hosts.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(rows)) => (Some(Ok(())), rows.is_empty(), rows.clone()),
        Some(Err(err)) => (Some(Err(err.clone())), false, Vec::new()),
    };

    let columns = vec![
        Column::new(t!("builds-field-server").to_string(), |host: &PnexHost| {
            rsx! { {host_display(&host.host)} }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("edgerefs-col-created").to_string(), |host: &PnexHost| {
            rsx! { {date_label(&host.created_at)} }
        })
        .with_td_class("text-gray-600").secondary(),
        Column::new(t!("common-actions").to_string(), move |host: &PnexHost| {
            let host_edit = host.clone();
            let id_delete = host.id;
            let name_delete = host_display(&host.host);
            rsx! {
                if can_write {
                    div { class: "flex gap-2",
                        button {
                            class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                            onclick: move |_| edit_host.set(Some(Some(host_edit.clone()))),
                            {t!("edgerefs-edit")}
                        }
                        button {
                            class: DANGER_BTN,
                            onclick: move |_| delete_target.set(Some(("host", id_delete, name_delete.clone()))),
                            icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                            {t!("common-delete")}
                        }
                    }
                }
            }
        }).actions(),
    ];

    rsx! {
        ListStates {
            state: list_state,
            is_empty,
            empty_message: t!("edgerefs-empty-hosts").to_string(),
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|host: &PnexHost| host.id.to_string()),
            }
        }
    }
}

/// Formulaire WiFi (création/édition) — upsert POST, clé = SSID : le champ
/// passe en lecture seule en édition (changer la clé créerait une autre
/// entrée en laissant l'ancienne).
#[component]
fn WifiForm(
    existing: Option<WifiCredential>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let is_edit = existing.is_some();
    let edit_id = existing.as_ref().map(|w| w.id);
    let mut ssid = use_signal(|| {
        existing
            .as_ref()
            .map(|w| w.ssid.clone())
            .unwrap_or_default()
    });
    // Vault field (secrets.md S6): type a value or pick a secret; an
    // existing entry shows "set · <name>".
    let password =
        use_signal(|| SecretDraft::from_view(existing.as_ref().and_then(|w| w.password.clone())));
    let can_manage = can_manage_secrets();
    let mut busy = use_signal(|| false);

    let ssid_placeholder = t!("builds-field-ssid");
    let submit = move |_| {
        let ssid_value = ssid().trim().to_string();
        let password_input = password().to_input();
        if ssid_value.is_empty() || password_input.is_none() {
            toasts::error("edgerefs-form-incomplete");
            return;
        }
        busy.set(true);
        spawn(async move {
            let input = pnex_core::WifiCredentialInput {
                ssid: ssid_value,
                password: password_input,
                ..Default::default()
            };
            // Édition = PUT in-place (id conservé, renommage possible,
            // 409 si le ssid visé existe ailleurs) ; création = upsert POST.
            let outcome = match edit_id {
                Some(id) => api::wifi::update(id, input).await,
                None => api::wifi::create(input).await,
            };
            match outcome {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(err) => {
                    busy.set(false);
                    toasts::error(err);
                }
            }
        });
    };

    rsx! {
        crate::components::modal::Modal {
            title: if is_edit { t!("edgerefs-edit-wifi") } else { t!("edgerefs-new-wifi") },
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-4",
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block",
                        {t!("builds-field-ssid")}
                    }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        r#type: "text",
                        placeholder: "{ssid_placeholder}",
                        value: "{ssid}",
                        oninput: move |e| ssid.set(e.value()),
                    }
                }
                SecretField {
                    label: t!("builds-field-wifi-password").to_string(),
                    draft: password,
                    can_manage,
                }
                // D118: no automatic rebuild, flashed devices keep the old
                // credentials until reflashed one by one.
                if is_edit {
                    p { class: "text-xs text-amber-700 bg-amber-50 border border-amber-200 rounded-lg p-2",
                        {t!("edgerefs-wifi-rotation-hint")}
                    }
                }
                div { class: crate::components::modal::MODAL_FOOTER,
                    button {
                        class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                        r#type: "button",
                        disabled: busy,
                        onclick: submit,
                        if busy() {
                            {t!("common-loading")}
                        } else {
                            {t!("common-save")}
                        }
                    }
                }
            }
        }
    }
}

/// Formulaire serveur PNeX (création/édition) — upsert POST, clé = host.
#[component]
fn HostForm(existing: Option<PnexHost>, on_close: Callback<()>, on_saved: Callback<()>) -> Element {
    let is_edit = existing.is_some();
    let edit_id = existing.as_ref().map(|h| h.id);
    // New entry: prefilled with the page's host — behind the TLS edge it is
    // exactly what devices must reach over wss (same default as the wizard).
    let mut host = use_signal(|| {
        existing
            .as_ref()
            .map(|h| h.host.clone())
            .unwrap_or_else(crate::util::default_device_host)
    });
    let mut busy = use_signal(|| false);

    let host_placeholder = t!("builds-field-server");

    let submit = move |_| {
        let host_value = host().trim().to_string();
        if host_value.is_empty() {
            toasts::error("edgerefs-form-incomplete");
            return;
        }
        busy.set(true);
        spawn(async move {
            let input = pnex_core::PnexHostInput { host: host_value };
            // Édition = PUT in-place (id conservé, renommage possible, 409
            // si l'hôte visé existe ailleurs) ; création = upsert POST.
            let outcome = match edit_id {
                Some(id) => api::hosts::update(id, input).await,
                None => api::hosts::create(input).await,
            };
            match outcome {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(err) => {
                    busy.set(false);
                    toasts::error(err);
                }
            }
        });
    };

    rsx! {
        crate::components::modal::Modal {
            title: if is_edit { t!("edgerefs-edit-host") } else { t!("edgerefs-new-host") },
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-4",
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block",
                        {t!("builds-field-server")}
                    }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        r#type: "text",
                        placeholder: "{host_placeholder}",
                        value: "{host}",
                        oninput: move |e| host.set(e.value()),
                    }
                    if crate::util::is_loopback_host(&host()) {
                        p { class: "mt-1 text-xs text-amber-600",
                            {t!("devices-host-loopback-hint")}
                        }
                    }
                }
                // Détection LAN côté serveur : préremplit l'hôte (la
                // sauvegarde reste un clic « Save » plus bas).
                crate::components::lan_detect::LanDetect { on_pick: move |picked| host.set(picked) }
                div { class: crate::components::modal::MODAL_FOOTER,
                    button {
                        class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                        r#type: "button",
                        disabled: busy,
                        onclick: submit,
                        if busy() {
                            {t!("common-loading")}
                        } else {
                            {t!("common-save")}
                        }
                    }
                }
            }
        }
    }
}
