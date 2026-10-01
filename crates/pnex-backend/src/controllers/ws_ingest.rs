//! Ingestion télémétrie — WS `/ws/sensor/ingest`.
//!
//! Parity with the legacy `SensorIngest` channel:
//! - auth par query string `?token=<b64(token)>&device_id=<b64(device_id)>`
//!   (le serveur décode les deux et trime — les valeurs encodées à la
//!   firmware `echo | base64` portent un `\n` trailing) ;
//! - toutes les frames, dans les deux sens, sont du **texte**
//!   `base64(nonce 12 ‖ ChaCha20-nu ct)` avec nonce frais par message
//!   (D8 : sans Poly1305 — pas d'AEAD) ;
//! - `PING` (casse ignorée) → `PONG` ; `key=value` (1 mesure/frame, pas de
//!   JSON, pas de timestamp device) → `ok` ; erreurs chiffrées
//!   `error:*` / `ERROR:decryption_failed` ;
//! - close codes : 4001 auth échouée, 4002 sans token, 4003 déjà connecté,
//!   4005 token invalide en session, 4006 mismatch token/device,
//!   4008 sans clé.
//!
//! Deliberate hardenings over the legacy implementation:
//! - **anti-clone double étage** : map des sessions ouvertes en-process
//!   (rejet 4003 immédiat, y compris course < TTL) puis fallback PG
//!   `device_states` (couvre un autre process / un crash sans close) ;
//! - **clean lease release** on disconnect (the legacy implementation kept
//!   the 12 s window) — an immediate reconnect of the real device is accepted;
//! - `last_seen` refreshed on **every valid frame** (the legacy implementation
//!   counted only PINGs: a device sending only measurements looked inactive);
//! - token/device revalidation **with a 10 s cache** (§7.8: the legacy cache
//!   was dead code, the DB was queried on every frame at ~10 fps).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use crate::models::_entities::{device_registries, device_tokens, predefined_devices};
use crate::services::device_liveness;
use crate::services::settings::IngestSettings;
use crate::services::telemetry::{self, TelemetryPoint};

// ───────────────────────── Aides chiffrement ─────────────────────────

// Shared with the edge agent (D95): single implementation in pnex-core.
pub(crate) use pnex_core::frame::{decrypt_frame, encrypt_frame};

// ──────────────── Normalisation des noms de mesures ────────────────

/// Harmonisation capability ↔ mesure (D16) — la fonction vit dans pnex-core
/// (feature `naming`) : le nœud `device` du runtime de flows (Phase 6) doit
/// lire les séries sous le MÊME nom normalisé que l'ingestion les écrit.
/// Re-exportée ici pour ne pas déplacer les points d'appel.
pub(crate) use pnex_core::normalize_measurement_name;

// ───────────────────────── Sessions ouvertes ─────────────────────────

/// Devices with an ingest WS session open in THIS process: session id +
/// a "superseded" signal. Admission itself is decided by the atomic lease
/// claim in Postgres (cross-pod); when a newer session of the same device
/// is admitted on this pod, replacing the entry drops the old sender and
/// the old (half-open) session ends.
static OPEN_SESSIONS: LazyLock<Mutex<HashMap<i64, (String, oneshot::Sender<()>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Silence watchdog of an ingest session: no frame for this long → the
/// session is closed (half-open TCP, power-cycled board). The firmware
/// pings every 5 s — same bound as `/ws/device`.
const INGEST_WATCHDOG_SECS: u64 = 45;

/// Removes the device from the open sessions on exit (all paths) — only
/// while the entry is still this session's.
struct SessionGuard(i64, String);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let mut open = OPEN_SESSIONS.lock().expect("sessions");
        if open.get(&self.0).is_some_and(|(s, _)| *s == self.1) {
            open.remove(&self.0);
        }
    }
}

// ─────────────────────── Admission throttling ───────────────────────

static ADMISSION: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// Per-pod admission semaphore (device handshakes and announces), sized
/// once from `settings.ingestion.admission_concurrency`.
fn admission(settings: &IngestSettings) -> Arc<Semaphore> {
    ADMISSION
        .get_or_init(|| Arc::new(Semaphore::new(settings.admission_concurrency.max(1))))
        .clone()
}

/// Handshake slot, waited at most `admission_wait_ms` (`None` → the caller
/// closes with 1013 so the device retries later).
pub(crate) async fn admission_permit(settings: &IngestSettings) -> Option<OwnedSemaphorePermit> {
    let wait = Duration::from_millis(settings.admission_wait_ms);
    match tokio::time::timeout(wait, admission(settings).acquire_owned()).await {
        Ok(Ok(permit)) => Some(permit),
        _ => {
            tracing::warn!("device admission saturated — handshake deferred (1013)");
            None
        }
    }
}

/// Admission slot for in-session provisioning work (waits, never rejects).
pub(crate) async fn admission_slot(settings: &IngestSettings) -> Option<OwnedSemaphorePermit> {
    admission(settings).acquire_owned().await.ok()
}

// ───────────────────────── Snapshot validé ─────────────────────────

/// Vue du device validée (rafraîchie au rythme du cache de revalidation) :
/// porte le routage org (D2 — suit un changement d'org du device) et les
/// règles de validation des mesures.
pub(crate) struct Snapshot {
    pub(crate) device_registry_id: i64,
    pub(crate) org_id: i64,
    pub(crate) device_id: String,
    pub(crate) pred_dev: String,
    allow_dynamic: bool,
    discovered: HashSet<String>,
    max_unique: i32,
    /// Capacités du predefined (validation stricte des non-dynamiques).
    capabilities: HashSet<String>,
}

impl Snapshot {
    /// Charge le device d'un token actif + son contexte de validation.
    /// `Ok(None)` = token inconnu/inactif (→ 4001) ; le mismatch device_id
    /// est départagé par l'appelant (→ 4006) sur la ligne registre.
    pub(crate) async fn load(
        db: &DatabaseConnection,
        token: &str,
    ) -> Result<Option<(device_tokens::Model, device_registries::Model)>> {
        let Some((tok, Some(device))) = device_tokens::Entity::find()
            .find_also_related(device_registries::Entity)
            .filter(device_tokens::Column::Token.eq(token))
            .one(db)
            .await
            .map_err(|_| Error::InternalServerError)?
        else {
            return Ok(None);
        };
        if !tok.is_active {
            return Ok(None);
        }
        Ok(Some((tok, device)))
    }

    /// Assemble le snapshot validé (predefined + capacités + découverte).
    async fn from_device(
        db: &DatabaseConnection,
        device: device_registries::Model,
    ) -> Result<Self> {
        let Some(predefined) = predefined_devices::Entity::find_by_id(device.predefined_device_id)
            .one(db)
            .await
            .map_err(|_| Error::InternalServerError)?
        else {
            return Err(Error::InternalServerError);
        };
        let caps = super::devices::capabilities_of(db, &[predefined.id])
            .await?
            .remove(&predefined.id)
            .unwrap_or_default();
        Ok(Self {
            device_registry_id: device.id,
            org_id: device.org_id,
            device_id: device.device_id.clone(),
            pred_dev: predefined.name,
            allow_dynamic: device.allow_dynamic_measurements,
            discovered: super::devices::discovered_names(&device.discovered_measurements)
                .into_iter()
                .map(|n| normalize_measurement_name(&n))
                .collect(),
            max_unique: device.max_unique_measurements,
            capabilities: caps
                .into_iter()
                .map(|c| normalize_measurement_name(&c.name))
                .collect(),
        })
    }
}

// ─────────────────────────── Handler ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct IngestQuery {
    token: Option<String>,
    device_id: Option<String>,
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/ws")
        .add("/sensor/ingest", get(ws_ingest))
}

/// Accepts the upgrade then closes immediately with the code — legacy
/// `close(code=…)`; an HTTP status cannot carry a WS 4xxx close code.
pub(crate) fn reject(ws: WebSocketUpgrade, code: u16, reason: &'static str) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            })))
            .await;
    })
    .into_response()
}

/// Décode un paramètre query base64 → texte (trim : les valeurs encodées
/// côté firmware avec `echo | base64` portent un `\n`).
pub(crate) fn decode_param(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let bytes = STANDARD
        .decode(trimmed)
        .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed))
        .ok()?;
    String::from_utf8(bytes).ok().map(|s| s.trim().to_string())
}

async fn ws_ingest(
    State(ctx): State<AppContext>,
    Query(q): Query<IngestQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let settings = IngestSettings::from_config(&ctx.config);
    // Reconnect storm guard (bounded concurrent admissions per pod).
    let Some(permit) = admission_permit(&settings).await else {
        return reject(ws, 1013, "Try again later");
    };

    // Auth (historic order: 4002 without token, 4001 decode/lookup,
    // 4006 mismatch, 4008 without key, 4003 already connected).
    let Some(raw_token) = q.token.as_deref() else {
        return reject(ws, 4002, "No token provided");
    };
    let token = match decode_param(raw_token) {
        Some(t) if !t.is_empty() => t,
        _ => return reject(ws, 4001, "Authentication failed"),
    };
    // Legacy behavior: missing device_id → str != None comparison → 4006 mismatch.
    let device_id = match q.device_id.as_deref().map(decode_param) {
        Some(Some(d)) if !d.is_empty() => d,
        _ => return reject(ws, 4006, "Token device mismatch"),
    };

    let (tok, device) = match Snapshot::load(&ctx.db, &token).await {
        Ok(Some(found)) => found,
        _ => return reject(ws, 4001, "Authentication failed"),
    };
    if device.device_id != device_id {
        return reject(ws, 4006, "Token device mismatch");
    }
    let snap = match Snapshot::from_device(&ctx.db, device).await {
        Ok(s) => s,
        Err(_) => return reject(ws, 4001, "Authentication failed"),
    };
    let Some(key) = tok
        .encryption_key
        .as_deref()
        .and_then(|k| STANDARD.decode(k.trim()).ok())
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
    else {
        return reject(ws, 4008, "No encryption key");
    };

    // Anti-clone (cross-pod, atomic): the Valkey lease compare-and-set
    // decides — a live session on any pod → 4003; released, expired or
    // never seen → granted (immediate reconnect after a clean close).
    let session = uuid::Uuid::new_v4().simple().to_string();
    match device_liveness::claim(snap.device_registry_id, &session, settings.silence_ttl_secs).await
    {
        Ok(true) => {}
        Ok(false) => return reject(ws, 4003, "Device already connected"),
        Err(_) => return reject(ws, 1013, "Try again later"),
    }
    // A previous local session of the device is stale (the lease was
    // claimable): replacing its entry signals it to close.
    let (superseded_tx, superseded_rx) = oneshot::channel::<()>();
    OPEN_SESSIONS
        .lock()
        .expect("sessions")
        .insert(snap.device_registry_id, (session.clone(), superseded_tx));
    let guard = SessionGuard(snap.device_registry_id, session);
    drop(permit);

    let token_str = token.clone();
    ws.on_upgrade(move |socket| async move {
        session_loop(
            socket,
            &ctx,
            token_str,
            key,
            snap,
            guard,
            superseded_rx,
            settings,
        )
        .await;
    })
    .into_response()
}

/// Réponse chiffrée au device (`ok`, `PONG`, `error:*`).
async fn reply(socket: &mut WebSocket, key: &[u8; 32], plain: &str) {
    let _ = socket
        .send(Message::Text(encrypt_frame(plain, key).into()))
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn session_loop(
    mut socket: WebSocket,
    ctx: &AppContext,
    token: String,
    key: [u8; 32],
    mut snap: Snapshot,
    guard: SessionGuard,
    mut superseded: oneshot::Receiver<()>,
    settings: IngestSettings,
) {
    let cache = Duration::from_secs(settings.token_cache_secs);
    let rebuild_every = Duration::from_secs(settings.snapshot_rebuild_secs);
    let throttle = settings.liveness_touch_interval();
    let mut last_validation = Instant::now();
    let mut last_rebuild = Instant::now();
    let mut device_version = None;
    let mut last_touch = Instant::now();
    // Les rejets de déchiffrement ne sont pas loggés par frame (~10 fps) :
    // un avertissement unique par session suffit à voir une firmware qui
    // parle en clair ou une clé désynchronisée.
    let mut warned_decrypt_failure = false;

    loop {
        let incoming = tokio::select! {
            _ = &mut superseded => {
                tracing::info!(device = %snap.device_id, "ingest session superseded — closing");
                break;
            }
            incoming = tokio::time::timeout(
                Duration::from_secs(INGEST_WATCHDOG_SECS),
                socket.recv(),
            ) => incoming,
        };
        let Ok(incoming) = incoming else {
            tracing::warn!(
                device = %snap.device_id,
                "ingest session silent > {INGEST_WATCHDOG_SECS} s — closing (half-open)"
            );
            break;
        };
        let Some(Ok(msg)) = incoming else { break };
        let Message::Text(text) = msg else {
            // v1 : texte uniquement ; close/erreurs terminent la boucle au
            // prochain tour (recv → None).
            continue;
        };

        // Revalidation périodique (4005 : token/device invalidé en session).
        // Cheap path: one token+device query; the snapshot (predefined,
        // capabilities) is rebuilt only when the device row changed, or
        // every `snapshot_rebuild_secs` as a safety net.
        if last_validation.elapsed() >= cache {
            let force = last_rebuild.elapsed() >= rebuild_every;
            let fresh = async {
                let (_, device) = Snapshot::load(&ctx.db, &token).await.ok()??;
                if device.device_id != snap.device_id {
                    return None;
                }
                if !force && device_version == Some(device.updated_at) {
                    return Some(None);
                }
                let version = device.updated_at;
                match Snapshot::from_device(&ctx.db, device).await {
                    Ok(s) => Some(Some((s, version))),
                    // Transient rebuild failure: keep the current snapshot.
                    Err(_) => Some(None),
                }
            }
            .await;
            match fresh {
                Some(Some((s, version))) => {
                    snap = s;
                    device_version = Some(version);
                    last_rebuild = Instant::now();
                }
                Some(None) => {}
                None => {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 4005,
                            reason: "Token invalid".into(),
                        })))
                        .await;
                    break;
                }
            }
            last_validation = Instant::now();
        }

        let plain = match decrypt_frame(&text, &key) {
            Some(p) => p,
            None => {
                if !warned_decrypt_failure {
                    warned_decrypt_failure = true;
                    tracing::warn!(
                        device = %snap.device_id,
                        "frame WS indéchiffrable — firmware en clair ou clé ≠ device_tokens.encryption_key ?"
                    );
                }
                reply(&mut socket, &key, "ERROR:decryption_failed").await;
                continue;
            }
        };
        let trimmed = plain.trim();

        // Heartbeat standalone.
        if trimmed.eq_ignore_ascii_case("PING") {
            if last_touch.elapsed() >= throttle {
                last_touch = Instant::now();
                if let Ok(false) = device_liveness::touch_owned(
                    snap.device_registry_id,
                    &guard.1,
                    settings.silence_ttl_secs,
                )
                .await
                {
                    tracing::info!(device = %snap.device_id, "lease held by a newer session — closing");
                    break;
                }
            }
            reply(&mut socket, &key, "PONG").await;
            continue;
        }

        // Mesure `key=value` (split sur le premier `=`).
        let Some((name, value)) = trimmed.split_once('=') else {
            reply(&mut socket, &key, "error:invalid_format").await;
            continue;
        };
        if name.is_empty() {
            reply(&mut socket, &key, "error:empty_key").await;
            continue;
        }
        if name.len() > 100 {
            reply(&mut socket, &key, "error:measurement_name_too_long").await;
            continue;
        }
        // Harmonisation capability ↔ mesure (D16) : le nom est normalisé
        // AVANT validation/découverte/stockage — `Soil-Moisture`,
        // `soil moisture` et `soil_moisture` désignent la même mesure.
        let name = normalize_measurement_name(name);
        if name.is_empty() {
            reply(&mut socket, &key, "error:invalid_format").await;
            continue;
        }

        // Validation: strict → model capabilities; dynamic → capped discovery
        // (`ping=x` follows the same path — as in the legacy implementation).
        if !snap.allow_dynamic && !snap.capabilities.contains(&name) {
            reply(
                &mut socket,
                &key,
                &format!(
                    "error:invalid_capability:measurement '{name}' not in device capabilities"
                ),
            )
            .await;
            continue;
        }
        if snap.allow_dynamic && !snap.discovered.contains(&name) {
            if snap.discovered.len() as i32 >= snap.max_unique {
                reply(&mut socket, &key, "error:too_many_measurements").await;
                continue;
            }
            snap.discovered.insert(name.clone());
            persist_discovered(&ctx.db, snap.device_registry_id, &snap.discovered).await;
        }

        if last_touch.elapsed() >= throttle {
            last_touch = Instant::now();
            if let Ok(false) = device_liveness::touch_owned(
                snap.device_registry_id,
                &guard.1,
                settings.silence_ttl_secs,
            )
            .await
            {
                tracing::info!(device = %snap.device_id, "lease held by a newer session — closing");
                break;
            }
        }
        telemetry::sink().send(TelemetryPoint {
            org_id: snap.org_id,
            device_registry_id: snap.device_registry_id,
            device_id: snap.device_id.clone(),
            pred_dev: snap.pred_dev.clone(),
            metric_name: name.to_string(),
            value: value.to_string(),
            timestamp: chrono::Utc::now(),
            ts_source: "server",
            source_type: "sensor",
            record: true,
        });
        reply(&mut socket, &key, "ok").await;
    }

    // Disconnect: releases the lease (immediate reconnect possible) and leaves
    // an honest last_seen; `active` will flip to false on the next pass of the
    // reaper — sole writer, as in the legacy implementation.
    // Owner-checked: a superseded session never releases the newer lease.
    let _ = device_liveness::release(&ctx.db, snap.device_registry_id, &guard.1).await;
    drop(guard);
}

/// Écrit les mesures découvertes dans le JSONB du registre (clés → true,
/// lecture par `devices::discovered_names`).
async fn persist_discovered(
    db: &DatabaseConnection,
    device_registry_id: i64,
    discovered: &HashSet<String>,
) {
    let map: HashMap<&str, bool> = discovered.iter().map(|n| (n.as_str(), true)).collect();
    let mut active: device_registries::ActiveModel =
        device_registries::Entity::find_by_id(device_registry_id)
            .one(db)
            .await
            .ok()
            .flatten()
            .map(|m| m.into())
            .unwrap_or_default();
    active.discovered_measurements = Set(Some(serde_json::to_value(map).unwrap_or_default()));
    let _ = active.update(db).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(plain: &str) -> String {
        let key = [7u8; 32];
        let wire = encrypt_frame(plain, &key);
        assert_eq!(decrypt_frame(&wire, &key).as_deref(), Some(plain));
        wire
    }

    /// Fresh nonce per message: two encryptions of the same plaintext
    /// differ on the wire (parity with the legacy per-message os.urandom(12)).
    #[test]
    fn nonce_frais_par_message() {
        let key = [7u8; 32];
        assert_ne!(roundtrip("ok"), roundtrip("ok"));
        assert_eq!(decrypt_frame("pas-base64!!", &key), None);
        assert_eq!(decrypt_frame(&STANDARD.encode([0u8; 5]), &key), None);
    }

    /// Cycle complet chiffre/déchiffre sur des payloads réalistes.
    #[test]
    fn cycle_chiffrement_payloads_reels() {
        roundtrip("PING");
        roundtrip("PONG");
        roundtrip("soil_moisture=42");
        roundtrip("error:invalid_capability:measurement 'x' not in device capabilities");
    }

    /// D16 : styles d'écriture variés → même nom canonique.
    #[test]
    fn normalisation_noms_de_mesures() {
        assert_eq!(normalize_measurement_name("soil_moisture"), "soil_moisture");
        assert_eq!(normalize_measurement_name("Soil-Moisture"), "soil_moisture");
        assert_eq!(
            normalize_measurement_name("  soil  moisture "),
            "soil_moisture"
        );
        assert_eq!(
            normalize_measurement_name("Température Extérieure"),
            "temperature_exterieure"
        );
        assert_eq!(normalize_measurement_name("PH;2"), "ph_2");
        assert_eq!(normalize_measurement_name("---"), "");
        assert_eq!(normalize_measurement_name("___x___"), "x");
    }
}
