//! Endpoint interne d'écriture device — `POST /internal/flow/device-write`.
//!
//! Consommateur : le runtime de flows (nœud `pnex-device-write`) — le
//! backend ne liant jamais le moteur, le downlink transite par HTTP interne
//! (école `/internal/notify/deliver` : le runtime n'a jamais d'accès direct
//! au WS device). Le backend applique les mêmes règles que le POST commands
//! utilisateur : pin output-capable (`digital_out`/`pwm_out`), digital =
//! booléen 0/1, pwm = duty % 0..=100 (le firmware met à l'échelle en 8-bit),
//! `caps::validate` au set_mode, device hors ligne → 409.
//!
//! Auth : jeton de service dans `x-pnex-flow-token` comparé à
//! `settings.flow.runtime_token` (env `PNEX_FLOW_RUNTIME_TOKEN` prioritaire)
//! — absent/vide ⇒ 401 systématique (fail-closed, école
//! `/internal/notify/deliver`). Le token n'apparaît jamais dans les logs.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;

use crate::controllers::ws_device;
use crate::models::_entities::{device_capability_instances, device_registries};
use crate::services::flow::FlowSettings;
use pnex_core::{Mode, ServerMsg};

const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";

/// Serialized size cap of a custom-firmware command value (D146).
const COMMAND_VALUE_MAX_BYTES: usize = 512;

#[derive(Debug, Deserialize)]
pub struct DeviceWriteInput {
    pub org_id: i64,
    /// Slug du device cible.
    pub device_id: String,
    /// Map `{pin label: valeur}` — digital = bool/0-1, pwm = duty 0..=100.
    #[serde(default)]
    pub values: serde_json::Map<String, serde_json::Value>,
    /// Custom-firmware commands `{name: value}` (D146) — each one is sent as
    /// `ServerMsg::Command { name, args: {"value": value} }`, restricted to
    /// the commands of the device's last announce.
    #[serde(default)]
    pub commands: serde_json::Map<String, serde_json::Value>,
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/internal/flow/device-write", post(device_write))
        // Recorded video segments from the `video-record` node (D78) — raw
        // MJPEG-AVI body, metadata in the query string.
        // JSON events from the `event-log` node (D84) → O2 logs stream.
        .add("/internal/flow/event", post(flow_event))
        // Time ranges from the `range-upsert` node (D169).
        .add("/internal/flow/time-range", post(flow_time_range))
        .add("/internal/flow/video-annotations", post(video_annotations))
        // Vault secret of a deployed notify channel (D115, lot S4).
        .add("/internal/flow/secret/{id}", get(flow_secret))
        .add(
            "/internal/flow/video-segment",
            post(video_segment)
                .layer(axum::extract::DefaultBodyLimit::max(SEGMENT_MAX_BYTES))
                .layer(axum::middleware::from_fn(
                    crate::services::compute_limits::upload_gate,
                )),
        )
}

/// Upper bound of one uploaded segment (the node flushes at
/// `max_segment_mb`, 32 MB by default, capped at 256 MB by its config).
const SEGMENT_MAX_BYTES: usize = 260 * 1024 * 1024;

/// Fail-closed service-token check shared by the internal flow endpoints.
pub(crate) fn flow_token_ok(ctx: &AppContext, headers: &HeaderMap) -> bool {
    let Some(expected) = FlowSettings::from_config(&ctx.config)
        .device_write
        .map(|(_, t)| t)
        .filter(|t| !t.is_empty())
    else {
        return false;
    };
    headers
        .get(FLOW_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| crate::auth::service_token::matches(v, &expected))
}

/// Fencing (D106): a write stamped by a worker that no longer owns the org
/// (moved placement, superseded boot) is refused — a partitioned or zombie
/// worker never actuates twice. `None` = allowed.
async fn fenced(ctx: &AppContext, headers: &HeaderMap, org_id: i64) -> Option<Response> {
    let fence = crate::services::flow_cluster::fence_header(headers);
    if crate::services::flow_cluster::fence_ok(&ctx.db, org_id, fence).await {
        return None;
    }
    tracing::warn!(
        org_id,
        fence,
        "flow write refused: worker does not own the org (fenced)"
    );
    // Runtime diagnostic (verbatim in the node's last error), no UI code.
    Some(
        (
            StatusCode::CONFLICT,
            axum::Json(serde_json::json!({ "code": "fenced" })),
        )
            .into_response(),
    )
}

#[derive(Debug, Deserialize)]
pub struct SecretQuery {
    pub org_id: i64,
}

/// `GET /internal/flow/secret/{id}?org_id=` — the value of a vault secret
/// for the flow runtime (D115). Checks: service token, fencing (the worker
/// owns the org), secret of that org, referenced by a deployed flow of the
/// org (D111). Not found and not referenced answer the same 404: the
/// runtime cannot probe the vault. The value is never logged.
async fn flow_secret(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    axum::extract::Query(q): axum::extract::Query<SecretQuery>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, q.org_id).await {
        return Ok(r);
    }
    let not_referenced = || {
        (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "code": pnex_core::err_codes::SECRET_NOT_REFERENCED })),
        )
            .into_response()
    };
    if !crate::services::secrets::notify::reachable_from_deployed_flows(&ctx.db, q.org_id, id)
        .await?
    {
        tracing::warn!(org_id = q.org_id, %id, "flow secret refused: not referenced");
        return Ok(not_referenced());
    }
    let ring = crate::services::secrets::Keyring::from_config(&ctx.config).map_err(|e| {
        tracing::error!(error = %e, "secrets keyring unavailable");
        Error::InternalServerError
    })?;
    match crate::services::secrets::store::reveal(&ctx.db, &ring, Some(q.org_id), id).await {
        Ok(value) => Ok(axum::Json(serde_json::json!({ "value": value })).into_response()),
        Err(crate::services::secrets::store::StoreError::NotFound) => Ok(not_referenced()),
        Err(e) => {
            tracing::error!(error = %e, %id, "flow secret could not be read");
            Err(Error::InternalServerError)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct VideoSegmentQuery {
    pub org_id: i64,
    /// Device slug (device camera recordings).
    #[serde(default)]
    pub device_id: String,
    /// Media stream slug (IP stream recordings, D175); exactly one of
    /// `device_id` / `media_stream` is set.
    #[serde(default)]
    pub media_stream: String,
    pub flow_id: Option<i64>,
    pub node_id: String,
    pub stream: String,
    /// Unix ms of the first / last frame.
    pub started_ms: i64,
    pub ended_ms: i64,
    pub frames: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub retention_days: u32,
}

async fn video_segment(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<VideoSegmentQuery>,
    body: axum::body::Bytes,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, q.org_id).await {
        return Ok(r);
    }
    let started = chrono::DateTime::from_timestamp_millis(q.started_ms);
    let ended = chrono::DateTime::from_timestamp_millis(q.ended_ms);
    let (Some(started_at), Some(ended_at)) = (started, ended) else {
        return Err(bad_request(
            "video-segment-invalid",
            "Invalid video segment",
        ));
    };
    if body.len() < 12
        || &body[0..4] != b"RIFF"
        || q.frames <= 0
        || ended_at < started_at
        || q.stream.trim().is_empty()
    {
        return Err(bad_request(
            "video-segment-invalid",
            "Invalid video segment",
        ));
    }
    // The source is resolved inside the stamped org only (R1): a slug of
    // another org reads as unknown.
    let (source, device_slug) = match (q.device_id.is_empty(), q.media_stream.is_empty()) {
        (false, true) => {
            let device = device_registries::Entity::find()
                .filter(device_registries::Column::OrgId.eq(q.org_id))
                .filter(device_registries::Column::DeviceId.eq(&q.device_id))
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?;
            let Some(device) = device else {
                return Ok(StatusCode::NOT_FOUND.into_response());
            };
            (
                crate::services::video::SegmentSource::Device(device.id),
                device.device_id,
            )
        }
        (true, false) => {
            let found = crate::services::media_ingest::streams::find_by_slug(
                &ctx.db,
                q.org_id,
                &q.media_stream,
            )
            .await
            .map_err(|_| Error::InternalServerError)?;
            let Some(stream) = found else {
                return Ok(StatusCode::NOT_FOUND.into_response());
            };
            (
                crate::services::video::SegmentSource::Stream(stream.id),
                String::new(),
            )
        }
        _ => {
            return Err(bad_request(
                "video-segment-invalid",
                "Invalid video segment",
            ))
        }
    };
    let stream: String = q.stream.trim().chars().take(128).collect();
    let node_id: String = q.node_id.chars().take(64).collect();
    let row = crate::services::video::write_segment(
        &ctx,
        crate::services::video::NewSegment {
            org_id: q.org_id,
            source,
            flow_id: q.flow_id,
            node_id,
            stream,
            started_at,
            ended_at,
            frame_count: q.frames,
            width: q.width,
            height: q.height,
            retention_days: q.retention_days,
        },
        body,
    )
    .await?;
    format::json(crate::controllers::cameras::segment_dto(&row, &device_slug))
}

async fn device_write(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: axum::Json<DeviceWriteInput>,
) -> Result<Response> {
    // Fail-closed : pas de token configuré ⇒ aucune écriture externe.
    let Some(expected) = FlowSettings::from_config(&ctx.config)
        .device_write
        .map(|(_, t)| t)
        .filter(|t| !t.is_empty())
    else {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    };
    let supplied = headers
        .get(FLOW_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !crate::auth::service_token::matches(supplied, &expected) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, body.org_id).await {
        return Ok(r);
    }
    if body.values.is_empty() && body.commands.is_empty() {
        return Err(bad_request(
            "flow-device-write-values-empty",
            "Values must not be empty.",
        ));
    }

    // Device de l'org (slug), même masquage 404 que les routes org — one
    // joined query for the device and its pin instances (hot path: a flow
    // may write at a high rate).
    let found = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(body.org_id))
        .filter(device_registries::Column::DeviceId.eq(&body.device_id))
        .find_with_related(device_capability_instances::Entity)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let Some((device, rows)) = found.into_iter().next() else {
        tracing::error!(
            "device-write : device {} org {} introuvable",
            body.device_id,
            body.org_id
        );
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    // Pas de session vivante ⇒ rien ne part (même convention que POST
    // commands : jamais d'attente serveur). The session is resolved once
    // (one route lookup) for every value of the request.
    let Some(target) = ws_device::target_of(device.id).await else {
        return Ok((
            StatusCode::CONFLICT,
            axum::Json(serde_json::json!({ "code": "offline" })),
        )
            .into_response());
    };

    let mut results = Vec::with_capacity(body.values.len() + body.commands.len());
    let mut any_sent = false;
    for (label, value) in &body.values {
        let Some(row) = rows.iter().find(|r| {
            pnex_core::normalize_measurement_name(&r.label)
                == pnex_core::normalize_measurement_name(label)
        }) else {
            // Per-pin diagnostics surface in the node's `last_error` —
            // canonical English, verbatim display (runtime diagnostics
            // exception, no machine code on this shape).
            results.push(serde_json::json!({ "pin": label, "ok": false, "err": "unknown pin" }));
            continue;
        };
        let mode = str_to_mode(&row.mode);
        let err = match mode {
            Mode::DigitalOut => check_digital(value),
            Mode::PwmOut => check_duty(value),
            _ => Some("pin is not an output (digital_out/pwm_out required)".to_string()),
        };
        if let Some(err) = err {
            results.push(serde_json::json!({ "pin": label, "ok": false, "err": err }));
            continue;
        }
        if ws_device::push_to(
            device.id,
            &target,
            ServerMsg::Write {
                cmd_id: uuid::Uuid::new_v4().simple().to_string(),
                gpio: row.gpio as u16,
                value: value.clone(),
            },
        )
        .await
        {
            any_sent = true;
            results.push(serde_json::json!({ "pin": label, "ok": true }));
        } else {
            results.push(serde_json::json!({ "pin": label, "ok": false, "err": "offline" }));
        }
    }
    for (name, value) in &body.commands {
        // Bounded: a device reads frames of 1 KiB (ESP8266) to 4 KiB (ESP32);
        // a larger one is unreadable and the command silently lost.
        if value.to_string().len() > COMMAND_VALUE_MAX_BYTES {
            results.push(serde_json::json!({
                "command": name,
                "ok": false,
                "err": "command value too large",
            }));
            continue;
        }
        if !super::pins::announced_command(&device, name) {
            results.push(serde_json::json!({
                "command": name,
                "ok": false,
                "err": "command not announced by the device firmware",
            }));
            continue;
        }
        let msg = ServerMsg::Command {
            cmd_id: uuid::Uuid::new_v4().simple().to_string(),
            name: name.clone(),
            args: serde_json::json!({ "value": value }),
        };
        if ws_device::push_to(device.id, &target, msg).await {
            any_sent = true;
            results.push(serde_json::json!({ "command": name, "ok": true }));
        } else {
            results.push(serde_json::json!({ "command": name, "ok": false, "err": "offline" }));
        }
    }
    let _ = any_sent;
    format::json(serde_json::json!({ "results": results }))
}

/// Digital : `true/false` ou `0/1` (même contrat que POST commands).
fn check_digital(v: &serde_json::Value) -> Option<String> {
    let ok = matches!(
        v,
        serde_json::Value::Bool(true) | serde_json::Value::Bool(false)
    ) || v == &serde_json::json!(0)
        || v == &serde_json::json!(1);
    if ok {
        None
    } else {
        Some("invalid digital value (true/false or 0/1 required)".into())
    }
}

/// PWM : duty % 0..=100, nombre fini.
fn check_duty(v: &serde_json::Value) -> Option<String> {
    let Some(duty) = v.as_f64() else {
        return Some("invalid pwm value (number 0-100 required)".into());
    };
    if !duty.is_finite() || !(0.0..=100.0).contains(&duty) {
        return Some("duty must be between 0 and 100".into());
    }
    None
}

/// Mode colonne → Mode fil (miroir provisioning::str_to_mode_local).
fn str_to_mode(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "pwm_out" => Mode::PwmOut,
        "analog_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}

/// 400 with the machine code + canonical English description (frontend
/// resolves `err-<code>` at render time, verbatim fallback).
fn bad_request(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::BAD_REQUEST,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

async fn flow_event(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(ev): axum::Json<pnex_core::events::EventInput>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, ev.org_id).await {
        return Ok(r);
    }
    // The node normalizes, the backend re-checks (never trust the caller
    // with a raw stream name embedded in SQL later).
    if pnex_core::events::event_stream_name(&ev.stream).as_deref() != Some(ev.stream.as_str()) {
        return Err(bad_request(
            "event-stream-invalid",
            "Invalid event stream name",
        ));
    }
    match crate::services::events::record(&ctx, &ev).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => {
            // Runtime diagnostic: surfaced in the node's last error.
            tracing::warn!(error = %e, org_id = ev.org_id, "flow event write failed");
            Ok((
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({ "code": "events-unavailable", "error": e })),
            )
                .into_response())
        }
    }
}

/// Time range written by a `range-upsert` node (D169). The runtime is
/// untrusted: the flow must be one of `org_id`, the scope is resolved in
/// that org (a foreign stream answers 404 `time-range-scope-unknown`), the
/// origin is one a flow may write, and the fields go through the same
/// check as the HTTP API. Provenance `flow:<id>@<version>` is built here.
async fn flow_time_range(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::time_range::RangeUpsertRequest>,
) -> Result<Response> {
    use crate::services::time_ranges::{self, RangeError, Writer};
    use pnex_core::time_range::{flow_ref, RangeOrigin, RangeUpsertAck};
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    let coded = |status: StatusCode, code: &str| {
        (status, axum::Json(serde_json::json!({ "code": code }))).into_response()
    };
    let flow = crate::models::_entities::flows::Entity::find_by_id(req.flow_id)
        .filter(crate::models::_entities::flows::Column::OrgId.eq(req.org_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if flow.is_none() {
        return Ok(coded(StatusCode::NOT_FOUND, "flow-not-found"));
    }
    let origin = RangeOrigin::from_wire(&req.origin).filter(|o| RangeOrigin::FLOW.contains(o));
    let Some(origin) = origin else {
        return Ok(coded(
            StatusCode::BAD_REQUEST,
            "range-upsert-origin-invalid",
        ));
    };
    let mut input = req.range;
    // The scope is the node's, never the payload's.
    input.scope_kind = None;
    input.scope_id = None;
    if input
        .external_id
        .as_deref()
        .is_none_or(|e| e.trim().is_empty())
    {
        return Ok((
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "external_id": pnex_core::err_codes::FIELD_REQUIRED })),
        )
            .into_response());
    }
    let result =
        match time_ranges::resolve_scope(&ctx.db, req.org_id, &req.scope_kind, &req.scope_id).await
        {
            Ok(scope) => {
                let writer = Writer {
                    origin,
                    source_ref: Some(flow_ref(req.flow_id, req.version)),
                };
                time_ranges::upsert(&ctx.db, req.org_id, &scope, &input, &writer).await
            }
            Err(e) => Err(e),
        };
    match result {
        Ok((row, created)) => Ok(axum::Json(RangeUpsertAck {
            id: row.id.to_string(),
            created,
        })
        .into_response()),
        Err(RangeError::Invalid { field, token }) => Ok((
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ field: token })),
        )
            .into_response()),
        Err(RangeError::ScopeUnknown | RangeError::NotFound) => Ok(coded(
            StatusCode::NOT_FOUND,
            pnex_core::err_codes::TIME_RANGE_SCOPE_UNKNOWN,
        )),
        Err(RangeError::Db(e)) => {
            tracing::error!(error = %e, org_id = req.org_id, "flow time range write failed");
            Err(Error::InternalServerError)
        }
    }
}

/// Annotation layer batch of a detection node (D105) → OpenObserve
/// `camera_detections`. The camera must belong to the org; an O2 failure
/// answers 502 (the node logs it and moves on).
async fn video_annotations(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(batch): axum::Json<pnex_core::camera::AnnotationBatch>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, batch.org_id).await {
        return Ok(r);
    }
    if batch.layer_id.trim().is_empty() || batch.layer_id.len() > 128 {
        return Err(bad_request(
            "video-segment-invalid",
            "Invalid annotation batch",
        ));
    }
    let known = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(batch.org_id))
        .filter(device_registries::Column::DeviceId.eq(&batch.device_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if !known {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    match crate::services::video_annotations::record(&ctx, &batch).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => {
            tracing::warn!(error = %e, org_id = batch.org_id, "video annotations write failed");
            Ok((
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({ "code": "events-unavailable", "error": e })),
            )
                .into_response())
        }
    }
}
