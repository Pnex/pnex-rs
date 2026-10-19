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
        .add("/uploads/captions", post(upload_captions))
        .add(
            "/uploads/live",
            post(upload_live)
                .layer(axum::extract::DefaultBodyLimit::max(
                    pnex_core::browser_agent::VIDEO_MAX + 1024 * 1024,
                ))
                .layer(axum::middleware::from_fn(
                    crate::services::compute_limits::upload_gate,
                )),
        )
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
        .add(
            "/uploads/proof",
            post(upload_proof).layer(axum::extract::DefaultBodyLimit::max(
                pnex_core::browser_agent::PROOF_SCREENSHOT_MAX + 1024 * 1024,
            )),
        )
        .add("/uploads/site-page", post(upload_site_page))
        .add("/uploads/crawl-done", post(upload_crawl_done))
        .add("/missions/{id}/pages", get(site_pages))
        .add("/proofs", get(proofs))
        .add("/proofs/verify", get(verify_proofs))
        .add("/proofs/{id}", get(proof))
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
        upload_captions,
        upload_live,
        upload_proof,
        upload_site_page,
        upload_crawl_done,
        site_pages,
        proofs,
        verify_proofs,
        proof,
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
        ExtensionError::TextStoreUnavailable => coded(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::MEDIA_TRANSCRIPTS_UNAVAILABLE,
            "The text store is unreachable: nothing recorded, retry later.",
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
        ExtensionError::LimitReached { max } => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::EXTENSION_LIMIT_REACHED,
            "The organization reached its limit of browser agents or missions.",
            Some(serde_json::json!({ "max": max.to_string() })),
        )),
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
        (status = 409, description = "`extension-install-paired-elsewhere`: the install is paired with another org; `extension-limit-reached` (`args.max`): the org holds as many agents as allowed", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn pair(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(req): Json<PairRequest>,
) -> Result<Response> {
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
    summary = "Rename a browser agent or change its mode",
    description = "Renames a browser agent and/or sets its mode: `shared` (every platform but the exclusive ones) or `dedicated:<platform>` (that platform only, D269); absent fields unchanged, the mode applies at the agent's next connection. Remove an agent with `DELETE /api/v1/devices/{id}`. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Agent id (device registry id)")),
    request_body = BrowserAgentPatch,
    responses(
        (status = 200, body = pnex_core::browser_agent::BrowserAgent),
        (status = 400, description = "`field-invalid`: `errors.fields.name` `required` or `max_length:N`, `errors.fields.mode` `invalid`", body = crate::openapi::ErrorDetail),
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
    match extension::update_agent(&ctx.db, org.org.id, id, &input).await {
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
/// Fallback only: behind a proxy that ends TLS the scheme may be wrong.
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
fn zip_extension(
    dir: &std::path::Path,
    server: Option<&str>,
    firefox: bool,
) -> std::io::Result<Vec<u8>> {
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
            // One manifest per browser: Firefox's replaces Chrome's.
            let name = match name.as_str() {
                "server.json" => continue,
                "manifest.firefox.json" if !firefox => continue,
                "manifest.json" if firefox => continue,
                "manifest.firefox.json" => "manifest.json".to_string(),
                _ => name,
            };
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

/// Browser of the extension download.
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DownloadQuery {
    /// `chrome` (default) or `firefox`.
    browser: Option<String>,
}

/// `GET /api/v1/extension/download` — public.
#[utoipa::path(
    get,
    path = "/api/v1/extension/download",
    tag = "extension",
    operation_id = "extension_download",
    summary = "Download the browser extension (public)",
    description = "Zip of the PNeX browser agent extension shipped with this server, with this server's address preset (`server.json`) so the user only signs in: Chrome by default, `browser=firefox` for Firefox. Load it unpacked from the browser's extensions page. No authentication: it holds no secret.",
    params(DownloadQuery),
    responses(
        (status = 200, description = "Zip archive", content_type = "application/zip", body = Vec<u8>),
        (status = 404, description = "`extension-dist-not-found`: this server ships no extension build", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn download(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    Query(q): Query<DownloadQuery>,
) -> Result<Response> {
    let firefox = q.browser.as_deref() == Some("firefox");
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
    // The configured public origin (issuer, like /meta/version) first: a
    // proxy in front (TLS ended upstream) rewrites X-Forwarded-Proto to
    // http. Request headers only without a public HTTPS issuer (dev).
    let issuer = crate::auth::settings::RauthySettings::from_config(&ctx.config)
        .ok()
        .and_then(|s| s.issuer_url);
    let server = super::meta::public_origin(issuer.as_deref()).or_else(|| public_base(&headers));
    let bytes =
        tokio::task::spawn_blocking(move || zip_extension(&dir, server.as_deref(), firefox))
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
                if firefox {
                    "attachment; filename=\"pnex-browser-agent-firefox.zip\""
                } else {
                    "attachment; filename=\"pnex-browser-agent.zip\""
                },
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
        .hit(
            &bucket,
            UPLOADS_PER_MINUTE,
            std::time::Duration::from_secs(60),
        )
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
    let Ok(snapshot) = serde_json::from_slice::<pnex_core::browser_agent::PageSnapshot>(&raw)
    else {
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
    description = "Called by a browser agent for a post its `follow_account` mission saw and the server asked to capture, no user session: `Authorization: Bearer <agent token>`, `X-Pnex-Mission`, `X-Pnex-Post` (post id on the platform), optional `X-Pnex-Part` (1-based, default 1: a long recording is sent in parts of 30 minutes), `X-Pnex-Ts` and `X-Pnex-Signature` (install key signature of the timestamp, mission, post, part and SHA-256 of the file, usable once). Body: the mp4 file or WebM recording. Part 1 is stored as a `video` media of the library with the post's provenance and marks the registry row captured; each next part is a media of its own (`part`, `part_of`). Rate-limited per agent.",
    request_body(content = Vec<u8>, content_type = "video/mp4"),
    responses(
        (status = 201, body = pnex_core::browser_agent::SocialVideo),
        (status = 400, description = "`field-invalid`: empty or larger than the cap (`max_length:N`), or not an mp4 (`media-format-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`: unknown token, bad or reused signature, body not matching it", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found` or `extension-video-not-found` (not in the registry, already captured, or a part already stored / before part 1)", body = crate::openapi::ErrorDetail),
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
    // Absent: the whole file (part 1); a refused value fails the signature.
    let part = match header("x-pnex-part").as_str() {
        "" => 1,
        p => p.parse::<u32>().unwrap_or(0),
    };
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::video_upload_message(
            device_id,
            ts,
            &mission_id,
            &post,
            part,
            &body_sha,
        )
    })
    .await?;
    match extension::store_video(&ctx, &agent, &mission_id, &post, part, raw).await {
        Ok(video) => Ok((StatusCode::CREATED, format::json(video)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/uploads/captions` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/uploads/captions",
    tag = "extension",
    operation_id = "extension_upload_captions",
    summary = "Upload the platform captions of a post",
    description = "Called by a browser agent for a post its `follow_account` mission saw, no user session: `Authorization: Bearer <agent token>`, `X-Pnex-Ts` and `X-Pnex-Signature` (install key signature of the timestamp, mission, post and SHA-256 of the body, usable once). Stores the captions as a transcription run of origin `platform` on the post's video media, created without a file when the video is not downloaded. Rate-limited per agent.",
    request_body = pnex_core::browser_agent::CaptionsUpload,
    responses(
        (status = 201, body = pnex_core::browser_agent::SocialVideo),
        (status = 400, description = "`field-invalid`: `errors.fields.cues` `invalid:<reason>` (empty, too many, bad times or language, text too long)", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`: unknown token, bad or reused signature, body not matching it", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found` or `extension-video-not-found`", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`rate-limited`: too many captures", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable` (Valkey) or `media-transcripts-unavailable` (text store): nothing recorded", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn upload_captions(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let Ok(upload) = serde_json::from_slice::<pnex_core::browser_agent::CaptionsUpload>(&raw)
    else {
        return Ok(super::field_error("body", err_codes::FIELD_INVALID));
    };
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::captions_upload_message(
            device_id,
            ts,
            &upload.mission_id,
            &upload.platform_id,
            &body_sha,
        )
    })
    .await?;
    match extension::store_captions(&ctx, &agent, &upload).await {
        Ok(video) => Ok((StatusCode::CREATED, format::json(video)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/uploads/live` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/uploads/live",
    tag = "extension",
    operation_id = "extension_upload_live",
    summary = "Upload a part of a live recorded by a browser agent",
    description = "Called by a browser agent for its `watch_live` missions, no user session: `Authorization: Bearer <agent token>`, `X-Pnex-Mission`, `X-Pnex-Part` (1, 2…), optional `X-Pnex-Started` (RFC 3339), `X-Pnex-Ts` and `X-Pnex-Signature` (install key signature of the timestamp, mission, part and SHA-256 of the file, usable once). Body: the WebM recording of the part. Stores it as a `video` media of the library with the live's provenance. Rate-limited per agent.",
    request_body(content = Vec<u8>, content_type = "video/webm"),
    responses(
        (status = 201, body = pnex_core::browser_agent::LivePartStored),
        (status = 400, description = "`field-invalid`: not a WebM recording, or larger than the cap", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`: unknown token, bad or reused signature, body not matching it", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found`: not a running live mission of this agent", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`rate-limited`: too many captures", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable`: Valkey unreachable", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn upload_live(
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
    let mission_id = header("x-pnex-mission");
    let Ok(part) = header("x-pnex-part").parse::<u32>() else {
        return Ok(super::field_error("x-pnex-part", err_codes::FIELD_INVALID));
    };
    let started = header("x-pnex-started");
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::live_upload_message(device_id, ts, &mission_id, part, &body_sha)
    })
    .await?;
    let started = (!started.is_empty()).then_some(started.as_str());
    match extension::store_live_part(&ctx, &agent, &mission_id, part, started, raw).await {
        Ok(stored) => Ok((StatusCode::CREATED, format::json(stored)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/uploads/proof` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/uploads/proof",
    tag = "extension",
    operation_id = "extension_upload_proof",
    summary = "Upload a part of a web proof",
    description = "Called by a browser agent for its `capture_proof` tasks (asked by the server for relevant pages new or changed), no user session: `Authorization: Bearer <agent token>`, `X-Pnex-Mission`, `X-Pnex-Part` (`screenshot`: full-page PNG; `html`: rendered DOM, cleaned again by the server and only ever served as a download; `text`: a page snapshot JSON), `X-Pnex-Ts` and `X-Pnex-Signature` (install key signature of the timestamp, mission, part and SHA-256 of the body, usable once). Once the three parts are in, the server seals the proof: manifest with its reception time, the SHA-256 of each part and of the org's previous manifest. Rate-limited per agent.",
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 201, body = pnex_core::browser_agent::ProofPartStored),
        (status = 400, description = "`field-invalid`: unknown part, empty, too large, not a PNG or not a snapshot", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`: unknown token, bad or reused signature, body not matching it", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found`: not an open proof task of this agent", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`rate-limited`: too many captures", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable`: Valkey unreachable", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn upload_proof(
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
    let mission_id = header("x-pnex-mission");
    let Some(part) = pnex_core::browser_agent::ProofPart::parse(&header("x-pnex-part")) else {
        return Ok(super::field_error("x-pnex-part", err_codes::FIELD_INVALID));
    };
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::proof_upload_message(device_id, ts, &mission_id, part, &body_sha)
    })
    .await?;
    match crate::services::proofs::store_part(&ctx, &agent, &mission_id, part, raw).await {
        Ok(stored) => Ok((StatusCode::CREATED, format::json(stored)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/uploads/site-page` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/uploads/site-page",
    tag = "extension",
    operation_id = "extension_upload_site_page",
    summary = "Report a page of an agent site crawl",
    description = "Called by a browser agent walking a site for a `crawl_site` mission run by agents (pages rendered by JavaScript), no user session, signed like a page capture (`X-Pnex-Ts`, `X-Pnex-Signature` over the timestamp, the mission and the SHA-256 of the body). The page must belong to the declared site; it enters the site registry with the same relevance rules as a server crawl. Rate-limited per agent.",
    request_body = pnex_core::browser_agent::SitePageReport,
    responses(
        (status = 201, body = pnex_core::browser_agent::SitePage),
        (status = 400, description = "`field-invalid`: off the site, text too long, registry full", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found`: not an agent crawl of this agent", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`rate-limited`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn upload_site_page(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let Ok(report) = serde_json::from_slice::<pnex_core::browser_agent::SitePageReport>(&raw)
    else {
        return Ok(super::field_error("body", err_codes::FIELD_INVALID));
    };
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::upload_message(device_id, ts, &report.mission_id, &body_sha)
    })
    .await?;
    match crate::services::crawl::store_report(&ctx, &agent, &report).await {
        Ok(page) => Ok((StatusCode::CREATED, format::json(page)).into_response()),
        Err(e) => extension_error(e),
    }
}

/// `POST /api/v1/extension/uploads/crawl-done` — agent credentials, no user.
#[utoipa::path(
    post,
    path = "/api/v1/extension/uploads/crawl-done",
    tag = "extension",
    operation_id = "extension_upload_crawl_done",
    summary = "End an agent site crawl",
    description = "Called by a browser agent at the end of a `crawl_site` walk, signed like a page capture: the pages it read; a complete walk counts the pages of the site it did not see (`gone` after two crawls without them).",
    request_body = pnex_core::browser_agent::CrawlDone,
    responses(
        (status = 204, description = "Recorded"),
        (status = 400, description = "`field-invalid`", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`extension-upload-refused`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`extension-mission-not-found`", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`rate-limited`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`extension-ticket-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn upload_crawl_done(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let Ok(done) = serde_json::from_slice::<pnex_core::browser_agent::CrawlDone>(&raw) else {
        return Ok(super::field_error("body", err_codes::FIELD_INVALID));
    };
    let body_sha = sha256_hex(&raw);
    let agent = signed_upload(&ctx, &headers, |device_id, ts| {
        pnex_core::browser_agent::upload_message(device_id, ts, &done.mission_id, &body_sha)
    })
    .await?;
    match crate::services::crawl::agent_done(&ctx, &agent, &done).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => extension_error(e),
    }
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SitePagesQuery {
    /// `listed`, `relevant`, `captured` or `gone`.
    state: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/extension/missions/{id}/pages` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/missions/{id}/pages",
    tag = "extension",
    operation_id = "extension_site_pages_list",
    summary = "List the pages of a crawled site",
    description = "The site registry of a `crawl_site` mission, most recently seen first: state (`listed`, `relevant`, `captured`, `gone`), first and last seen, text fingerprint, the document holding the versions of its text and its last proof. Paginated (`limit`/`offset`). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Mission id (UUID)"), SitePagesQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::browser_agent::SitePage>),
        (status = 404, description = "`extension-mission-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn site_pages(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<SitePagesQuery>,
) -> Result<Response> {
    if let Err(e) = extension::get_mission(&ctx.db, org.org.id, id).await {
        return extension_error(e);
    }
    let state = q.state.as_deref().filter(|s| !s.is_empty());
    match crate::services::crawl::pages(&ctx.db, org.org.id, id, state).await {
        Ok(rows) => paged(
            &format!("/api/v1/extension/missions/{id}/pages"),
            &PageQuery {
                limit: q.limit,
                offset: q.offset,
            },
            rows,
        ),
        Err(e) => extension_error(e.into()),
    }
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ProofsQuery {
    /// Registry page (UUID).
    #[param(value_type = Option<String>, format = "uuid")]
    page_id: Option<Uuid>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/extension/proofs` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/proofs",
    tag = "extension",
    operation_id = "extension_proofs_list",
    summary = "List the web proofs",
    description = "Proofs of the org, newest first (`page_id` filters on a registry page): parts (screenshot, HTML, text media with their SHA-256), manifest and chain number once sealed. Paginated (`limit`/`offset`). Any member.",
    params(ProofsQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::browser_agent::WebProof>)),
    security(("bearer" = [], "org" = []))
)]
async fn proofs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ProofsQuery>,
) -> Result<Response> {
    match crate::services::proofs::list(&ctx.db, org.org.id, q.page_id).await {
        Ok(rows) => paged(
            "/api/v1/extension/proofs",
            &PageQuery {
                limit: q.limit,
                offset: q.offset,
            },
            rows,
        ),
        Err(e) => extension_error(e.into()),
    }
}

/// `GET /api/v1/extension/proofs/{id}` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/proofs/{id}",
    tag = "extension",
    operation_id = "extension_proofs_get",
    summary = "Get a web proof",
    description = "One proof: parts and, once sealed, its manifest (requested and final address, agent, mission, server reception time, SHA-256 of each part and of the previous manifest). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Proof id (UUID)")),
    responses(
        (status = 200, body = pnex_core::browser_agent::WebProof),
        (status = 404, description = "`extension-mission-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn proof(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match crate::services::proofs::get(&ctx.db, org.org.id, id).await {
        Ok(Some(p)) => format::json(p),
        Ok(None) => extension_error(ExtensionError::MissionNotFound),
        Err(e) => extension_error(e.into()),
    }
}

/// `GET /api/v1/extension/proofs/verify` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/extension/proofs/verify",
    tag = "extension",
    operation_id = "extension_proofs_verify",
    summary = "Verify the chain of web proofs",
    description = "Recomputes the org's chain of sealed proofs: each manifest's SHA-256, its link to the previous manifest, and each part's SHA-256 against its stored media. Answers the first broken proof, if any. Any member.",
    responses((status = 200, body = pnex_core::browser_agent::ProofChainCheck)),
    security(("bearer" = [], "org" = []))
)]
async fn verify_proofs(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match crate::services::proofs::verify(&ctx.db, org.org.id).await {
        Ok(check) => format::json(check),
        Err(e) => extension_error(e.into()),
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
        (status = 409, description = "`extension-limit-reached` (`args.max`): the org holds as many missions as allowed", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_mission(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<MissionCreate>,
) -> Result<Response> {
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
        (status = 404, description = "`extension-mission-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove_mission(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
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
        std::fs::write(dir.join("manifest.firefox.json"), "{\"firefox\":1}").unwrap();
        let bytes = zip_extension(&dir, Some("https://p.io"), false).unwrap();
        let firefox = zip_extension(&dir, None, true).unwrap();
        let mut ff = zip::ZipArchive::new(std::io::Cursor::new(firefox)).unwrap();
        let mut manifest = String::new();
        std::io::Read::read_to_string(&mut ff.by_name("manifest.json").unwrap(), &mut manifest)
            .unwrap();
        assert_eq!(manifest, "{\"firefox\":1}");
        assert!(ff.by_name("manifest.firefox.json").is_err());
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
