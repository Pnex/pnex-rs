//! Internal device write endpoint — `POST /internal/flow/device-write`.
//!
//! Consumer: the flow runtime (`pnex-device-write` node) — the
//! backend never linking the engine, the downlink goes through internal HTTP
//! (`/internal/notify/deliver` school: the runtime never has direct access
//! to the device WS). The backend applies the same rules as the user's POST
//! commands: output-capable pin (`digital_out`/`pwm_out`), digital =
//! boolean 0/1, pwm = duty % 0..=100 (the firmware scales it to 8-bit),
//! `caps::validate` on set_mode, device offline → 409.
//!
//! Auth: service token in `x-pnex-flow-token` compared with
//! `settings.flow.runtime_token` (env `PNEX_FLOW_RUNTIME_TOKEN` takes precedence)
//! — absent/empty ⇒ systematic 401 (fail-closed, `/internal/notify/deliver`
//! school). The token never appears in the logs.

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
    /// Slug of the target device.
    pub device_id: String,
    /// Map `{pin label: value}` — digital = bool/0-1, pwm = duty 0..=100.
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
        // Org model calls of the vision-caption / media-fuse nodes (D195/D196).
        .add("/internal/flow/ai/caption", post(flow_ai_caption))
        // Ontology nodes (partner-watch.md D312).
        .add("/internal/flow/ontology/query", post(flow_ontology_query))
        .add("/internal/flow/ontology/upsert", post(flow_ontology_upsert))
        .add("/internal/flow/ontology/link", post(flow_ontology_link))
        // Watch check runs of check_record nodes (network-check.md D426).
        .add("/internal/flow/ontology/check", post(flow_ontology_check))
        // Office documents of document_text / sheet_write nodes (D416, D420).
        .add("/internal/flow/document-text", post(flow_document_text))
        .add("/internal/flow/sheet-write", post(flow_sheet_write))
        // Stored mails replayed to mail_received nodes (mail.md D464).
        .add("/internal/flow/mail/replay", post(flow_mail_replay))
        .add("/internal/flow/mail/ack", post(flow_mail_ack))
        // Website navigation rules played by flows (site-recipes.md D327).
        .add("/internal/flow/site-recipe", post(flow_site_recipe))
        // Web search candidates of web_search nodes (web-presence.md D361).
        .add("/internal/flow/web-search", post(flow_web_search))
        .add("/internal/flow/org-resolve", post(flow_org_resolve))
        .add("/internal/flow/web-visit", post(flow_web_visit))
        // Server crawls started by site_crawl nodes (network-check.md D407).
        .add("/internal/flow/site-crawl", post(flow_site_crawl))
        // Pages written by page-write nodes (D209).
        .add("/internal/flow/page-write", post(flow_page_write))
        .add("/internal/flow/page-pdf", post(flow_page_pdf))
        // Doubtful frames of vision-detect nodes → Vision Lab (M4).
        .add("/internal/flow/lab-review", post(flow_lab_review))
        .add("/internal/flow/ai/fuse", post(flow_ai_fuse))
        // Generic text of the org model (llm-text node, D208).
        .add("/internal/flow/ai/text", post(flow_ai_text))
        // Typed decisions of the text-decide node, local Laya model (D197).
        .add("/internal/flow/ai/decide", post(flow_ai_decide))
        // Vault secret of a deployed notify channel (D115, lot S4).
        .add("/internal/flow/secret/{id}", get(flow_secret))
        // PDF exported by a page-pdf node, attached by pnex-notify (D209).
        .add("/internal/flow/media/{id}", get(flow_media))
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

/// `GET /internal/flow/media/{id}?org_id=` — a file attached by a
/// `pnex-notify` node (D209). Least privilege: only a PDF exported by a
/// `page_pdf` node of that org is served, never any other media; unknown
/// and not exported answer the same 404.
async fn flow_media(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    axum::extract::Query(q): axum::extract::Query<SecretQuery>,
) -> Result<Response> {
    use crate::models::_entities::{doc_version_exports, media_assets, media_versions};
    use base64::Engine as _;
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, q.org_id).await {
        return Ok(r);
    }
    let exported = doc_version_exports::Entity::find()
        .filter(doc_version_exports::Column::OrgId.eq(q.org_id))
        .filter(doc_version_exports::Column::AssetId.eq(id))
        .one(&ctx.db)
        .await?
        .is_some();
    let version = match media_assets::Entity::find_by_id(id)
        .filter(media_assets::Column::OrgId.eq(q.org_id))
        .one(&ctx.db)
        .await?
        .and_then(|a| a.current_version_id)
    {
        Some(v) if exported => media_versions::Entity::find_by_id(v).one(&ctx.db).await?,
        _ => None,
    };
    let Some(version) = version else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if version.size_bytes > pnex_notify::ATTACHMENT_MAX_BYTES as i64 {
        return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
    }
    let store = crate::services::media::MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|_| Error::NotFound)?;
    Ok(axum::Json(serde_json::json!({
        "filename": version.filename,
        // Exports are always PDFs; the stored type is octet-stream (SEC-5).
        "content_type": "application/pdf",
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
    .into_response())
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
    // Fail-closed: no configured token ⇒ no external write.
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

    // Device of the org (slug), same 404 masking as the org routes — one
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
            "device-write: device {} org {} not found",
            body.device_id,
            body.org_id
        );
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    // No live session ⇒ nothing goes out (same convention as POST
    // commands: never a server-side wait). The session is resolved once
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

/// Digital: `true/false` or `0/1` (same contract as POST commands).
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

/// PWM: duty % 0..=100, finite number.
fn check_duty(v: &serde_json::Value) -> Option<String> {
    let Some(duty) = v.as_f64() else {
        return Some("invalid pwm value (number 0-100 required)".into());
    };
    if !duty.is_finite() || !(0.0..=100.0).contains(&duty) {
        return Some("duty must be between 0 and 100".into());
    }
    None
}

/// Column mode → wire mode (mirror of provisioning::str_to_mode_local).
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

/// Runtime-facing outcome of a model call: 200 + body, or a status with
/// `{code, error}` (verbatim diagnostic shown in the node status).
/// SEC-34: the org named by an AI call must have a deployed flow holding a
/// node of the calling kind — a compromised runtime can no longer spend
/// the model and quota of an org that never asked for it.
async fn ai_node_deployed(
    ctx: &AppContext,
    org_id: i64,
    is_caller: fn(&pnex_core::flow::FlowNodeKind) -> bool,
) -> Option<Response> {
    let flows = match crate::controllers::flows::deployed_flows_with_versions(&ctx.db, org_id).await
    {
        Ok(f) => f,
        Err(_) => return Some(StatusCode::INTERNAL_SERVER_ERROR.into_response()),
    };
    let found = flows.into_iter().any(|(_, version)| {
        serde_json::from_value::<pnex_core::flow::FlowGraph>(version.graph)
            .is_ok_and(|g| g.nodes.iter().any(|n| is_caller(&n.kind)))
    });
    if found {
        return None;
    }
    tracing::warn!(
        org_id,
        "flow model call refused: no deployed node of that kind"
    );
    Some(
        (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "code": "flow-ai-node-absent" })),
        )
            .into_response(),
    )
}

fn flow_ai_reply<T: serde::Serialize>(
    org_id: i64,
    res: Result<T, crate::services::flow_ai::FlowAiError>,
) -> Response {
    use crate::services::flow_ai::FlowAiError;
    match res {
        Ok(body) => axum::Json(body).into_response(),
        Err(e) => {
            let status = match &e {
                FlowAiError::NotConfigured => StatusCode::CONFLICT,
                FlowAiError::Quota(_) => StatusCode::TOO_MANY_REQUESTS,
                FlowAiError::NoFrame => StatusCode::NOT_FOUND,
                FlowAiError::Invalid(_) => StatusCode::BAD_REQUEST,
                FlowAiError::Ai(_) => StatusCode::BAD_GATEWAY,
                FlowAiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            };
            tracing::warn!(org_id, error = %e, "flow model call failed");
            (
                status,
                axum::Json(serde_json::json!({ "code": e.code(), "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

/// Answer of an ontology node call; refusals are runtime diagnostics
/// (verbatim in the node status), ontology errors keep their code.
fn ontology_reply<T: serde::Serialize>(
    org_id: i64,
    r: std::result::Result<T, crate::services::ontology::flow_nodes::NodeError>,
) -> Result<Response> {
    use crate::services::ontology::flow_nodes::NodeError;
    let refused = |status: StatusCode, code: &str| {
        tracing::warn!(org_id, code, "flow ontology call refused");
        Ok((status, axum::Json(serde_json::json!({ "code": code }))).into_response())
    };
    match r {
        Ok(v) => Ok(axum::Json(v).into_response()),
        Err(NodeError::NotDeployed) => refused(StatusCode::FORBIDDEN, "flow-node-not-deployed"),
        Err(NodeError::Budget) => refused(StatusCode::TOO_MANY_REQUESTS, "flow-ontology-budget"),
        Err(NodeError::Onto(e)) => crate::controllers::ontology::onto_error(e),
    }
}

async fn flow_ontology_query(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::ontology::flow::FlowQueryRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::ontology::flow_nodes::query(&ctx.db, &req).await;
    ontology_reply(req.node.org_id, r)
}

/// `POST /internal/flow/mail/replay` — the stored mails a `mail_received`
/// node missed since its cursor (mail.md D464).
async fn flow_mail_replay(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::mail::MailReplayRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::mail::replay::replay(&ctx.db, &req)
        .await
        .map(|messages| pnex_core::mail::MailReplayReply { messages });
    ontology_reply(req.node.org_id, r)
}

/// `POST /internal/flow/mail/ack` — the node emitted a mail: its cursor
/// moves forward.
async fn flow_mail_ack(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::mail::MailAckRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::mail::replay::ack(&ctx.db, &req).await;
    ontology_reply(req.node.org_id, r.map(|()| serde_json::json!({})))
}

async fn flow_ontology_upsert(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::ontology::flow::FlowUpsertRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::ontology::flow_nodes::upsert(&ctx.db, &req).await;
    ontology_reply(req.node.org_id, r)
}

/// `POST /internal/flow/site-recipe` — one run of a deployed
/// `site_recipe` node, waited for. A failing run answers 200 on its port;
/// only a forged or stale node is refused.
async fn flow_site_recipe(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::site_recipe::SiteRecipeNodeRequest>,
) -> Result<Response> {
    use crate::services::ontology::flow_nodes::NodeError;
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    match crate::services::site_recipes::flow_node::run(&ctx, &req).await {
        Ok(reply) => Ok(axum::Json(reply).into_response()),
        Err(NodeError::NotDeployed) => {
            tracing::warn!(
                org_id = req.node.org_id,
                "flow site recipe call refused: node not deployed"
            );
            Ok((
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "code": "flow-node-not-deployed" })),
            )
                .into_response())
        }
        Err(e) => {
            tracing::warn!(org_id = req.node.org_id, error = ?e, "flow site recipe failed");
            Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// `POST /internal/flow/web-search` — one query of a deployed `web_search`
/// node. A failed search answers 200 on port 1; only a forged or stale
/// node is refused.
async fn flow_web_search(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::web_search::WebSearchNodeRequest>,
) -> Result<Response> {
    use crate::services::ontology::flow_nodes::NodeError;
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    match crate::services::web_search::run(&ctx, &req).await {
        Ok(reply) => Ok(axum::Json(reply).into_response()),
        Err(NodeError::NotDeployed) => {
            tracing::warn!(
                org_id = req.node.org_id,
                "flow web search refused: node not deployed"
            );
            Ok((
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "code": "flow-node-not-deployed" })),
            )
                .into_response())
        }
        Err(e) => {
            tracing::warn!(org_id = req.node.org_id, error = ?e, "flow web search failed");
            Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// `POST /internal/flow/org-resolve` — one lookup of a deployed
/// `org_resolve` node (account-mapping.md D332). A failed lookup answers
/// 200 on port 2; only a forged or stale node is refused.
async fn flow_org_resolve(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::org_resolve::OrgResolveNodeRequest>,
) -> Result<Response> {
    use crate::services::ontology::flow_nodes::NodeError;
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    match crate::services::org_resolve::run(&ctx, &req).await {
        Ok(reply) => Ok(axum::Json(reply).into_response()),
        Err(NodeError::NotDeployed) => {
            tracing::warn!(
                org_id = req.node.org_id,
                "flow org resolve refused: node not deployed"
            );
            Ok((
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "code": "flow-node-not-deployed" })),
            )
                .into_response())
        }
        Err(e) => {
            tracing::warn!(org_id = req.node.org_id, error = ?e, "flow org resolve failed");
            Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// `POST /internal/flow/web-visit` — one visit of a deployed `web_visit`
/// node (web-presence.md D364). A failed visit answers 200 on port 1; only
/// a forged or stale node is refused.
async fn flow_web_visit(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::web_visit::WebVisitNodeRequest>,
) -> Result<Response> {
    use crate::services::ontology::flow_nodes::NodeError;
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    match crate::services::web_visit::run(&ctx, &req).await {
        Ok(reply) => Ok(axum::Json(reply).into_response()),
        Err(NodeError::NotDeployed) => {
            tracing::warn!(
                org_id = req.node.org_id,
                "flow web visit refused: node not deployed"
            );
            Ok((
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "code": "flow-node-not-deployed" })),
            )
                .into_response())
        }
        Err(e) => {
            tracing::warn!(org_id = req.node.org_id, error = ?e, "flow web visit failed");
            Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// `POST /internal/flow/site-crawl` — one site of a deployed `site_crawl`
/// node (network-check.md D407), answered when the crawl ends. A refused
/// crawl answers 200 on its failure port; only a forged or stale node is
/// refused.
async fn flow_site_crawl(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::site_crawl::SiteCrawlNodeRequest>,
) -> Result<Response> {
    use crate::services::ontology::flow_nodes::NodeError;
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    match crate::services::site_crawl::run(&ctx, &req).await {
        Ok(reply) => Ok(axum::Json(reply).into_response()),
        Err(NodeError::NotDeployed) => Ok((
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "code": "flow-node-not-deployed" })),
        )
            .into_response()),
        Err(e) => {
            tracing::warn!(org_id = req.node.org_id, error = ?e, "flow site crawl failed");
            Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

async fn flow_ontology_link(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::ontology::flow::FlowLinkRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::ontology::flow_nodes::link(&ctx.db, &req).await;
    ontology_reply(req.node.org_id, r)
}

async fn flow_ontology_check(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::ontology::check::CheckRecordRequest>,
) -> Result<Response> {
    use crate::services::ontology::cases::watch;
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::ontology::flow_nodes::check(&ctx.db, &req)
        .await
        .map(|rec| {
            watch::journal(req.node.org_id, rec.line);
            rec.ack
        });
    ontology_reply(req.node.org_id, r)
}

/// Answer of an office node call; refusals are runtime diagnostics with
/// their machine code (node status).
fn office_reply<T: serde::Serialize>(
    org_id: i64,
    r: std::result::Result<T, crate::services::office::OfficeError>,
) -> Result<Response> {
    use crate::services::office::OfficeError;
    let refused = |status: StatusCode, body: serde_json::Value| {
        tracing::warn!(org_id, %body, "flow office call refused");
        Ok((status, axum::Json(body)).into_response())
    };
    match r {
        Ok(v) => Ok(axum::Json(v).into_response()),
        Err(OfficeError::Node(e)) => ontology_reply::<()>(org_id, Err(e)),
        Err(OfficeError::NotFound) => refused(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "code": pnex_core::err_codes::ONTOLOGY_NOT_FOUND }),
        ),
        Err(OfficeError::Invalid(field)) => refused(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "code": pnex_core::err_codes::FIELD_INVALID, "args": { "field": field } }),
        ),
        Err(OfficeError::Coded(code)) => refused(
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "code": code }),
        ),
        Err(OfficeError::Internal(e)) => {
            tracing::warn!(org_id, error = %e, "flow office call failed");
            Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

async fn flow_document_text(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::office::DocumentTextRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::office::document_text(&ctx, &req).await;
    office_reply(req.node.org_id, r)
}

async fn flow_sheet_write(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::office::SheetWriteRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.node.org_id).await {
        return Ok(r);
    }
    let r = crate::services::office::sheet_write(&ctx, &req).await;
    office_reply(req.node.org_id, r)
}

async fn flow_page_write(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::pages::PageWriteRequest>,
) -> Result<Response> {
    use crate::services::pages::flow_write::{self, FlowPageError};
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    let valkey = crate::services::pages::valkey_url(&ctx.config);
    Ok(match flow_write::write(&ctx.db, valkey, &req).await {
        Ok(out) => (StatusCode::CREATED, axum::Json(out)).into_response(),
        Err(e) => {
            let status = match &e {
                FlowPageError::Invalid(_) => StatusCode::BAD_REQUEST,
                FlowPageError::NotFound => StatusCode::NOT_FOUND,
                FlowPageError::Page(crate::services::pages::PageError::Refused(_)) => {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            tracing::warn!(org_id = req.org_id, error = %e, "flow page write refused");
            (
                status,
                axum::Json(serde_json::json!({ "code": e.code(), "error": e.to_string() })),
            )
                .into_response()
        }
    })
}

async fn flow_page_pdf(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::pages::PagePdfRequest>,
) -> Result<Response> {
    use crate::services::pages::flow_write::{self, FlowPageError};
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    Ok(match flow_write::export_pdf(&ctx, &req).await {
        Ok(out) => axum::Json(out).into_response(),
        Err(e) => {
            let status = match &e {
                FlowPageError::Invalid(_) => StatusCode::BAD_REQUEST,
                FlowPageError::NotFound => StatusCode::NOT_FOUND,
                FlowPageError::Page(crate::services::pages::PageError::Refused(_)) => {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            tracing::warn!(org_id = req.org_id, error = %e, "flow page pdf refused");
            (
                status,
                axum::Json(serde_json::json!({ "code": e.code(), "error": e.to_string() })),
            )
                .into_response()
        }
    })
}

async fn flow_lab_review(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::lab::LabReviewRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    Ok(match crate::services::lab::review(&ctx, &req).await {
        Ok(img) => (StatusCode::CREATED, axum::Json(img)).into_response(),
        Err(e) => (
            e.status,
            axum::Json(serde_json::json!({ "error": e.code, "description": e.message })),
        )
            .into_response(),
    })
}

async fn flow_ai_caption(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::vision_ai::CaptionRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    if let Some(r) = ai_node_deployed(&ctx, req.org_id, |k| {
        matches!(k, pnex_core::flow::FlowNodeKind::VisionCaption { .. })
    })
    .await
    {
        return Ok(r);
    }
    let res = crate::services::flow_ai::caption(&ctx, &req).await;
    Ok(flow_ai_reply(req.org_id, res))
}

async fn flow_ai_text(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::vision_ai::LlmTextRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    if let Some(r) = ai_node_deployed(&ctx, req.org_id, |k| {
        matches!(k, pnex_core::flow::FlowNodeKind::LlmText { .. })
    })
    .await
    {
        return Ok(r);
    }
    let res = crate::services::flow_ai::text(&ctx, &req).await;
    Ok(flow_ai_reply(req.org_id, res))
}

async fn flow_ai_fuse(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::vision_ai::FuseRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    if let Some(r) = ai_node_deployed(&ctx, req.org_id, |k| {
        matches!(k, pnex_core::flow::FlowNodeKind::MediaFuse { .. })
    })
    .await
    {
        return Ok(r);
    }
    let res = crate::services::flow_ai::fuse(&ctx, &req).await;
    Ok(flow_ai_reply(req.org_id, res))
}

async fn flow_ai_decide(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<pnex_core::decide::DecideRequest>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(r) = fenced(&ctx, &headers, req.org_id).await {
        return Ok(r);
    }
    if let Some(r) = ai_node_deployed(&ctx, req.org_id, |k| {
        matches!(k, pnex_core::flow::FlowNodeKind::TextDecide { .. })
    })
    .await
    {
        return Ok(r);
    }
    let res = crate::services::decide::decide(&ctx, &req).await;
    Ok(flow_ai_reply(req.org_id, res))
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
