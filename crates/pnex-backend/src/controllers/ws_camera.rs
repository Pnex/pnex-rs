//! Camera WebSockets (camera-video.md D73/D75).
//!
//! - `GET /ws/camera?device_id=<b64>` + `Authorization: Bearer <b64>`
//!   (D154) — device uplink, binary
//!   Noise NNpsk0 link (D156, same key as `/ws/device`) in raw bytes: the
//!   device's first binary frame is the first Noise message, the server
//!   answers the second, then each binary frame is the sealed
//!   `PXC1 header ‖ JPEG` (chunked Noise messages when larger than 64 KiB). Auth and close codes identical to
//!   `/ws/device` (4002/4001/4006/4008), anti-clone through a dedicated
//!   `CAMERA_SESSIONS` registry (4003) plus a cross-pod Valkey claim
//!   (`camera::claim_uplink`, refreshed by the session; a lost claim closes
//!   with 4003). The token is revalidated periodically like `/ws/device`
//!   (4005 when revoked). Text frames: sealed `PING` → sealed `PONG`.
//! - `GET /ws/camera/live?token=<JWT>&org=<id>&device=<pk>` — browser live
//!   view: raw JPEG binary frames, latest frame first, then every frame of
//!   the CameraHub broadcast. No storage, no polling.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use base64::Engine as _;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, ExprTrait, QueryFilter};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use super::ws_ingest::{decode_param, reject, FrameCodec, Snapshot};
use crate::auth::{jwks, provisioning, settings::RauthySettings};
use crate::models::_entities::{device_registries, organization_members};
use crate::services::camera;
use crate::services::settings::IngestSettings;
use pnex_core::camera::{FrameHeader, DEFAULT_MAX_FRAME_BYTES};

/// Silence on the uplink (no frame, no ping) → close. The device streams
/// at ≥ 1 fps when enabled and closes the socket when disabled.
const UPLINK_WATCHDOG_SECS: u64 = 45;
/// Live viewer heartbeat and watchdog (school of `/ws/notify`).
const LIVE_HEARTBEAT_SECS: u64 = 30;
const LIVE_WATCHDOG_SECS: u64 = 60;

static CAMERA_SESSIONS: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct UplinkGuard {
    org_id: i64,
    device: i64,
    /// Cross-pod claim session id.
    session: String,
}

impl Drop for UplinkGuard {
    fn drop(&mut self) {
        CAMERA_SESSIONS
            .lock()
            .expect("camera sessions")
            .remove(&self.device);
        camera::set_uplink(self.org_id, self.device, false);
        // Drop may run outside a runtime (process teardown): the claim then
        // expires on its own.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let (device, session) = (self.device, std::mem::take(&mut self.session));
            handle.spawn(async move { camera::release_uplink(device, &session).await });
        }
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/ws")
        .add("/camera", get(ws_camera))
        .add("/camera/live", get(ws_camera_live))
}

fn max_frame_bytes() -> usize {
    std::env::var("PNEX_CAMERA_MAX_FRAME_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_FRAME_BYTES)
}

// ───────────────────────────── Device uplink ─────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CameraQuery {
    device_id: Option<String>,
}

async fn ws_camera(
    State(ctx): State<AppContext>,
    Query(q): Query<CameraQuery>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let ingest = crate::services::settings::IngestSettings::from_config(&ctx.config);
    if !super::ws_ingest::arrived_over_tls(&headers, &ingest) {
        return reject(ws, super::ws_ingest::CLOSE_TLS_REQUIRED, "TLS required");
    }
    let Some(raw_token) = super::ws_ingest::device_token(&headers) else {
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
    // D153 (L4): the TLS client certificate must be this device's.
    if !matches!(
        super::ws_ingest::client_cert_matches(&ctx.db, &headers, &ingest, device.id).await,
        Ok(true)
    ) {
        return reject(
            ws,
            super::ws_ingest::CLOSE_CLIENT_CERT,
            "Client certificate required",
        );
    }
    let Some(key) = tok
        .encryption_key
        .as_deref()
        .and_then(|k| {
            base64::engine::general_purpose::STANDARD
                .decode(k.trim())
                .ok()
        })
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
    else {
        return reject(ws, 4008, "No encryption key");
    };
    let settings = IngestSettings::from_config(&ctx.config);
    let max = max_frame_bytes();
    let session = UplinkSession {
        db: ctx.db.clone(),
        token,
        revalidate_every: Duration::from_secs(settings.token_cache_secs),
    };
    let max_wire = max
        + max.div_ceil(pnex_core::frame::NOISE_MAX_CHUNK) * pnex_core::frame::NOISE_TAG_LEN
        + 64;
    ws.max_message_size(max_wire)
        .on_upgrade(move |mut socket| async move {
            // Handshake FIRST: only a peer holding the device key takes the
            // anti-clone slot (a stolen token alone cannot keep the camera out).
            let Some(codec) = FrameCodec::accept(&mut socket, &key, &device.device_id, true).await
            else {
                return;
            };
            let fresh = CAMERA_SESSIONS
                .lock()
                .expect("camera sessions")
                .insert(device.id);
            if !fresh {
                return close_with(&mut socket, 4003, "Camera already connected").await;
            }
            let guard = UplinkGuard {
                org_id: device.org_id,
                device: device.id,
                session: uuid::Uuid::new_v4().simple().to_string(),
            };
            // Cross-pod anti-clone: another pod may hold a live uplink.
            if !camera::claim_uplink(device.id, &guard.session).await {
                return close_with(&mut socket, 4003, "Camera already connected").await;
            }
            camera::set_uplink(device.org_id, device.id, true);
            uplink_loop(socket, codec, device, guard, max, session).await;
        })
        .into_response()
}

/// What the uplink loop needs to revalidate its token.
struct UplinkSession {
    db: DatabaseConnection,
    token: String,
    revalidate_every: Duration,
}

/// Token still active and still bound to the same device?
async fn token_still_valid(db: &DatabaseConnection, token: &str, device_id: &str) -> bool {
    matches!(Snapshot::load(db, token).await, Ok(Some((_, d))) if d.device_id == device_id)
}

async fn close_with(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

async fn uplink_loop(
    mut socket: WebSocket,
    mut codec: FrameCodec,
    device: device_registries::Model,
    guard: UplinkGuard,
    max: usize,
    session: UplinkSession,
) {
    let mut bad_frames: u32 = 0;
    let slug: std::sync::Arc<str> = std::sync::Arc::from(device.device_id.as_str());
    let mut last_validation = tokio::time::Instant::now();
    let mut last_claim = tokio::time::Instant::now();
    loop {
        // Periodic token revalidation (4005, parity with /ws/device).
        if last_validation.elapsed() >= session.revalidate_every {
            if !token_still_valid(&session.db, &session.token, &device.device_id).await {
                tracing::info!(device = %device.device_id, "ws/camera: token revoked, closing");
                close_with(&mut socket, 4005, "Token invalid").await;
                break;
            }
            last_validation = tokio::time::Instant::now();
        }
        // Cross-pod claim refresh: losing it means another uplink won.
        if last_claim.elapsed() >= camera::UPLINK_CLAIM_REFRESH {
            if !camera::refresh_uplink(device.id, &guard.session).await {
                tracing::warn!(device = %device.device_id, "ws/camera: uplink claimed elsewhere, closing");
                close_with(&mut socket, 4003, "Camera already connected").await;
                break;
            }
            last_claim = tokio::time::Instant::now();
        }
        let incoming =
            tokio::time::timeout(Duration::from_secs(UPLINK_WATCHDOG_SECS), socket.recv()).await;
        let msg = match incoming {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => {
                tracing::debug!(device = %device.device_id, "ws/camera: silent uplink, closing");
                break;
            }
        };
        match msg {
            Message::Binary(wire) => {
                if wire.len()
                    > max
                        + max.div_ceil(pnex_core::frame::NOISE_MAX_CHUNK)
                            * pnex_core::frame::NOISE_TAG_LEN
                        + pnex_core::frame::NOISE_TAG_LEN
                {
                    bad_frames += 1;
                    continue;
                }
                let Some(plain) = codec.open_bytes(&wire) else {
                    // A rejected single-chunk frame leaves the Noise counter
                    // untouched (dropped, link kept); a multi-chunk one
                    // rejected midway desynchronizes it: close.
                    if wire.len() > pnex_core::frame::NOISE_MAX_MSG {
                        tracing::warn!(device = %device.device_id, "ws/camera: unauthenticated frame, closing");
                        break;
                    }
                    bad_frames += 1;
                    continue;
                };
                match FrameHeader::parse(&plain) {
                    Ok((header, _)) => {
                        let jpeg = Bytes::from(plain).slice(pnex_core::camera::FRAME_HEADER_LEN..);
                        camera::publish(device.org_id, device.id, &slug, header, jpeg);
                    }
                    Err(e) => {
                        bad_frames += 1;
                        // Authenticated but malformed header: log once per burst.
                        if bad_frames.is_power_of_two() {
                            tracing::warn!(device = %device.device_id, bad_frames, "ws/camera: {e}");
                        }
                    }
                }
            }
            // Keepalive inside the Noise link too (base64 text frames).
            Message::Text(t) => {
                if codec.open(t.as_str()).as_deref().map(str::trim) != Some("PING") {
                    tracing::warn!(device = %device.device_id, "ws/camera: unauthenticated text frame, closing");
                    break;
                }
                if socket
                    .send(Message::Text(codec.seal("PONG").into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

// ───────────────────────────── Browser live ─────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LiveQuery {
    token: Option<String>,
    org: Option<i64>,
    device: Option<i64>,
}

async fn ws_camera_live(
    State(ctx): State<AppContext>,
    Query(q): Query<LiveQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = q.token.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
        return reject(ws, 4002, "No token provided");
    };
    let (Some(org_id), Some(device_pk)) = (q.org, q.device) else {
        return reject(ws, 4006, "No org or device provided");
    };
    let Ok(settings) = RauthySettings::from_config(&ctx.config) else {
        return reject(ws, 1011, "Server config error");
    };
    let claims = match jwks::verifier_for(&settings).await.verify(token).await {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "ws/camera/live: JWT rejected");
            return reject(ws, 4001, "Invalid token");
        }
    };
    let Ok(user) = provisioning::get_or_create_user(&ctx.db, &claims).await else {
        return reject(ws, 1011, "Server error");
    };
    let member = organization_members::Entity::find()
        .filter(
            organization_members::Column::UserId
                .eq(user.id)
                .and(organization_members::Column::OrgId.eq(org_id)),
        )
        .one(&ctx.db)
        .await;
    if !matches!(member, Ok(Some(_))) {
        return reject(ws, 4006, "Not a member of this org");
    }
    // Device of the org (cross-org = same answer as unknown).
    let device = device_registries::Entity::find_by_id(device_pk)
        .filter(device_registries::Column::OrgId.eq(org_id))
        .one(&ctx.db)
        .await;
    if !matches!(device, Ok(Some(_))) {
        return reject(ws, 4004, "Unknown camera");
    }
    let guard = camera::viewer_join(&ctx.db, org_id, device_pk).await;
    ws.on_upgrade(move |socket| live_loop(socket, org_id, device_pk, guard))
        .into_response()
}

async fn live_loop(mut socket: WebSocket, org_id: i64, device: i64, _guard: camera::ViewerGuard) {
    let (latest, mut rx) = camera::subscribe(org_id, device);
    if let Some(frame) = latest {
        if socket
            .send(Message::Binary(frame.jpeg.clone()))
            .await
            .is_err()
        {
            return;
        }
    }
    // First tick one period away (a bare `interval` fires immediately).
    let period = Duration::from_secs(LIVE_HEARTBEAT_SECS);
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            frame = rx.recv() => match frame {
                Ok(frame) => {
                    if socket.send(Message::Binary(frame.jpeg.clone())).await.is_err() {
                        break;
                    }
                }
                // Slow viewer: skip what was missed, keep going.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            _ = heartbeat.tick() => {
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    break;
                }
            }
            incoming = tokio::time::timeout(Duration::from_secs(LIVE_WATCHDOG_SECS), socket.recv()) => {
                match incoming {
                    Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
    }
}
