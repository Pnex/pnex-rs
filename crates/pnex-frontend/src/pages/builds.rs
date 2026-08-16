//! Builds firmware — Phase 6 : formulaire de build (device + WiFi + hôte),
//! liste des records (badges de phase), téléchargement du binaire,
//! suppression. Suivi de l'avancement par **polling** (~5 s tant qu'un
//! build est queued/running — le WS de notification est différé).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::pager::Pager;
use crate::state::{org, session, toasts};
use crate::util::{default_host, save_blob, sleep};

const PAGE_SIZE: i64 = 10;

/// Rôle de l'utilisateur dans l'org courante (« owner »/« admin »/« viewer »)
/// — même aide locale que devices.rs.
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

#[component]
pub fn Builds() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut page = use_signal(|| 0i64);
    // Polling : un seul minuteur à la fois, relancé tant qu'un build vole.
    let mut polling = use_signal(|| false);

    let mut form_device = use_signal(String::new);
    let mut form_ssid = use_signal(String::new);
    let mut form_wifi_password = use_signal(String::new);
    let mut form_host = use_signal(default_host);

    let can_write = current_role()
        .is_some_and(|role| matches!(role.as_str(), "owner" | "admin"));

    let list = use_resource(move || {
        let filters = api::builds::BuildFilters {
            device_id: None,
            success: None,
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::builds::list(&filters).await
        }
    });
    // Devices de l'org pour le <select> du formulaire (page max).
    let devices = use_resource(move || {
        let filters = api::devices::DeviceFilters {
            limit: Some(100),
            ..Default::default()
        };
        async move { api::devices::list(&filters).await }
    });

    // Polling ~5 s tant qu'un record de la page est queued/running.
    if let Some(Ok(paged)) = &*list.read() {
        let in_flight = paged
            .results
            .iter()
            .any(|r| matches!(r.build_phase.as_deref(), Some("queued") | Some("running")));
        if in_flight && !polling() {
            polling.set(true);
            spawn(async move {
                sleep(Duration::from_secs(5)).await;
                polling.set(false);
                reload.with_mut(|r| *r += 1);
            });
        }
    }

    let submit = move |_| {
        let device_id = form_device().trim().to_string();
        let ssid = form_ssid().trim().to_string();
        let host = form_host().trim().to_string();
        if device_id.is_empty() || ssid.is_empty() || host.is_empty() {
            toasts::error("builds-form-incomplete");
            return;
        }
        // Modèle du device sélectionné (le contrôleur vérifie la cohérence).
        let Some(Ok(paged)) = &*devices.read() else {
            toasts::error("builds-form-incomplete");
            return;
        };
        let Some(device) = paged.results.iter().find(|d| d.device_id == device_id) else {
            toasts::error("builds-form-incomplete");
            return;
        };
        let params = pnex_core::CreateBuild {
            wifi_ssid: ssid,
            wifi_password: form_wifi_password(),
            predefined_device_name: device.predefined_device_name.clone(),
            pnex_host: host,
            device_id: device_id.clone(),
        };
        spawn(async move {
            match api::builds::create(params).await {
                Ok(_) => {
                    toasts::success("builds-launched");
                    page.set(0);
                    reload.with_mut(|r| *r += 1);
                }
                Err(err) => toasts::error(err.message),
            }
        });
    };

    rsx! {
        div { class: "p-6",
            div { class: "mb-8",
                h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-builds")} }
                p { class: "text-gray-600 mt-2", {t!("builds-subtitle")} }
            }

            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                // ── Formulaire de build ──
                if can_write {
                    div { class: "mb-8 p-4 bg-white border border-gray-200 rounded-xl",
                        h2 { class: "text-lg font-semibold text-gray-900 mb-4", {t!("builds-form-title")} }
                        div { class: "grid gap-3 sm:grid-cols-2 lg:grid-cols-4",
                            label { class: "block",
                                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("builds-field-device")} }
                                select {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                    onchange: move |event| form_device.set(event.value()),
                                    option { value: "", selected: form_device().is_empty(), {t!("builds-field-device-placeholder")} }
                                    {match &*devices.read() {
                                        Some(Ok(paged)) => rsx! {
                                            for device in &paged.results {
                                                option {
                                                    value: "{device.device_id}",
                                                    selected: form_device() == device.device_id,
                                                    {device.device_id.clone()}
                                                }
                                            }
                                        },
                                        _ => rsx! {},
                                    }}
                                }
                            }
                            label { class: "block",
                                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("builds-field-ssid")} }
                                input {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                    r#type: "text",
                                    value: "{form_ssid}",
                                    oninput: move |event| form_ssid.set(event.value()),
                                }
                            }
                            label { class: "block",
                                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("builds-field-wifi-password")} }
                                input {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                    r#type: "password",
                                    value: "{form_wifi_password}",
                                    oninput: move |event| form_wifi_password.set(event.value()),
                                }
                            }
                            label { class: "block",
                                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("builds-field-server")} }
                                input {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                    r#type: "text",
                                    placeholder: "dev1.pnex.io",
                                    value: "{form_host}",
                                    oninput: move |event| form_host.set(event.value()),
                                }
                            }
                        }
                        button {
                            class: "mt-4 px-4 py-2 bg-indigo-600 text-white text-sm font-medium rounded-lg hover:bg-indigo-700 transition-colors disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: !can_write || form_device().is_empty(),
                            onclick: submit,
                            {t!("builds-submit")}
                        }
                    }
                }

                // ── Liste des records ──
                {match &*list.read() {
                    Some(Ok(paged)) if paged.results.is_empty() && paged.count == 0 => rsx! {
                        p { class: "text-gray-500 text-center py-12", {t!("builds-empty")} }
                    },
                    Some(Ok(paged)) => rsx! {
                        div { class: "bg-white border border-gray-200 rounded-xl overflow-hidden",
                            table { class: "w-full text-sm",
                                thead {
                                    tr { class: "bg-gray-50 text-left text-xs uppercase tracking-wide text-gray-500",
                                        th { class: "px-4 py-3", {t!("builds-col-device")} }
                                        th { class: "px-4 py-3", {t!("builds-col-phase")} }
                                        th { class: "px-4 py-3", {t!("builds-col-date")} }
                                        th { class: "px-4 py-3 text-right", {t!("builds-col-actions")} }
                                    }
                                }
                                tbody {
                                    for record in &paged.results {
                                        {build_row(record, can_write)}
                                    }
                                }
                            }
                        }
                        Pager {
                            count: paged.count,
                            page_size: PAGE_SIZE,
                            page,
                            on_navigate: move |target| {
                                page.set(target);
                                reload.with_mut(|r| *r += 1);
                            }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        p { class: "text-red-600 text-center py-12", {err.message.clone()} }
                    },
                    _ => rsx! {
                        p { class: "text-gray-400 text-center py-12", {t!("common-loading")} }
                    },
                }}
            }
        }
    }
}

/// Ligne d'un record : device, badge de phase, date locale, actions
/// (télécharger si réussi, supprimer sinon).
fn build_row(record: &pnex_core::BuildRecord, can_write: bool) -> Element {
    let device_id = record.device_id.clone().unwrap_or_default();
    let (phase_badge_class, phase_label) = phase_badge(record.build_phase.as_deref());
    // Captures par valeur pour les closures 'static des spawn.
    let download_id = device_id.clone();
    let delete_id = record.id;

    let download = move |_| {
        let device_id = download_id.clone();
        spawn(async move {
            match api::builds::download(&device_id).await {
                Ok(bytes) => {
                    save_blob(&format!("{device_id}-firmware.bin"), &bytes);
                }
                Err(err) => toasts::error(err.message),
            }
        });
    };
    let delete = move |_| {
        spawn(async move {
            match api::builds::delete(delete_id).await {
                Ok(()) => toasts::success("builds-deleted"),
                Err(err) => toasts::error(err.message),
            }
        });
    };

    rsx! {
        tr { class: "border-t border-gray-100 hover:bg-gray-50",
            td { class: "px-4 py-3 font-medium text-gray-900", {device_id.clone()} }
            td { class: "px-4 py-3", span { class: phase_badge_class, {phase_label} } }
            td { class: "px-4 py-3 text-gray-500", {date_label(&record.updated_at)} }
            td { class: "px-4 py-3 text-right",
                if record.success {
                    button {
                        class: "px-3 py-1.5 bg-indigo-600 text-white text-xs font-medium rounded-lg hover:bg-indigo-700 transition-colors",
                        onclick: download,
                        {t!("builds-download")}
                    }
                } else if can_write {
                    button {
                        class: "px-3 py-1.5 border border-red-200 text-red-600 text-xs font-medium rounded-lg hover:bg-red-50 transition-colors",
                        onclick: delete,
                        {t!("builds-delete")}
                    }
                }
            }
        }
    }
}

/// Badge (classes Tailwind littérales) + libellé i18n par phase.
fn phase_badge(phase: Option<&str>) -> (String, String) {
    let (class, key) = match phase {
        Some("queued") => ("bg-gray-100 text-gray-600", "builds-phase-queued"),
        Some("running") => ("bg-blue-100 text-blue-700 animate-pulse", "builds-phase-running"),
        Some("succeeded") => ("bg-green-100 text-green-700", "builds-phase-succeeded"),
        Some("failed") => ("bg-red-100 text-red-700", "builds-phase-failed"),
        _ => ("bg-gray-100 text-gray-400", "builds-phase-queued"),
    };
    (
        format!("inline-block px-2 py-0.5 rounded-full text-xs font-medium {class}"),
        t!(key),
    )
}

/// Date locale compacte (dernier changement de phase).
fn date_label(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|ts| {
            let local = ts.with_timezone(&chrono::Local);
            local.format("%d/%m %H:%M:%S").to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}
