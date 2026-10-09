//! Pickers du référentiel Edge pour le wizard device : credentials WiFi et
//! hosts serveur PNeX, alimentés par `api::wifi` / `api::hosts` (fin de la
//! ressaisie à chaque device). Mini-formulaire d'ajout inline (ouvert par
//! défaut si le référentiel est vide — cas post-migration) + suppression.
//!
//! `selected` est un signal du parent (wizard) : le picker ne fait que le
//! tenir à jour ; the submit sends the entry id in `CreateBuild` (the
//! password stays in the vault, secrets.md S6).

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::icons;
use super::lan_detect::LanDetect;
use crate::api;
use crate::state::toasts;

/// Host display — devices always connect over TLS (D70: wss only).
fn host_display(host: &str) -> String {
    format!("wss://{host}")
}

#[component]
pub fn WifiCredentialPicker(mut selected: Signal<Option<pnex_core::WifiCredential>>) -> Element {
    let mut generation = use_signal(|| 0u32);
    let mut new_ssid = use_signal(String::new);
    let mut new_password = use_signal(String::new);
    // None = pas encore chargé ; le mini-form s'ouvre tout seul si le
    // référentiel est vide (cas post-migration).
    let mut show_add = use_signal(|| None::<bool>);

    let rows = use_resource(move || {
        let _ = generation();
        async move { api::wifi::list().await }
    });

    // Preselect the first entry (zero retyping). The select must never
    // display an entry while `selected` is None: dioxus-web applies
    // `value: ""` before the options exist, so the browser auto-picks the
    // first non-empty-value option and shows a phantom selection — clicking
    // Continue would then fail with wizard-config-incomplete even though an
    // entry appears selected. Mirroring the forced display in the signal
    // keeps state and DOM in sync.
    use_effect(move || {
        if let Some(Ok(rows)) = &*rows.read() {
            if selected.peek().is_none() {
                if let Some(first) = rows.first() {
                    selected.set(Some(first.clone()));
                }
            }
        }
    });

    // Labels i18n résolus hors rsx + id courant du select.
    let ssid_placeholder = t!("builds-field-ssid");
    let password_placeholder = t!("builds-field-wifi-password");
    let delete_title = t!("wizard-ref-delete");
    let selected_id = selected().map(|c| c.id.to_string()).unwrap_or_default();

    // Sauvegarde inline (upsert) puis sélection de l'entrée créée/mise à jour.
    let save_wifi = move |_| {
        let ssid = new_ssid().trim().to_string();
        let password = new_password();
        if ssid.is_empty() || password.is_empty() {
            toasts::error("wizard-config-incomplete");
            return;
        }
        spawn(async move {
            let input = pnex_core::WifiCredentialInput {
                ssid,
                password: Some(pnex_core::SecretFieldInput::Value { value: password }),
                ..Default::default()
            };
            match api::wifi::create(input).await {
                Ok(created) => {
                    selected.set(Some(created));
                    show_add.set(Some(false));
                    new_ssid.set(String::new());
                    new_password.set(String::new());
                    generation += 1;
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    rsx! {
        div { class: "sm:col-span-2 space-y-2",
            label {
                id: "edge-refs-wifi-label",
                r#for: "edge-refs-wifi-select",
                class: "text-xs font-medium text-gray-500 mb-1 block",
                {t!("wizard-wifi-select")}
            }
            match rows.cloned() {
                Some(Ok(rows)) => {
                    let expanded = show_add().unwrap_or(rows.is_empty());
                    if expanded {
                        rsx! {
                            div {
                                class: "grid gap-2 sm:grid-cols-2",
                                role: "group",
                                aria_labelledby: "edge-refs-wifi-label",
                                input {
                                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                    placeholder: "{ssid_placeholder}",
                                    value: "{new_ssid}",
                                    oninput: move |e| new_ssid.set(e.value()),
                                }
                                input {
                                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                    r#type: "password",
                                    placeholder: "{password_placeholder}",
                                    value: "{new_password}",
                                    oninput: move |e| new_password.set(e.value()),
                                }
                                div { class: "sm:col-span-2 flex items-center gap-2",
                                    button {
                                        class: "px-3 py-1.5 text-xs font-medium text-white bg-blue-600 hover:bg-blue-700 rounded-lg transition-colors",
                                        r#type: "button",
                                        onclick: save_wifi,
                                        {t!("wizard-ref-add")}
                                    }
                                    button {
                                        class: "px-3 py-1.5 text-xs font-medium text-gray-500 hover:text-gray-700 transition-colors",
                                        r#type: "button",
                                        onclick: move |_| show_add.set(Some(false)),
                                        {t!("wizard-ref-cancel")}
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! {
                            div { class: "flex items-center gap-2",
                                select {
                                    id: "edge-refs-wifi-select",
                                    class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                    value: "{selected_id}",
                                    onchange: {
                                        // Clone possédé par la closure (le
                                        // scrutateur du match ne vit pas dans
                                        // les captures du rsx).
                                        let rows = rows.clone();
                                        move |e| {
                                            let id: i64 = e.value().parse().unwrap_or(0);
                                            selected.set(rows.iter().find(|r| r.id == id).cloned());
                                        }
                                    },
                                    if selected().is_none() {
                                        option { value: "", {t!("wizard-ref-pick")} }
                                    }
                                    for row in &rows {
                                        option { value: "{row.id}", {row.ssid.clone()} }
                                    }
                                }
                                if selected().is_some() {
                                    button {
                                        class: "p-2 text-gray-400 hover:text-red-600 transition-colors",
                                        title: "{delete_title}",
                                        r#type: "button",
                                        onclick: move |_| {
                                            let Some(cur) = selected() else { return };
                                            selected.set(None);
                                            spawn(async move {
                                                if let Err(e) = api::wifi::delete(cur.id).await {
                                                    toasts::error(e);
                                                }
                                                generation += 1;
                                            });
                                        },
                                        icons::Trash2 { class: "h-4 w-4" }
                                    }
                                }
                                button {
                                    class: "px-2 py-1.5 text-xs font-medium text-blue-600 hover:text-blue-800 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                                    r#type: "button",
                                    onclick: move |_| show_add.set(Some(!expanded)),
                                    {if expanded { "✕" } else { "+" }}
                                }
                            }
                        }
                    }
                }
                Some(Err(err)) => rsx! {
                    div { class: "flex items-center gap-2 text-sm text-red-600",
                        icons::AlertTriangle { class: "h-4 w-4" }
                        span { "{err.message}" }
                        button {
                            class: "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50",
                            r#type: "button",
                            onclick: move |_| generation += 1,
                            {t!("common-retry")}
                        }
                    }
                },
                None => rsx! {
                    p { class: "text-xs text-gray-400", {t!("common-loading")} }
                },
            }
        }
    }
}

/// Server host picker. When the deployment imposes the host
/// (`PNEX_PROD_HOST`), it is shown read-only and preselected; otherwise the
/// org's referential is offered (pick, add, LAN scan).
#[component]
pub fn PnexHostPicker(selected: Signal<Option<pnex_core::PnexHost>>) -> Element {
    let locked = use_resource(|| async { api::hosts::locked().await });
    match locked.cloned() {
        None => rsx! {
            div { class: "sm:col-span-2 text-xs text-gray-400", {t!("wizard-host-loading")} }
        },
        // No imposed host, or an older server without the endpoint.
        Some(Ok(Some(host))) => rsx! {
            LockedHostView { host, selected }
        },
        Some(_) => rsx! {
            FreeHostPicker { selected }
        },
    }
}

/// Read-only display of the imposed host; keeps `selected` on it so the
/// callers' guards and build payloads need no special case.
#[component]
fn LockedHostView(host: String, mut selected: Signal<Option<pnex_core::PnexHost>>) -> Element {
    let imposed = host.clone();
    use_effect(move || {
        let current = selected.peek().as_ref().map(|h| h.host.clone());
        if current.as_deref() != Some(imposed.as_str()) {
            selected.set(Some(pnex_core::PnexHost {
                id: 0,
                org_id: 0,
                host: imposed.clone(),
                created_at: String::new(),
                updated_at: String::new(),
            }));
        }
    });
    rsx! {
        div { class: "sm:col-span-2 space-y-1",
            p { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("wizard-host-select")} }
            div { class: "flex items-center gap-2 rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 text-sm text-gray-700",
                icons::Server { class: "h-4 w-4 text-gray-400" }
                span { class: "font-mono", "wss://{host}" }
            }
            p { class: "text-xs text-gray-500", {t!("wizard-host-locked-hint")} }
        }
    }
}

#[component]
fn FreeHostPicker(mut selected: Signal<Option<pnex_core::PnexHost>>) -> Element {
    let mut generation = use_signal(|| 0u32);
    let mut new_host = use_signal(crate::util::default_device_host);
    // None = pas encore chargé ; le mini-form s'ouvre tout seul si vide.
    let mut show_add = use_signal(|| None::<bool>);

    let rows = use_resource(move || {
        let _ = generation();
        async move { api::hosts::list().await }
    });

    // Preselect the first entry (zero retyping) — same phantom-selection
    // rationale as in WifiCredentialPicker: the browser displays the first
    // non-empty-value option regardless of the signal, so the signal must
    // carry that entry for Continue's guard to match what is on screen.
    use_effect(move || {
        if let Some(Ok(rows)) = &*rows.read() {
            if selected.peek().is_none() {
                if let Some(first) = rows.first() {
                    selected.set(Some(first.clone()));
                }
            }
        }
    });

    // Labels i18n résolus hors rsx + id courant du select.
    let host_placeholder = t!("builds-field-server");
    let delete_title = t!("wizard-ref-delete");
    let selected_id = selected().map(|h| h.id.to_string()).unwrap_or_default();

    // Sauvegarde inline (upsert sur (org, host)) puis sélection — partagé
    // entre le bouton « Ajouter » du mini-form et le pick du scan LAN.
    let register_host = move |host: String| {
        spawn(async move {
            // Always wss (D70) — the server enforces it anyway.
            let input = pnex_core::PnexHostInput { host };
            match api::hosts::create(input).await {
                Ok(created) => {
                    selected.set(Some(created));
                    show_add.set(Some(false));
                    new_host.set(String::new());
                    generation += 1;
                }
                Err(err) => toasts::error(err),
            }
        });
    };
    let save_host = move |_| {
        let host = new_host().trim().to_string();
        if host.is_empty() {
            toasts::error("wizard-config-incomplete");
            return;
        }
        register_host(host);
    };

    rsx! {
        div { class: "sm:col-span-2 space-y-2",
            label {
                id: "edge-refs-host-label",
                r#for: "edge-refs-host-select",
                class: "text-xs font-medium text-gray-500 mb-1 block",
                {t!("wizard-host-select")}
            }
            match rows.cloned() {
                Some(Ok(rows)) => {
                    let expanded = show_add().unwrap_or(rows.is_empty());
                    if expanded {
                        rsx! {
                            div {
                                class: "grid gap-2 sm:grid-cols-2",
                                role: "group",
                                aria_labelledby: "edge-refs-host-label",
                                label { class: "sm:col-span-2 block",
                                    input {
                                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                        placeholder: "{host_placeholder}",
                                        value: "{new_host}",
                                        oninput: move |e| new_host.set(e.value()),
                                    }
                                    if crate::util::is_loopback_host(&new_host()) {
                                        p { class: "mt-1 text-xs text-amber-600", {t!("devices-host-loopback-hint")} }
                                    }
                                }
                                div { class: "sm:col-span-2 flex items-center gap-2",
                                    button {
                                        class: "px-3 py-1.5 text-xs font-medium text-white bg-blue-600 hover:bg-blue-700 rounded-lg transition-colors",
                                        r#type: "button",
                                        onclick: save_host,
                                        {t!("wizard-ref-add")}
                                    }
                                    button {
                                        class: "px-3 py-1.5 text-xs font-medium text-gray-500 hover:text-gray-700 transition-colors",
                                        r#type: "button",
                                        onclick: move |_| show_add.set(Some(false)),
                                        {t!("wizard-ref-cancel")}
                                    }
                                }
                                // Détection LAN côté serveur : un hit
                                // s'enregistre direct dans le référentiel.
                                div { class: "sm:col-span-2",
                                    LanDetect {
                                        on_pick: move |host| {
                                            register_host(host);
                                        },
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! {
                            div { class: "flex items-center gap-2",
                                select {
                                    id: "edge-refs-host-select",
                                    class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                    value: "{selected_id}",
                                    onchange: {
                                        // Clone possédé par la closure (le
                                        // scrutateur du match ne vit pas dans
                                        // les captures du rsx).
                                        let rows = rows.clone();
                                        move |e| {
                                            let id: i64 = e.value().parse().unwrap_or(0);
                                            selected.set(rows.iter().find(|r| r.id == id).cloned());
                                        }
                                    },
                                    if selected().is_none() {
                                        option { value: "", {t!("wizard-ref-pick")} }
                                    }
                                    for row in &rows {
                                        option { value: "{row.id}", {host_display(&row.host)} }
                                    }
                                }
                                if selected().is_some() {
                                    button {
                                        class: "p-2 text-gray-400 hover:text-red-600 transition-colors",
                                        title: "{delete_title}",
                                        r#type: "button",
                                        onclick: move |_| {
                                            let Some(cur) = selected() else { return };
                                            selected.set(None);
                                            spawn(async move {
                                                if let Err(e) = api::hosts::delete(cur.id).await {
                                                    toasts::error(e);
                                                }
                                                generation += 1;
                                            });
                                        },
                                        icons::Trash2 { class: "h-4 w-4" }
                                    }
                                }
                                button {
                                    class: "px-2 py-1.5 text-xs font-medium text-blue-600 hover:text-blue-800 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                                    r#type: "button",
                                    onclick: move |_| show_add.set(Some(!expanded)),
                                    {if expanded { "✕" } else { "+" }}
                                }
                            }
                        }
                    }
                }
                Some(Err(err)) => rsx! {
                    div { class: "flex items-center gap-2 text-sm text-red-600",
                        icons::AlertTriangle { class: "h-4 w-4" }
                        span { "{err.message}" }
                        button {
                            class: "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50",
                            r#type: "button",
                            onclick: move |_| generation += 1,
                            {t!("common-retry")}
                        }
                    }
                },
                None => rsx! {
                    p { class: "text-xs text-gray-400", {t!("common-loading")} }
                },
            }
        }
    }
}
