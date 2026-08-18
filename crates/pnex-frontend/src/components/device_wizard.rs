//! Assistant d'enregistrement d'un device — portage du `DeviceWizard.tsx`
//! du POC React (pnex-ui) : stepper, identifiant + générateur aléatoire,
//! métadonnées optionnelles, modèle en cartes groupées (dynamique vs
//! traditionnel), WiFi, revue.
//!
//! Au terme : les types custom affichent le token + un snippet Python de
//! publisher (interpolé) ; les types traditionnels déclenchent le build et
//! suivent sa progression **dans la modale** (polling ~5 s — au-delà du POC,
//! directive utilisateur).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::badges::{date_label, phase_badge};
use super::flash_modal::FlashModal;
use super::icons;
use super::modal::Modal;
use crate::api;
use crate::state::toasts;
use crate::util::{copy_text, default_host, save_blob, sleep, ws_ingest_url};

/// Types custom (mesures dynamiques) — parité back `allow_dynamic`.
fn is_custom(name: &str) -> bool {
    matches!(name, "custom_sensor" | "custom_device")
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Identity,
    Model,
    /// Étape 3 des deux chemins : WiFi (traditionnel) ou revue (custom).
    Config,
    Review,
    /// Custom créé : token + snippet Python.
    CustomDone,
    /// Traditionnel créé : build auto suivi en direct.
    BuildProgress,
    /// Device inactif connu → réactivation 200 (pas de nouveau token).
    Reactivated,
}

// ── Générateur d'identifiants (shuffle du POC, unique-names-generator) ──

const ADJECTIVES: [&str; 24] = [
    "amber", "brave", "calm", "dusk", "eager", "fuzzy", "gentle", "happy", "icy", "jolly",
    "keen", "lucky", "mellow", "nimble", "olive", "proud", "quiet", "rapid", "silly", "tidy",
    "urban", "vivid", "witty", "young",
];
const ANIMALS: [&str; 24] = [
    "otter", "falcon", "ibex", "lynx", "marmot", "newt", "osprey", "puffin", "quokka", "robin",
    "serval", "tapir", "urchin", "viper", "walrus", "yak", "zebra", "badger", "crane", "dolphin",
    "ermine", "ferret", "gannet", "heron",
];

/// Identifiant kebab-case aléatoire (adjectif-animal, ≤ 16 chars).
fn random_device_id() -> String {
    let mut bytes = [0u8; 2];
    let _ = getrandom::getrandom(&mut bytes);
    let raw = format!(
        "{}-{}",
        ADJECTIVES[(bytes[0] as usize) % ADJECTIVES.len()],
        ANIMALS[(bytes[1] as usize) % ANIMALS.len()],
    );
    truncate_chars(&raw, 16)
}

/// Troncature respectant les frontières UTF-8.
fn truncate_chars(input: &str, max: usize) -> String {
    match input.char_indices().nth(max) {
        Some((idx, _)) => input[..idx].to_string(),
        None => input.to_string(),
    }
}

/// Lignes clé/valeur → objet JSON (lignes vides ignorées, `None` si vide).
fn rows_to_json(rows: &[(String, String)]) -> Option<serde_json::Value> {
    let map: serde_json::Map<String, serde_json::Value> = rows
        .iter()
        .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
        .map(|(k, v)| {
            (
                k.trim().to_string(),
                serde_json::Value::String(v.trim().to_string()),
            )
        })
        .collect();
    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}

/// Recherche multi-champs du catalogue (insensible à la casse).
fn model_matches(pd: &pnex_core::PredefinedDevice, term: &str) -> bool {
    let haystack = format!(
        "{} {} {} {} {} {}",
        pd.name,
        pd.pretty_name.as_deref().unwrap_or_default(),
        pd.description.as_deref().unwrap_or_default(),
        pd.device_type,
        pd.board,
        pd.capabilities.join(" "),
    )
    .to_lowercase();
    haystack.contains(term)
}

// ── Snippet Python (template du CustomDeviceSetup.tsx, marqueurs remplacés) ──

const PY_TEMPLATE: &str = r#"#!/usr/bin/env python3
"""
Simple Dynamic Device Publisher
================================

A minimal example of publishing custom sensor data to PNEX.

Usage:
    1. pip install websockets
    2. python publisher.py

To compile into standalone binary:
    pip install pyinstaller
    pyinstaller --onefile publisher.py
"""

import asyncio
import websockets
import base64
import sys

# ============ CONFIGURATION ============
DEVICE_TOKEN = "__TOKEN__"
DEVICE_ID = "__DEVICE_ID__"
PRED_DEV = "__PRED_DEV__"
WS_URL = "__WS_URL__"
PUBLISH_INTERVAL = 60  # seconds
# ========================================


def read_sensors():
    """Read your custom sensors and return {measurement_name: value}."""
    # CUSTOMIZE THIS FUNCTION WITH YOUR SENSOR READING LOGIC
    import random

    return {
        "temperature": round(20 + random.uniform(-5, 5), 2),
        "humidity": round(50 + random.uniform(-10, 10), 2),
        "soil_moisture": round(random.uniform(30, 80), 2),
        "custom_sensor_1": round(random.uniform(0, 100), 2),
    }


async def publish_loop(websocket):
    """Main publishing loop"""
    while True:
        try:
            measurements = read_sensors()
            for name, value in measurements.items():
                message = f"{name}={value}"
                await websocket.send(message)
                response = await websocket.recv()
                if response == "ok":
                    print(f"OK {message}")
                else:
                    print(f"ERR {message} - {response}", file=sys.stderr)
            await asyncio.sleep(PUBLISH_INTERVAL)
        except Exception as e:
            print(f"Error in publish loop: {e}", file=sys.stderr)
            raise


async def main():
    """Connect and start publishing"""
    token_b64 = base64.b64encode(DEVICE_TOKEN.encode()).decode()
    device_id_b64 = base64.b64encode(DEVICE_ID.encode()).decode()
    pred_dev_b64 = base64.b64encode(PRED_DEV.encode()).decode()
    url = f"{WS_URL}?token={token_b64}&device_id={device_id_b64}&pred_dev={pred_dev_b64}"

    print(f"Connecting to {WS_URL}...")
    print(f"Device ID: {DEVICE_ID}")
    print(f"Publishing every {PUBLISH_INTERVAL}s")

    while True:
        try:
            async with websockets.connect(url) as websocket:
                print("Connected!")
                await publish_loop(websocket)
        except websockets.exceptions.ConnectionClosed:
            print("Connection closed. Reconnecting in 5s...", file=sys.stderr)
            await asyncio.sleep(5)
        except Exception as e:
            print(f"Error: {e}. Reconnecting in 5s...", file=sys.stderr)
            await asyncio.sleep(5)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("Stopped by user")
        sys.exit(0)
"#;

#[component]
pub fn DeviceWizard(on_close: Callback<()>, on_changed: Callback<()>) -> Element {
    let mut step = use_signal(|| Step::Identity);
    let mut device_id = use_signal(String::new);
    let mut meta_rows = use_signal(Vec::<(String, String)>::new);
    let selected = use_signal(|| None::<pnex_core::PredefinedDevice>);
    let mut model_search = use_signal(String::new);
    // Paramètres de build (étape WiFi) — secrets jamais persistés, ils ne
    // transitent que la queue de build.
    let mut ssid = use_signal(String::new);
    let mut wifi_password = use_signal(String::new);
    let mut host = use_signal(default_host);
    // wss (TLS, industriel) ou ws (local) — défaut selon le protocole de la
    // page, l'utilisateur peut inverser.
    let mut ws_ssl = use_signal(crate::util::default_ws_ssl);
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
    let copied_token = use_signal(|| false);
    let copied_key = use_signal(|| false);
    let copied_script = use_signal(|| false);

    let catalogue = use_resource(|| async move { api::devices::predefined_devices().await });

    let custom = selected().as_ref().is_some_and(|p| is_custom(&p.name));

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
        if meta_rows()
            .iter()
            .any(|(k, v)| k.trim().is_empty() && !v.trim().is_empty())
        {
            toasts::error("wizard-metadata-key-required");
            return;
        }
        step.set(Step::Model);
    };
    let go_identity = move |_| step.set(Step::Identity);
    let go_config = move |_| {
        if selected().is_none() {
            toasts::error("devices-model-required");
            return;
        }
        step.set(Step::Config);
    };
    let go_model_back = move |_| step.set(Step::Model);
    let go_review = move |_| {
        if ssid().trim().is_empty()
            || wifi_password().is_empty()
            || host().trim().is_empty()
        {
            toasts::error("wizard-config-incomplete");
            return;
        }
        step.set(Step::Review);
    };

    // ── Création (+ build auto pour les traditionnels) ──
    let submit = move |_| {
        let Some(model) = selected() else { return };
        let id = device_id().trim().to_string();
        let metadata = rows_to_json(&meta_rows());
        creating.set(true);
        spawn(async move {
            let outcome = api::devices::create(pnex_core::CreateDevice {
                device_id: id.clone(),
                predefined_device_name: model.name.clone(),
                metadata,
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
                            created.set(Some(device));
                            on_changed.call(());
                            if is_custom(&model.name) {
                                step.set(Step::CustomDone);
                            } else {
                                // Build automatique, suivi dans la modale —
                                // les erreurs (403 quota, 429 intervalle) sont
                                // affichées sans casser l'écran token.
                                build_record.set(None);
                                step.set(Step::BuildProgress);
                                let params = pnex_core::CreateBuild {
                                    device_id: id,
                                    predefined_device_name: model.name.clone(),
                                    wifi_ssid: ssid().trim().to_string(),
                                    wifi_password: wifi_password(),
                                    pnex_host: host().trim().to_string(),
                                    ws_ssl: ws_ssl(),
                                };
                                if let Err(err) = api::builds::create(params).await {
                                    build_launch_error.set(Some(err.message));
                                }
                            }
                        }
                        _ => toasts::success("devices-created"),
                    }
                }
                Err(err) => toasts::error(err.message),
            }
        });
    };

    // ── Polling du build tant qu'il vole (écran BuildProgress) ──
    if step() == Step::BuildProgress && build_launch_error().is_none() {
        let in_flight = match build_record() {
            None => true,
            Some(record) => {
                !matches!(record.build_phase.as_deref(), Some("succeeded") | Some("failed"))
            }
        };
        if in_flight && !polling() {
            polling.set(true);
            let polled_id = created()
                .map(|device| device.device_id)
                .unwrap_or_default();
            spawn(async move {
                sleep(Duration::from_secs(5)).await;
                polling.set(false);
                // Un record par (org, device_id) : limit=1 = build courant.
                let filters = api::builds::BuildFilters {
                    device_id: Some(polled_id),
                    success: None,
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

    // ── Copier (token / clé / script) avec retour « Copié » 2 s ──
    let copy = |text: String, mut flag: Signal<bool>| {
        copy_text(&text);
        flag.set(true);
        spawn(async move {
            sleep(Duration::from_secs(2)).await;
            flag.set(false);
        });
    };
    let token_info = created().and_then(|device| device.device_token);
    let token_for_copy = token_info.clone();
    let copy_token = move |_| {
        if let Some(token) = token_for_copy.clone() {
            copy(token.token, copied_token);
        }
    };
    let key_for_copy = token_info.clone();
    let copy_key = move |_| {
        if let Some(key) = key_for_copy.clone().and_then(|token| token.encryption_key) {
            copy(key, copied_key);
        }
    };
    let python_script = created().map(|device| {
        let token = device
            .device_token
            .map(|t| t.token)
            .unwrap_or_default();
        PY_TEMPLATE
            .replace("__TOKEN__", &token)
            .replace("__DEVICE_ID__", &device.device_id)
            .replace("__PRED_DEV__", &device.predefined_device_name)
            .replace("__WS_URL__", &ws_ingest_url())
    });
    let script_for_copy = python_script.clone();
    let copy_script = move |_| {
        if let Some(script) = script_for_copy.clone() {
            copy(script, copied_script);
        }
    };

    // Téléchargement du binaire réussi.
    let download = move |_| {
        let Some(device) = created() else { return };
        let dev_id = device.device_id;
        spawn(async move {
            match api::builds::download(&dev_id).await {
                Ok(bytes) => save_blob(&format!("{dev_id}-firmware.bin"), &bytes),
                Err(err) => toasts::error(err.message),
            }
        });
    };

    // Cartes du catalogue filtrées par la recherche.
    let visible_models: Vec<pnex_core::PredefinedDevice> = match &*catalogue.read() {
        Some(Ok(models)) => {
            let term = model_search().trim().to_lowercase();
            models
                .iter()
                .filter(|pd| term.is_empty() || model_matches(pd, &term))
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    };
    let custom_models: Vec<pnex_core::PredefinedDevice> =
        visible_models.iter().filter(|pd| is_custom(&pd.name)).cloned().collect();
    let traditional_models: Vec<pnex_core::PredefinedDevice> = visible_models
        .iter()
        .filter(|pd| !is_custom(&pd.name))
        .cloned()
        .collect();

    let busy = creating();

    rsx! {
        Modal {
            title: t!("devices-register-title"),
            max_width: "max-w-2xl".to_string(),
            on_close,
            div { class: "space-y-5",
                if !matches!(step(), Step::CustomDone | Step::BuildProgress | Step::Reactivated) {
                    {stepper(step(), custom)}
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
                                div { class: "flex items-center justify-between mb-2",
                                    span { class: "text-xs font-semibold text-gray-500 uppercase tracking-wider",
                                        {t!("wizard-metadata-title")}
                                    }
                                    button {
                                        class: "text-sm text-blue-600 hover:text-blue-700",
                                        r#type: "button",
                                        onclick: move |_| meta_rows.with_mut(|rows| rows.push((String::new(), String::new()))),
                                        icons::Plus { class: "h-4 w-4 inline mr-1" }
                                        {t!("wizard-metadata-add")}
                                    }
                                }
                                for (index, row) in meta_rows().iter().enumerate() {
                                    div { class: "flex gap-2 mb-2", key: "{index}",
                                        input {
                                            class: "w-1/3 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                            r#type: "text",
                                            placeholder: t!("wizard-metadata-key"),
                                            value: "{row.0}",
                                            oninput: move |event| {
                                                meta_rows.with_mut(|rows| rows[index].0 = event.value());
                                            },
                                        }
                                        input {
                                            class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                            r#type: "text",
                                            placeholder: t!("wizard-metadata-value"),
                                            value: "{row.1}",
                                            oninput: move |event| {
                                                meta_rows.with_mut(|rows| rows[index].1 = event.value());
                                            },
                                        }
                                        button {
                                            class: "px-2 text-gray-400 hover:text-red-600 transition-colors",
                                            r#type: "button",
                                            onclick: move |_| meta_rows.with_mut(|rows| { rows.remove(index); }),
                                            icons::Trash2 { class: "h-4 w-4" }
                                        }
                                    }
                                }
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

                            if !custom_models.is_empty() {
                                div {
                                    h4 { class: "text-xs font-semibold text-amber-600 uppercase tracking-wider mb-2",
                                        icons::Zap { class: "h-3.5 w-3.5 inline mr-1" }
                                        {t!("wizard-model-section-custom")}
                                    }
                                    div { class: "grid gap-2 mb-4",
                                        for pd in custom_models {
                                            {model_card(pd, selected)}
                                        }
                                    }
                                }
                            }

                            if !traditional_models.is_empty() {
                                div {
                                    h4 { class: "text-xs font-semibold text-blue-600 uppercase tracking-wider mb-2",
                                        {t!("wizard-model-section-traditional")}
                                    }
                                    div { class: "grid gap-2",
                                        for pd in traditional_models {
                                            {model_card(pd, selected)}
                                        }
                                    }
                                }
                            }

                            if visible_models.is_empty() {
                                p { class: "text-sm text-gray-400 text-center py-6", {t!("wizard-model-none")} }
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
                                    onclick: go_config,
                                    {t!("wizard-next")}
                                }
                            }
                        }
                    },

                    // ── Étape 3 (custom) : revue — pas de WiFi ──
                    Step::Config if custom => rsx! {
                        {review_panel(&device_id(), &selected(), &meta_rows(), None, None, false, false)}
                        p { class: "text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded-lg p-3",
                            {t!("wizard-custom-review-note")}
                        }
                        div { class: "flex justify-between pt-2",
                            button {
                                class: "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors",
                                r#type: "button",
                                onclick: go_model_back,
                                {t!("wizard-back")}
                            }
                            button {
                                class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                                r#type: "button",
                                disabled: busy,
                                onclick: submit,
                                if busy { {t!("common-loading")} } else { {t!("wizard-create")} }
                            }
                        }
                    },

                    // ── Étape 3 (traditionnel) : WiFi ──
                    Step::Config => rsx! {
                        div { class: "space-y-4",
                            div { class: "grid gap-3 sm:grid-cols-2",
                                label { class: "block",
                                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("builds-field-ssid")} }
                                    input {
                                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                        r#type: "text",
                                        value: "{ssid}",
                                        oninput: move |event| ssid.set(event.value()),
                                    }
                                }
                                label { class: "block",
                                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("builds-field-wifi-password")} }
                                    input {
                                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                        r#type: "password",
                                        value: "{wifi_password}",
                                        oninput: move |event| wifi_password.set(event.value()),
                                    }
                                }
                                label { class: "block sm:col-span-2",
                                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("builds-field-server")} }
                                    input {
                                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                        r#type: "text",
                                        placeholder: "dev1.pnex.io",
                                        value: "{host}",
                                        oninput: move |event| host.set(event.value()),
                                    }
                                }
                                label { class: "flex items-start gap-2 sm:col-span-2 select-none" ,
                                    input {
                                        class: "mt-0.5 h-4 w-4 accent-blue-600",
                                        r#type: "checkbox",
                                        checked: ws_ssl(),
                                        onchange: move |event| ws_ssl.set(event.checked()),
                                    }
                                    span {
                                        span { class: "text-xs font-medium text-gray-500 block", {t!("builds-field-ws-ssl")} }
                                        span { class: "text-xs text-gray-400", {t!("builds-field-ws-ssl-help")} }
                                    }
                                }
                            }
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
                    Step::Review => rsx! {
                        {review_panel(&device_id(), &selected(), &meta_rows(), Some(ssid().as_str()), Some(host().as_str()), ws_ssl(), true)}
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
                                if busy { {t!("common-loading")} } else { {t!("wizard-create-build")} }
                            }
                        }
                    },

                    // ── Custom créé : token + snippet Python ──
                    Step::CustomDone => rsx! {
                        div { class: "space-y-4",
                            {token_warning()}
                            {token_blocks(&token_info, copied_token(), copied_key(), copy_token, copy_key)}
                            div {
                                div { class: "flex items-center justify-between mb-2",
                                    h4 { class: "text-sm font-semibold text-gray-900", {t!("wizard-script-title")} }
                                    button {
                                        class: "text-sm text-blue-600 hover:text-blue-700",
                                        r#type: "button",
                                        onclick: copy_script,
                                        if copied_script() { {t!("wizard-copied")} } else { {t!("wizard-copy")} }
                                    }
                                }
                                pre { class: "p-3 bg-gray-900 text-gray-100 rounded-lg text-xs overflow-x-auto max-h-72",
                                    if let Some(script) = &python_script {
                                        {script.clone()}
                                    }
                                }
                            }
                            div { class: "flex justify-end",
                                button {
                                    class: "px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors text-sm font-semibold",
                                    r#type: "button",
                                    onclick: move |_| on_close.call(()),
                                    {t!("common-close")}
                                }
                            }
                        }
                    },

                    // ── Traditionnel créé : build en cours dans la modale ──
                    Step::BuildProgress => rsx! {
                        div { class: "space-y-4",
                            {token_blocks(&token_info, copied_token(), copied_key(), copy_token, copy_key)}
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
                                                span { class: phase_badge(record.build_phase.as_deref()).0,
                                                    {phase_badge(record.build_phase.as_deref()).1}
                                                }
                                                span { class: "text-xs text-gray-400", {date_label(&record.updated_at)} }
                                            }
                                            if record.success {
                                                div { class: "space-y-2",
                                                    button {
                                                        class: "w-full px-4 py-2 bg-indigo-600 text-white rounded-lg hover:bg-indigo-700 transition-colors text-sm font-medium",
                                                        r#type: "button",
                                                        onclick: download,
                                                        icons::Download { class: "h-4 w-4 inline mr-1" }
                                                        {t!("builds-download")}
                                                    }
                                                    // Flash direct en Web Serial (Chromium —
                                                    // le modal avertit sinon).
                                                    button {
                                                        class: "w-full px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors text-sm font-medium",
                                                        r#type: "button",
                                                        onclick: move |_| flash_open.set(true),
                                                        icons::Zap { class: "h-4 w-4 inline mr-1" }
                                                        {t!("devices-flash")}
                                                    }
                                                }
                                            } else if record.build_phase.as_deref() == Some("failed") {
                                                div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                                    {t!("wizard-build-failed")}
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

/// Stepper numéroté — 3 items pour un modèle custom, 4 sinon.
fn stepper(current: Step, custom: bool) -> Element {
    let items: Vec<(Step, &'static str)> = if custom {
        vec![
            (Step::Identity, "wizard-step-identity"),
            (Step::Model, "wizard-step-model"),
            (Step::Config, "wizard-step-review"),
        ]
    } else {
        vec![
            (Step::Identity, "wizard-step-identity"),
            (Step::Model, "wizard-step-model"),
            (Step::Config, "wizard-step-wifi"),
            (Step::Review, "wizard-step-review"),
        ]
    };
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
                    span {
                        class: if index <= current_index {
                            "flex items-center justify-center w-7 h-7 rounded-full text-xs font-semibold bg-blue-600 text-white"
                        } else {
                            "flex items-center justify-center w-7 h-7 rounded-full text-xs font-semibold bg-gray-200 text-gray-500"
                        },
                        "{index + 1}"
                    }
                    span {
                        class: if index <= current_index {
                            "text-xs font-medium text-blue-700"
                        } else {
                            "text-xs text-gray-400"
                        },
                        {t!(*key)}
                    }
                }
            }
        }
    }
}

/// Carte de modèle du catalogue — highlight si sélectionnée.
fn model_card(
    pd: pnex_core::PredefinedDevice,
    mut selected: Signal<Option<pnex_core::PredefinedDevice>>,
) -> Element {
    let name = pd.name.clone();
    let is_selected = selected().as_ref().is_some_and(|s| s.name == name);
    let custom = is_custom(&pd.name);
    let (border, chip) = if custom {
        ("border-amber-300 bg-amber-50", "bg-amber-100 text-amber-800")
    } else {
        ("border-blue-200 bg-white", "bg-blue-100 text-blue-800")
    };
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
            onclick: move |_| selected.set(Some(pd.clone())),
            div { class: "flex items-center justify-between",
                span { class: "text-sm font-semibold text-gray-900", {pretty} }
                if check {
                    icons::Check { class: "h-4 w-4 text-blue-600" }
                } else if custom {
                    icons::Zap { class: "h-4 w-4 text-amber-500" }
                }
            }
            if let Some(description) = pd.description.as_deref().filter(|d| !d.is_empty()) {
                p { class: "text-xs text-gray-500 mt-1", {description} }
            }
            div { class: "flex flex-wrap gap-1 mt-2",
                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium {chip}", {pd.board.clone()} }
                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-gray-100 text-gray-700", {pd.device_type.clone()} }
                for cap in shown_caps {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-gray-100 text-gray-600", {cap} }
                }
                if hidden_caps > 0 {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-gray-100 text-gray-400", "+{hidden_caps}" }
                }
            }
        }
    }
}

/// Panneau de revue (custom étape 3 / traditionnel étape 4).
#[allow(clippy::too_many_arguments)]
fn review_panel(
    device_id: &str,
    selected: &Option<pnex_core::PredefinedDevice>,
    rows: &[(String, String)],
    ssid: Option<&str>,
    host: Option<&str>,
    ws_ssl: bool,
    with_build: bool,
) -> Element {
    let model = selected
        .as_ref()
        .map(|pd| pd.pretty_name.clone().unwrap_or_else(|| pd.name.clone()))
        .unwrap_or_default();
    let custom = selected.as_ref().is_some_and(|pd| is_custom(&pd.name));
    let metadata = rows_to_json(rows)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".into());
    rsx! {
        div { class: "space-y-3 text-sm",
            div { class: "flex justify-between border-b border-gray-100 pb-2",
                span { class: "text-gray-500", {t!("devices-col-id")} }
                code { class: "font-medium text-gray-900", {device_id} }
            }
            div { class: "flex justify-between border-b border-gray-100 pb-2",
                span { class: "text-gray-500", {t!("devices-col-model")} }
                span { class: "font-medium text-gray-900",
                    {model}
                    if custom {
                        span { class: "ml-2 inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-amber-100 text-amber-800",
                            {t!("wizard-model-section-custom")}
                        }
                    }
                }
            }
            div { class: "flex justify-between gap-4 border-b border-gray-100 pb-2",
                span { class: "text-gray-500 shrink-0", {t!("devices-metadata")} }
                code { class: "text-gray-900 break-all text-right", {metadata} }
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
                        {format!("{}://{}", if ws_ssl { "wss" } else { "ws" }, host.unwrap_or_default())}
                    }
                }
            }
        }
    }
}

/// Avertissement « sauvegardez le token maintenant » (affichage unique).
fn token_warning() -> Element {
    rsx! {
        div { class: "flex items-start gap-3 bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-800",
            icons::AlertTriangle { class: "h-5 w-5 shrink-0 animate-pulse" }
            {t!("wizard-token-warning")}
        }
    }
}

/// Blocs token + clé de chiffrement avec boutons copier — réutilisés par
/// l'écran custom et l'écran de suivi build.
fn token_blocks(
    token_info: &Option<pnex_core::DeviceTokenInfo>,
    copied_token: bool,
    copied_key: bool,
    copy_token: impl FnMut(Event<MouseData>) + 'static,
    copy_key: impl FnMut(Event<MouseData>) + 'static,
) -> Element {
    match token_info {
        None => rsx! {},
        Some(token) => rsx! {
            div { class: "space-y-3",
                div {
                    div { class: "flex items-center justify-between mb-1",
                        span { class: "text-xs text-gray-500", {t!("devices-token-value")} }
                        button {
                            class: "text-sm text-blue-600 hover:text-blue-700",
                            r#type: "button",
                            onclick: copy_token,
                            if copied_token { {t!("wizard-copied")} } else { {t!("wizard-copy")} }
                        }
                    }
                    code { class: "block p-3 bg-gray-50 rounded-lg text-sm break-all", {token.token.clone()} }
                }
                div {
                    div { class: "flex items-center justify-between mb-1",
                        span { class: "text-xs text-gray-500", {t!("devices-encryption-key")} }
                        button {
                            class: "text-sm text-blue-600 hover:text-blue-700",
                            r#type: "button",
                            onclick: copy_key,
                            if copied_key { {t!("wizard-copied")} } else { {t!("wizard-copy")} }
                        }
                    }
                    code { class: "block p-3 bg-gray-50 rounded-lg text-sm break-all",
                        {token.encryption_key.clone().unwrap_or_else(|| "—".into())}
                    }
                }
            }
        },
    }
}
