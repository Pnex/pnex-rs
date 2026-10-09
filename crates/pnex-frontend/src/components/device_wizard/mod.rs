//! Device registration wizard — port of the React POC `DeviceWizard.tsx`:
//! stepper, identifier + random generator, optional D42 labels, model cards,
//! board variant, firmware (generic PneX or a custom firmware project),
//! debug screen (generic firmware only), WiFi, review; then the server
//! build is followed **inside the modal** (polling ~5 s).
//!
//! The former Tier 2 « custom_device » path (copy-paste platformio.ini +
//! sketch) is gone: the custom firmware IDE replaces it.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::badges::{date_label, phase_badge};
use super::edge_refs_picker::{PnexHostPicker, WifiCredentialPicker};
use super::flash_modal::FlashModal;
use super::icons;
use super::labels_editor::LabelChipsInput;
use super::modal::Modal;
use crate::api;
use crate::components::board_pinout_editor as board_pinout;
use crate::flash;
use crate::state::toasts;
use crate::util::sleep;

mod firmware_pick;
mod naming;
mod screen_pick;

use firmware_pick::*;
use naming::*;
use screen_pick::*;

/// Retired Tier 2 model (custom_device), replaced by the custom firmware
/// IDE: never offered, even if an old catalogue row still exists.
fn is_custom(name: &str) -> bool {
    matches!(name, "custom_device")
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Identity,
    Model,
    /// Étape 3 : WiFi / serveur.
    Config,
    Review,
    /// Traditionnel créé : build auto suivi en direct.
    BuildProgress,
    /// Device inactif connu → réactivation 200 (pas de nouveau token).
    Reactivated,
    /// Edge agent (D95) created: enrollment code + install commands.
    AgentInstall,
}

/// Edge agent model (D95): no firmware, no WiFi — installed on a machine.
fn is_agent(name: &str) -> bool {
    name == pnex_core::EDGE_AGENT_PREDEF
}

#[component]
pub fn DeviceWizard(on_close: Callback<()>, on_changed: Callback<()>) -> Element {
    let mut step = use_signal(|| Step::Identity);
    let mut device_id = use_signal(String::new);
    // D42 labels collected before the device exists, written through the
    // resources labels API right after creation.
    let labels = use_signal(pnex_core::resources::LabelSet::new);
    let selected = use_signal(|| None::<pnex_core::PredefinedDevice>);
    let mut model_search = use_signal(String::new);
    // Variante de board (chips du Model step) — figée à l'enregistrement
    // via CreateDevice.board_id ; None = board par défaut du modèle.
    let variant_board_id = use_signal(|| None::<i64>);
    // Board en aperçu (modale plein format, œil sur la chip) — None = fermée.
    let mut preview_board = use_signal(|| None::<api::boards::Board>);
    // Écran de debug choisi au wizard (aucun/OLED/TFT — appliqué au device
    // juste avant le 1er build ; outer None = pas touché, défaut = aucun,
    // sauf écran soudé seedé côté serveur). Réinitialisé si la variante
    // change (les options/câblage dépendent de la board).
    let mut screen_pick = use_signal(|| None::<Option<String>>);
    // Custom firmware chosen at provisioning (D94) — None = generic preset;
    // reset with the variant (compatibility depends on the board chip).
    let mut firmware_pick = use_signal(|| None::<i64>);
    use_effect(move || {
        variant_board_id();
        selected();
        screen_pick.set(None);
        firmware_pick.set(None);
    });
    // Paramètres de build (étape Config) — piochés dans les référentiels
    // Edge (wifi_credentials / pnex_hosts, org-scoped) : zéro ressaisie.
    // Le payload `CreateBuild` est résolu au submit depuis les entrées
    // sélectionnées (contrat inchangé).
    let wifi_sel = use_signal(|| None::<pnex_core::WifiCredential>);
    let host_sel = use_signal(|| None::<pnex_core::PnexHost>);
    let mut creating = use_signal(|| false);
    let mut created = use_signal(|| None::<pnex_core::Device>);
    let mut build_record = use_signal(|| None::<pnex_core::BuildRecord>);
    let mut build_launch_error = use_signal(|| None::<String>);
    let mut reactivation_msg = use_signal(String::new);
    // Polling : un seul minuteur à la fois (pattern page Builds).
    let mut polling = use_signal(|| false);
    // Flash navigateur du firmware fraîchement buildé (Web Serial).
    let mut flash_open = use_signal(|| false);
    // Retour visuel des boutons copier (« Copié » pendant 2 s).

    let catalogue = use_resource(|| async move { api::devices::predefined_devices().await });
    // Catalogue des variantes de board (GET /boards) — chargé une fois.
    let boards_resource = use_resource(|| async move { api::boards::list(None).await });

    // ── Validation + transitions ──
    let go_model = move |_| {
        let id = device_id().trim().to_string();
        if id.is_empty() {
            toasts::error("devices-id-required");
            return;
        }
        if id.chars().count() > 16 {
            toasts::error("wizard-id-too-long");
            return;
        }
        step.set(Step::Model);
    };
    let go_identity = move |_| step.set(Step::Identity);
    let mut agent_pk = use_signal(|| None::<i64>);
    let go_config = move |_| {
        let Some(model) = selected() else {
            toasts::error("devices-model-required");
            return;
        };
        if is_agent(&model.name) {
            // Edge agent: created right away, then installed on a machine.
            let id = device_id().trim().to_string();
            let initial_labels = labels();
            creating.set(true);
            spawn(async move {
                let outcome = api::devices::create(pnex_core::CreateDevice {
                    device_id: id,
                    predefined_device_name: model.name.clone(),
                    board_id: None,
                    firmware_project_id: None,
                })
                .await;
                creating.set(false);
                match outcome {
                    Ok(body) => match body.get("id").and_then(serde_json::Value::as_i64) {
                        Some(pk) => {
                            write_initial_labels(pk, &initial_labels).await;
                            agent_pk.set(Some(pk));
                            on_changed.call(());
                            step.set(Step::AgentInstall);
                        }
                        None => {
                            // 200 = reactivation of a known inactive device.
                            reactivation_msg.set(
                                body.get("detail")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                            on_changed.call(());
                            step.set(Step::Reactivated);
                        }
                    },
                    Err(err) => toasts::error(err),
                }
            });
            return;
        }
        step.set(Step::Config);
    };
    let go_model_back = move |_| step.set(Step::Model);
    let go_review = move |_| {
        if wifi_sel().is_none() || host_sel().is_none() {
            toasts::error("wizard-config-incomplete");
            return;
        }
        step.set(Step::Review);
    };

    // ── Création (+ build auto pour les traditionnels) ──
    let submit = move |_| {
        let Some(model) = selected() else { return };
        // Résolution du référentiel (wifi/host) avant le spawn — les gardes
        // go_review / garde custom ont déjà validé, ce match est un filet
        // de sécurité.
        let (creds, srv) = match (wifi_sel(), host_sel()) {
            (Some(c), Some(h)) => (c, h),
            _ => {
                toasts::error("wizard-config-incomplete");
                return;
            }
        };
        let id = device_id().trim().to_string();
        let initial_labels = labels();
        creating.set(true);
        spawn(async move {
            let outcome = api::devices::create(pnex_core::CreateDevice {
                device_id: id.clone(),
                predefined_device_name: model.name.clone(),
                board_id: variant_board_id(),
                firmware_project_id: firmware_pick(),
            })
            .await;
            creating.set(false);
            match outcome {
                Ok(body) => {
                    // 200 → réactivation : pas de nouveau token ni de build auto.
                    if let Some(detail) = body.get("detail").and_then(|d| d.as_str()) {
                        reactivation_msg.set(detail.to_string());
                        step.set(Step::Reactivated);
                        on_changed.call(());
                        return;
                    }
                    match serde_json::from_value::<pnex_core::Device>(body) {
                        Ok(device) if device.device_token.is_some() => {
                            let device_pk = device.id;
                            write_initial_labels(device_pk, &initial_labels).await;
                            created.set(Some(device));
                            on_changed.call(());
                            {
                                // Build automatique, suivi dans la modale —
                                // les erreurs (403 quota, 429 intervalle) sont
                                // affichées sans casser l'écran token.
                                build_record.set(None);
                                // Écran choisi au wizard : appliqué au device
                                // AVANT le 1er build (réserves + defines).
                                // Never with a custom firmware (no screen driver).
                                if let Some(pick) =
                                    screen_pick().filter(|_| firmware_pick().is_none())
                                {
                                    if let Err(msg) =
                                        board_pinout::set_screen(device_pk, pick).await
                                    {
                                        step.set(Step::BuildProgress);
                                        build_launch_error.set(Some(msg));
                                        return;
                                    }
                                }
                                step.set(Step::BuildProgress);
                                let params = pnex_core::CreateBuild {
                                    device_id: id,
                                    // The build references the entry's vault secret (secrets.md S6).
                                    wifi_credential_id: creds.id,
                                    pnex_host: srv.host,
                                };
                                if let Err(err) = api::builds::create(params).await {
                                    build_launch_error.set(Some(err.message));
                                }
                            }
                        }
                        _ => toasts::success("devices-created"),
                    }
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    // ── Polling du build tant qu'il vole (écran BuildProgress) ──
    if step() == Step::BuildProgress && build_launch_error().is_none() {
        let in_flight = match build_record() {
            None => true,
            Some(record) => !matches!(
                Some(record.build_phase.as_str()),
                Some("succeeded") | Some("failed")
            ),
        };
        if in_flight && !polling() {
            polling.set(true);
            let polled_id = created().map(|device| device.device_id).unwrap_or_default();
            spawn(async move {
                sleep(Duration::from_secs(5)).await;
                polling.set(false);
                // Un record par (org, device_id) : limit=1 = build courant.
                let filters = api::builds::BuildFilters {
                    device_id: Some(polled_id),
                    build_phase: None,
                    limit: Some(1),
                    offset: None,
                };
                if let Ok(paged) = api::builds::list(&filters).await {
                    if let Some(record) = paged.results.into_iter().next() {
                        build_record.set(Some(record));
                    }
                }
            });
        }
    }

    // Cartes du catalogue filtrées par la recherche.
    let visible_models: Vec<pnex_core::PredefinedDevice> = match &*catalogue.read() {
        Some(Ok(models)) => {
            let term = model_search().trim().to_lowercase();
            models
                .iter()
                // Tier 2 « custom_device » is superseded by the custom
                // firmware IDE (custom-firmware.md): never offered.
                .filter(|pd| !is_custom(&pd.name))
                .filter(|pd| term.is_empty() || model_matches(pd, &term))
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    };
    // One section per product family (edge-model.md §2 bis).
    let family_models = |family: pnex_core::DeviceFamily| -> Vec<pnex_core::PredefinedDevice> {
        visible_models
            .iter()
            .filter(|pd| pnex_core::DeviceFamily::of(&pd.name) == family)
            .cloned()
            .collect()
    };
    let model_sections = [
        (
            t!("wizard-model-section-generic"),
            t!("wizard-model-section-generic-help"),
            family_models(pnex_core::DeviceFamily::Generic),
        ),
        (
            t!("wizard-model-section-predefined"),
            t!("wizard-model-section-predefined-help"),
            family_models(pnex_core::DeviceFamily::Predefined),
        ),
        (
            t!("wizard-model-section-agent"),
            t!("wizard-model-section-agent-help"),
            family_models(pnex_core::DeviceFamily::Agent),
        ),
    ];
    // A custom firmware may only replace the generic firmware.
    let custom_firmware_allowed = selected()
        .as_ref()
        .is_some_and(|p| pnex_core::DeviceFamily::of(&p.name).accepts_custom_firmware());

    let busy = creating();

    rsx! {
        Modal {
            title: t!("devices-register-title"),
            max_width: "max-w-2xl".to_string(),
            on_close,
            div { class: "space-y-5",
                if !matches!(step(), Step::BuildProgress | Step::Reactivated | Step::AgentInstall) {
                    {stepper(step())}
                }

                match step() {
                    // ── Étape 1 : identifiant + métadonnées ──
                    Step::Identity => rsx! {
                        div { class: "space-y-4",
                            p { class: "text-sm text-gray-600", {t!("wizard-identity-help")} }
                            div { class: "flex gap-2",
                                input {
                                    class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                                    r#type: "text",
                                    maxlength: "16",
                                    placeholder: t!("devices-new-placeholder"),
                                    value: "{device_id}",
                                    oninput: move |event| device_id.set(event.value()),
                                }
                                button {
                                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm text-gray-600 hover:bg-gray-50 transition-colors",
                                    r#type: "button",
                                    onclick: move |_| device_id.set(random_device_id()),
                                    icons::RefreshCw { class: "h-4 w-4 inline mr-1" }
                                    {t!("wizard-shuffle")}
                                }
                            }
                            span { class: "text-[11px] text-gray-400", "{device_id().chars().count()}/16" }

                            div {
                                span { class: "block mb-2 text-xs font-semibold text-gray-500 uppercase tracking-wider",
                                    {t!("wizard-labels-title")}
                                }
                                LabelChipsInput { labels, can_write: true }
                            }

                            div { class: "flex justify-end",
                                button {
                                    class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium",
                                    r#type: "button",
                                    onclick: go_model,
                                    {t!("wizard-next")}
                                }
                            }
                        }
                    },

                    // ── Étape 2 : modèle (cartes groupées) ──
                    Step::Model => rsx! {
                        div { class: "space-y-4",
                            div { class: "relative",
                                icons::Search { class: "h-4 w-4 absolute left-3 top-3 text-gray-400" }
                                input {
                                    class: "w-full pl-9 pr-3 py-2 border border-gray-300 rounded-lg text-sm",
                                    r#type: "search",
                                    placeholder: t!("wizard-model-search"),
                                    value: "{model_search}",
                                    oninput: move |event| model_search.set(event.value()),
                                }
                            }

                            for (title, help, models) in model_sections {
                                if !models.is_empty() {
                                    div {
                                        h4 { class: "text-xs font-semibold text-blue-600 uppercase tracking-wider",
                                            {title}
                                        }
                                        p { class: "text-xs text-gray-400 mb-2", {help} }
                                        div { class: "grid gap-2",
                                            for pd in models {
                                                {model_card(pd, selected, variant_board_id)}
                                            }
                                        }
                                    }
                                }
                            }

                            if visible_models.is_empty() {
                                p { class: "text-sm text-gray-400 text-center py-6", {t!("wizard-model-none")} }
                            }

                            // Variantes de board du modèle générique sélectionné
                            // (chips .chipsel du prototype) — None = défaut.
                            {
                                let variants = variant_list(
                                    boards_resource
                                        .value()
                                        .cloned()
                                        .map(|r| r.ok())
                                        .flatten(),
                                    selected().as_ref(),
                                );
                                if !variants.is_empty() {
                                    // Board effective (variante choisie ou
                                    // défaut du modèle) — porte les écrans
                                    // proposés au wizard.
                                    let effective_board = variant_board_id()
                                        .and_then(|id| {
                                            variants.iter().find(|b| b.id == id)
                                        })
                                        .or_else(|| {
                                            variants
                                                .iter()
                                                .find(|b| {
                                                    selected().as_ref().is_some_and(|p| p.board == b.name)
                                                })
                                        })
                                        .cloned();
                                    let effective_soc = effective_board.as_ref().map(|b| b.soc.clone());
                                    rsx! {
                                        div { class: "border-t border-gray-100 pt-3",
                                            h4 { class: "text-xs font-semibold text-gray-600 uppercase tracking-wider mb-2",
                                                {t!("wizard-board-variant")}
                                            }
                                            p { class: "text-xs text-gray-400 mb-2", {t!("wizard-board-variant-help")} }
                                            div { class: "flex flex-wrap gap-2",
                                                for v in variants {
                                                    VariantChip {
                                                        board: v,
                                                        selected_id: variant_board_id,
                                                        default_board_name: selected().as_ref().map(|p| p.board.clone()),
                                                        on_preview: move |b| preview_board.set(Some(b)),
                                                    }
                                                }
                                            }
                                            if let Some(pb) = preview_board() {
                                                board_pinout::BoardPreviewModal {
                                                    board: pb,
                                                    on_close: move |_| preview_board.set(None),
                                                }
                                            }
                                            if let Some(soc) = effective_soc.filter(|_| custom_firmware_allowed) {
                                                FirmwarePickBlock { soc, firmware_pick }
                                            }
                                            // Debug screen = generic firmware only: a custom
                                            // firmware owns every pin (no hidden driver).
                                            if firmware_pick().is_none() {
                                                if let Some(b) = effective_board {
                                                    if !b.screens().is_empty() {
                                                        ScreenPickBlock {
                                                            board: b,
                                                            screen_pick,
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    rsx! {}
                                }
                            }

                            div { class: "flex justify-between pt-2",
                                button {
                                    class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                                    r#type: "button",
                                    onclick: go_identity,
                                    {t!("wizard-back")}
                                }
                                button {
                                    class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium",
                                    r#type: "button",
                                    disabled: busy,
                                    onclick: go_config,
                                    if selected().as_ref().is_some_and(|p| is_agent(&p.name)) {
                                        {t!("wizard-agent-create")}
                                    } else {
                                        {t!("wizard-next")}
                                    }
                                }
                            }
                        }
                    },

                    Step::AgentInstall => rsx! {
                        div { class: "space-y-4",
                            h3 { class: "text-sm font-semibold text-gray-700", {t!("agent-install-title")} }
                            if let Some(pk) = agent_pk() {
                                crate::components::agent_panel::AgentInstall { device_pk: pk }
                            }
                            div { class: "flex justify-end",
                                button {
                                    class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium",
                                    r#type: "button",
                                    onclick: move |_| on_close.call(()),
                                    {t!("common-close")}
                                }
                            }
                        }
                    },

                    Step::Config => rsx! {
                        div { class: "space-y-4",
                            WifiCredentialPicker { selected: wifi_sel }
                            PnexHostPicker { selected: host_sel }
                            p { class: "text-xs text-gray-400", {t!("wizard-config-help")} }
                            div { class: "flex justify-between pt-2",
                                button {
                                    class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                                    r#type: "button",
                                    onclick: go_model_back,
                                    {t!("wizard-back")}
                                }
                                button {
                                    class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium",
                                    r#type: "button",
                                    onclick: go_review,
                                    {t!("wizard-next")}
                                }
                            }
                        }
                    },

                    // ── Étape 4 (traditionnel) : revue ──
                    Step::Review => {
                        // Valeurs résolues depuis les entrées du référentiel.
                        let (rev_ssid, rev_host) = match (wifi_sel(), host_sel()) {
                            (Some(c), Some(h)) => (Some(c.ssid.clone()), Some(h.host.clone())),
                            _ => (None, None),
                        };
                        rsx! {
                            {
                                review_panel(
                                    &device_id(),
                                    &selected(),
                                    labels,
                                    rev_ssid.as_deref(),
                                    rev_host.as_deref(),
                                    true,
                                )
                            }
                            p { class: "text-sm text-indigo-700 bg-indigo-50 border border-indigo-200 rounded-lg p-3",
                                {t!("wizard-review-build-note")}
                            }
                            div { class: "flex justify-between pt-2",
                                button {
                                    class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                                    r#type: "button",
                                    onclick: move |_| step.set(Step::Config),
                                    {t!("wizard-back")}
                                }
                                button {
                                    class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                                    r#type: "button",
                                    disabled: busy,
                                    onclick: submit,
                                    if busy {
                                        {t!("common-loading")}
                                    } else {
                                        {t!("wizard-create-build")}
                                    }
                                }
                            }
                        }
                    }
                    Step::BuildProgress => rsx! {
                        div { class: "space-y-4",
                            if let Some(message) = build_launch_error() {
                                div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                    p { {t!("wizard-build-launch-failed")} }
                                    p { class: "mt-1 font-medium", {message} }
                                }
                            } else {
                                match build_record() {
                                    None => rsx! {
                                        div { class: "flex items-center gap-3 text-sm text-gray-600",
                                            span { class: "animate-spin inline-block rounded-full h-5 w-5 border-b-2 border-blue-600" }
                                            {t!("wizard-build-pending")}
                                        }
                                    },
                                    Some(record) => rsx! {
                                        div { class: "space-y-2",
                                            div { class: "flex items-center justify-between",
                                                span { class: phase_badge(Some(record.build_phase.as_str())).0,
                                                    {phase_badge(Some(record.build_phase.as_str())).1}
                                                }
                                                span { class: "text-xs text-gray-400", {date_label(&record.updated_at)} }
                                            }
                                            if record.succeeded() {
                                                div { class: "space-y-2",
                                                    // Flash direct en Web Serial (Chromium —
                                                    // le modal avertit sinon). Le binaire reste
                                                    // téléchargeable depuis la liste des devices.
                                                    // Android: no flashing path, point to a computer instead.
                                                    if flash::offered() {
                                                        button {
                                                            class: "w-full px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors text-sm font-medium",
                                                            r#type: "button",
                                                            onclick: move |_| flash_open.set(true),
                                                            icons::Zap { class: "h-4 w-4 inline mr-1" }
                                                            {t!("devices-flash")}
                                                        }
                                                    } else {
                                                        p { class: "text-sm text-gray-600", {t!("wizard-flash-from-computer")} }
                                                    }
                                                }
                                            } else if Some(record.build_phase.as_str()) == Some("failed") {
                                                div { class: "space-y-2",
                                                    crate::components::build_failure::BuildFailureNote {
                                                        code: record.failure_code.clone(),
                                                        detail: record.failure_detail.clone(),
                                                    }
                                                    p { class: "text-sm text-red-700", {t!("wizard-build-failed")} }
                                                }
                                            } else {
                                                div { class: "flex items-center gap-3 text-sm text-gray-600",
                                                    span { class: "animate-spin inline-block rounded-full h-5 w-5 border-b-2 border-blue-600" }
                                                    {t!("wizard-build-pending")}
                                                }
                                            }
                                        }
                                    },
                                }
                            }
                            // Reassurance: closing is safe — the build runs in the
                            // server worker, the binary persists and credentials
                            // stay reachable (masked) on the device page.
                            div { class: "flex items-start gap-3 bg-blue-50 border border-blue-200 rounded-lg p-3 text-sm text-blue-800",
                                icons::Info { class: "h-5 w-5 shrink-0" }
                                {t!("wizard-build-close-hint")}
                            }
                            div { class: "flex justify-end",
                                button {
                                    class: "px-4 py-2 text-sm font-semibold text-white bg-blue-600 rounded-lg hover:bg-blue-700 transition-colors",
                                    r#type: "button",
                                    onclick: move |_| on_close.call(()),
                                    {t!("common-close")}
                                }
                            }
                        }
                    },

                    // ── Réactivation : pas de nouveau token ──
                    Step::Reactivated => rsx! {
                        div { class: "space-y-4",
                            div { class: "flex items-start gap-3 bg-blue-50 border border-blue-200 rounded-lg p-4 text-sm text-blue-800",
                                icons::Info { class: "h-5 w-5 shrink-0" }
                                div {
                                    p { {t!("wizard-reactivated")} }
                                    p { class: "mt-1 text-blue-600", {reactivation_msg()} }
                                }
                            }
                            div { class: "flex justify-end",
                                button {
                                    class: "px-4 py-2 text-sm font-semibold text-white bg-blue-600 rounded-lg hover:bg-blue-700 transition-colors",
                                    r#type: "button",
                                    onclick: move |_| on_close.call(()),
                                    {t!("common-close")}
                                }
                            }
                        }
                    },
                }
            }
        }

        // Flash navigateur du firmware buildé (Web Serial — la modale se
        // superpose à celle du wizard, l'état se réinitialise à l'ouverture).
        if flash_open() {
            if let Some(device) = created() {
                FlashModal {
                    key: "{device.device_id}",
                    device_id: device.device_id,
                    on_close: move |_| flash_open.set(false),
                }
            }
        }
    }
}

/// Stepper numéroté (4 étapes).
fn stepper(current: Step) -> Element {
    let items: Vec<(Step, &'static str)> = vec![
        (Step::Identity, "wizard-step-identity"),
        (Step::Model, "wizard-step-model"),
        (Step::Config, "wizard-step-wifi"),
        (Step::Review, "wizard-step-review"),
    ];
    let current_index = items
        .iter()
        .position(|(step, _)| *step == current)
        .unwrap_or(0);
    rsx! {
        div { class: "flex items-center flex-wrap gap-x-2 gap-y-1",
            for (index, (_, key)) in items.iter().enumerate() {
                div { class: "flex items-center gap-1.5", key: "{index}",
                    if index > 0 {
                        span { class: "text-gray-300 mx-0.5", "→" }
                    }
                    span { class: if index <= current_index { "flex items-center justify-center w-7 h-7 rounded-full text-xs font-semibold bg-blue-600 text-white" } else { "flex items-center justify-center w-7 h-7 rounded-full text-xs font-semibold bg-gray-200 text-gray-500" },
                        "{index + 1}"
                    }
                    // Phone: only the current step keeps its label, so the
                    // four steps fit on one row.
                    span { class: step_label_class(index, current_index), {t!(* key)} }
                }
            }
        }
    }
}

/// Label classes of a wizard step (full literals for the Tailwind scan).
fn step_label_class(index: usize, current: usize) -> &'static str {
    if index == current {
        "text-xs font-medium text-blue-700"
    } else if index < current {
        "hidden text-xs font-medium text-blue-700 sm:inline"
    } else {
        "hidden text-xs text-gray-400 sm:inline"
    }
}

/// Carte de modèle du catalogue — highlight si sélectionnée.
fn model_card(
    pd: pnex_core::PredefinedDevice,
    mut selected: Signal<Option<pnex_core::PredefinedDevice>>,
    mut variant_board_id: Signal<Option<i64>>,
) -> Element {
    let name = pd.name.clone();
    let is_selected = selected().as_ref().is_some_and(|s| s.name == name);
    let (border, chip) = ("border-blue-200 bg-white", "bg-blue-100 text-blue-800");
    let (ring, check) = if is_selected {
        ("ring-2 ring-blue-500 border-blue-500", true)
    } else {
        ("", false)
    };
    let shown_caps: Vec<String> = pd.capabilities.iter().take(3).cloned().collect();
    let hidden_caps = pd.capabilities.len().saturating_sub(3);
    let pretty = pd.pretty_name.clone().unwrap_or_else(|| pd.name.clone());
    rsx! {
        button {
            class: "text-left w-full p-3 rounded-lg border transition-colors hover:border-blue-400 {border} {ring}",
            r#type: "button",
            onclick: move |_| {
                // Nouveau modèle → variante réinitialisée (None = défaut).
                variant_board_id.set(None);
                selected.set(Some(pd.clone()));
            },
            div { class: "flex items-center justify-between",
                span { class: "text-sm font-semibold text-gray-900", {pretty} }
                if check {
                    icons::Check { class: "h-4 w-4 text-blue-600" }
                }
            }
            // description_i18n (fluent key) resolved client-side when
            // present; description stays the English fallback + search
            // field.
            if let Some(key) = pd.description_i18n.as_deref().filter(|d| !d.is_empty()) {
                p { class: "text-xs text-gray-500 mt-1",
                    {crate::api::error_i18n::resolve(key, None)}
                }
            } else if let Some(description) = pd.description.as_deref().filter(|d| !d.is_empty()) {
                p { class: "text-xs text-gray-500 mt-1", {description} }
            }
            div { class: "flex flex-wrap gap-1 mt-2",
                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium {chip}",
                    {pd.board.clone()}
                }
                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-gray-100 text-gray-700",
                    {pd.device_type.clone()}
                }
                for cap in shown_caps {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-gray-100 text-gray-600",
                        {cap}
                    }
                }
                if hidden_caps > 0 {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-gray-100 text-gray-400",
                        "+{hidden_caps}"
                    }
                }
            }
        }
    }
}

/// Variantes de board du modèle sélectionné : boards de même SoC que le
/// board par défaut du modèle (résolu par nom). Vide hors générique.
fn variant_list(
    boards: Option<Vec<api::boards::Board>>,
    selected: Option<&pnex_core::PredefinedDevice>,
) -> Vec<api::boards::Board> {
    let (Some(boards), Some(pd)) = (boards, selected) else {
        return Vec::new();
    };
    let Some(soc) = boards
        .iter()
        .find(|b| b.name == pd.board)
        .map(|b| b.soc.clone())
    else {
        return Vec::new();
    };
    boards.into_iter().filter(|b| b.soc == soc).collect()
}

/// Chip de variante (style .chipsel du prototype pnex-pinout.html) —
/// compacte ; l'œil ouvre la modale plein format (`BoardPreviewModal`).
#[component]
fn VariantChip(
    board: api::boards::Board,
    mut selected_id: Signal<Option<i64>>,
    default_board_name: Option<String>,
    on_preview: Callback<api::boards::Board>,
) -> Element {
    let is_default = default_board_name.as_deref() == Some(board.name.as_str());
    let id = board.id;
    let selected_now =
        selected_id().is_some_and(|sid| sid == id) || (selected_id().is_none() && is_default);
    let per_side = board
        .layout
        .get("per_side")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
        * 2;
    let meta = format!("{} · {} pins", board.soc, per_side);
    let (border, ring) = if selected_now {
        ("border-blue-500", "ring-2 ring-blue-500")
    } else {
        ("border-gray-300", "")
    };
    // Badge réservé aux écrans VRAIMENT soudés (`builtin: true`, forcés) —
    // une simple option externe déclarée n'affiche rien : le bloc Screen
    // du wizard porte déjà ce choix.
    let builtin_screen = board
        .screens()
        .iter()
        .any(|s| s.get("builtin").and_then(|b| b.as_bool()) == Some(true));
    rsx! {
        div {
            class: "relative flex flex-col items-start gap-0.5 text-left rounded-xl border px-3.5 py-2.5 transition-colors hover:border-blue-400 {border} {ring} bg-white cursor-pointer select-none",
            role: "button",
            tabindex: "0",
            aria_pressed: selected_now,
            onclick: move |_| selected_id.set(Some(id)),
            span { class: "text-sm font-semibold text-gray-900 pr-6",
                {board.pretty_name.clone().unwrap_or_else(|| board.name.clone())}
            }
            span { class: "text-[11px] font-mono text-gray-400", "{meta}" }
            if builtin_screen {
                span { class: "inline-flex items-center gap-1 text-[10px] text-teal-700 bg-teal-50 rounded-full px-1.5 py-0.5",
                    {t!("board-peripheral-screen")}
                }
            }
            button {
                class: "absolute top-2 right-2 p-1 rounded-md text-gray-400 hover:text-blue-600 hover:bg-blue-50 transition-colors",
                r#type: "button",
                title: t!("board-pinout-preview"),
                aria_label: t!("board-pinout-preview"),
                onclick: move |ev| {
                    ev.stop_propagation();
                    on_preview.call(board.clone());
                },
                icons::Eye { class: Some("h-4 w-4".into()) }
            }
        }
    }
}

/// Panneau de revue (étape 4).
#[allow(clippy::too_many_arguments)]
fn review_panel(
    device_id: &str,
    selected: &Option<pnex_core::PredefinedDevice>,
    labels: Signal<pnex_core::resources::LabelSet>,
    ssid: Option<&str>,
    host: Option<&str>,
    with_build: bool,
) -> Element {
    let model = selected
        .as_ref()
        .map(|pd| pd.pretty_name.clone().unwrap_or_else(|| pd.name.clone()))
        .unwrap_or_default();
    let has_labels = !labels.read().is_empty();
    rsx! {
        div { class: "space-y-3 text-sm",
            div { class: "flex justify-between border-b border-gray-100 pb-2",
                span { class: "text-gray-500", {t!("devices-col-id")} }
                code { class: "font-medium text-gray-900", {device_id} }
            }
            div { class: "flex justify-between border-b border-gray-100 pb-2",
                span { class: "text-gray-500", {t!("devices-col-model")} }
                span { class: "font-medium text-gray-900", {model} }
            }
            div { class: "flex justify-between gap-4 border-b border-gray-100 pb-2",
                span { class: "text-gray-500 shrink-0", {t!("resources-labels-title")} }
                if has_labels {
                    LabelChipsInput { labels, can_write: false }
                } else {
                    span { class: "text-gray-900", "—" }
                }
            }
            if with_build {
                div { class: "flex justify-between border-b border-gray-100 pb-2",
                    span { class: "text-gray-500", {t!("builds-field-ssid")} }
                    span { class: "font-medium text-gray-900", {ssid.unwrap_or_default()} }
                }
                div { class: "flex justify-between border-b border-gray-100 pb-2",
                    span { class: "text-gray-500", {t!("builds-field-wifi-password")} }
                    span { class: "font-medium text-gray-900", "•••" }
                }
                div { class: "flex justify-between",
                    span { class: "text-gray-500", {t!("builds-field-server")} }
                    span { class: "font-medium text-gray-900",
                        {format!("wss://{}", host.unwrap_or_default())}
                    }
                }
            }
        }
    }
}

/// Writes the labels collected by the wizard on the freshly created device
/// (D42). A failure only toasts: the device itself is already registered.
async fn write_initial_labels(device_pk: i64, labels: &pnex_core::resources::LabelSet) {
    if labels.is_empty() {
        return;
    }
    if let Err(err) = api::resources::put_labels(
        pnex_core::resources::KIND_DEVICE,
        &device_pk.to_string(),
        labels,
    )
    .await
    {
        toasts::error(format!("{err}"));
    }
}
