//! Incoming webhooks (connectors.md D795). Public: the signature with the connector's
//! secret is the authentication, and the org comes from the connector (R1).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use loco_rs::prelude::*;
use pnex_core::connectors::WEBHOOK_BODY_MAX;
use pnex_core::err_codes;

use crate::services::hooks::{self, HookRefusal};

pub fn routes() -> Routes {
    Routes::new().prefix("/api/v1").add(
        "/hooks/{id}",
        post(receive).layer(axum::extract::DefaultBodyLimit::max(WEBHOOK_BODY_MAX)),
    )
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(receive))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

/// `POST /api/v1/hooks/{id}`
#[utoipa::path(
    post,
    path = "/api/v1/hooks/{id}",
    tag = "connectors",
    operation_id = "hooks_receive",
    summary = "Receive a webhook call",
    description = "Keeps one JSON call for the webhook connector `id`, read later by an import into a table. Public: the call must carry the HMAC-SHA256 signature of its body (or of `<timestamp>.<body>` when the connector names a timestamp header) made with the connector's secret, hex encoded in the connector's signature header. 1 MiB at most.",
    params(("id" = String, Path, format = "uuid", description = "Webhook connector id (UUID)")),
    request_body(content = serde_json::Value, content_type = "application/json"),
    responses(
        (status = 202, description = "Kept"),
        (status = 400, description = "`hook-body-invalid`", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`hook-signature-invalid`: no such webhook, or a missing, wrong or stale signature", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`hook-inbox-full`", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn receive(
    State(ctx): State<AppContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let refused = |status, code: &str, text: &str| {
        Err(super::coded_error(status, code, text.to_string(), None))
    };
    let Ok(id) = id.parse::<uuid::Uuid>() else {
        return refused(
            StatusCode::UNAUTHORIZED,
            err_codes::HOOK_SIGNATURE_INVALID,
            "Missing or invalid signature.",
        );
    };
    let Ok(ring) = crate::controllers::secrets::keyring(&ctx) else {
        return refused(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::HOOK_INBOX_FULL,
            "The webhook cannot keep calls right now.",
        );
    };
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    match hooks::receive(&ctx.db, &ring, id, header, &body).await {
        Ok(()) => Ok(StatusCode::ACCEPTED.into_response()),
        Err(HookRefusal::Signature) => refused(
            StatusCode::UNAUTHORIZED,
            err_codes::HOOK_SIGNATURE_INVALID,
            "Missing or invalid signature.",
        ),
        Err(HookRefusal::Body) => refused(
            StatusCode::BAD_REQUEST,
            err_codes::HOOK_BODY_INVALID,
            "The body is not JSON.",
        ),
        Err(HookRefusal::Full | HookRefusal::Unavailable) => refused(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::HOOK_INBOX_FULL,
            "The webhook cannot keep calls right now.",
        ),
    }
}
