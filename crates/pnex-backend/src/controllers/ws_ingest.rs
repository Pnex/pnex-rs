//! Ingestion télémétrie — WS `/ws/sensor/ingest`.
//!
//! Parité `SensorIngest` Django (`docs/phase0/ws-channels-crypto.md` §2.1) :
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
//! Durcissements assumés vs Django :
//! - **anti-clone double étage** : map des sessions ouvertes en-process
//!   (rejet 4003 immédiat, y compris course < TTL) puis fallback PG
//!   `device_states` (couvre un autre process / un crash sans close) ;
//! - **libération propre du bail** à la déconnexion (Django gardait la
//!   fenêtre de 12 s) — un reconnect immédiat du vrai device est accepté ;
//! - `last_seen` rafraîchi sur **toute frame valide** (Django ne comptait
//!   que les PING : un device ne envoyant que des mesures passait inactif) ;
//! - revalidation token/device **avec cache 10 s** (§7.8 : le cache Django
//!   était du code mort, la DB était requêtée à chaque frame à ~10 fps).

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::{ChaCha20, Key, Nonce};
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use crate::models::_entities::{device_registries, device_tokens, predefined_devices};
use crate::services::device_liveness;
use crate::services::settings::IngestSettings;
use crate::services::telemetry::{self, TelemetryPoint};

// ───────────────────────── Aides chiffrement ─────────────────────────

/// Déchiffre une frame device : `base64(nonce 12 ‖ ct)` → plaintext UTF-8.
/// None = illisible (→ `ERROR:decryption_failed`, parité Django qui
/// valait len(key)==32 et len(combined)≥12).
fn decrypt_frame(raw: &str, key: &[u8; 32]) -> Option<String> {
    let trimmed = raw.trim();
    let bytes = STANDARD
        .decode(trimmed)
        .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed))
        .ok()?;
    if bytes.len() < 12 {
        return None;
    }
    let (nonce, ct) = bytes.split_at(12);
    let mut buf = ct.to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(nonce)).apply_keystream(&mut buf);
    String::from_utf8(buf).ok()
}

/// Chiffre une frame serveur : plaintext → `base64(nonce 12 ‖ ct)`.
fn encrypt_frame(plain: &str, key: &[u8; 32]) -> String {
    let mut nonce = [0u8; 12];
    use rand::RngCore;
    rand::rng().fill_bytes(&mut nonce);
    let mut buf = plain.as_bytes().to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(&nonce)).apply_keystream(&mut buf);
    let mut wire = nonce.to_vec();
    wire.extend_from_slice(&buf);
    STANDARD.encode(wire)
}

// ──────────────── Normalisation des noms de mesures ────────────────

/// Harmonisation capability ↔ mesure (D16) : trim, pliage des accents
/// (deunicode), minuscules, tout non `[a-z0-9_:]` → `_` (répétitions
/// fondues, `_` de bord supprimés). `Soil-Moisture`, `soil moisture` et
/// `soil_moisture` → `soil_moisture`. Vide si le nom n'est que des
/// séparateurs (→ `error:invalid_format` côté appelant). Appliquée avant
/// validation stricte, découverte dynamique ET stockage — la même mesure a
/// le même nom partout (O2 compris, où promwrite n'a plus qu'un rôle de
/// garde-fou).
pub(crate) fn normalize_measurement_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_sep = false; // séparateur fondu, flushé devant du contenu
    for c in deunicode::deunicode(raw.trim()).chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == ':' {
            if pending_sep {
                out.push('_');
                pending_sep = false;
            }
            out.push(c);
        } else {
            pending_sep = !out.is_empty();
        }
    }
    out
}

// ───────────────────────── Sessions ouvertes ─────────────────────────

/// Devices avec une session WS ouverte dans CE process — étage 1 de
/// l'anti-clone (rejet 4003 immédiat, sans course).
static OPEN_SESSIONS: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Retire le device des sessions ouvertes à la sortie, tous chemins compris.
struct SessionGuard(i64);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        OPEN_SESSIONS.lock().expect("sessions").remove(&self.0);
    }
}

// ───────────────────────── Snapshot validé ─────────────────────────

/// Vue du device validée (rafraîchie au rythme du cache de revalidation) :
/// porte le routage org (D2 — suit un changement d'org du device) et les
/// règles de validation des mesures.
struct Snapshot {
    device_registry_id: i64,
    org_id: i64,
    device_id: String,
    pred_dev: String,
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
    async fn load(
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

/// Accepte l'upgrade puis referme immédiatement avec le code — Django
/// `close(code=…)` ; un statut HTTP ne peut pas porter un 4xxx WS.
fn reject(ws: WebSocketUpgrade, code: u16, reason: &'static str) -> Response {
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
fn decode_param(raw: &str) -> Option<String> {
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

    // Auth (ordre Django : 4002 sans token, 4001 décodage/lookup,
    // 4006 mismatch, 4008 sans clé, 4003 déjà connecté).
    let Some(raw_token) = q.token.as_deref() else {
        return reject(ws, 4002, "No token provided");
    };
    let token = match decode_param(raw_token) {
        Some(t) if !t.is_empty() => t,
        _ => return reject(ws, 4001, "Authentication failed"),
    };
    // Django : device_id absent → compare str != None → mismatch 4006.
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

    // Anti-clone : (a) session ouverte en-process → 4003 immédiat ;
    // (b) fallback PG — last_seen frais d'une session non refermée
    // (crash du process précédent sans close, ou autre réplica).
    {
        let mut open = OPEN_SESSIONS.lock().expect("sessions");
        if open.contains(&snap.device_registry_id) {
            return reject(ws, 4003, "Device already connected");
        }
        open.insert(snap.device_registry_id);
    }
    let guard = SessionGuard(snap.device_registry_id);
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

    // Prise de bail : le device est vu maintenant (le reaper activera).
    let _ = device_liveness::touch(&ctx.db, snap.device_registry_id, Some(true)).await;

    let token_str = token.clone();
    ws.on_upgrade(move |socket| async move {
        session_loop(socket, &ctx, token_str, key, snap, guard, settings).await;
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
    settings: IngestSettings,
) {
    let cache = Duration::from_secs(settings.token_cache_secs);
    let throttle = Duration::from_secs(1);
    let mut last_validation = Instant::now();
    let mut last_touch = Instant::now();
    // Les rejets de déchiffrement ne sont pas loggés par frame (~10 fps) :
    // un avertissement unique par session suffit à voir une firmware qui
    // parle en clair ou une clé désynchronisée.
    let mut warned_decrypt_failure = false;

    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(text) = msg else {
            // v1 : texte uniquement ; close/erreurs terminent la boucle au
            // prochain tour (recv → None).
            continue;
        };

        // Revalidation périodique (4005 : token/device invalidé en session).
        if last_validation.elapsed() >= cache {
            let fresh = async {
                let (_, device) = Snapshot::load(&ctx.db, &token).await.ok()??;
                if device.device_id != snap.device_id {
                    return None;
                }
                Snapshot::from_device(&ctx.db, device).await.ok()
            }
            .await;
            match fresh {
                Some(s) => snap = s,
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
                let _ = device_liveness::touch(&ctx.db, snap.device_registry_id, None).await;
                last_touch = Instant::now();
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

        // Validation : strict → capacités du modèle ; dynamic → découverte
        // plafonnée (`ping=x` suit le même chemin — parité Django).
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
            let _ = device_liveness::touch(&ctx.db, snap.device_registry_id, None).await;
            last_touch = Instant::now();
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
        });
        reply(&mut socket, &key, "ok").await;
    }

    // Déconnexion : libère le bail (reconnect immédiat possible) et laisse
    // un last_seen honnête ; `active` passera false au prochain passage du
    // reaper — seul écrivain, parité Django.
    let _ = device_liveness::release(&ctx.db, snap.device_registry_id).await;
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

    /// Nonce frais par message : deux chiffrements du même plaintext
    /// diffèrent sur le wire (parité os.urandom(12) par message Django).
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
