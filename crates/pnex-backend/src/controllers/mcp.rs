//! `POST /api/v1/mcp` — MCP server over Streamable HTTP (D245): JSON-RPC
//! with JSON answers, no session, no server-sent stream. Tools and replays
//! live in `crate::services::mcp`. Clients sign in by themselves: a 401
//! points to the protected resource metadata (RFC 9728), which names Rauthy
//! as the authorization server (OAuth code + PKCE, refresh token).

use axum::extract::State;
use axum::http::header::WWW_AUTHENTICATE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use loco_rs::prelude::*;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::services::mcp::{self, AppRouter, PROTOCOL_VERSIONS};

const INSTRUCTIONS: &str = "Tools are operations of the PneX HTTP API, run with your own \
account and rights. Org-scoped tools take `X-Org-Id`: list your organisations with \
`user_info_get`. Lists are paginated (`limit`, `offset`, answer `{count, results}`). Deleting data, acting on \
devices and deploying or stopping flows are not available here: they stay in the PneX UI.";

/// JSON-RPC endpoint of the MCP server.
#[utoipa::path(
    post,
    path = "/api/v1/mcp",
    tag = "mcp",
    operation_id = "mcp",
    summary = "MCP server (JSON-RPC)",
    description = "Model Context Protocol endpoint (Streamable HTTP, JSON answers, stateless): `initialize`, `ping`, `tools/list`, `tools/call`. The tools are the operations of this spec listed as exposed (D245); a call runs with the caller's token and rights. `X-Org-Id` on this request is the default organisation of the calls. Notifications answer 202.",
    request_body = Value,
    responses(
        (status = 200, description = "JSON-RPC answer", body = Value),
        (status = 202, description = "Notification accepted")
    ),
    security(("bearer" = []))
)]
#[debug_handler]
async fn rpc(
    State(ctx): State<AppContext>,
    auth: std::result::Result<AuthUser, loco_rs::Error>,
    headers: HeaderMap,
    Json(msg): Json<Value>,
) -> Result<Response> {
    if let Err(err) = auth {
        let mut res = err.into_response();
        let challenge = origin(&ctx).and_then(|o| {
            HeaderValue::from_str(&format!("Bearer resource_metadata=\"{o}{METADATA}\"")).ok()
        });
        if let (StatusCode::UNAUTHORIZED, Some(value)) = (res.status(), challenge) {
            res.headers_mut().insert(WWW_AUTHENTICATE, value);
        }
        return Ok(res);
    }
    // Notifications (and client answers) carry no id: nothing to answer.
    let Some(id) = msg.get("id").cloned() else {
        return Ok(axum::http::StatusCode::ACCEPTED.into_response());
    };
    let params = &msg["params"];
    let result = match msg["method"].as_str().unwrap_or_default() {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOL_VERSIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&PROTOCOL_VERSIONS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "pnex", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": mcp::tool_list() })),
        "tools/call" => match ctx.shared_store.get::<AppRouter>() {
            Some(AppRouter(router)) => mcp::call(router, &headers, params).await,
            None => Err((-32603, "server not ready".to_string())),
        },
        other => Err((-32601, format!("method `{other}` not found"))),
    };
    let answer = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    };
    format::json(answer)
}

/// Path of the protected resource metadata of `/api/v1/mcp` (RFC 9728).
const METADATA: &str = "/.well-known/oauth-protected-resource/api/v1/mcp";

/// Browser-facing origin (from the issuer); `None` without a public HTTPS
/// issuer, where clients keep a pasted bearer header.
fn origin(ctx: &AppContext) -> Option<String> {
    let issuer = crate::auth::settings::RauthySettings::from_config(&ctx.config)
        .ok()
        .and_then(|s| s.issuer_url);
    crate::controllers::meta::public_origin(issuer.as_deref())
}

/// Protected resource metadata: where an MCP client signs its user in.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-protected-resource/api/v1/mcp",
    tag = "mcp",
    operation_id = "mcp_resource_metadata",
    security(()),
    summary = "OAuth metadata of the MCP endpoint (public)",
    description = "RFC 9728 protected resource metadata: the MCP resource URL and its authorization server (the Rauthy issuer). Public.",
    responses(
        (status = 200, description = "Protected resource metadata", body = Value),
        (status = 404, description = "No public HTTPS issuer configured")
    )
)]
#[debug_handler]
async fn resource_metadata(State(ctx): State<AppContext>) -> Result<Response> {
    let issuer = crate::auth::settings::RauthySettings::from_config(&ctx.config)
        .ok()
        .map(|s| s.issuer());
    let (Some(origin), Some(issuer)) = (origin(&ctx), issuer) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    format::json(json!({
        "resource": format!("{origin}/api/v1/mcp"),
        "authorization_servers": [issuer],
        "scopes_supported": ["openid", "profile", "email"],
        "bearer_methods_supported": ["header"],
        "resource_name": "PneX",
    }))
}

/// Any other `/.well-known/*`: a plain 404, never the SPA `index.html`.
/// OAuth clients probe several discovery URLs in turn (RFC 8414 path
/// insertion first) and abort on a 200 that is not JSON.
#[utoipa::path(
    get,
    path = "/.well-known/{rest}",
    tag = "mcp",
    operation_id = "well_known_not_found",
    security(()),
    params(("rest" = String, Path, description = "Unknown well-known path")),
    summary = "Unknown well-known path (public)",
    description = "Always 404, so OAuth discovery clients fall through to the next metadata URL instead of parsing the web app's HTML. Public.",
    responses((status = 404, description = "Not a served well-known document"))
)]
#[debug_handler]
async fn well_known_not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/api/v1/mcp", post(rpc))
        .add(METADATA, get(resource_metadata))
        .add("/.well-known/{*rest}", get(well_known_not_found))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(rpc, resource_metadata, well_known_not_found))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
