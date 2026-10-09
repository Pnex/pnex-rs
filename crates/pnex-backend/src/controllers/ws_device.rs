//! Canal device bidirectionnel — WS `/ws/device` (Brick 0, brick0.md §3).
//!
//! Auth and framing from `device_link` (`authenticate_device`, Noise
//! NNpsk0 transport frames), PING/PONG; messages métier JSON tagué `t`
//! (`pnex_core::proto` — miroir firmware C++). Sémantique RPC à la
//! ThingsBoard : toute commande porte un `cmd_id`, le device répond `Ack`.
//!
//! - `Announce` → `services::provisioning::admit` (policy Validated) →
//!   `ProvisionAck` avec la pin map complète ;
//! - `StateReport` → OpenObserve metrics (telemetry sink,
//!   series `<label normalisé>{device_id, pred_dev, source_type=generic_gpio}`) ;
//! - downlink : registre `DEVICE_SESSIONS` (mpsc par device) où poussent les
//!   commandes REST (`controllers/pins.rs`) ; le loop `select` interleaving
//!   uplink/downlink ;
//! - anti-clone: Valkey lease (4003 while a live session holds it),
//!   close codes 4001/4002/4003/4005/4011/4013/4014).

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tokio::sync::mpsc;

use super::device_link::{active_token, close_socket, reject, FrameCodec};
use crate::models::_entities::{
    device_capability_instances, device_registries, predefined_devices,
};
use crate::services::settings::IngestSettings;
use crate::services::telemetry::{self, TelemetryPoint};
use crate::services::{device_liveness, provisioning};
use pnex_core::{DeviceMsg, Mode, SafeState, ServerMsg};

// ─────────────── Registres de session (downlink + last values) ───────────────

/// Sessions device ouvertes dans CE process : device_registry_id → canal
/// downlink. L'entrée est posée AVANT le spawn du loop et retirée à sa
/// sortie — les commandes REST poussent dedans (409 si absent = offline).
static DEVICE_SESSIONS: LazyLock<Mutex<HashMap<i64, LocalSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One local device session: its lease/route session id and downlink.
/// Replacing the entry (a newer session admitted on this pod while the old
/// one is half-open) drops the old sender: the old loop sees its downlink
/// closed and ends.
struct LocalSession {
    session: String,
    tx: mpsc::UnboundedSender<Downlink>,
}

/// One downlink item: a message built on this pod, or an already
/// serialized one relayed by the cross-pod device bus (D107).
pub(crate) enum Downlink {
    Msg(ServerMsg),
    Json(String),
}

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
/// Registry cleanup of one device session (id, session id): only the
/// entries still owned by this session are removed — a newer session of
/// the same device admitted meanwhile keeps its registrations.
struct DeviceSessionGuard(i64, String);

impl Drop for DeviceSessionGuard {
    fn drop(&mut self) {
        let mine = {
            let mut open = DEVICE_SESSIONS.lock().expect("sessions");
            let mine = open.get(&self.0).is_some_and(|s| s.session == self.1);
            if mine {
                open.remove(&self.0);
            }
            mine
        };
        if mine {
            LAST_VALUES.lock().expect("last_values").remove(&self.0);
        }
        // Cross-pod route (D107): released by this session only.
        let (id, session) = (self.0, self.1.clone());
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { crate::services::device_bus::release(id, &session).await });
        }
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
    /// gpio → cadence de lecture persistée (0 = pas de subscribe) —
    /// re-poussée après chaque `Announce` (restauration du desired-state).
    pub(crate) intervals: HashMap<i32, u32>,
    /// Custom metric ids (`metric` caps) of the session's announce (D87) —
    /// a gpio-less StateReport must name one of them.
    pub(crate) metrics: HashSet<String>,
}

impl DeviceSnapshot {
    /// Recharge les instances persistées après admission (ou re-announce).
    async fn reload_pins(&mut self, db: &DatabaseConnection) {
        let (pins, intervals) = load_pins(db, self.device_registry_id).await;
        self.pins = pins;
        self.intervals = intervals;
    }
}

// ─────────────────────────────── Handler ───────────────────────────────

pub fn routes() -> Routes {
    Routes::new().prefix("/ws").add("/device", get(ws_device))
}

async fn ws_device(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let settings = IngestSettings::from_config(&ctx.config);
    if !super::device_link::arrived_over_tls(&headers, &settings) {
        return reject(ws, super::device_link::CLOSE_TLS_REQUIRED, "TLS required");
    }
    // Reconnect storm guard: bounded concurrent admissions per pod; a
    // device that cannot get a slot in time is told to retry (1013).
    let Some(permit) = super::device_link::admission_permit(&settings).await else {
        return reject(ws, 1013, "Try again later");
    };
    let super::device_link::DeviceAuth { token, device, key } =
        match super::device_link::authenticate_device(&ctx.db, &headers, &settings).await {
            Ok(auth) => auth,
            Err(refusal) => return reject(ws, refusal.code, refusal.reason),
        };
    let device_id = device.device_id.clone();

    // Snapshot at connect: a generic model takes its labels from the board
    // overlay, a custom firmware device from the instances persisted at
    // admission. Neither (no overlay, no custom firmware) → 4007.
    let snap = match build_snapshot(&ctx.db, &device).await {
        Ok(s) => s,
        Err(_) => return reject(ws, 4007, "No device snapshot"),
    };

    drop(permit);
    let token_owned = token;
    let settings_owned = settings;
    ws.on_upgrade(move |mut socket| async move {
        // Handshake FIRST: only a peer that proved it holds the device key
        // takes the anti-clone lease (a stolen token alone cannot keep the
        // real device out).
        let Some(codec) = FrameCodec::accept(&mut socket, &key, &device_id, false).await else {
            return;
        };
        // Anti-clone (cross-pod, atomic): the Valkey lease compare-and-set
        // decides — granted when no live session holds it (released, expired
        // after the silence TTL, or never seen). A live session on any pod →
        // 4003. The reaper remains the sole writer of `active`.
        let session = uuid::Uuid::new_v4().simple().to_string();
        match device_liveness::claim(snap.device_registry_id, &session, settings_owned.silence_ttl_secs).await
        {
            Ok(true) => {}
            Ok(false) => return close_socket(&mut socket, 4003, "Device already connected").await,
            Err(_) => return close_socket(&mut socket, 1013, "Try again later").await,
        }
        // Local registry: the downlink is registered before the loop (a
        // command arriving meanwhile waits for it). A previous local session
        // of the device is necessarily stale (the lease was claimable):
        // replacing it closes its downlink, which ends its loop.
        let (downlink_tx, downlink_rx) = mpsc::unbounded_channel::<Downlink>();
        let superseded = DEVICE_SESSIONS.lock().expect("sessions").insert(
            snap.device_registry_id,
            LocalSession {
                session: session.clone(),
                tx: downlink_tx,
            },
        );
        if superseded.is_some() {
            tracing::warn!(device = %snap.device_id, "stale local session superseded by a new connection");
        }
        // The guard (registry cleanup on exit) is created right after the insert.
        let guard = DeviceSessionGuard(snap.device_registry_id, session);
        // Commands issued on other pods now route here (D107).
        crate::services::device_bus::claim(snap.device_registry_id, &guard.1).await;
        session_loop(
            socket,
            ctx,
            token_owned,
            codec,
            snap,
            downlink_rx,
            guard,
            settings_owned,
        )
        .await;
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
    mut codec: FrameCodec,
    mut snap: DeviceSnapshot,
    mut downlink: mpsc::UnboundedReceiver<Downlink>,
    guard: DeviceSessionGuard,
    settings: IngestSettings,
) {
    let cache = Duration::from_secs(settings.token_cache_secs);
    let rebuild_every = Duration::from_secs(settings.snapshot_rebuild_secs);
    let throttle = settings.liveness_touch_interval();
    let mut last_validation = Instant::now();
    let mut last_rebuild = Instant::now();
    let mut fingerprint: Option<SnapshotFingerprint> = None;
    let mut last_touch = Instant::now();
    let mut last_route = Instant::now();
    let mut ota = OtaCache::default();
    // Actuations awaiting their `Ack` (lost-command tracing).
    let mut acks = crate::services::cmd_acks::PendingAcks::default();
    let mut ack_tick = tokio::time::interval(Duration::from_secs(1));
    ack_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Edge agent session state (D95) — `None` for microcontrollers.
    let mut agent = (snap.pred_dev == pnex_core::EDGE_AGENT_PREDEF).then(|| {
        crate::services::edge_agent::AgentSession::new(
            snap.device_registry_id,
            crate::services::edge_agent::AGENT_DEFAULT_MAX_KEYS,
        )
    });
    loop {
        tokio::select! {
            // ── Downlink : commande REST à pousser (chiffrée) ──
            item = downlink.recv() => {
                let Some(item) = item else {
                    // The registry entry was replaced: a newer session of
                    // this device was admitted, this one is superseded.
                    tracing::info!(device = %snap.device_id, "session superseded — closing");
                    break;
                };
                let plain = match item {
                    Downlink::Json(p) => {
                        if p.contains("\"ota_available\"") {
                            if let Ok(ServerMsg::OtaAvailable { cmd_id, .. }) = serde_json::from_str::<ServerMsg>(&p) {
                                ota.note(cmd_id);
                            }
                        }
                        if p.contains("\"write\"") || p.contains("\"command\"") {
                            if let Ok(msg) = serde_json::from_str::<ServerMsg>(&p) {
                                acks.track(&msg, &p, Instant::now());
                            }
                        }
                        p
                    }
                    Downlink::Msg(msg) => {
                        if let ServerMsg::OtaAvailable { cmd_id, .. } = &msg {
                            ota.note(cmd_id.clone());
                        }
                        match serde_json::to_string(&msg) {
                            Ok(p) => {
                                acks.track(&msg, &p, Instant::now());
                                p
                            }
                            Err(e) => { tracing::warn!(device = %snap.device_id, "unserializable command: {e}"); continue; }
                        }
                    }
                };
                let _ = socket.send(Message::Text(codec.seal(&plain).into())).await;
            }
            // ── Actuations without `Ack`: resend a write once, report the
            // loss otherwise (custom-firmware.md §14.6) ──
            _ = ack_tick.tick(), if !acks.is_empty() => {
                use crate::services::cmd_acks::Overdue;
                for late in acks.overdue(Instant::now()) {
                    match late {
                        Overdue::Resend { cmd_id, what, frame } => {
                            tracing::warn!(device = %snap.device_id, cmd = %cmd_id, %what, "command not acknowledged, pushed again");
                            let _ = socket.send(Message::Text(codec.seal(&frame).into())).await;
                        }
                        Overdue::Lost { cmd_id, what } => {
                            tracing::warn!(device = %snap.device_id, cmd = %cmd_id, %what, "command not acknowledged by the device");
                        }
                    }
                }
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
                // Periodic token revalidation (4005).
                if last_validation.elapsed() >= cache {
                    let force = last_rebuild.elapsed() >= rebuild_every;
                    match revalidate(&ctx.db, &token, &snap, &mut fingerprint, force).await {
                        Revalidation::Unchanged => {}
                        Revalidation::Fresh(mut fresh) => {
                            // Session-scoped state from the announce survives
                            // the DB reload (custom metrics, D87).
                            fresh.metrics = std::mem::take(&mut snap.metrics);
                            snap = *fresh;
                            last_rebuild = Instant::now();
                        }
                        Revalidation::Invalid => {
                            let _ = socket.send(Message::Close(Some(CloseFrame {
                                code: 4005, reason: "Token invalid".into(),
                            }))).await;
                            break;
                        }
                    }
                    last_validation = Instant::now();
                }
                let plain = match codec.open(&text) {
                    Some(p) => p,
                    None => { tracing::warn!(device = %snap.device_id, "frame device indéchiffrable"); continue; }
                };
                // Frame-level PING/PONG — le firmware
                // pingue toutes les 5 s ; sans réponse il ferme après 15 s
                // (PONG timeout) et boucle reconnexion. Manquait sur /ws/device
                // (leçon 2026-09-02 : sessions de 15 s, device « actif » mais
                // jamais provisionné).
                let is_ping = plain.trim().eq_ignore_ascii_case("ping");
                if is_ping {
                    let _ = socket
                        .send(Message::Text(codec.seal("PONG").into()))
                        .await;
                } else {
                    match serde_json::from_str::<DeviceMsg>(&plain) {
                        Ok(DeviceMsg::Announce { fw, .. }) if agent.is_some() => {
                            // Edge agent (D95): no pins, no manifest — the
                            // registration (token) is the admission.
                            if let Some(sess) = agent.as_mut() {
                                handle_agent_announce(&ctx, &mut socket, &mut codec, &snap, sess, &fw).await;
                            }
                        }
                        Ok(DeviceMsg::Batch { epoch, points }) => {
                            let Some(sess) = agent.as_mut() else {
                                tracing::debug!(device = %snap.device_id, "batch from a non-agent device — ignored");
                                continue;
                            };
                            let who = crate::services::edge_agent::AgentIdentity {
                                org_id: snap.org_id,
                                device_registry_id: snap.device_registry_id,
                                device_id: &snap.device_id,
                                pred_dev: &snap.pred_dev,
                            };
                            if let Some(up_to_seq) =
                                crate::services::edge_agent::ingest_batch(&ctx, sess, &who, &epoch, points).await
                            {
                                send_server_msg(&mut socket, &mut codec, &ServerMsg::BatchAck { epoch, up_to_seq }).await;
                            }
                        }
                        Ok(DeviceMsg::Announce { chip, board, fw, caps, pins }) => {
                            // Provisioning is DB heavy: bounded per pod like
                            // the handshake (waits, never rejects).
                            let _slot = super::device_link::admission_slot(&settings).await;
                            if let Some(cmd_id) = handle_announce(&ctx, &mut socket, &mut codec, &mut snap, &chip, &board, &fw, caps.as_deref(), pins.as_deref()).await {
                                ota.note(cmd_id);
                            }
                        }
                        Ok(DeviceMsg::StateReport { gpio: None, value, cap_id, .. }) => {
                            // D87: custom metric of a custom firmware.
                            handle_metric_report(&snap, cap_id.as_deref(), value);
                        }
                        Ok(DeviceMsg::StateReport { gpio: Some(gpio), value, cap_id, uptime_ms, boot_id, seq }) => {
                            handle_state_report(&ctx, &snap, gpio, value, cap_id.as_deref()).await;
                            // Champs d'horloge D46 : dédup/reconstruction quand le
                            // buffering existera (F3) — journalisés en debug pour
                            // la traçabilité du fil (ne polluent pas les séries).
                            if let (Some(b), Some(s)) = (boot_id.as_deref(), seq) {
                                tracing::debug!(
                                    device = %snap.device_id, gpio, boot_id = b, seq = s, uptime_ms = ?uptime_ms,
                                    "state_report horodaté device"
                                );
                            }
                        }
                        Ok(DeviceMsg::Ack { cmd_id, ok, err }) => {
                            if let Some((what, rtt)) = acks.ack(&cmd_id, Instant::now()) {
                                tracing::debug!(device = %snap.device_id, cmd = %cmd_id, %what, ok, rtt_ms = rtt.as_millis() as u64, "command acknowledged");
                            }
                            if !ok {
                                tracing::warn!(device = %snap.device_id, cmd = %cmd_id, "commande refusée par le device : {:?}", err);
                                // OTA: an explicit device refusal fails the active
                                // assignment (cmd_id correlation, single session).
                                // Most refusals are not OTA: the session cache
                                // skips the lookup when no OTA is in flight.
                                if let Some(row) = ota.active_for(&ctx.db, snap.device_registry_id, &cmd_id).await {
                                    let _ = crate::services::ota::transition(
                                        &ctx.db,
                                        row,
                                        &snap.device_id,
                                        crate::services::ota::ST_FAILED,
                                        None,
                                        err.as_deref().or(Some("refused")),
                                    )
                                    .await;
                                }
                            }
                        }
                        Ok(DeviceMsg::OtaState { cmd_id, phase, progress, err }) => {
                            // OTA progress: correlate the active assignment by
                            // cmd_id and persist the transition (journal on
                            // terminal); any uplink frame feeds the watchdog.
                            if let Some(row) = ota.active_for(&ctx.db, snap.device_registry_id, &cmd_id).await {
                                let result = match phase.as_str() {
                                    "downloading" => {
                                        crate::services::ota::transition(&ctx.db, row, &snap.device_id, crate::services::ota::ST_DOWNLOADING, progress.map(|p| p as i32), None).await
                                    }
                                    "flashing" => {
                                        crate::services::ota::transition(&ctx.db, row, &snap.device_id, crate::services::ota::ST_FLASHING, None, None).await
                                    }
                                    "failed" => {
                                        crate::services::ota::transition(&ctx.db, row, &snap.device_id, crate::services::ota::ST_FAILED, None, err.as_deref().or(Some("device error"))).await
                                    }
                                    other => {
                                        tracing::debug!(device = %snap.device_id, "unknown ota phase: {other}");
                                        Ok(row) // tolerate unknown phases (ControlSpec.kind school)
                                    }
                                };
                                if let Err(e) = result {
                                    tracing::warn!(device = %snap.device_id, "ota state persist failed: {e}");
                                }
                            }
                        }
                        Ok(DeviceMsg::RegState { entries }) => {
                            // Diagnostic des régulations embarquées (cartes
                            // mixtes) — toléré absent (le générique ne l'émet
                            // jamais). Routé vers la télémétrie O2 (F2 : lève la
                            // limite « journalisé, pas encore routé » du §8 de
                            // control-cards.md).
                            for e in &entries {
                                handle_reg_diag(&snap, e);
                            }
                            tracing::debug!(
                                device = %snap.device_id,
                                "reg_state : {} régulation(s) rapportée(s)",
                                entries.len()
                            );
                        }
                        Err(e) => {
                            tracing::debug!(device = %snap.device_id, "message non reconnu : {e}");
                        }
                    }
                }
                // Liveness (throttled, owner-checked): a session whose lease
                // was claimed by a newer one is superseded and closes.
                if last_touch.elapsed() >= throttle {
                    last_touch = Instant::now();
                    if let Ok(false) =
                        device_liveness::touch_owned(snap.device_registry_id, &guard.1, settings.silence_ttl_secs).await
                    {
                        tracing::info!(device = %snap.device_id, "lease held by a newer session — closing");
                        break;
                    }
                }
                if last_route.elapsed() >= crate::services::device_bus::ROUTE_REFRESH {
                    crate::services::device_bus::refresh(snap.device_registry_id, &guard.1).await;
                    last_route = Instant::now();
                }
            }
        }
    }
    // Sortie de session : bail liveness libéré (owner-checked) + registres
    // (guard). The shared GPIO last values are cleared only by the session
    // that still owned the lease.
    if let Ok(true) = device_liveness::release(&ctx.db, snap.device_registry_id, &guard.1).await {
        crate::services::last_cache::gpio_send(
            &ctx.config,
            crate::services::last_cache::GpioOp::Clear {
                device_registry_id: snap.device_registry_id,
            },
        )
        .await;
    }
    drop(guard);
}

/// Per-session OTA correlation cache: OTA frames (`OtaState`, refused
/// `Ack`) only hit the database when their `cmd_id` is the one of an
/// assignment this session delivered, or when the last miss is old enough
/// (an assignment delivered by another path is still found).
#[derive(Default)]
struct OtaCache {
    /// `cmd_id` of the last OTA offer that went through this session.
    known: Option<String>,
    /// Last lookup that found no matching active assignment.
    miss_at: Option<Instant>,
}

/// Negative cache window of [`OtaCache`].
const OTA_MISS_TTL: Duration = Duration::from_secs(30);

impl OtaCache {
    fn note(&mut self, cmd_id: String) {
        self.known = Some(cmd_id);
        self.miss_at = None;
    }

    /// `true` when a database lookup is worth it for `cmd_id`.
    fn should_lookup(&self, cmd_id: &str) -> bool {
        self.known.as_deref() == Some(cmd_id)
            || self.miss_at.is_none_or(|t| t.elapsed() >= OTA_MISS_TTL)
    }

    /// Active assignment correlated with `cmd_id`, if any.
    async fn active_for(
        &mut self,
        db: &DatabaseConnection,
        device_registry_id: i64,
        cmd_id: &str,
    ) -> Option<crate::models::ota_assignments::Model> {
        if !self.should_lookup(cmd_id) {
            return None;
        }
        match crate::services::ota::newest_active(db, device_registry_id).await {
            Ok(Some(row)) if row.cmd_id.as_deref() == Some(cmd_id) => {
                self.known = Some(cmd_id.to_string());
                Some(row)
            }
            Ok(_) => {
                if self.known.as_deref() == Some(cmd_id) {
                    self.known = None;
                }
                self.miss_at = Some(Instant::now());
                None
            }
            Err(_) => None,
        }
    }
}

/// Edge agent announce (D95): stamps the agent version, answers the
/// runtime parameters. No admission step — pins and firmware do not apply.
async fn handle_agent_announce(
    ctx: &AppContext,
    socket: &mut WebSocket,
    codec: &mut FrameCodec,
    snap: &DeviceSnapshot,
    sess: &mut crate::services::edge_agent::AgentSession,
    fw: &str,
) {
    if let Ok(Some(dev)) = device_registries::Entity::find_by_id(snap.device_registry_id)
        .one(&ctx.db)
        .await
    {
        let max_keys = dev.max_unique_measurements;
        if dev.fw_version.as_deref() != Some(fw) {
            let mut active: device_registries::ActiveModel = dev.into();
            active.fw_version = sea_orm::Set(Some(fw.chars().take(64).collect()));
            let _ = sea_orm::ActiveModelTrait::update(active, &ctx.db).await;
        }
        *sess = crate::services::edge_agent::AgentSession::new(snap.device_registry_id, max_keys);
    }
    tracing::info!(device = %snap.device_id, version = fw, "edge agent announced");
    let cfg = ServerMsg::AgentConfig {
        max_batch: crate::services::edge_agent::AGENT_MAX_BATCH,
        max_keys: sess.max_keys(),
    };
    send_server_msg(socket, codec, &cfg).await;
}

/// Outcome of a periodic token revalidation.
enum Revalidation {
    /// Token or device invalidated → 4005.
    Invalid,
    /// Token valid, nothing the snapshot depends on changed.
    Unchanged,
    /// Token valid, snapshot rebuilt from the database.
    Fresh(Box<DeviceSnapshot>),
}

/// What the session snapshot depends on, read cheaply: device row version
/// and pin instances (count + newest update). Equal fingerprints skip the
/// full rebuild (board overlay, predefined, pins).
#[derive(Clone, PartialEq, Eq)]
struct SnapshotFingerprint {
    device_updated_at: sea_orm::prelude::DateTimeWithTimeZone,
    instances: i64,
    instances_updated_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
}

async fn fingerprint_of(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Option<SnapshotFingerprint> {
    use sea_orm::sea_query::{Expr, Func};
    use sea_orm::QuerySelect;
    let (instances, instances_updated_at): (i64, Option<sea_orm::prelude::DateTimeWithTimeZone>) =
        device_capability_instances::Entity::find()
            .select_only()
            .column_as(
                Expr::from(Func::count(Expr::col(
                    device_capability_instances::Column::Id,
                ))),
                "n",
            )
            .column_as(
                Expr::from(Func::max(Expr::col(
                    device_capability_instances::Column::UpdatedAt,
                ))),
                "m",
            )
            .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
            .into_tuple()
            .one(db)
            .await
            .ok()??;
    Some(SnapshotFingerprint {
        device_updated_at: device.updated_at,
        instances,
        instances_updated_at,
    })
}

/// Revalidation périodique du token en session (4005 si invalidé).
/// Cheap path: one token+device query and one aggregate over the pin
/// instances; the full snapshot is rebuilt only when that fingerprint
/// changed (or when `force`, the periodic safety net).
async fn revalidate(
    db: &DatabaseConnection,
    token: &str,
    snap: &DeviceSnapshot,
    fingerprint: &mut Option<SnapshotFingerprint>,
    force: bool,
) -> Revalidation {
    let Ok(Some((_, device))) = active_token(db, token).await else {
        return Revalidation::Invalid;
    };
    if device.device_id != snap.device_id {
        return Revalidation::Invalid;
    }
    let fresh_fp = fingerprint_of(db, &device).await;
    if !force && fresh_fp.is_some() && *fingerprint == fresh_fp {
        return Revalidation::Unchanged;
    }
    match build_snapshot(db, &device).await {
        Ok(fresh) => {
            *fingerprint = fresh_fp;
            Revalidation::Fresh(Box::new(fresh))
        }
        // Transient rebuild failure: keep the current snapshot (the token
        // itself is valid), retry at the next revalidation.
        Err(_) => Revalidation::Unchanged,
    }
}

/// Announce → admission (Validated) → refresh snapshot → ProvisionAck.
/// Generic model: overlay admission; custom firmware: admission by the
/// declared pins (`Announce.pins`).
#[allow(clippy::too_many_arguments)] // même école que session_loop
async fn handle_announce(
    ctx: &AppContext,
    socket: &mut WebSocket,
    codec: &mut FrameCodec,
    snap: &mut DeviceSnapshot,
    chip: &str,
    board: &str,
    fw: &str,
    caps: Option<&[pnex_core::CapDesc]>,
    pins: Option<&[pnex_core::PinDecl]>,
) -> Option<String> {
    // Manifeste de capacités (D47) : attestation pour un firmware compilé
    // par le serveur (l'overlay board reste l'autorité des pins — bascule
    // douce §7) ; « le serveur sait quelle version tourne où » (réservation
    // OTA, §9) = ce journal. Un fil historique sans manifeste reste valide.
    if let Some(caps) = caps {
        let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
        tracing::info!(device = %snap.device_id, fw, "manifeste annoncé : [{}]", ids.join(", "));
    }
    // Custom metrics accepted for this session (D87).
    snap.metrics = caps
        .unwrap_or_default()
        .iter()
        .filter(|c| c.family == "metric")
        .map(|c| c.id.clone())
        .collect();
    tracing::info!(device = %snap.device_id, board, fw, "announce device générique");
    let device = match device_registries::Entity::find_by_id(snap.device_registry_id)
        .one(&ctx.db)
        .await
    {
        Ok(Some(d)) => d,
        _ => {
            send_server_msg(
                socket,
                codec,
                &ServerMsg::Reject {
                    reason: "device introuvable".into(),
                },
            )
            .await;
            return None;
        }
    };
    // Observed SoC: recorded at the first announce, fallback of
    // `device_soc` when the board's SoC is unknown. Best-effort.
    if device.soc.is_none() {
        provisioning::persist_observed_soc(&ctx.db, device.id, chip).await;
    }
    // OTA inventory ("who runs where", edge-model §9): persist the announced
    // fw version and the `ota` admission cap (best-effort — a DB hiccup must
    // not break provisioning). The comparison skips a no-op write.
    let ota_cap = caps.is_some_and(|cs| cs.iter().any(|c| c.id == "ota"));
    // Announced manifest persisted for the UI (metrics list, command
    // picker) and the custom command guard (D88).
    let caps_json = caps.and_then(|cs| serde_json::to_value(cs).ok());
    if device.fw_version.as_deref() != Some(fw)
        || device.ota_ready != Some(ota_cap)
        || device.announced_caps != caps_json
    {
        let mut upd: device_registries::ActiveModel = device.clone().into();
        upd.fw_version = Set(Some(fw.to_string()));
        upd.ota_ready = Set(Some(ota_cap));
        upd.announced_caps = Set(caps_json);
        if let Err(e) = upd.update(&ctx.db).await {
            tracing::warn!(device = %snap.device_id, "fw inventory persist failed: {e}");
        }
    }
    // `cmd_id` of an OTA offer sent by this announce (session OTA cache).
    let mut offered: Option<String> = None;
    match provisioning::admit(
        &ctx.db,
        &device,
        Some(provisioning::AdmissionManifest { pins }),
    )
    .await
    {
        Ok(specs) => {
            snap.reload_pins(&ctx.db).await;
            send_server_msg(socket, codec, &ServerMsg::ProvisionAck { caps: specs }).await;
            // Restauration du desired-state : le ProvisionAck ne porte pas
            // les cadences — re-pousser les Subscribe persistés, sinon un
            // reflash/reconnect perdait les lectures périodiques alors que
            // la base reste la source de vérité (leçon 2026-09-03).
            for (&gpio, &ms) in &snap.intervals {
                if ms > 0 {
                    send_server_msg(
                        socket,
                        codec,
                        &ServerMsg::Subscribe {
                            cmd_id: uuid::Uuid::new_v4().simple().to_string(),
                            gpio: gpio as u16,
                            interval_ms: ms,
                        },
                    )
                    .await;
                }
            }
            // Cartes de régulation mixtes (control-cards) : re-cast du
            // desired-state APRÈS le ProvisionAck + Subscribe (l'apply
            // ProvisionAck du firmware réinitialise ses pins). Un reflash/
            // reconnect retrouve ses régulations sans EEPROM — même leçon
            // que les cadences re-poussées ci-dessus. Vide → rien à caster.
            match crate::services::regulator::configs_for_device(&ctx.db, snap.device_registry_id)
                .await
            {
                Ok(configs) if !configs.is_empty() => {
                    send_server_msg(
                        socket,
                        codec,
                        &ServerMsg::ControlConfig {
                            cmd_id: uuid::Uuid::new_v4().simple().to_string(),
                            configs,
                        },
                    )
                    .await;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        device = %snap.device_id,
                        "re-cast des régulations à l'announce impossible : {e}"
                    );
                }
            }
            // Camera (camera-video.md D76): a `camera` cap creates the
            // settings row on first announce, then the current CameraConfig
            // (enabled = demand) is cast — same re-push school as above.
            if caps.is_some_and(|cs| cs.iter().any(|c| c.id == "camera")) {
                match crate::services::camera::ensure_row(&ctx.db, &device).await {
                    Ok(row) => {
                        let settings = crate::services::camera::settings_of(&row);
                        crate::services::camera::arm_fps_guard(row.org_id, device.id, settings.fps);
                        // Cluster-wide demand: viewers and flows of every pod.
                        let enabled = crate::services::camera::wanted(
                            &settings,
                            device.org_id,
                            device.id,
                            &device.device_id,
                        )
                        .await;
                        send_server_msg(
                            socket,
                            codec,
                            &crate::services::camera::config_msg(&settings, enabled),
                        )
                        .await;
                    }
                    Err(e) => {
                        tracing::warn!(device = %snap.device_id, "camera settings row failed: {e}");
                    }
                }
            }
            // OTA pickup (edge-model §9): every announce resolves or
            // delivers the desired update — same re-push school as the
            // Subscribe/ControlConfig blocks above. A post-reboot announce
            // with fw >= target flips the row to succeeded (also rescues a
            // watchdog-timeout row when the reboot actually landed).
            if let Ok(Some(row)) =
                crate::services::ota::newest_for_device(&ctx.db, snap.device_registry_id).await
            {
                let at_target = pnex_core::fw_at_least(fw, &row.target_version);
                let rescuable = !crate::services::ota::is_terminal(&row.state)
                    || (row.state == crate::services::ota::ST_FAILED
                        && row.error.as_deref() == Some("timeout"));
                if at_target && rescuable {
                    if row.state != crate::services::ota::ST_SUCCEEDED {
                        let _ = crate::services::ota::transition(
                            &ctx.db,
                            row,
                            &snap.device_id,
                            crate::services::ota::ST_SUCCEEDED,
                            Some(100),
                            None,
                        )
                        .await;
                    }
                } else if !crate::services::ota::is_terminal(&row.state) {
                    let cmd_id = uuid::Uuid::new_v4().simple().to_string();
                    let mut upd: crate::models::ota_assignments::ActiveModel = row.clone().into();
                    upd.cmd_id = Set(Some(cmd_id.clone()));
                    if let Err(e) = upd.update(&ctx.db).await {
                        tracing::warn!(device = %snap.device_id, "ota cmd_id refresh failed: {e}");
                    }
                    let sig = crate::services::ota_signing::sign_for_order(
                        &ctx.db,
                        &ctx.config,
                        &snap.device_id,
                        &row.target_version,
                        &row.sha256,
                    )
                    .await;
                    if let Some(sig) = sig {
                        send_server_msg(
                            socket,
                            codec,
                            &ServerMsg::OtaAvailable {
                                cmd_id: cmd_id.clone(),
                                version: row.target_version.clone(),
                                url: format!(
                                    "/api/v1/ota/firmware/{}/{}",
                                    snap.device_id, row.target_version
                                ),
                                sha256: row.sha256.clone(),
                                size: row.size_bytes.map(|s| s as u64),
                                sig,
                            },
                        )
                        .await;
                        offered = Some(cmd_id);
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!(device = %snap.device_id, "admission refusée : {e}");
            send_server_msg(
                socket,
                codec,
                &ServerMsg::Reject {
                    reason: format!("admission refusée : {e}"),
                },
            )
            .await;
        }
    }
    offered
}

/// StateReport → mémoire last_values (GET /pins) + sortie metrics O2
/// (série = label normalisé D16, même sortie que l'ingest).
///
/// F2 (D47, bascule douce) : un `cap_id` présent route la série
/// sémantiquement (`{org}/{device}/{capacité}`, source_type `generic_cap`)
/// ; absent → routage par label overlay inchangé (autorité du pin_slave).
/// LAST_VALUES reste mis à jour par gpio dans les deux cas (UI /pins).
async fn handle_state_report(
    ctx: &AppContext,
    snap: &DeviceSnapshot,
    gpio: u16,
    value: serde_json::Value,
    cap_id: Option<&str>,
) {
    let Some(pin) = snap.pins.get(&(gpio as i32)) else {
        tracing::debug!(device = %snap.device_id, gpio, "StateReport pour un pin inconnu — ignoré");
        return;
    };
    let label = snap
        .labels
        .get(&(gpio as i32))
        .cloned()
        .unwrap_or_else(|| gpio.to_string());
    // Routage de la série : capacité déclarée > label overlay. La présence
    // d'un cap_id ne crée JAMAIS une série « à côté » de l'UI : LAST_VALUES
    // reste alimenté par gpio (bascule douce §7).
    let name = match cap_id {
        Some(cap) if !cap.is_empty() => super::device_link::normalize_measurement_name(cap),
        _ => super::device_link::normalize_measurement_name(&label),
    };
    if name.is_empty() {
        return;
    }
    {
        let mut lv = LAST_VALUES.lock().expect("last_values");
        lv.entry(snap.device_registry_id)
            .or_default()
            .insert(gpio as i32, value.clone());
    }
    // Cross-pod copy for GET /pins served by any pod (queued, never blocks).
    crate::services::last_cache::gpio_send(
        &ctx.config,
        crate::services::last_cache::GpioOp::Put {
            device_registry_id: snap.device_registry_id,
            gpio: gpio as i32,
            value: value.to_string(),
        },
    )
    .await;
    // Prometheus n'a pas de booléens (remote-write = f64) : un pin digital
    // est stocké 1/0. La valeur brute reste dans LAST_VALUES pour l'UI
    // (HIGH/LOW). Avant ce fix, `promwrite::series_of` parsait un f64 et
    // TOUS les StateReports digitaux étaient silencieusement jetés —
    // « aucune donnée en visualisation » malgré un subscribe 1 s (leçon
    // 2026-09-03).
    let numeric = match &value {
        serde_json::Value::Bool(b) => serde_json::json!(i64::from(*b)),
        other => other.clone(),
    };
    telemetry::sink().send(TelemetryPoint {
        org_id: snap.org_id,
        device_registry_id: snap.device_registry_id,
        device_id: snap.device_id.clone(),
        pred_dev: snap.pred_dev.clone(),
        metric_name: name,
        value: numeric.to_string(),
        timestamp: chrono::Utc::now(),
        ts_source: "server",
        source_type: if cap_id.is_some() {
            "generic_cap"
        } else {
            "generic_gpio"
        },
        record: true,
    });
    let _ = pin;
}

/// Gpio-less StateReport (D87) → O2 series `{org}/{device}/{metric}`,
/// source_type `custom_metric`. The metric must be announced by this
/// session (a stray id is dropped, never a new series); non-numeric values
/// are dropped too (Prometheus series are f64 — booleans become 1/0).
fn handle_metric_report(snap: &DeviceSnapshot, cap_id: Option<&str>, value: serde_json::Value) {
    let Some(cap) = cap_id.filter(|c| snap.metrics.contains(*c)) else {
        tracing::debug!(device = %snap.device_id, ?cap_id, "metric not announced — dropped");
        return;
    };
    let numeric = match &value {
        serde_json::Value::Bool(b) => serde_json::json!(i64::from(*b)),
        serde_json::Value::Number(_) => value.clone(),
        _ => {
            tracing::debug!(device = %snap.device_id, cap, "non-numeric metric value — dropped");
            return;
        }
    };
    let name = super::device_link::normalize_measurement_name(cap);
    if name.is_empty() {
        return;
    }
    telemetry::sink().send(TelemetryPoint {
        org_id: snap.org_id,
        device_registry_id: snap.device_registry_id,
        device_id: snap.device_id.clone(),
        pred_dev: snap.pred_dev.clone(),
        metric_name: name,
        value: numeric.to_string(),
        timestamp: chrono::Utc::now(),
        ts_source: "server",
        source_type: "custom_metric",
        record: true,
    });
}

/// Une entrée `RegState` → points de télémétrie O2 (F2 : lève la limite
/// « journalisé, pas encore routé » du §8 de control-cards.md). Trois séries
/// par régulation : `<node>_sensor`, `<node>_out`, `<node>_cycles` —
/// `source_type=reg_state`. Mesure NaN (capteur illisible) = pas de point
/// (trou, jamais de donnée inventée) ; sortie/cycles toujours finis.
fn handle_reg_diag(snap: &DeviceSnapshot, e: &pnex_core::RegDiag) {
    let push = |suffix: &str, value: serde_json::Value| {
        let name = super::device_link::normalize_measurement_name(&format!(
            "{node_id}_{suffix}",
            node_id = e.node_id
        ));
        if name.is_empty() {
            return;
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
            source_type: "reg_state",
            record: true,
        });
    };
    if e.sensor_value.is_finite() {
        push("sensor", serde_json::json!(e.sensor_value));
    }
    push("out", serde_json::json!(e.output_pct));
    push("cycles", serde_json::json!(e.cycle_count));
}

/// Envoi serveur → device (chiffré).
async fn send_server_msg(socket: &mut WebSocket, codec: &mut FrameCodec, msg: &ServerMsg) {
    let Ok(plain) = serde_json::to_string(msg) else {
        return;
    };
    let _ = socket.send(Message::Text(codec.seal(&plain).into())).await;
}

/// Snapshot device complet : identité + carte gpio→label + contraintes
/// par pin (instances persistées).
///
/// Generic model: labels from the board overlay (`pred_dev` = overlay
/// board). Custom firmware: labels of the instances persisted at admission
/// (the sketch's pins — empty until the first announce), `pred_dev` =
/// model name. Edge agent: no pins, `pred_dev` = model name.
async fn build_snapshot(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<DeviceSnapshot> {
    let pd = predefined_devices::Entity::find_by_id(device.predefined_device_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| Error::string("predefined device not found"))?;
    let sketch_or_agent =
        device.firmware_project_id.is_some() || pd.name == pnex_core::EDGE_AGENT_PREDEF;
    let overlay = match sketch_or_agent {
        true => None,
        false => Some(provisioning::load_overlay(db, device).await?.0),
    };
    let (labels, pred_dev) = match overlay {
        Some(overlay) => {
            let labels: HashMap<i32, String> = overlay
                .pins
                .iter()
                .map(|p| (p.gpio as i32, p.label.clone()))
                .collect();
            (labels, overlay.board.clone())
        }
        None => {
            let rows = device_capability_instances::Entity::find()
                .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
                .all(db)
                .await
                .unwrap_or_default();
            let labels: HashMap<i32, String> =
                rows.iter().map(|r| (r.gpio, r.label.clone())).collect();
            (labels, pd.name.clone())
        }
    };
    let (pins, intervals) = load_pins(db, device.id).await;
    let snap = DeviceSnapshot {
        device_registry_id: device.id,
        org_id: device.org_id,
        device_id: device.device_id.clone(),
        pred_dev,
        labels,
        pins,
        intervals,
        metrics: HashSet::new(),
    };
    Ok(snap)
}

/// Instances persistées → cartes gpio → (contraintes validées, cadences).
/// Source de vérité : la base, remplie à l'admission et par les SetMode/
/// Subscribe REST — les modes survivent aux re-announce (upsert), les
/// cadences sont re-poussées après chaque Announce.
#[allow(clippy::type_complexity)]
async fn load_pins(
    db: &DatabaseConnection,
    device_registry_id: i64,
) -> (HashMap<i32, pnex_core::ValidatedPin>, HashMap<i32, u32>) {
    let rows = device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(device_registry_id))
        .all(db)
        .await
        .unwrap_or_default();
    let mut pins = HashMap::new();
    let mut intervals = HashMap::new();
    for r in &rows {
        let cfg: pnex_core::ModeOpts = r
            .config
            .as_ref()
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .unwrap_or_default();
        pins.insert(
            r.gpio,
            pnex_core::ValidatedPin {
                gpio: r.gpio as u16,
                mode: str_to_mode(&r.mode),
                pullup: cfg.pullup.unwrap_or(false),
                safe_state: cfg.safe_state.unwrap_or(SafeState::Low),
            },
        );
        let interval = r
            .config
            .as_ref()
            .and_then(|c| c.get("interval_ms"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(0);
        intervals.insert(r.gpio, interval);
    }
    (pins, intervals)
}

/// Mode fil (snake_case) → Mode code. `analog_in` (base) et `adc_in`
/// (fil) acceptés — même alias que pins.rs / provisioning.rs.
fn str_to_mode(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "pwm_out" => Mode::PwmOut,
        "analog_in" | "adc_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}

/// Where a command for a device goes: the local session, or the pod the
/// device bus route designates.
#[derive(Clone)]
pub(crate) enum Target {
    Local(mpsc::UnboundedSender<Downlink>),
    /// Raw bus route (`<pod>|<session>`) of a session on another pod — or a
    /// superseded local one: the route is the authority.
    Remote {
        route: String,
        fallback: Option<mpsc::UnboundedSender<Downlink>>,
    },
}

/// Resolves where the live session of a device is (`None` = offline).
///
/// With the device bus on, the Valkey route is authoritative: the local
/// session is used only when the route designates it (session id compare).
/// A stale local session (half-open TCP awaiting its watchdog) never wins
/// over the route pointing to the pod that holds the live session. Without
/// a route (Valkey flushed, claim failed) or with Valkey unreachable, the
/// local session is the best effort.
pub(crate) async fn target_of(device_registry_id: i64) -> Option<Target> {
    use crate::services::device_bus::{lookup, Route};
    let local = DEVICE_SESSIONS
        .lock()
        .expect("sessions")
        .get(&device_registry_id)
        .map(|s| (s.session.clone(), s.tx.clone()));
    match lookup(device_registry_id).await {
        Route::Off | Route::Unknown | Route::Missing => local.map(|(_, tx)| Target::Local(tx)),
        route @ Route::At(_) => {
            if let Some((session, tx)) = &local {
                if route.is_local_session(session) {
                    return Some(Target::Local(tx.clone()));
                }
            }
            let Route::At(raw) = route else {
                unreachable!()
            };
            Some(Target::Remote {
                route: raw,
                fallback: local.map(|(_, tx)| tx),
            })
        }
    }
}

/// Delivers one command to a resolved target (`false` = not delivered).
pub(crate) async fn push_to(device_registry_id: i64, target: &Target, msg: ServerMsg) -> bool {
    match target {
        Target::Local(tx) => tx.send(Downlink::Msg(msg)).is_ok(),
        Target::Remote { route, fallback } => {
            let Ok(json) = serde_json::to_string(&msg) else {
                return false;
            };
            if crate::services::device_bus::send_via(device_registry_id, route, json).await {
                return true;
            }
            // The routed pod is gone (stale route removed): last resort,
            // the local session if any.
            fallback
                .as_ref()
                .is_some_and(|tx| tx.send(Downlink::Msg(msg)).is_ok())
        }
    }
}

/// Pushes a command to the device session, wherever it lives (see
/// [`target_of`]). `false` = offline (no session on any pod).
pub(crate) async fn push_command(device_registry_id: i64, msg: ServerMsg) -> bool {
    match target_of(device_registry_id).await {
        Some(target) => push_to(device_registry_id, &target, msg).await,
        None => false,
    }
}

/// Test hook: registers a local session entry (no socket) and returns its
/// downlink receiver.
#[cfg(test)]
pub(crate) fn register_local_for_test(
    device_registry_id: i64,
    session: &str,
) -> mpsc::UnboundedReceiver<Downlink> {
    let (tx, rx) = mpsc::unbounded_channel();
    DEVICE_SESSIONS.lock().expect("sessions").insert(
        device_registry_id,
        LocalSession {
            session: session.to_string(),
            tx,
        },
    );
    rx
}

/// Forwards a serialized command relayed by the device bus to the local
/// session (`false`: the session is not on this pod any more).
pub(crate) fn push_local_json(device_registry_id: i64, json: String) -> bool {
    match DEVICE_SESSIONS
        .lock()
        .expect("sessions")
        .get(&device_registry_id)
    {
        Some(s) => s.tx.send(Downlink::Json(json)).is_ok(),
        None => false,
    }
}

/// Live session for this device, on this pod or another one (D107).
pub(crate) async fn is_connected(device_registry_id: i64) -> bool {
    target_of(device_registry_id).await.is_some()
}

/// GPIO last values of a device for the Pins panel: the shared Valkey copy
/// (written by whichever pod holds the session), else this pod's map.
pub(crate) async fn last_values(
    config: &loco_rs::config::Config,
    device_registry_id: i64,
) -> HashMap<i32, serde_json::Value> {
    if let Some(values) = crate::services::last_cache::gpio_values(config, device_registry_id).await
    {
        return values;
    }
    LAST_VALUES
        .lock()
        .expect("last_values")
        .get(&device_registry_id)
        .cloned()
        .unwrap_or_default()
}
