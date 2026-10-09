//! Shared plumbing of the device links (`/ws/device`, `/ws/camera`, OTA
//! download): authentication, TLS edge checks, Noise framing, admission
//! throttling.
//!
//! - auth (`authenticate_device`): `Authorization: Bearer <token>` (D154:
//!   never in the URL; the device is the token's own), TLS edge required
//!   (4013), client certificate of that device (4014);
//! - then a Noise NNpsk0 handshake (D156, `FrameCodec::accept`): the
//!   device's first frame is the first Noise message, the server answers
//!   the second; every later frame, both ways, is a Noise transport message
//!   (4011 when the handshake fails);
//! - close codes: 4001 auth failed, 4002 no token, 4011 handshake failed,
//!   4013 TLS required, 4014 client certificate refused.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::models::_entities::{device_registries, device_tokens};
use crate::services::settings::IngestSettings;

// ───────────────────────── Noise framing ─────────────────────────

// Shared with the edge agent (D95): single implementation in pnex-core.
use pnex_core::frame::Link;

/// Max wait for the device's first Noise message after the upgrade.
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Close code of a failed Noise handshake (wrong key, wrong device id,
/// firmware built before the Noise link, D157).
pub(crate) const CLOSE_HANDSHAKE_FAILED: u16 = 4011;

/// Encrypted link of one device connection (Noise NNpsk0, D156).
pub(crate) struct FrameCodec(Link);

impl FrameCodec {
    /// Runs the server side of the Noise handshake right after the
    /// upgrade: reads the device's first message (base64 text, or raw bytes
    /// on the camera link), answers the second one. On failure the socket
    /// is closed with 4011 and `None` is returned.
    pub(crate) async fn accept(
        socket: &mut WebSocket,
        key: &[u8; 32],
        device_id: &str,
        binary: bool,
    ) -> Option<Self> {
        let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            loop {
                match socket.recv().await {
                    Some(Ok(Message::Text(t))) if !binary => {
                        return pnex_core::frame::decode_handshake(t.as_str());
                    }
                    Some(Ok(Message::Binary(b))) if binary => return Some(b.to_vec()),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    _ => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();
        let accepted = first.and_then(|msg1| pnex_core::frame::respond(key, device_id, &msg1));
        let Some((link, msg2)) = accepted else {
            tracing::warn!(device = %device_id, "Noise handshake failed (key, device id or firmware older than the Noise link)");
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: CLOSE_HANDSHAKE_FAILED,
                    reason: "Handshake failed".into(),
                })))
                .await;
            return None;
        };
        let answer = if binary {
            Message::Binary(msg2.into())
        } else {
            Message::Text(pnex_core::frame::encode_handshake(&msg2).into())
        };
        socket.send(answer).await.ok()?;
        Some(Self(link))
    }

    /// Text frame for the device.
    pub(crate) fn seal(&mut self, plain: &str) -> String {
        self.0.seal_text(plain)
    }

    /// Text frame of the device; `None` = forged, modified or replayed.
    pub(crate) fn open(&mut self, raw: &str) -> Option<String> {
        self.0.open_text(raw)
    }

    /// Binary frame of the device (camera uplink).
    pub(crate) fn open_bytes(&mut self, wire: &[u8]) -> Option<Vec<u8>> {
        self.0.open(wire)
    }
}

// ──────────────── Measurement names ────────────────

/// Harmonisation capability ↔ mesure (D16) — la fonction vit dans pnex-core
/// (feature `naming`) : le nœud `device` du runtime de flows (Phase 6) doit
/// lire les séries sous le MÊME nom normalisé que l'ingestion les écrit.
/// Re-exportée ici pour ne pas déplacer les points d'appel.
pub(crate) use pnex_core::normalize_measurement_name;

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

// ───────────────────────── Device token ─────────────────────────

/// Active token row and its device. `Ok(None)` = unknown or inactive token
/// (→ 4001).
pub(crate) async fn active_token(
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

/// Device credential of a request (D154): `Authorization: Bearer
/// <b64(token)>` only — never a URL, which ends up in access logs.
pub(crate) fn device_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// True when the request carries the edge secret (`X-Pnex-Edge`): only
/// then are the edge's headers trusted. Constant-time comparison.
pub(crate) fn from_trusted_edge(
    headers: &axum::http::HeaderMap,
    settings: &crate::services::settings::IngestSettings,
) -> bool {
    let Some(secret) = settings.edge_secret.as_deref() else {
        return false;
    };
    let Some(sent) = headers.get("x-pnex-edge").map(|v| v.as_bytes()) else {
        return false;
    };
    sent.len() == secret.len()
        && sent
            .iter()
            .zip(secret.as_bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// D154: a device link must have come through the TLS edge (its header,
/// trusted only from the edge itself).
pub(crate) fn arrived_over_tls(
    headers: &axum::http::HeaderMap,
    settings: &crate::services::settings::IngestSettings,
) -> bool {
    !settings.require_tls
        || (from_trusted_edge(headers, settings)
            && headers
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|p| p.trim().eq_ignore_ascii_case("https")))
}

/// Close code of a device link refused for its client certificate.
pub(crate) const CLOSE_CLIENT_CERT: u16 = 4014;

/// D153 (L4): the client certificate the edge saw on the TLS link
/// (`X-Client-Cert`, URL-encoded PEM — nginx `$ssl_client_escaped_cert`)
/// must be a live certificate of device `device_pk`. `Ok(true)` when the
/// requirement is off.
pub async fn client_cert_matches(
    db: &sea_orm::DatabaseConnection,
    headers: &axum::http::HeaderMap,
    settings: &crate::services::settings::IngestSettings,
    device_pk: i64,
) -> Result<bool, sea_orm::DbErr> {
    if !settings.require_client_cert {
        return Ok(true);
    }
    if !from_trusted_edge(headers, settings) {
        return Ok(false);
    }
    let Some(escaped) = headers
        .get("x-client-cert")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
    else {
        return Ok(false);
    };
    let pem = percent_encoding::percent_decode_str(escaped).decode_utf8_lossy();
    match crate::services::device_pki::verify_client_cert(db, &pem).await? {
        Ok((_, pk)) if pk == device_pk => Ok(true),
        Ok((_, pk)) => {
            tracing::warn!(
                device_pk,
                cert_device_pk = pk,
                "client certificate of another device"
            );
            Ok(false)
        }
        Err(refusal) => {
            tracing::warn!(device_pk, ?refusal, "client certificate refused");
            Ok(false)
        }
    }
}

/// Close code of a device link refused because it did not arrive over TLS.
pub(crate) const CLOSE_TLS_REQUIRED: u16 = 4013;

/// Refused device admission: websocket close code + reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub code: u16,
    pub reason: &'static str,
}

const AUTH_FAILED: Refusal = Refusal {
    code: 4001,
    reason: "Authentication failed",
};

/// Authenticated device link (D153/D154): the token, its device and the
/// Noise pre-shared key.
pub(crate) struct DeviceAuth {
    pub token: String,
    pub device: device_registries::Model,
    pub key: [u8; 32],
}

/// Authenticates a device link, the same way on every device route: the
/// raw token in `Authorization: Bearer` (the device is the token's own, no
/// claimed id to cross-check), the TLS client certificate of that device,
/// and its Noise key. Close codes: 4002 no token, 4001 unknown or inactive
/// token, 4014 client certificate refused.
pub(crate) async fn authenticate_device(
    db: &DatabaseConnection,
    headers: &axum::http::HeaderMap,
    settings: &IngestSettings,
) -> std::result::Result<DeviceAuth, Refusal> {
    let Some(token) = device_token(headers) else {
        return Err(Refusal {
            code: 4002,
            reason: "No token provided",
        });
    };
    let Ok(Some((token_row, device))) = active_token(db, token).await else {
        return Err(AUTH_FAILED);
    };
    if !matches!(
        client_cert_matches(db, headers, settings, device.id).await,
        Ok(true)
    ) {
        return Err(Refusal {
            code: CLOSE_CLIENT_CERT,
            reason: "Client certificate required",
        });
    }
    let key = STANDARD
        .decode(token_row.encryption_key.trim())
        .ok()
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
        .ok_or(AUTH_FAILED)?;
    Ok(DeviceAuth {
        token: token.to_string(),
        device,
        key,
    })
}

/// Closes an upgraded socket with a WS close code.
pub(crate) async fn close_socket(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full encrypt/decrypt cycle over realistic payloads on a Noise link.
    #[test]
    fn link_roundtrip_real_payloads() {
        let key = [7u8; 32];
        let (init, msg1) = pnex_core::frame::Initiator::start(&key, "dev");
        let (mut srv, msg2) = pnex_core::frame::respond(&key, "dev", &msg1).expect("respond");
        let mut dev = init.finish(&msg2).expect("finish");
        for plain in [
            "PING",
            "PONG",
            "soil_moisture=42",
            "error:invalid_capability:measurement 'x' not in device capabilities",
        ] {
            assert_eq!(srv.open_text(&dev.seal_text(plain)).as_deref(), Some(plain));
        }
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

#[cfg(test)]
mod d154_tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn token_comes_from_the_authorization_header_only() {
        let mut headers = HeaderMap::new();
        assert_eq!(device_token(&headers), None);
        headers.insert("authorization", HeaderValue::from_static("Bearer dG9r"));
        assert_eq!(device_token(&headers), Some("dG9r"));
        headers.insert("authorization", HeaderValue::from_static("Basic abc"));
        assert_eq!(device_token(&headers), None);
        headers.insert("authorization", HeaderValue::from_static("Bearer   "));
        assert_eq!(device_token(&headers), None);
    }

    #[test]
    fn device_links_need_the_tls_edge() {
        let strict = IngestSettings {
            edge_secret: Some("edge-secret-0123456789".into()),
            ..IngestSettings::default()
        };
        assert!(strict.require_tls, "secure by default");
        let mut headers = HeaderMap::new();
        assert!(!arrived_over_tls(&headers, &strict));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        // Forged by a client reaching the backend directly: no edge secret.
        assert!(!arrived_over_tls(&headers, &strict));
        headers.insert(
            "x-pnex-edge",
            HeaderValue::from_static("edge-secret-WRONG-0000"),
        );
        assert!(!arrived_over_tls(&headers, &strict));
        headers.insert(
            "x-pnex-edge",
            HeaderValue::from_static("edge-secret-0123456789"),
        );
        assert!(arrived_over_tls(&headers, &strict));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(!arrived_over_tls(&headers, &strict));
        // No secret configured: the edge headers are never trusted.
        let unset = IngestSettings::default();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(!arrived_over_tls(&headers, &unset));
        let open = IngestSettings {
            require_tls: false,
            ..IngestSettings::default()
        };
        assert!(arrived_over_tls(&HeaderMap::new(), &open));
    }
}
