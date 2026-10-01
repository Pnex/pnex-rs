//! Gestion des appareils — portée du `Devices.tsx` React sur l'API Phase 4 :
//! liste filtrable (type, statut, device_id), enregistrement via l'assistant
//! modal (`components/device_wizard.rs` : build auto suivi en modale, snippet
//! Python pour les customs), détail (token de provisioning, métadonnées
//! JSON), suppression. Le scoping org vient du client (`X-Org-Id`), l'écriture
//! est réservée owner/admin (le serveur force, l'UI masque).
//!
//! Le détail est piloté par un signal local `selected` + `key` (cf. orgs.rs).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::Device;

use crate::api;
use crate::components::badges::{date_label, phase_badge};
use crate::components::board_pinout_editor as board_pinout;
use crate::components::crud::filters::{FilterBar, RefreshButton, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::edge_refs_picker::{PnexHostPicker, WifiCredentialPicker};
use crate::components::flash_modal::FlashModal;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::state::devices::OPEN_DEVICE;
use crate::state::{org, session, toasts};
use crate::util::{save_blob, sleep};

/// Rôle de l'utilisateur dans l'org courante (« owner »/« admin »/« viewer »).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

/// Cible du modal de déploiement OTA — tout ce que le modal doit afficher
/// et décider : version cible vs courante, fraîcheur du build vis-à-vis
/// des sources embarquées, raccourci rebuild.
#[derive(Clone, PartialEq)]
struct OtaTarget {
    device_pk: i64,
    device_id: String,
    /// Dernier build réussi : (version, date) — None = aucun build.
    latest_build: Option<(String, String)>,
    /// Version courante annoncée par le device (None = jamais annoncé).
    device_version: Option<String>,
    /// Le build compile une arborescence firmware antérieure au serveur
    /// courant (sources_stale) → avertissement + raccourci rebuild.
    stale: bool,
    can_rebuild: bool,
    /// Modèle du device (raccourci rebuild — types custom exclus, O3).
    model: String,
}

/// Devices and edge agents (D95: an agent is a device) share one page;
/// the `Agent` type filter narrows the list to agents.
#[component]
pub fn Devices() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| None::<i64>);

    // Global search deep link (D69): consume the requested device id once
    // (guard prevents the effect from re-arming — see flows.rs comment).
    let mut deep_link_done = use_signal(|| false);
    use_effect(move || {
        if deep_link_done() {
            return;
        }
        if let Some(id) = OPEN_DEVICE() {
            selected.set(Some(id));
            OPEN_DEVICE.with_mut(|f| *f = None);
            deep_link_done.set(true);
        }
    });
    let mut filter_type = use_signal(|| "all".to_string());
    let mut filter_status = use_signal(|| "all".to_string());
    let mut filter_capability = use_signal(String::new);
    let search = use_signal(String::new);
    // Page courante (0-based) — remise à 0 à chaque changement de filtre.
    let mut page = use_signal(|| 0i64);
    // Assistant d'enregistrement (mont/démont = état propre à chaque ouverture).
    let mut wizard_open = use_signal(|| false);
    // Cible du modal de recompilation (device_id, modèle).
    let mut rebuild_target = use_signal(|| None::<(i64, String, String)>);
    // Cible du modal de flash navigateur (device_id).
    let mut flash_target = use_signal(|| None::<(i64, String, bool)>);
    // Cible du modal de déploiement OTA (struct OtaTarget — infra).
    let mut ota_target = use_signal(|| None::<OtaTarget>);
    // Polling : un seul minuteur à la fois, relancé tant qu'un build vole.
    let mut polling = use_signal(|| false);

    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    let list = use_resource(move || {
        let filters = api::devices::DeviceFilters {
            device_type: match filter_type().as_str() {
                "all" => None,
                other => Some(other.to_string()),
            },
            capability: {
                let value = filter_capability().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            },
            device_id: None,
            search: {
                let value = search().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
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

    // Polling ~5 s tant qu'un build d'un device de la page est queued/running
    // (colonne Firmware — le WS de notification reste différé) — ou qu'un
    // déploiement OTA est actif sur un device de la page.
    if let Some(Ok(paged)) = &*list.read() {
        let in_flight = paged.results.iter().any(|device| {
            device.ota.is_some()
                || device.latest_build.as_ref().is_some_and(|build| {
                    matches!(
                        build.build_phase.as_deref(),
                        Some("queued") | Some("running")
                    )
                })
        });
        if in_flight && !polling() {
            polling.set(true);
            spawn(async move {
                sleep(Duration::from_secs(5)).await;
                polling.set(false);
                reload.with_mut(|r| *r += 1);
            });
        }
    }

    // Lecture synchrone de la ressource (doctrine socle CRUD).
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
    // (Copy) de la page ; les helpers de ligne (badges, téléchargement,
    // flash) reprennent le rendu exact de l'ancienne device_row.
    let columns = vec![
        Column::new(t!("devices-col-id").to_string(), |device: &Device| {
            rsx! {
                code { class: "text-sm", {device.device_id.clone()} }
            }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("devices-col-type").to_string(), |device: &Device| {
            let (type_badge_class, type_label) = type_badge(&device.device_type);
            rsx! {
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {type_badge_class}",
                    {type_label}
                }
            }
        }),
        Column::new(t!("devices-col-model").to_string(), |device: &Device| {
            rsx! { {device.predefined_device_name.clone()} }
        })
        .with_td_class("text-gray-600"),
        Column::new(t!("devices-col-status").to_string(), |device: &Device| {
            rsx! {
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
        }),
        Column::new(
            t!("devices-col-firmware").to_string(),
            move |device: &Device| {
                // Edge agent (D95): no firmware — show the agent version.
                if device.predefined_device_name == pnex_core::EDGE_AGENT_PREDEF {
                    let version = device.fw_version.clone().unwrap_or_else(|| "—".into());
                    return rsx! {
                        span { class: "font-mono text-[11px] text-gray-500", "pnex-agent {version}" }
                    };
                }
                // Badge + date du dernier build, téléchargement si succès.
                let firmware = device.latest_build.as_ref().map(|build| {
                    let (class, label) = phase_badge(build.build_phase.as_deref());
                    (class, label, date_label(&build.updated_at), build.success)
                });
                let downloadable = firmware.as_ref().is_some_and(|f| f.3);
                let download_id = device.device_id.clone();
                let flash_pk = device.id;
                let flash_generic = device.predefined_device_name == "generic_esp8266";
                let flash_id = device.device_id.clone();
                // OTA: le bouton s'affiche dès que le device a attesté la cap
                // `ota` et qu'aucun déploiement n'est actif — même SANS build
                // réussi (le modal gère l'état « aucun firmware » : deploy
                // grisé + raccourci rebuild).
                let ota_ready = device.ota_ready && device.ota.is_none();
                let ota_connected = device.connected;
                let ota_latest = device.latest_build.as_ref().filter(|b| b.success).map(|b| {
                    (
                        b.fw_version.clone().unwrap_or_else(|| "?".to_string()),
                        date_label(&b.updated_at),
                    )
                });
                let ota_stale = device
                    .latest_build
                    .as_ref()
                    .and_then(|b| b.sources_stale)
                    .unwrap_or(false);
                let ota_can_rebuild = can_write && !device.allow_dynamic_measurements;
                let ota_model = device.predefined_device_name.clone();
                let ota_pk = device.id;
                let ota_device = device.device_id.clone();
                let ota_state = device.ota.clone();
                let ota_current = device.fw_version.clone();
                rsx! {
                    div { class: "flex flex-col gap-0.5",
                        match &firmware {
                            Some((class, label, date, _)) => rsx! {
                                span { class: "{class} w-fit", {label.clone()} }
                                span { class: "text-[11px] text-gray-400", {date.clone()} }
                            },
                            None => rsx! {
                                span { class: "text-xs text-gray-400", {t!("devices-build-never")} }
                            },
                        }
                        // Version courante du firmware (announce) — build #N.
                        if let Some(v) = &device.fw_version {
                            span { class: "text-[11px] text-gray-500", "build #{v}" }
                        }
                        // Déploiement OTA actif → progression live.
                        if let Some(st) = &ota_state {
                            span {
                                class: "text-[11px] font-medium text-indigo-600",
                                {ota_state_label(st)}
                            }
                        }
                        if downloadable {
                            div { class: "flex items-center gap-3",
                                button {
                                    class: "w-fit px-2 py-0.5 text-xs text-indigo-600 hover:text-indigo-700 transition-colors",
                                    r#type: "button",
                                    onclick: move |_| {
                                        let id = download_id.clone();
                                        spawn(async move {
                                            match api::builds::download(&id).await {
                                                Ok(bytes) => save_blob(&format!("{id}-firmware.bin"), &bytes),
                                                Err(err) => toasts::error(err),
                                            }
                                        });
                                    },
                                    icons::Download { class: "h-3.5 w-3.5 inline mr-0.5" }
                                    {t!("builds-download")}
                                }
                                // Flash navigateur (Web Serial — Chromium
                                // uniquement, le modal affiche l'avertissement
                                // sinon).
                                button {
                                    class: "w-fit px-2 py-0.5 text-xs text-emerald-600 hover:text-emerald-700 transition-colors",
                                    r#type: "button",
                                    title: t!("devices-flash-title"),
                                    onclick: move |_| {
                                        flash_target.set(Some((flash_pk, flash_id.clone(), flash_generic)));
                                    },
                                    icons::Zap { class: "h-3.5 w-3.5 inline mr-0.5" }
                                    {t!("devices-flash")}
                                }
                            }
                        }
                        if ota_ready {
                            button {
                                class: "w-fit px-2 py-0.5 text-xs text-violet-600 hover:text-violet-700 transition-colors disabled:opacity-40 disabled:cursor-not-allowed",
                                r#type: "button",
                                // Device hors ligne → deploy grisé (le
                                // serveur mettrait en queue, mais on veut
                                // le geste explicite).
                                disabled: !ota_connected,
                                title: if ota_connected {
                                    t!("devices-ota-title")
                                } else {
                                    t!("devices-ota-offline-title")
                                },
                                onclick: move |_| {
                                    ota_target.set(Some(OtaTarget {
                                        device_pk: ota_pk,
                                        device_id: ota_device.clone(),
                                        latest_build: ota_latest.clone(),
                                        device_version: ota_current.clone(),
                                        stale: ota_stale,
                                        can_rebuild: ota_can_rebuild,
                                        model: ota_model.clone(),
                                    }));
                                },
                                icons::Upload { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("devices-ota-deploy")}
                            }
                        }
                    }
                }
            },
        ),
        Column::new(t!("common-actions").to_string(), move |device: &Device| {
            let pk = device.id;
            // O3 : jamais de build proposé pour les types custom (le back
            // échoue de toute façon — le workspace firmware ne les
            // contient pas).
            let can_rebuild = can_write && !device.allow_dynamic_measurements;
            let rebuild_pk = device.id;
            let rebuild_id = device.device_id.clone();
            let rebuild_model = device.predefined_device_name.clone();
            rsx! {
                div { class: "flex items-center gap-1.5",
                    if can_rebuild {
                        button {
                            class: "px-3 py-1 text-sm bg-amber-100 text-amber-700 rounded-lg hover:bg-amber-200 transition-colors",
                            title: t!("devices-rebuild-title"),
                            onclick: move |_| {
                                rebuild_target.set(Some((rebuild_pk, rebuild_id.clone(), rebuild_model.clone())));
                            },
                            icons::Wrench { class: "h-3.5 w-3.5 inline mr-0.5" }
                            {t!("devices-rebuild")}
                        }
                    }
                    button {
                        class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors",
                        onclick: move |_| selected.set(Some(pk)),
                        {t!("devices-detail")}
                    }
                }
            }
        }),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-devices").to_string(),
            subtitle: Some(t!("devices-subtitle").to_string()),
            can_write: can_write,
            // Détail device ouvert → pas de « + Register » (on n'enregistre
            // pas un device depuis la fiche d'un autre).
            add_label: if selected().is_none() {
                Some(t!("devices-register").to_string())
            } else {
                None
            },
            on_add: move |_| wizard_open.set(true),
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
                        // Filtres (type, statut, capacité, recherche, refresh).
                        FilterBar {
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
                                option { value: "agent", selected: filter_type() == "agent", {t!("agent-badge")} }
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
                            SearchInput {
                                placeholder: t!("devices-search-placeholder").to_string(),
                                value: search,
                                on_submit: move |_| {
                                    page.set(0);
                                    reload.with_mut(|r| *r += 1);
                                },
                            }
                            RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
                        }

                        ListStates {
                            state: list_state,
                            is_empty: is_empty,
                            empty_message: t!("devices-empty").to_string(),
                            DataTable {
                                columns: columns,
                                rows: rows,
                                row_key: RowKey::new(|device: &Device| device.id.to_string()),
                            }
                            ListPager { count: count, page: page }
                        }

                        // Assistant d'enregistrement (monté à la demande :
                        // l'état interne se réinitialise à chaque ouverture).
                        if wizard_open() {
                            crate::components::device_wizard::DeviceWizard {
                                on_close: move |_| wizard_open.set(false),
                                on_changed: move |_| reload.with_mut(|r| *r += 1),
                            }
                        }

                        // Recompilation d'un device de la liste.
                        if let Some((rebuild_pk, rebuild_id, rebuild_model)) = rebuild_target() {
                            RebuildModal {
                                key: "{rebuild_id}",
                                device_pk: rebuild_pk,
                                device_id: rebuild_id,
                                model: rebuild_model,
                                on_close: move |_| rebuild_target.set(None),
                                on_launched: move |_| {
                                    rebuild_target.set(None);
                                    reload.with_mut(|r| *r += 1);
                                },
                            }
                        }

                        // Flash navigateur d'un device de la liste (Web Serial,
                        // Chromium — l'état interne se réinitialise à chaque
                        // ouverture).
                        // Flash navigateur d'un device de la liste (Web Serial,
                        // Chromium — l'état interne se réinitialise à chaque
                        // ouverture).
                        if let Some((_, flash_id, _)) = flash_target() {
                            FlashModal {
                                key: "{flash_id}",
                                device_id: flash_id,
                                on_close: move |_| flash_target.set(None),
                            }
                        }

                        // Déploiement OTA d'un device de la liste.
                        if let Some(t) = ota_target() {
                            OtaModal {
                                key: "{t.device_id}",
                                target: t,
                                on_close: move |_| ota_target.set(None),
                                // Raccourci rebuild : ferme le modal OTA et
                                // ouvre le RebuildModal sur le même device.
                                on_rebuild: move |_| {
                                    if let Some(t) = ota_target() {
                                        ota_target.set(None);
                                        rebuild_target
                                            .set(Some((t.device_pk, t.device_id, t.model)));
                                    }
                                },
                                on_launched: move |_| {
                                    ota_target.set(None);
                                    reload.with_mut(|r| *r += 1);
                                },
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Modal de recompilation : la connectivité vient des référentiels Edge
/// (wifi_credentials / pnex_hosts) via les mêmes pickers que le wizard —
/// plus de ressaisie des secrets à chaque build, l'entrée est résolue au
/// submit vers le payload `CreateBuild` (contrat inchangé).
///
/// Écran de debug : le choix (aucun / OLED / TFT) se fait ICI, au moment
/// du flash — câblage fixé par la board (légende affichée), appliqué au
/// device (`PUT /peripherals`) juste avant le build si le choix change.
/// L'écran sert au debug de mise en service ; le retirer plus tard
/// (conso / switch alim) = re-passer par « Aucun écran » ici.
#[component]
fn RebuildModal(
    device_pk: i64,
    device_id: String,
    model: String,
    on_close: Callback<()>,
    on_launched: Callback<()>,
) -> Element {
    // Entrées référentiel — mêmes pickers que le wizard device (présélection
    // de la 1re entrée, ajout inline, suppression).
    let wifi_sel = use_signal(|| None::<pnex_core::WifiCredential>);
    let host_sel = use_signal(|| None::<pnex_core::PnexHost>);
    let mut launching = use_signal(|| false);

    // État écran du device (pinout = lecture DB, marche hors ligne) :
    // `screen_pick` outer None = pas encore touché → état courant repris.
    let pinout_res = use_resource(move || async move { api::pins::pinout(device_pk).await });
    let mut screen_pick = use_signal(|| None::<Option<String>>);

    // Données board du device pour le picker + la légende de câblage.
    let board = pinout_res
        .value()
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .and_then(|p| p.board.clone());
    let pins = pinout_res
        .value()
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|p| p.pins.clone())
        .unwrap_or_default();
    let has_screens = board.as_ref().is_some_and(|b| {
        b.peripherals
            .get("screens")
            .and_then(|s| s.as_array())
            .is_some_and(|a| !a.is_empty())
    });

    // Captures par valeur pour la closure 'static du spawn (les props
    // restent utilisées par le rendu).
    let submit_device = device_id.clone();
    let submit_model = model.clone();
    let submit = move |_| {
        // Résolution référentiel avant le spawn — entrée manquante = garde
        // (théorique : les pickers présélectionnent).
        let Some(creds) = wifi_sel() else {
            toasts::error("devices-rebuild-incomplete");
            return;
        };
        let Some(srv) = host_sel() else {
            toasts::error("devices-rebuild-incomplete");
            return;
        };
        launching.set(true);
        let params = pnex_core::CreateBuild {
            device_id: submit_device.clone(),
            predefined_device_name: submit_model.clone(),
            wifi_ssid: creds.ssid,
            // The build references the entry's vault secret (secrets.md S6).
            wifi_credential_id: Some(creds.id),
            wifi_password: String::new(),
            pnex_host: srv.host,
            // Always wss (D70).
            ws_ssl: true,
        };
        spawn(async move {
            // 1. Écran — appliqué AVANT le build si le choix a changé (les
            // pins/réserves + le define en dépendent). Échec = pas de build.
            if let Some(b) = pinout_res
                .value()
                .read()
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .and_then(|p| p.board.clone())
            {
                let current = board_pinout::screen_state_kind(&b);
                let effective = screen_pick().unwrap_or_else(|| current.clone());
                if effective != current {
                    if let Err(msg) = board_pinout::set_screen(device_pk, effective).await {
                        launching.set(false);
                        toasts::error(msg);
                        return;
                    }
                }
            }
            // 2. Build.
            match api::builds::create(params).await {
                Ok(_) => {
                    toasts::success("builds-launched");
                    on_close.call(());
                    on_launched.call(());
                }
                Err(err) => {
                    launching.set(false);
                    toasts::error(err);
                }
            }
        });
    };

    // Options du picker, actives selon le choix courant du modal (outer
    // None = état courant du device repris tel quel).
    let mut options = board
        .as_ref()
        .map(|b| board_pinout::screen_options(b))
        .unwrap_or_default();
    let current_kind = board.as_ref().and_then(board_pinout::screen_state_kind);
    let effective = screen_pick().unwrap_or_else(|| current_kind.clone());
    for o in &mut options {
        o.active = o.kind.as_ref() == effective.as_ref();
    }

    rsx! {
        Modal {
            title: t!("devices-rebuild-title"),
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-4",
                p { class: "text-sm text-gray-600",
                    code { class: "font-medium", {device_id.clone()} }
                    " · "
                    {model.clone()}
                }
                // Mêmes pickers que le wizard — leur racine porte
                // `sm:col-span-2`, inoffensif hors grille.
                WifiCredentialPicker { selected: wifi_sel }
                PnexHostPicker { selected: host_sel }
                if has_screens {
                    div { class: "space-y-1",
                        span { class: "text-xs text-gray-400", {t!("board-screen-toggle")} }
                        div { class: "flex flex-wrap gap-1",
                            for opt in options {
                                button {
                                    key: "{opt.label}",
                                    class: if opt.active {
                                        "inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium bg-teal-600 text-white hover:bg-teal-700 disabled:opacity-50"
                                    } else {
                                        "inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium border border-teal-300 text-teal-700 hover:bg-teal-50 disabled:opacity-50"
                                    },
                                    disabled: launching() || opt.locked,
                                    title: if opt.locked {
                                        t!("board-screen-locked").to_string()
                                    } else {
                                        String::new()
                                    },
                                    onclick: move |_| screen_pick.set(Some(opt.kind.clone())),
                                    {opt.label.clone()}
                                }
                            }
                        }
                        if let Some((name, rows)) = board.as_ref().and_then(|b| {
                            board_pinout::screen_wiring(b, &pins, effective.as_deref())
                        }) {
                            board_pinout::ScreenWiring { name, rows }
                        }
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
                        class: "px-4 py-2 bg-amber-600 text-white rounded-lg hover:bg-amber-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                        r#type: "button",
                        disabled: launching(),
                        onclick: submit,
                        icons::Wrench { class: "h-4 w-4 inline mr-1" }
                        {t!("builds-submit")}
                    }
                }
            }
        }
    }
}

/// Confirmation de déploiement OTA — montre CE qui va être déployé (build
/// cible + date de compilation) et SUR QUOI (version annoncée du device),
/// gère l'état « aucun firmware » (deploy grisé + raccourci rebuild) et
/// l'avertissement de fraîcheur (`stale` : le serveur courant embarque des
/// sources firmware différentes de celles du build).
#[component]
fn OtaModal(
    target: OtaTarget,
    on_close: Callback<()>,
    on_rebuild: Callback<()>,
    on_launched: Callback<()>,
) -> Element {
    let mut launching = use_signal(|| false);
    let has_build = target.latest_build.is_some();
    let device_id = target.device_id.clone();
    let device_pk = target.device_pk;
    // Rebuild reuses the record id → same version: redeploy needs force.
    let same_version = target
        .latest_build
        .as_ref()
        .map(|(v, _)| Some(v.as_str()) == target.device_version.as_deref())
        .unwrap_or(false);
    let show_rebuild = target.can_rebuild && (!has_build || target.stale);

    rsx! {
        // Modal sans backdrop (doctrine EditorShell : pas de backdrop plein
        // écran sous les popovers/modales simples).
        div { class: "fixed inset-0 z-50 flex items-center justify-center pointer-events-none",
            div { class: "pointer-events-auto bg-white rounded-xl shadow-xl border border-gray-200 p-6 w-[26rem]",
                h3 { class: "text-lg font-semibold text-gray-900 mb-2", {t!("devices-ota-title")} }
                if let Some((version, date)) = &target.latest_build {
                    p { class: "text-sm text-gray-600 mb-1",
                        {t!("devices-ota-target-line", device: device_id.clone(), version: version.clone())}
                    }
                    p { class: "text-xs text-gray-500 mb-1",
                        {t!("devices-ota-built", date: date.clone())}
                    }
                    if same_version {
                        p { class: "text-xs font-medium text-indigo-600 mb-1",
                            {t!("devices-ota-same")}
                        }
                    } else if target.device_version.is_none() {
                        p { class: "text-xs text-gray-500 mb-1",
                            {t!("devices-ota-unknown-current")}
                        }
                    }
                } else {
                    p { class: "text-sm text-gray-600 mb-1", {t!("devices-ota-unknown-target", device: device_id.clone())} }
                    p { class: "text-xs text-amber-600 mb-1", {t!("devices-ota-none")} }
                }
                if target.stale && has_build {
                    p { class: "text-xs text-amber-600 mb-1", {t!("devices-ota-stale")} }
                }
                p { class: "text-xs text-gray-500 mb-4", {t!("devices-ota-hint")} }
                div { class: "flex justify-end gap-2",
                    button {
                        class: "px-3 py-1.5 text-sm rounded-lg border border-gray-300 text-gray-700 hover:bg-gray-50",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    if show_rebuild {
                        button {
                            class: "px-3 py-1.5 text-sm rounded-lg bg-amber-100 text-amber-700 hover:bg-amber-200",
                            onclick: move |_| on_rebuild.call(()),
                            icons::Wrench { class: "h-4 w-4 inline mr-1" }
                            {t!("devices-rebuild")}
                        }
                    }
                    button {
                        class: "px-3 py-1.5 text-sm rounded-lg bg-violet-600 text-white hover:bg-violet-700 disabled:opacity-50",
                        disabled: launching() || !has_build,
                        onclick: move |_| {
                            launching.set(true);
                            spawn(async move {
                                // Redeploying the running version (rebuild
                                // reuses the record id = same version) needs
                                // the force flag, else the API 409s.
                                let body = if same_version {
                                    serde_json::json!({ "force": true })
                                } else {
                                    serde_json::json!({})
                                };
                                match api::ota::deploy(device_pk, body).await {
                                    Ok(res) => {
                                        if res["pushed"] == serde_json::json!(false) {
                                            toasts::info(t!("devices-ota-offline-queued").to_string());
                                        } else {
                                            toasts::info(t!("devices-ota-started").to_string());
                                        }
                                        on_launched.call(());
                                    }
                                    Err(err) => {
                                        toasts::error(err);
                                        launching.set(false);
                                    }
                                }
                            });
                        },
                        {if launching() { t!("common-loading").to_string() } else { t!("devices-ota-confirm").to_string() }}
                    }
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
        "agent" => t!("agent-badge"),
        other => other.to_string(),
    };
    let badge = match device_type {
        "sensor" => "bg-blue-100 text-blue-800",
        "actuator" => "bg-amber-100 text-amber-800",
        "mixed" => "bg-purple-100 text-purple-800",
        "agent" => "bg-emerald-100 text-emerald-800",
        _ => "bg-gray-100 text-gray-800",
    };
    (badge, label)
}

/// Localized OTA deployment label (states = the wire strings) + progress.
fn ota_state_label(st: &pnex_core::OtaAssignmentState) -> String {
    let base: String = match st.state.as_str() {
        "pending" => t!("devices-ota-pending").to_string(),
        "downloading" => match st.progress {
            Some(p) => format!("{} {p} %", t!("devices-ota-downloading")),
            None => t!("devices-ota-downloading").to_string(),
        },
        "flashing" => t!("devices-ota-flashing").to_string(),
        other => other.to_string(),
    };
    if let Some(err) = &st.error {
        format!("{base} — {err}")
    } else {
        base
    }
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
            format!(
                "{} {}",
                t!("devices-last-seen-at"),
                local.format("%H:%M:%S")
            )
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
            // Edge agent (D95): no firmware, pins or provisioning secrets —
            // credentials only travel through the enrollment code.
            let is_agent = device.predefined_device_name == pnex_core::EDGE_AGENT_PREDEF;
            // Dernier build (colonne Firmware en détail).
            let firmware_badge = device.latest_build.as_ref().map(|build| {
                let (class, label) = phase_badge(build.build_phase.as_deref());
                (class, label, date_label(&build.updated_at))
            });
            rsx! {
                div { class: "space-y-4",
                    // Header card — identity, statuses, delete.
                    div { class: "bg-white rounded-lg shadow-sm",
                        div { class: "flex flex-wrap items-center gap-3 p-5",
                            // Convention retour des éditeurs (functions.rs) :
                            // bouton carré ArrowLeft + titre à côté.
                            button {
                                class: "rounded-lg border border-gray-300 p-2.5 text-gray-600 hover:bg-gray-50 transition-colors",
                                title: t!("devices-back"),
                                onclick: move |_| on_back.call(()),
                                icons::ArrowLeft { class: "h-4 w-4" }
                            }
                            h2 { class: "font-mono text-lg font-semibold text-gray-900",
                                code { {device_id.clone()} }
                            }
                            if token.as_ref().is_some_and(|t| t.is_active) {
                                span { class: "inline-flex items-center rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-800",
                                    {t!("devices-badge-provisioned")}
                                }
                            }
                            if let Some((class, label, date)) = &firmware_badge {
                                span { class: "{class}", {label.clone()} }
                                span { class: "text-xs text-gray-400", {date.clone()} }
                            }
                            {last_seen_label(&device.last_seen)}
                            div { class: "flex-1" }
                            if can_write {
                                button {
                                    class: "rounded-lg px-3 py-1.5 text-sm font-medium text-red-700 hover:bg-red-50 transition-colors",
                                    onclick: move |_| confirm_delete.set(true),
                                    icons::Trash2 { class: "mr-1 inline h-4 w-4" }
                                    {t!("devices-delete")}
                                }
                            }
                        }
                    }

                    // Board card — SVG pinout editor (generic, profile v2)
                    // or outputs black box (predefined).
                    div { class: "bg-white rounded-lg shadow-sm",
                        if is_agent {
                            crate::components::agent_panel::AgentPanel { device_pk, can_write }
                        } else if device.predefined_device_name.starts_with("generic") {
                            crate::components::board_pinout_editor::BoardPinoutEditor {
                                device_pk,
                                can_write,
                            }
                        } else {
                            div { class: "p-6",
                                if !capabilities.is_empty() {
                                    div { class: "mb-4",
                                        h3 { class: "mb-3 text-sm font-semibold uppercase tracking-wider text-gray-500", {t!("devices-capabilities")} }
                                        div { class: "flex flex-wrap gap-2",
                                            for cap in capabilities {
                                                span {
                                                    key: "{cap.id}",
                                                    class: "rounded-full bg-gray-100 px-2.5 py-0.5 text-xs font-medium text-gray-700",
                                                    title: "{cap.mode}",
                                                    {cap.name.clone()}
                                                }
                                            }
                                        }
                                    }
                                }
                                {black_box_outputs(&device)}
                            }
                        }
                    }

                    // Provisioning token card (read-only).
                    if let Some(token_info) = token.as_ref().filter(|_| !is_agent) {
                        SecretCard {
                            title: t!("devices-token"),
                            hint: t!("devices-token-hint"),
                            value: token_info.token.clone(),
                            active_badge: token_info
                                .is_active
                                .then(|| t!("devices-token-active")),
                            created: token_info.created.as_deref().map(date_label),
                            created_label: t!("devices-token-created"),
                        }
                    }

                    // Encryption key card (read-only).
                    if let Some(enc_key) = token
                        .as_ref()
                        .filter(|_| !is_agent)
                        .and_then(|t| t.encryption_key.clone())
                    {
                        SecretCard {
                            title: t!("devices-encryption-key"),
                            hint: t!("devices-enckey-hint"),
                            value: enc_key,
                            active_badge: None,
                            created: None,
                            created_label: String::new(),
                        }
                    }

                    // Labels card — key/value pills, JSON as expert mode.
                    div { class: "bg-white rounded-lg shadow-sm",
                        div { class: "p-5 sm:p-6",
                            crate::components::kv_pills_editor::KvPillsEditor {
                                key: "{device_pk}-{reload}",
                                initial: device.metadata.clone().unwrap_or(serde_json::Value::Null),
                                title: t!("devices-labels-title"),
                                hint: t!("devices-labels-hint"),
                                can_write,
                                on_save: move |value| {
                                    spawn(async move {
                                        match api::devices::update_metadata(device_pk, value).await {
                                            Ok(_) => toasts::success("toast-saved"),
                                            Err(err) => toasts::error(err),
                                        }
                                        refresh.call(());
                                    });
                                },
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
                                    Ok(()) => {
                                        toasts::success("toast-saved");
                                        // Navigation + refresh APRÈS la requête :
                                        // spawn est lié au scope du composant — un
                                        // on_back synchrone démonterait DeviceDetail
                                        // et annulerait la tâche (requête avortée,
                                        // ni toast ni suppression).
                                        on_back.call(());
                                        on_changed.call(());
                                    }
                                    Err(err) => toasts::error(err),
                                }
                            });
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

/// Read-only "secret" card (provisioning token, encryption key) — value
/// masked by default, Reveal/Hide + Copy, optional badge and "created at"
/// row. Mockup look.
#[component]
fn SecretCard(
    title: String,
    hint: String,
    value: String,
    /// Optional badge (e.g. "Active" for an active token).
    active_badge: Option<String>,
    /// Optional "created at" row (already formatted, local time).
    created: Option<String>,
    created_label: String,
) -> Element {
    let mut revealed = use_signal(|| false);
    let mask = "•".repeat(28);
    rsx! {
        div { class: "bg-white rounded-lg shadow-sm",
            div { class: "space-y-3 p-5 sm:p-6",
                div { class: "flex flex-wrap items-center gap-2",
                    h3 { class: "text-sm font-semibold uppercase tracking-wider text-gray-500", "{title}" }
                    div { class: "ml-auto flex items-center gap-2",
                        span { class: "inline-flex items-center rounded-full border border-gray-200 bg-gray-50 px-2 py-0.5 text-xs font-medium text-gray-500",
                            {t!("devices-readonly-badge")}
                        }
                        if let Some(badge) = active_badge {
                            span { class: "inline-flex items-center rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-800",
                                {badge}
                            }
                        }
                    }
                }
                p { class: "text-xs text-gray-400", "{hint}" }
                div { class: "flex flex-wrap items-center gap-2",
                    code { class: "min-w-0 flex-1 truncate rounded-lg border border-gray-200 bg-gray-50 px-3 py-2.5 font-mono text-sm",
                        {if revealed() { value.clone() } else { mask.clone() }}
                    }
                    button {
                        class: "rounded-lg border border-gray-300 px-3 py-2 text-xs font-medium text-gray-700 hover:bg-gray-50",
                        onclick: move |_| revealed.toggle(),
                        {if revealed() { t!("devices-token-hide-btn") } else { t!("devices-token-reveal") }}
                    }
                    button {
                        class: "rounded-lg border border-gray-300 px-3 py-2 text-xs font-medium text-gray-700 hover:bg-gray-50",
                        onclick: move |_| {
                            copy_to_clipboard(&value);
                            toasts::success(t!("toast-copied"));
                        },
                        {t!("devices-token-copy")}
                    }
                }
                if let Some(created) = created {
                    div { class: "flex items-center gap-3 border-t border-gray-100 pt-3",
                        span { class: "w-40 shrink-0 text-xs font-semibold text-gray-500", {created_label} }
                        span { class: "font-mono text-sm text-gray-700", {created} }
                    }
                }
            }
        }
    }
}

/// Clipboard copy — Clipboard API (secure context) with an `execCommand`
/// fallback for insecure contexts (LAN over http).
fn copy_to_clipboard(value: &str) {
    let escaped = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
    let js = format!(
        "(async () => {{ const s = {escaped}; \
         try {{ await navigator.clipboard.writeText(s); }} \
         catch (e) {{ const t = document.createElement('textarea'); \
         t.value = s; t.style.position = 'fixed'; t.style.opacity = '0'; \
         document.body.appendChild(t); t.select(); \
         try {{ document.execCommand('copy'); }} catch (e2) {{}} t.remove(); }} }})()"
    );
    let _ = dioxus::document::eval(&js);
}

/// Boîte noire des devices prédéfinis (soil_sensor…) : les outputs qu'ils
/// génèrent, rien de configurable — simple et déterministe (décision
/// utilisateur 2026-09-20).
fn black_box_outputs(device: &pnex_core::Device) -> Element {
    if device.capabilities.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "space-y-2",
            h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider mb-3",
                {t!("blackbox-outputs-title")}
            }
            div { class: "rounded-lg border border-gray-800 bg-gray-900 p-4 space-y-2",
                div { class: "flex items-center gap-2",
                    span { class: "inline-block h-2.5 w-2.5 rounded-full bg-green-500" }
                    span { class: "font-mono text-xs text-gray-400",
                        {device.device_id.clone()}
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    for cap in &device.capabilities {
                        span {
                            key: "{cap.id}",
                            class: "inline-flex items-center gap-1 rounded-full bg-gray-800 px-2.5 py-1 text-xs font-mono text-green-400",
                            title: "{cap.mode}",
                            {cap.name.clone()}
                        }
                    }
                }
            }
        }
    }
}
