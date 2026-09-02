//! Canal device bidirectionnel — WS `/ws/device` (Brick 0, brick0.md §3).
//!
//! Framing identique à `/ws/sensor/ingest` (auth query b64, frames texte
//! `base64(nonce‖ChaCha20-nu)`, PING/PONG) ; messages métier JSON tagué `t`
//! (`pnex_core::proto` — miroir firmware C++). Sémantique RPC à la
//! ThingsBoard : toute commande porte un `cmd_id`, le device répond `Ack`.
//!
//! - `Announce` → `services::provisioning::admit` (policy Validated) →
//!   `ProvisionAck` avec la pin map complète ;
//! - `StateReport` → sortie metrics OpenObserve (même chemin que l'ingest,
//!   séries `<label normalisé>{device_id, pred_dev, source_type=generic_gpio}`) ;
//! - downlink : registre `DEVICE_SESSIONS` (mpsc par device) où poussent les
//!   commandes REST (`controllers/pins.rs`) ; le loop `select` interleaving
//!   uplink/downlink ;
//! - anti-clone : mêmes mécanismes que l'ingest (4003 immédiat en-process,
//!   fallback PG frais, close codes 4001/4002/4003/4005/4006/4008).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use base64::Engine as _;
use axum::response::Response;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use tokio::sync::mpsc;

use super::ws_ingest::{decode_param, decrypt_frame, encrypt_frame, reject, Snapshot};
use crate::models::_entities::{device_capability_instances, device_registries};
use crate::services::{device_liveness, provisioning};
use crate::services::settings::IngestSettings;
use crate::services::telemetry::{self, TelemetryPoint};
use pnex_core::{DeviceMsg, Mode, SafeState, ServerMsg};

// ─────────────── Registres de session (downlink + last values) ───────────────

/// Sessions device ouvertes dans CE process : device_registry_id → canal
/// downlink. L'entrée est posée AVANT le spawn du loop et retirée à sa
/// sortie — les commandes REST poussent dedans (409 si absent = offline).
static DEVICE_SESSIONS: LazyLock<Mutex<HashMap<i64, mpsc::UnboundedSender<ServerMsg>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Dernières valeurs rapportées par gpio (mémoire de session) — alimente
/// GET /pins (« — » si offline). Vidées à la sortie de session.
pub(crate) static LAST_VALUES: LazyLock<Mutex<HashMap<i64, HashMap<i32, serde_json::Value>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Watchdog d'inactivité d'une session device : AUCUNE frame reçue pendant
/// cette durée → la session est fermée (garde libéré, reconnexion
/// possible). Le firmware pingue toutes les 5 s quand il est vivant ; 45 s
/// de silence = pair mort (power cycle, reflash — la carte meurt sans
/// close TCP). Sans lui, la tâche reste parkée sur `socket.recv()` à jamais
/// (TCP half-open) : l'entrée `DEVICE_SESSIONS` rejette alors toute
/// reconnexion en 4003 « Device already connected » — une carte reflashée
/// restait verrouillée dehors jusqu'au redémarrage du serveur (leçon
/// 2026-09-02).
const DEVICE_WATCHDOG_SECS: u64 = 45;

/// Retire le device des registres à la sortie, tous chemins compris.
struct DeviceSessionGuard(i64);

impl Drop for DeviceSessionGuard {
    fn drop(&mut self) {
        DEVICE_SESSIONS.lock().expect("sessions").remove(&self.0);
        LAST_VALUES.lock().expect("last_values").remove(&self.0);
    }
}

// ─────────────────────────── Snapshot device ───────────────────────────

/// État validé du device pour la session : identité + carte label/gpio +
/// contraintes validées par pin (rafraîchi à chaque `Announce`).
pub(crate) struct DeviceSnapshot {
    pub(crate) device_registry_id: i64,
    pub(crate) org_id: i64,
    pub(crate) device_id: String,
    pub(crate) pred_dev: String,
    /// gpio → label overlay (dénormalisé pour l'affichage /pins).
    pub(crate) labels: HashMap<i32, String>,
    /// gpio → contraintes validées (mode courant, safe_state, pullup).
    pub(crate) pins: HashMap<i32, pnex_core::ValidatedPin>,
}

impl DeviceSnapshot {
    /// Recharge les instances persistées après admission (ou re-announce).
    async fn reload_pins(&mut self, db: &DatabaseConnection) {
        self.pins = load_pins(db, self.device_registry_id).await;
    }
}

// ─────────────────────────────── Handler ───────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DeviceQuery {
    token: Option<String>,
    device_id: Option<String>,
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/ws")
        .add("/device", get(ws_device))
}

async fn ws_device(
    State(ctx): State<AppContext>,
    Query(q): Query<DeviceQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let settings = IngestSettings::from_config(&ctx.config);
    // Auth (ordre ingest : 4002 → 4001 → 4006 → 4008 → 4003).
    let Some(raw_token) = q.token.as_deref() else {
        return reject(ws, 4002, "No token provided");
    };
    let token = match decode_param(raw_token) {
        Some(t) if !t.is_empty() => t,
        _ => return reject(ws, 4001, "Authentication failed"),
    };
    let device_id = match q.device_id.as_deref().map(decode_param) {
        Some(Some(d)) if !d.is_empty() => d,
        _ => return reject(ws, 4006, "Token device mismatch"),
    };

    let (tok, device) = match Snapshot::load(&ctx.db, &token).await {
        Ok(Some(found)) => found,
        _ => return reject(ws, 4001, "Authentication failed"),
    };
    if device.device_id != device_id {
        return reject(ws, 4006, "Token/device mismatch");
    }
    let Some(key) = tok
        .encryption_key
        .as_deref()
        .and_then(|k| base64::engine::general_purpose::STANDARD.decode(k.trim()).ok())
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
    else {
        return reject(ws, 4008, "No encryption key");
    };

    // Admission préalable : /ws/device est réservé aux devices avec overlay
    // (génériques). Un device compilé (soil_sensor…) n'a rien à y faire.
    let pred = device.predefined_device_id;
    let _ = pred;
    let snap = match build_snapshot(&ctx.db, &device).await {
        Ok(s) => s,
        Err(_) => return reject(ws, 4007, "Not a generic device"),
    };

    // Anti-clone étage 1 : session ouverte en-process → 4003 immédiat.
    // Le canal downlink est créé ici et inséré dans le registre AVANT
    // l'upgrade — une commande REST arrivant pendant l'upgrade est donc
    // déjà routable (elle attendra le loop select).
    let (downlink_tx, downlink_rx) = mpsc::unbounded_channel::<ServerMsg>();
    {
        let mut open = DEVICE_SESSIONS.lock().expect("sessions");
        if open.contains_key(&snap.device_registry_id) {
            return reject(ws, 4003, "Device already connected");
        }
        open.insert(snap.device_registry_id, downlink_tx);
    }
    // Le guard (nettoyage des registres à la sortie) est créé IMMÉDIATEMENT
    // après l'insertion — un rejet 4003 du fallback PG ci-dessous doit
    // laisser les registres propres.
    let guard = DeviceSessionGuard(snap.device_registry_id);
    // Anti-clone étage 2 : fallback PG — last_seen frais d'une session non
    // refermée (crash sans close, ou autre réplica).
    if let Ok(Some(state)) = device_liveness::state_of(&ctx.db, snap.device_registry_id).await {
        if state.connected
            && device_liveness::is_fresh(
                state.last_seen_at.with_timezone(&chrono::Utc),
                settings.silence_ttl_secs,
            )
        {
            return reject(ws, 4003, "Device already connected");
        }
    }

    // Prise de bail : le device est vu maintenant (le reaper désactivera
    // après TTL de silence, seul écrivain de `active` — parité ingest).
    let _ = device_liveness::touch(&ctx.db, snap.device_registry_id, Some(true)).await;
    let token_owned = token;
    let settings_owned = settings;
    ws.on_upgrade(move |socket| async move {
        session_loop(socket, ctx, token_owned, key, snap, downlink_rx, guard, settings_owned).await;
    })
    .into_response()
}

// ───────────────────────────── Boucle de session ─────────────────────────────

/// Boucle de session : `select` entre uplink (frames device) et downlink
/// (commandes REST poussées dans `DEVICE_SESSIONS`). Sortie = déconnexion
/// (guard drop → registres nettoyés + liveness release).
#[allow(clippy::too_many_arguments)]
async fn session_loop(
    mut socket: WebSocket,
    ctx: AppContext,
    token: String,
    key: [u8; 32],
    mut snap: DeviceSnapshot,
    mut downlink: mpsc::UnboundedReceiver<ServerMsg>,
    guard: DeviceSessionGuard,
    settings: IngestSettings,
) {
    let cache = Duration::from_secs(settings.token_cache_secs);
    let throttle = Duration::from_secs(1);
    let mut last_validation = Instant::now();
    let mut last_touch = Instant::now();
    let mut announced = false;
    loop {
        tokio::select! {
            // ── Downlink : commande REST à pousser (chiffrée) ──
            Some(msg) = downlink.recv() => {
                let plain = match serde_json::to_string(&msg) {
                    Ok(p) => p,
                    Err(e) => { tracing::warn!(device = %snap.device_id, "cmd non sérialisable : {e}"); continue; }
                };
                let _ = socket.send(Message::Text(encrypt_frame(&plain, &key).into())).await;
            }
            // ── Uplink : frame du device, sous watchdog d'inactivité (une
            // tâche parkée sur un TCP half-open ne meurt jamais seule) ──
            incoming = tokio::time::timeout(
                Duration::from_secs(DEVICE_WATCHDOG_SECS),
                socket.recv(),
            ) => {
                let incoming = match incoming {
                    Ok(incoming) => incoming,
                    Err(_) => {
                        tracing::warn!(
                            device = %snap.device_id,
                            "session muette > {DEVICE_WATCHDOG_SECS} s — fermeture anti-zombie"
                        );
                        break;
                    }
                };
                let Some(Ok(msg)) = incoming else { break };
                let Message::Text(text) = msg else { continue };
                // Revalidation périodique du token (4005, parité ingest).
                if last_validation.elapsed() >= cache {
                    match revalidate(&ctx.db, &token, &snap).await {
                        Some(fresh) => snap = fresh,
                        None => {
                            let _ = socket.send(Message::Close(Some(CloseFrame {
                                code: 4005, reason: "Token invalid".into(),
                            }))).await;
                            break;
                        }
                    }
                    last_validation = Instant::now();
                }
                let plain = match decrypt_frame(&text, &key) {
                    Some(p) => p,
                    None => { tracing::warn!(device = %snap.device_id, "frame device indéchiffrable"); continue; }
                };
                // PING/PONG au niveau frame (parité ingest) — le firmware
                // pingue toutes les 5 s ; sans réponse il ferme après 15 s
                // (PONG timeout) et boucle reconnexion. Manquait sur /ws/device
                // (leçon 2026-09-02 : sessions de 15 s, device « actif » mais
                // jamais provisionné).
                if plain.trim().eq_ignore_ascii_case("ping") {
                    let _ = socket
                        .send(Message::Text(encrypt_frame("PONG", &key).into()))
                        .await;
                    if last_touch.elapsed() >= throttle {
                        let _ = device_liveness::touch(&ctx.db, snap.device_registry_id, None).await;
                        last_touch = Instant::now();
                    }
                    continue;
                }
                match serde_json::from_str::<DeviceMsg>(&plain) {
                    Ok(DeviceMsg::Announce { chip, board, fw }) => {
                        handle_announce(&ctx, &mut socket, &key, &mut snap, &chip, &board, &fw).await;
                        announced = true;
                    }
                    Ok(DeviceMsg::StateReport { gpio, value }) => {
                        handle_state_report(&ctx, &snap, gpio, value).await;
                    }
                    Ok(DeviceMsg::Ack { cmd_id, ok, err }) => {
                        if !ok {
                            tracing::warn!(device = %snap.device_id, cmd = %cmd_id, "commande refusée par le device : {:?}", err);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(device = %snap.device_id, "message non reconnu : {e}");
                    }
                }
                if last_touch.elapsed() >= throttle {
                    let _ = device_liveness::touch(&ctx.db, snap.device_registry_id, None).await;
                    last_touch = Instant::now();
                }
                let _ = announced;
            }
        }
    }
    // Sortie de session : registres (guard) + bail liveness libérés.
    let _ = device_liveness::release(&ctx.db, snap.device_registry_id).await;
    drop(guard);
}

/// Revalidation périodique du token en session (4005 si invalidé).
async fn revalidate(db: &DatabaseConnection, token: &str, snap: &DeviceSnapshot) -> Option<DeviceSnapshot> {
    let (_, device) = Snapshot::load(db, token).await.ok()??;
    if device.device_id != snap.device_id {
        return None;
    }
    build_snapshot(db, &device).await.ok()
}

/// Announce → admission (Validated) → refresh snapshot → ProvisionAck.
/// Un device non générique n'aurait jamais dû arriver ici (4007 au connect).
async fn handle_announce(
    ctx: &AppContext,
    socket: &mut WebSocket,
    key: &[u8; 32],
    snap: &mut DeviceSnapshot,
    chip: &str,
    board: &str,
    fw: &str,
) {
    if chip != "esp8266" {
        send_server_msg(socket, key, &ServerMsg::Reject { reason: "chip non supporté (P0 : esp8266)".into() }).await;
        return;
    }
    tracing::info!(device = %snap.device_id, board, fw, "announce device générique");
    let device = match device_registries::Entity::find_by_id(snap.device_registry_id)
        .one(&ctx.db)
        .await
    {
        Ok(Some(d)) => d,
        _ => {
            send_server_msg(socket, key, &ServerMsg::Reject { reason: "device introuvable".into() }).await;
            return;
        }
    };
    match provisioning::admit(&ctx.db, &device).await {
        Ok(specs) => {
            snap.reload_pins(&ctx.db).await;
            send_server_msg(socket, key, &ServerMsg::ProvisionAck { caps: specs }).await;
        }
        Err(e) => {
            tracing::error!(device = %snap.device_id, "admission refusée : {e}");
            send_server_msg(socket, key, &ServerMsg::Reject {
                reason: "admission refusée : overlay board absent ou invalide".into(),
            }).await;
        }
    }
}

/// StateReport → mémoire last_values (GET /pins) + sortie metrics O2
/// (série = label normalisé D16, même sortie que l'ingest).
async fn handle_state_report(_ctx: &AppContext, snap: &DeviceSnapshot, gpio: u16, value: serde_json::Value) {
    let Some(pin) = snap.pins.get(&(gpio as i32)) else {
        tracing::debug!(device = %snap.device_id, gpio, "StateReport pour un pin inconnu — ignoré");
        return;
    };
    let label = snap.labels.get(&(gpio as i32)).cloned().unwrap_or_else(|| gpio.to_string());
    let name = super::ws_ingest::normalize_measurement_name(&label);
    if name.is_empty() {
        return;
    }
    {
        let mut lv = LAST_VALUES.lock().expect("last_values");
        lv.entry(snap.device_registry_id).or_default().insert(gpio as i32, value.clone());
    }
    telemetry::sink().send(TelemetryPoint {
        org_id: snap.org_id,
        device_registry_id: snap.device_registry_id,
        device_id: snap.device_id.clone(),
        pred_dev: snap.pred_dev.clone(),
        metric_name: name,
        value: value.to_string(),
        timestamp: chrono::Utc::now(),
        ts_source: "server",
        source_type: "generic_gpio",
    });
    let _ = pin;
}

/// Envoi serveur → device (chiffré).
async fn send_server_msg(socket: &mut WebSocket, key: &[u8; 32], msg: &ServerMsg) {
    let Ok(plain) = serde_json::to_string(msg) else { return };
    let _ = socket.send(Message::Text(encrypt_frame(&plain, key).into())).await;
}

/// Snapshot device complet : identité + carte gpio→label (overlay) +
/// contraintes par pin (instances persistées). Le parse d'overlay échoue
/// pour un device non générique → close 4007 au connect.
async fn build_snapshot(db: &DatabaseConnection, device: &device_registries::Model) -> Result<DeviceSnapshot> {
    let overlay = provisioning::load_overlay(db, device).await?;
    let labels: HashMap<i32, String> = overlay
        .pins
        .iter()
        .map(|p| (p.gpio as i32, p.label.clone()))
        .collect();
    let snap = DeviceSnapshot {
        device_registry_id: device.id,
        org_id: device.org_id,
        device_id: device.device_id.clone(),
        pred_dev: overlay.board.clone(),
        labels,
        pins: load_pins(db, device.id).await,
    };
    Ok(snap)
}

/// Instances persistées → carte gpio → contraintes validées (source de
/// vérité : la base, remplie à l'admission et par les SetMode REST).
async fn load_pins(db: &DatabaseConnection, device_registry_id: i64) -> HashMap<i32, pnex_core::ValidatedPin> {
    let rows = device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(device_registry_id))
        .all(db)
        .await
        .unwrap_or_default();
    rows.iter()
        .map(|r| {
            let cfg: pnex_core::ModeOpts = r
                .config
                .as_ref()
                .and_then(|c| serde_json::from_value(c.clone()).ok())
                .unwrap_or_default();
            (
                r.gpio,
                pnex_core::ValidatedPin {
                    gpio: r.gpio as u16,
                    mode: str_to_mode(&r.mode),
                    pullup: cfg.pullup.unwrap_or(false),
                    safe_state: cfg.safe_state.unwrap_or(SafeState::Low),
                },
            )
        })
        .collect()
}

/// Mode fil (snake_case) → Mode code.
fn str_to_mode(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "analog_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}

/// Pousser une commande au device connecté (mpsc, consommée par la boucle
/// de session qui chiffre et envoie). `false` = pas de session (offline).
pub(crate) fn push_command(device_registry_id: i64, msg: ServerMsg) -> bool {
    match DEVICE_SESSIONS.lock().expect("sessions").get(&device_registry_id) {
        Some(tx) => tx.send(msg).is_ok(),
        None => false,
    }
}

/// Session vivante pour ce device ?
pub(crate) fn is_connected(device_registry_id: i64) -> bool {
    DEVICE_SESSIONS.lock().expect("sessions").contains_key(&device_registry_id)
}
