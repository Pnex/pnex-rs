//! Browser agents API (extension-collector.md D254–D269).
//!
//! - management plane (user bearer + org): pairing, agents, missions. Org
//!   from the principal (R1), `can_write` at the head of every write (R2);
//! - data plane (agent): `POST /extension/ws-ticket`, device token in
//!   `Authorization`, request signed by the install key (D266), then
//!   `/ws/extension?ticket=` (`ws_extension.rs`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::browser_agent::{
    BrowserAgentPatch, MissionCreate, MissionPatch, PairRequest, TicketRequest,
};
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::extension::{self, ExtensionError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/extension")
        .add("/agents", get(agents).post(pair))
        .add("/agents/{id}", patch(update_agent))
        .add("/ws-ticket", post(ws_ticket))
        .add("/download", get(download))
        .add("/uploads/page", post(upload_page))
        .add(
            "/uploads/video",
            post(upload_video)
                .layer(axum::extract::DefaultBodyLimit::max(
                    pnex_core::browser_agent::VIDEO_MAX + 1024 * 1024,
                ))
                .layer(axum::middleware::from_fn(
                    crate::services::compute_limits::upload_gate,
                )),
        )
        .add("/videos", get(videos))
        .add("/missions", get(missions).post(create_mission))
        .add(
            "/missions/{id}",
            get(mission).patch(update_mission).delete(remove_mission),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        agents,
        pair,
        update_agent,
        ws_ticket,
        download,
        upload_page,
        upload_video,
        videos,
        missions,
        mission,
        create_mission,
        update_mission,
        remove_mission
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn require_write(org: &OrgContext) -> Result<()> {
    if org.can_write() {
        return Ok(());
    }
    Err(Error::CustomError(
        StatusCode::FORBIDDEN,
        ErrorDetail::new(
            err_codes::EXTENSION_WRITE_FORBIDDEN,
            "Viewer role: browser agents and missions are read-only.",
        ),
    ))
}

fn extension_error(e: ExtensionError) -> Result<Response> {
    let coded = |status, code: &str, msg: &str| {
        Err(Error::CustomError(status, ErrorDetail::new(code, msg)))
    };
    match e {
        ExtensionError::Invalid { field, token } => Ok(super::field_error(&field, &token)),
        ExtensionError::AgentNotFound => coded(
            StatusCode::NOT_FOUND,
            err_codes::EXTENSION_AGENT_NOT_FOUND,
            "Browser agent not found.",
        ),
        ExtensionError::MissionNotFound => coded(
            StatusCode::NOT_FOUND,
            err_codes::EXTENSION_MISSION_NOT_FOUND,
            "Mission not found.",
        ),
        ExtensionError::VideoNotFound => coded(
            StatusCode::NOT_FOUND,
            err_codes::EXTENSION_VIDEO_NOT_FOUND,
            "Video not in the registry of this mission, or already captured.",
        ),
        ExtensionError::VersionConflict { current } => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::EXTENSION_MISSION_VERSION_CONFLICT,
            "The mission changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current.to_string() })),
        )),
        ExtensionError::PairRefused => coded(
            StatusCode::BAD_REQUEST,
            err_codes::EXTENSION_PAIR_REFUSED,
            "Malformed install key or bad signature.",
        ),
        ExtensionError::PairedElsewhere => coded(
            StatusCode::CONFLICT,
            err_codes::EXTENSION_INSTALL_PAIRED_ELSEWHERE,
            "This browser install is paired with another organization.",
        ),
        ExtensionError::Db(e) => {
            tracing::error!(error = %e, "browser agent database error");
            Err(Error::InternalServerError)
        }
    }
}

/// Page of a list (`limit`/`offset`, D14).
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

fn paged<T: serde::Serialize>(path: &str, q: &PageQuery, items: Vec<T>) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let count = items.len() as i64;
    let (skip, take) = page.slice(items.len());
    let results: Vec<T> = items.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(path, &[], page, count, results))
}

/// `GET /api/v1/extension/agents` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/agents",
    tag = "extension",
    operation_id = "extension_agents_list",
    summary = "List the browser agents",
    description = "Browser agents paired with the org, newest first, with their fingerprint and whether they are connected now. Paginated (`limit`/`offset`). Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::browser_agent::BrowserAgent>)),
    security(("bearer" = [], "org" = []))
)]
async fn agents(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    match extension::list_agents(&ctx.db, org.org.id).await {
        Ok(rows) => paged("/api/v1/extension/agents", &q, rows),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/agents` — `can_write`: pairing from the
/// extension's options page.
#[utoipa::path(
    post,
    path = "/api/v1/extension/agents",
    tag = "extension",
    operation_id = "extension_agents_pair",
    summary = "Pair a browser extension",
    description = "Called by the extension right after the user signs in: the install key signs the org and a timestamp (proof of possession). Creates the browser agent of this install, or rotates the credentials of its existing agent in this org (any former session is cut). Returns the device token and Noise key once. Requires owner, admin or member role.",
    request_body = PairRequest,
    responses(
        (status = 201, body = pnex_core::browser_agent::PairResponse),
        (status = 400, description = "`extension-pair-refused`: malformed install key or bad signature", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`extension-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`extension-install-paired-elsewhere`: the install is paired with another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn pair(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(req): Json<PairRequest>,
) -> Result<Response> {
    require_write(&org)?;
    match extension::pair(&ctx.db, org.org.id, org.auth.user.id, &req).await {
        Ok(paired) => Ok((StatusCode::CREATED, format::json(paired)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `PATCH /api/v1/extension/agents/{id}` — `can_write`: rename.
#[utoipa::path(
    patch,
    path = "/api/v1/extension/agents/{id}",
    tag = "extension",
    operation_id = "extension_agents_update",
    summary = "Rename a browser agent",
    description = "Renames a browser agent (absent fields unchanged). Remove an agent with `DELETE /api/v1/devices/{id}`. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Agent id (device registry id)")),
    request_body = BrowserAgentPatch,
    responses(
        (status = 200, body = pnex_core::browser_agent::BrowserAgent),
        (status = 400, description = "`field-invalid`: `errors.fields.name` `required` or `max_length:N`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`extension-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-agent-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_agent(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<BrowserAgentPatch>,
) -> Result<Response> {
    require_write(&org)?;
    let Some(name) = input.name else {
        return match extension::list_agents(&ctx.db, org.org.id).await {
            Ok(rows) => match rows.into_iter().find(|a| a.id == id) {
                Some(a) => format::json(a),
                None => extension_error(ExtensionError::AgentNotFound),
            },
            Err(e) => extension_error(e),
        };
    };
    match extension::rename_agent(&ctx.db, org.org.id, id, &name).await {
        Ok(a) => format::json(a),
        Err(e) => extension_error(e),
    }
}

/// One-time ticket of `/ws/extension`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct ExtensionTicket {
    ticket: String,
    /// Lifetime of the ticket in seconds.
    expires_in: u64,
}

/// `POST /api/v1/extension/ws-ticket` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/ws-ticket",
    tag = "extension",
    operation_id = "extension_ws_ticket",
    summary = "Issue a browser agent websocket ticket",
    description = "Called by a browser agent, no user session: `Authorization: Bearer <agent token>` plus a body signed by the install key (timestamp within two minutes, each signature usable once). Returns a single-use ticket for `GET /ws/extension?ticket=`, since browsers cannot set headers on websockets nor present a client certificate.",
    request_body = TicketRequest,
    responses(
        (status = 200, body = ExtensionTicket),
        (status = 401, description = "`extension-ticket-refused`: unknown token, bad or reused signature, clock skew", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable`: Valkey unreachable", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn ws_ticket(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TicketRequest>,
) -> Result<Response> {
    let refused = || {
        Err(Error::CustomError(
            StatusCode::UNAUTHORIZED,
            ErrorDetail::new(
                err_codes::EXTENSION_TICKET_REFUSED,
                "Unknown agent token or bad signature.",
            ),
        ))
    };
    let Some(token) = super::device_link::device_token(&headers) else {
        return refused();
    };
    let agent = match extension::authenticate(&ctx.db, token, req.ts, &req.signature, |device_id| {
        pnex_core::browser_agent::ticket_message(device_id, req.ts)
    })
    .await
    {
        Ok(Some(agent)) => agent,
        Ok(None) => return refused(),
        Err(e) => {
            tracing::error!(error = %e, "browser agent lookup failed");
            return Err(Error::InternalServerError);
        }
    };
    if crate::services::shared_valkey::conn(&ctx.config)
        .await
        .is_none()
    {
        return Err(Error::CustomError(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorDetail::new(
                err_codes::EXTENSION_TICKET_UNAVAILABLE,
                "Websocket tickets are unavailable.",
            ),
        ));
    }
    // `None` past the Valkey check = the signature was already used.
    match extension::issue_ticket(&ctx.config, &agent, &req.signature).await {
        Some(ticket) => format::json(ExtensionTicket {
            ticket,
            expires_in: extension::TICKET_TTL_SECS,
        }),
        None => refused(),
    }
}

/// Unpacked extension shipped with the server (built by
/// `crates/pnex-extension/build-dist.sh`).
fn dist_dir() -> std::path::PathBuf {
    std::env::var("PNEX_EXTENSION_DIST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("../../dist/extension"))
}

/// Base URL the browser reached this server on (`X-Forwarded-*` from the
/// edge, else `Host`); `None` when the host is not a plain host[:port].
fn public_base(headers: &axum::http::HeaderMap) -> Option<String> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let host = header("x-forwarded-host")
        .or_else(|| header("host"))?
        .split(',')
        .next()?
        .trim();
    let plain = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    let scheme = match header("x-forwarded-proto") {
        Some(p) if p.trim().eq_ignore_ascii_case("https") => "https",
        _ => "http",
    };
    plain.then(|| format!("{scheme}://{host}"))
}

/// Zip of the unpacked extension plus `server.json` (this server's URL, so
/// the user only signs in).
fn zip_extension(dir: &std::path::Path, server: Option<&str>) -> std::io::Result<Vec<u8>> {
    use std::io::Write as _;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path
                .strip_prefix(dir)
                .map_err(std::io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            if name == "server.json" {
                continue;
            }
            zip.start_file(name, options)
                .map_err(std::io::Error::other)?;
            zip.write_all(&std::fs::read(&path)?)?;
        }
    }
    if let Some(server) = server {
        zip.start_file("server.json", options)
            .map_err(std::io::Error::other)?;
        zip.write_all(
            serde_json::json!({ "server": server })
                .to_string()
                .as_bytes(),
        )?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

/// `GET /api/v1/extension/download` — public.
#[utoipa::path(
    get,
    path = "/api/v1/extension/download",
    tag = "extension",
    operation_id = "extension_download",
    summary = "Download the browser extension (public)",
    description = "Zip of the PNeX browser agent extension shipped with this server, with this server's address preset (`server.json`) so the user only signs in. Load it unpacked from the browser's extensions page. No authentication: it holds no secret.",
    responses(
        (status = 200, description = "Zip archive", content_type = "application/zip", body = Vec<u8>),
        (status = 404, description = "`extension-dist-not-found`: this server ships no extension build", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn download(headers: axum::http::HeaderMap) -> Result<Response> {
    let dir = dist_dir();
    if !dir.join("manifest.json").is_file() {
        return Err(Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(
                err_codes::EXTENSION_DIST_NOT_FOUND,
                "This server ships no browser extension build.",
            ),
        ));
    }
    let server = public_base(&headers);
    let bytes = tokio::task::spawn_blocking(move || zip_extension(&dir, server.as_deref()))
        .await
        .map_err(|_| Error::InternalServerError)?
        .map_err(|e| {
            tracing::error!(error = %e, "browser extension zip failed");
            Error::InternalServerError
        })?;
    Ok((
        StatusCode::OK,
        [
            ("content-type", "application/zip"),
            (
                "content-disposition",
                "attachment; filename=\"pnex-browser-agent.zip\"",
            ),
        ],
        bytes,
    )
        .into_response())
}

/// Captures per agent and minute (a page every 20 s per site, a few sites).
const UPLOADS_PER_MINUTE: u32 = 30;

fn upload_refused() -> Error {
    Error::CustomError(
        StatusCode::UNAUTHORIZED,
        ErrorDetail::new(
            err_codes::EXTENSION_UPLOAD_REFUSED,
            "Unknown agent token or bad signature.",
        ),
    )
}

/// Agent of a signed upload (`Authorization`, `X-Pnex-Ts`,
/// `X-Pnex-Signature` over `message(device_id, ts)`), signature used once,
/// within the per-agent capture rate.
async fn signed_upload(
    ctx: &AppContext,
    headers: &axum::http::HeaderMap,
    message: impl FnOnce(&str, i64) -> String,
) -> Result<extension::AgentAuth> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let (Some(token), Some(ts), Some(signature)) = (
        super::device_link::device_token(headers),
        header("x-pnex-ts").and_then(|v| v.trim().parse::<i64>().ok()),
        header("x-pnex-signature"),
    ) else {
        return Err(upload_refused());
    };
    let agent = match extension::authenticate(&ctx.db, token, ts, signature, |device_id| {
        message(device_id, ts)
    })
    .await
    {
        Ok(Some(agent)) => agent,
        Ok(None) => return Err(upload_refused()),
        Err(e) => {
            tracing::error!(error = %e, "browser agent lookup failed");
            return Err(Error::InternalServerError);
        }
    };
    match extension::first_use(&ctx.config, signature).await {
        Some(true) => {}
        Some(false) => return Err(upload_refused()),
        None => {
            return Err(Error::CustomError(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorDetail::new(
                    err_codes::EXTENSION_TICKET_UNAVAILABLE,
                    "Browser agent captures are unavailable.",
                ),
            ))
        }
    }
    let limiter = crate::services::rate_limit::RateLimiter::new(
        crate::services::shared_valkey::conn(&ctx.config).await,
    );
    let bucket = format!("extension-upload:{}", agent.device_registry_id);
    if let crate::services::rate_limit::Decision::Deny { .. } = limiter
        .hit(&bucket, UPLOADS_PER_MINUTE, std::time::Duration::from_secs(60))
        .await
    {
        return Err(Error::CustomError(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorDetail::new(err_codes::RATE_LIMITED, "Too many captures."),
        ));
    }
    Ok(agent)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `POST /api/v1/extension/uploads/page` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/uploads/page",
    tag = "extension",
    operation_id = "extension_upload_page",
    summary = "Upload a page capture of a browser agent",
    description = "Called by a browser agent for its `collect_page` missions, no user session: `Authorization: Bearer <agent token>`, `X-Pnex-Ts` and `X-Pnex-Signature` (install key signature of the timestamp, the mission and the SHA-256 of the body, usable once). Stores the readable text as a version of the mission's Markdown document, indexed by the document search; an unchanged text adds no version. Rate-limited per agent.",
    request_body = pnex_core::browser_agent::PageSnapshot,
    responses(
        (status = 201, body = pnex_core::browser_agent::PageStored),
        (status = 400, description = "`field-invalid`: `errors.fields.text` `required` or `max_length:N`", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`: unknown token, bad or reused signature, body not matching it", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found`: not a running page mission of this agent", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`rate-limited`: too many captures", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable`: Valkey unreachable", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn upload_page(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let Ok(snapshot) = serde_json::from_slice::<pnex_core::browser_agent::PageSnapshot>(&raw) else {
        return Ok(super::field_error("body", err_codes::FIELD_INVALID));
    };
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::upload_message(device_id, ts, &snapshot.mission_id, &body_sha)
    })
    .await?;
    match extension::store_page(&ctx, &agent, &snapshot).await {
        Ok(stored) => Ok((StatusCode::CREATED, format::json(stored)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/uploads/video` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/uploads/video",
    tag = "extension",
    operation_id = "extension_upload_video",
    summary = "Upload a video captured by a browser agent",
    description = "Called by a browser agent for a post its `follow_account` mission saw and the server asked to capture, no user session: `Authorization: Bearer <agent token>`, `X-Pnex-Mission`, `X-Pnex-Post` (post id on the platform), `X-Pnex-Ts` and `X-Pnex-Signature` (install key signature of the timestamp, mission, post and SHA-256 of the file, usable once). Body: the mp4 file. Stores it as a `video` media of the library with the post's provenance and marks the registry row captured. Rate-limited per agent.",
    request_body(content = Vec<u8>, content_type = "video/mp4"),
    responses(
        (status = 201, body = pnex_core::browser_agent::SocialVideo),
        (status = 400, description = "`field-invalid`: empty or larger than the cap (`max_length:N`), or not an mp4 (`media-format-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`: unknown token, bad or reused signature, body not matching it", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found` or `extension-video-not-found` (not in the registry, or already captured)", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`rate-limited`: too many captures", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable`: Valkey unreachable", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn upload_video(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    };
    let (mission_id, post) = (header("x-pnex-mission"), header("x-pnex-post"));
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::video_upload_message(device_id, ts, &mission_id, &post, &body_sha)
    })
    .await?;
    match extension::store_video(&ctx, &agent, &mission_id, &post, raw).await {
        Ok(video) => Ok((StatusCode::CREATED, format::json(video)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `GET /api/v1/extension/videos` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/videos",
    tag = "extension",
    operation_id = "extension_videos_list",
    summary = "List the video registry",
    description = "Posts the browser agents saw on followed accounts, newest first: state (`seen`, `captured`) and the captured media. Paginated (`limit`/`offset`). Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::browser_agent::SocialVideo>)),
    security(("bearer" = [], "org" = []))
)]
async fn videos(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match extension::list_videos(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/extension/videos",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => extension_error(e),
    }
}

/// `GET /api/v1/extension/missions` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/missions",
    tag = "extension",
    operation_id = "extension_missions_list",
    summary = "List the browser agent missions",
    description = "Missions of the org, newest first, with the state last reported by their agent. Paginated (`limit`/`offset`). Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::browser_agent::Mission>)),
    security(("bearer" = [], "org" = []))
)]
async fn missions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    match extension::list_missions(&ctx.db, org.org.id).await {
        Ok(rows) => paged("/api/v1/extension/missions", &q, rows),
        Err(e) => extension_error(e),
    }
}

/// `GET /api/v1/extension/missions/{id}` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/missions/{id}",
    tag = "extension",
    operation_id = "extension_missions_get",
    summary = "Get a browser agent mission",
    description = "One mission with its last reported state. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Mission id (UUID)")),
    responses(
        (status = 200, body = pnex_core::browser_agent::Mission),
        (status = 404, description = "`extension-mission-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn mission(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match extension::get_mission(&ctx.db, org.org.id, id).await {
        Ok(m) => format::json(m),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/missions` — `can_write`.
#[utoipa::path(
    post,
    path = "/api/v1/extension/missions",
    tag = "extension",
    operation_id = "extension_missions_create",
    summary = "Create a browser agent mission",
    description = "Declares a mission (`collect_value`: refresh a page every `interval_secs`, at least 60, and read one value into a series). Any online browser agent of the org runs it, or only the pinned `agent_id`; one agent at a time per target. Requires owner, admin or member role.",
    request_body = MissionCreate,
    responses(
        (status = 201, body = pnex_core::browser_agent::Mission),
        (status = 400, description = "`field-invalid`: `errors.fields` tokens `required`, `max_length:N`, `invalid`, `not-found` (`agent_id`); spec fields as `spec.params.<name>`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`extension-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_mission(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<MissionCreate>,
) -> Result<Response> {
    require_write(&org)?;
    match extension::create_mission(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(m) => Ok((StatusCode::CREATED, format::json(m)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `PATCH /api/v1/extension/missions/{id}` — `can_write`, optimistic.
#[utoipa::path(
    patch,
    path = "/api/v1/extension/missions/{id}",
    tag = "extension",
    operation_id = "extension_missions_update",
    summary = "Update a browser agent mission",
    description = "Changes a mission on the version the caller read (`expected_version`); absent fields unchanged, `agent_id: null` unpins it. The mission state starts over (`idle`). Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Mission id (UUID)")),
    request_body = MissionPatch,
    responses(
        (status = 200, body = pnex_core::browser_agent::Mission),
        (status = 400, description = "`field-invalid`: `errors.fields` tokens `required`, `max_length:N`, `invalid`, `not-found` (`agent_id`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`extension-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`extension-mission-version-conflict` (`args.current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_mission(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(patch): Json<MissionPatch>,
) -> Result<Response> {
    require_write(&org)?;
    match extension::update_mission(&ctx.db, org.org.id, id, &patch).await {
        Ok(m) => format::json(m),
        Err(e) => extension_error(e),
    }
}

/// `DELETE /api/v1/extension/missions/{id}` — `can_write`.
#[utoipa::path(
    delete,
    path = "/api/v1/extension/missions/{id}",
    tag = "extension",
    operation_id = "extension_missions_delete",
    summary = "Delete a browser agent mission",
    description = "Deletes a mission; its agent stops it at its next refresh. Series already written stay. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Mission id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`extension-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove_mission(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    match extension::delete_mission(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => extension_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_base_reads_the_edge_then_host() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "localhost:5150".parse().unwrap());
        assert_eq!(public_base(&h).as_deref(), Some("http://localhost:5150"));
        h.insert("x-forwarded-host", "pnex.example.com".parse().unwrap());
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(public_base(&h).as_deref(), Some("https://pnex.example.com"));
        h.insert("x-forwarded-host", "evil\"}.com/x".parse().unwrap());
        assert_eq!(public_base(&h), None);
    }

    #[test]
    fn zip_holds_the_files_and_the_server() {
        let dir = std::env::temp_dir().join(format!("pnex-ext-zip-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::write(dir.join("manifest.json"), "{}").unwrap();
        std::fs::write(dir.join("pkg/a.js"), "x").unwrap();
        std::fs::write(dir.join("server.json"), "{\"server\":\"stale\"}").unwrap();
        let bytes = zip_extension(&dir, Some("https://p.io")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["manifest.json", "pkg/a.js", "server.json"]);
        let mut server = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("server.json").unwrap(), &mut server)
            .unwrap();
        assert_eq!(server, "{\"server\":\"https://p.io\"}");
    }
}
