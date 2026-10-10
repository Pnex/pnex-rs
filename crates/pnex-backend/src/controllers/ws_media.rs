//! Media link of a capture box (media-ingest.md D159, D160, lot 6b).
//!
//! `GET /ws/media` + `Authorization: Bearer <token>`: same admission as
//! `/ws/camera` (TLS edge, client certificate of the device, binary Noise
//! NNpsk0 link, D153–D158), edge agents only (4004 otherwise). Org and
//! device come from that identity (R1), never from a frame.
//!
//! - server → box (sealed text): [`MediaDownMsg::Streams`] on connect and
//!   whenever the list changes (checked every [`STREAMS_EVERY`]),
//!   [`MediaDownMsg::Ack`] after each segment, `PONG`;
//! - box → server: sealed binary `u16 len ‖ SegmentHeader ‖ WAV` (a stream
//!   this box does not capture = `not-found`, the 404 of the HTTP upload),
//!   sealed text [`MediaUpMsg::State`] and `PING`.
//!
//! Segments go through `segments::accept_upload`, the checks of the worker
//! upload; uploads are rate limited per device across pods (Valkey).

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use loco_rs::prelude::*;
use pnex_core::media_ingest::{
    decode_segment, DeviceStream, MediaDownMsg, MediaUpMsg, SEGMENT_SECS_MAX,
};
use pnex_media_capture::CaptureError;
use uuid::Uuid;

use super::device_link::{active_token, close_socket, reject, FrameCodec};
use crate::models::_entities::device_registries;
use crate::services::media_ingest::{boxes, segments};
use crate::services::rate_limit::{Decision, RateLimiter};
use crate::services::settings::IngestSettings;

/// How often the stream list (and its secrets) is re-read for the box.
const STREAMS_EVERY: Duration = Duration::from_secs(15);
/// Silence on the link (no segment, no ping) → close.
const WATCHDOG: Duration = Duration::from_secs(45);
/// Segment uploads per device and minute (a 10 s segment per stream is 6).
const UPLOADS_PER_MINUTE: u32 = 60;
/// Largest plaintext frame: the longest segment + overlap + slack, plus
/// its header.
const MAX_PLAIN: usize =
    44 + (SEGMENT_SECS_MAX as usize + 4) * pnex_core::media_ingest::SAMPLE_RATE as usize * 2 + 4096;

fn max_wire() -> usize {
    MAX_PLAIN
        + MAX_PLAIN.div_ceil(pnex_core::frame::NOISE_MAX_CHUNK) * pnex_core::frame::NOISE_TAG_LEN
        + pnex_core::frame::NOISE_TAG_LEN
}

pub fn routes() -> Routes {
    Routes::new().prefix("/ws").add("/media", get(ws_media))
}

async fn ws_media(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let ingest = IngestSettings::from_config(&ctx.config);
    if !super::device_link::arrived_over_tls(&headers, &ingest) {
        return reject(ws, super::device_link::CLOSE_TLS_REQUIRED, "TLS required");
    }
    let super::device_link::DeviceAuth { token, device, key } =
        match super::device_link::authenticate_device(&ctx.db, &headers, &ingest).await {
            Ok(auth) => auth,
            Err(refusal) => return reject(ws, refusal.code, refusal.reason),
        };
    if !super::edge_agents::is_agent(&ctx.db, &device).await {
        return reject(ws, 4004, "Not a capture box");
    }
    let revalidate_every = Duration::from_secs(ingest.token_cache_secs);
    ws.max_message_size(max_wire())
        .on_upgrade(move |mut socket| async move {
            let Some(codec) = FrameCodec::accept(&mut socket, &key, &device.device_id, true).await
            else {
                return;
            };
            session(socket, codec, ctx, token, device, revalidate_every).await;
        })
        .into_response()
}

async fn send(socket: &mut WebSocket, codec: &mut FrameCodec, msg: &MediaDownMsg) -> bool {
    let Ok(plain) = serde_json::to_string(msg) else {
        return false;
    };
    socket
        .send(Message::Text(codec.seal(&plain).into()))
        .await
        .is_ok()
}

async fn session(
    mut socket: WebSocket,
    mut codec: FrameCodec,
    ctx: AppContext,
    token: String,
    device: device_registries::Model,
    revalidate_every: Duration,
) {
    let limiter = RateLimiter::new(crate::services::shared_valkey::conn(&ctx.config).await);
    let ring = crate::services::secrets::Keyring::from_config(&ctx.config).ok();
    let mut sent: Option<Vec<DeviceStream>> = None;
    let mut tick = tokio::time::interval(STREAMS_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_validation = tokio::time::Instant::now();
    tracing::info!(device = %device.device_id, "media link up");
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if last_validation.elapsed() >= revalidate_every {
                    let valid = matches!(
                        active_token(&ctx.db, &token).await,
                        Ok(Some((_, d))) if d.id == device.id
                    );
                    if !valid {
                        close_socket(&mut socket, 4005, "Token invalid").await;
                        break;
                    }
                    last_validation = tokio::time::Instant::now();
                }
                match boxes::streams_for(&ctx.db, ring.as_ref(), &device).await {
                    Ok(list) if sent.as_ref() != Some(&list) => {
                        if !send(&mut socket, &mut codec, &MediaDownMsg::Streams { streams: list.clone() }).await {
                            break;
                        }
                        sent = Some(list);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(device = %device.device_id, error = %e, "media link: stream list unavailable"),
                }
            }
            incoming = tokio::time::timeout(WATCHDOG, socket.recv()) => {
                let msg = match incoming {
                    Ok(Some(Ok(m))) => m,
                    Ok(Some(Err(_))) | Ok(None) => break,
                    Err(_) => {
                        tracing::debug!(device = %device.device_id, "media link silent, closing");
                        break;
                    }
                };
                match msg {
                    Message::Binary(wire) => {
                        if wire.len() > max_wire() {
                            break;
                        }
                        // A refused frame (forged, replayed) desynchronizes
                        // the Noise counters: the link closes.
                        let Some(plain) = codec.open_bytes(&wire) else {
                            tracing::warn!(device = %device.device_id, "media link: unauthenticated frame, closing");
                            break;
                        };
                        let ack = upload(&ctx, &limiter, &device, &plain).await;
                        if !send(&mut socket, &mut codec, &ack).await {
                            break;
                        }
                    }
                    Message::Text(t) => {
                        let Some(plain) = codec.open(t.as_str()) else {
                            tracing::warn!(device = %device.device_id, "media link: unauthenticated text frame, closing");
                            break;
                        };
                        if plain.trim() == "PING" {
                            if socket.send(Message::Text(codec.seal("PONG").into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        if let Ok(MediaUpMsg::State { stream_id, state, error }) = serde_json::from_str(&plain) {
                            report(&ctx, &device, &stream_id, state, error.as_deref()).await;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    // shortcut: a box that reconnected on another pod before this one saw
    // the drop is marked offline until its next state report; a cross-pod
    // session claim would close that gap if it shows up in practice.
    if let Err(e) = boxes::mark_offline(&ctx.db, &device).await {
        tracing::warn!(device = %device.device_id, error = %e, "media link: offline state not saved");
    }
    tracing::info!(device = %device.device_id, "media link down");
}

/// Capture state reported by the box: only its own streams, only known
/// error codes (an untrusted box never writes free text, D160).
async fn report(
    ctx: &AppContext,
    device: &device_registries::Model,
    stream_id: &str,
    state: pnex_core::media_ingest::CaptureState,
    error: Option<&str>,
) {
    let Ok(id) = Uuid::parse_str(stream_id) else {
        return;
    };
    let error = error
        .and_then(CaptureError::from_code)
        .map(CaptureError::code);
    if let Err(e) = boxes::report_state(&ctx.db, device, id, state, error).await {
        tracing::warn!(device = %device.device_id, error = %e, "media link: capture state not saved");
    }
}

/// One uploaded segment → its acknowledgement.
async fn upload(
    ctx: &AppContext,
    limiter: &RateLimiter,
    device: &device_registries::Model,
    plain: &[u8],
) -> MediaDownMsg {
    let nack = |stream_id: String, seq: i64, code: &str| MediaDownMsg::Ack {
        stream_id,
        seq,
        ok: false,
        code: Some(code.to_string()),
    };
    let Some((header, wav)) = decode_segment(plain) else {
        return nack(String::new(), -1, "invalid");
    };
    let (stream_id, seq) = (header.stream_id.clone(), header.seq);
    let bucket = format!("media-upload:{}", device.id);
    if let Decision::Deny { .. } = limiter
        .hit(&bucket, UPLOADS_PER_MINUTE, Duration::from_secs(60))
        .await
    {
        return nack(stream_id, seq, "rate-limited");
    }
    let Ok(id) = Uuid::parse_str(&header.stream_id) else {
        return nack(stream_id, seq, "not-found");
    };
    let stream = match boxes::stream_of(&ctx.db, device, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return nack(stream_id, seq, "not-found"),
        Err(_) => return nack(stream_id, seq, "unavailable"),
    };
    match segments::accept_upload(
        ctx,
        &stream,
        header.seq,
        header.started_ms,
        header.ended_ms,
        header.clock,
        wav,
    )
    .await
    {
        Ok(_) => MediaDownMsg::Ack {
            stream_id,
            seq,
            ok: true,
            code: None,
        },
        Err(segments::UploadError::Invalid) => nack(stream_id, seq, "invalid"),
        Err(segments::UploadError::Unavailable) => nack(stream_id, seq, "unavailable"),
    }
}
