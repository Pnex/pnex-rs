//! MT5 terminals API (markets-research.md appendix E, D847–D849). The UI's
//! routes take the org from the principal (R1) with their permission applied
//! by `OrgContext` (R2), and never return a token (R4, R16) except inside
//! the generated EA. The EA's routes are `Machine`: its bearer token is the
//! authentication and the org is the terminal's own.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::mt5::{Mt5Backfill, Mt5TerminalInput};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::mt5::{self, CallRefusal, Mt5Error, TerminalPrincipal};

/// Largest call of bars and ticks (the EA cuts below it).
const INGEST_BODY_MAX: usize = 1024 * 1024;
/// Largest announce call (a few thousand symbols).
const ANNOUNCE_BODY_MAX: usize = 4 * 1024 * 1024;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/mt5/terminals", get(list).post(create))
        .add("/mt5/terminals/{id}", get(one).put(update).delete(remove))
        .add("/mt5/terminals/{id}/ea", post(ea))
        .add("/mt5/terminals/{id}/backfill", post(backfill))
        .add("/mt5/terminals/{id}/symbols", get(symbols))
        .add("/mt5/config", get(config))
        .add(
            "/mt5/announce",
            post(announce).layer(DefaultBodyLimit::max(ANNOUNCE_BODY_MAX)),
        )
        .add(
            "/mt5/ingest",
            post(ingest).layer(DefaultBodyLimit::max(INGEST_BODY_MAX)),
        )
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list, one, create, update, remove, ea, backfill, symbols, config, announce, ingest
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn mt5_error(e: Mt5Error) -> Result<Response> {
    Err(match e {
        Mt5Error::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        Mt5Error::NotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::MT5_TERMINAL_NOT_FOUND, "MT5 terminal not found."),
        ),
        Mt5Error::NameTaken => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::MT5_TERMINAL_NAME_TAKEN,
                "A terminal of the org already has this name.",
            ),
        ),
        Mt5Error::TooMany => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::MT5_TERMINALS_LIMIT,
            "The org already has as many terminals as it may.",
            Some(serde_json::json!({ "max": pnex_core::mt5::TERMINALS_MAX })),
        ),
        Mt5Error::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::MT5_TERMINAL_VERSION_CONFLICT,
            "The terminal changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current })),
        ),
        Mt5Error::Db(e) => {
            tracing::error!(error = %e, "mt5 terminal database error");
            Error::InternalServerError
        }
    })
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SymbolsQuery {
    /// A part of the symbol or of its description.
    q: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/mt5/terminals`
#[utoipa::path(
    get,
    path = "/api/v1/mt5/terminals",
    tag = "mt5",
    operation_id = "mt5_terminals_list",
    summary = "List the MT5 terminals",
    description = "MetaTrader 5 terminals registered as data sources of the org, with their presence and their selection, never their token. Requires `metatrader:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::mt5::Mt5Terminal>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match mt5::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/mt5/terminals",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => mt5_error(e),
    }
}

/// `POST /api/v1/mt5/terminals`
#[utoipa::path(
    post,
    path = "/api/v1/mt5/terminals",
    tag = "mt5",
    operation_id = "mt5_terminals_create",
    summary = "Register an MT5 terminal",
    description = "Registers a MetaTrader 5 terminal as a data source. Without `bars` it sends the 1-minute bars of its whole catalogue, without `ticks` no tick. It has no token until its Expert Advisor is generated. Requires `metatrader:write`.",
    request_body = Mt5TerminalInput,
    responses(
        (status = 201, body = pnex_core::mt5::Mt5Terminal),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`mt5-terminal-name-taken`, or `mt5-terminals-limit` (args `max`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<Mt5TerminalInput>,
) -> Result<Response> {
    match mt5::create(&ctx.db, org.org.id, &input).await {
        Ok(t) => Ok((StatusCode::CREATED, Json(t)).into_response()),
        Err(e) => mt5_error(e),
    }
}

/// `GET /api/v1/mt5/terminals/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/mt5/terminals/{id}",
    tag = "mt5",
    operation_id = "mt5_terminals_get",
    summary = "Get an MT5 terminal",
    description = "One terminal, its presence and selection, never its token. Requires `metatrader:read`.",
    params(("id" = String, Path, format = "uuid", description = "Terminal id (UUID)")),
    responses(
        (status = 200, body = pnex_core::mt5::Mt5Terminal),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mt5-terminal-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match mt5::get(&ctx.db, org.org.id, id).await {
        Ok(t) => format::json(t),
        Err(e) => mt5_error(e),
    }
}

/// `PUT /api/v1/mt5/terminals/{id}`
#[utoipa::path(
    put,
    path = "/api/v1/mt5/terminals/{id}",
    tag = "mt5",
    operation_id = "mt5_terminals_update",
    summary = "Replace an MT5 terminal",
    description = "Replaces the terminal's name, broker details and selection (`bars`, `ticks`: `*`, names or prefixes such as `XAU*`), on the version that was read (`expected_version`). An absent selection keeps the current one; the EA reads it within five minutes. Requires `metatrader:write`.",
    params(("id" = String, Path, format = "uuid", description = "Terminal id (UUID)")),
    request_body = Mt5TerminalInput,
    responses(
        (status = 200, body = pnex_core::mt5::Mt5Terminal),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mt5-terminal-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`mt5-terminal-name-taken` or `mt5-terminal-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<Mt5TerminalInput>,
) -> Result<Response> {
    match mt5::update(&ctx.db, org.org.id, id, &input).await {
        Ok(t) => format::json(t),
        Err(e) => mt5_error(e),
    }
}

/// `DELETE /api/v1/mt5/terminals/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/mt5/terminals/{id}",
    tag = "mt5",
    operation_id = "mt5_terminals_delete",
    summary = "Delete an MT5 terminal",
    description = "Removes the terminal, its catalogue and its waiting calls; its EA is refused from then on. The bars and ticks already in the tables are kept. Requires `metatrader:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Terminal id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mt5-terminal-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match mt5::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => mt5_error(e),
    }
}

/// `POST /api/v1/mt5/terminals/{id}/ea`
#[utoipa::path(
    post,
    path = "/api/v1/mt5/terminals/{id}/ea",
    tag = "mt5",
    operation_id = "mt5_terminals_ea",
    summary = "Generate the Expert Advisor of an MT5 terminal",
    description = "Returns the terminal's Expert Advisor (MQL5 source to compile in MetaEditor) with this server's address and a new token written in; the token is shown nowhere else, and the previous EA of the terminal stops working at once. Read only: the EA never places an order. Requires `metatrader:write`.",
    params(("id" = String, Path, format = "uuid", description = "Terminal id (UUID)")),
    responses(
        (status = 200, description = "The `.mq5` source", content_type = "text/plain", body = String),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mt5-terminal-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`mt5-base-unknown`: the server's public address cannot be told", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn ea(
    State(ctx): State<AppContext>,
    org: OrgContext,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(base) = super::extension::public_base(&headers) else {
        return Err(super::coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::MT5_BASE_UNKNOWN,
            "The public address of this server cannot be told.",
            None,
        ));
    };
    // Checked before a token is made: a refused address revokes nothing.
    if !pnex_core::mt5::base_ok(&base) {
        return Err(super::coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::MT5_BASE_UNKNOWN,
            "The public address of this server cannot be told.",
            None,
        ));
    }
    let (terminal, token) = match mt5::new_ea_token(&ctx.db, org.org.id, id).await {
        Ok(t) => t,
        Err(e) => return mt5_error(e),
    };
    let Some(source) = pnex_core::mt5::render_ea(&base, &token, &terminal.name) else {
        return Err(Error::InternalServerError);
    };
    tracing::info!(org = org.org.id, terminal = %id, "mt5 expert advisor generated");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"PnexTerminal.mq5\"",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        source,
    )
        .into_response())
}

/// `POST /api/v1/mt5/terminals/{id}/backfill`
#[utoipa::path(
    post,
    path = "/api/v1/mt5/terminals/{id}/backfill",
    tag = "mt5",
    operation_id = "mt5_terminals_backfill",
    summary = "Ask an MT5 terminal for past bars",
    description = "Asks the terminal to send again `days` of 1-minute bars of its selection, as far as its broker keeps them; the EA reads it within five minutes. Requires `metatrader:write`.",
    params(("id" = String, Path, format = "uuid", description = "Terminal id (UUID)")),
    request_body = Mt5Backfill,
    responses(
        (status = 200, body = pnex_core::mt5::Mt5Terminal),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mt5-terminal-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn backfill(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<Mt5Backfill>,
) -> Result<Response> {
    match mt5::request_backfill(&ctx.db, org.org.id, id, body.days).await {
        Ok(t) => format::json(t),
        Err(e) => mt5_error(e),
    }
}

/// `GET /api/v1/mt5/terminals/{id}/symbols`
#[utoipa::path(
    get,
    path = "/api/v1/mt5/terminals/{id}/symbols",
    tag = "mt5",
    operation_id = "mt5_terminals_symbols",
    summary = "List the symbols of an MT5 terminal",
    description = "The catalogue the terminal announced: description, currencies, digits, contract size, first 1-minute bar the broker keeps, and whether the selection sends its bars and ticks. Filter `q` on the symbol or its description. Requires `metatrader:read`. Paginated (`limit`/`offset`).",
    params(("id" = String, Path, format = "uuid", description = "Terminal id (UUID)"), SymbolsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::mt5::Mt5Symbol>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mt5-terminal-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn symbols(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<SymbolsQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let filters: Vec<(String, String)> = q.q.iter().map(|v| ("q".to_string(), v.clone())).collect();
    match mt5::symbols(
        &ctx.db,
        org.org.id,
        id,
        q.q.as_deref(),
        page.limit,
        page.offset,
    )
    .await
    {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/mt5/terminals/{id}/symbols"),
            &filters,
            page,
            count,
            rows,
        )),
        Err(e) => mt5_error(e),
    }
}

// ─────────────────────────── the Expert Advisor's side ───────────────────────────

/// The caller's terminal, from `Authorization: Bearer <terminal token>`.
async fn principal(ctx: &AppContext, headers: &HeaderMap) -> Result<TerminalPrincipal> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let found = match token {
        Some(t) => mt5::authenticate(&ctx.db, t).await.map_err(Error::DB)?,
        None => None,
    };
    found.ok_or_else(|| {
        super::coded_error(
            StatusCode::UNAUTHORIZED,
            err_codes::MT5_TOKEN_INVALID,
            "Unknown or replaced terminal token.",
            None,
        )
    })
}

fn refused(r: CallRefusal) -> Result<Response> {
    Err(match r {
        CallRefusal::Body(token) => super::coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::MT5_BODY_INVALID,
            "Not a call of the MT5 terminal API.",
            Some(serde_json::json!({ "token": token })),
        ),
        CallRefusal::Full => super::coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::MT5_INBOX_FULL,
            "Too many calls of this terminal are waiting.",
            None,
        ),
    })
}

fn json_body(body: &Bytes) -> Result<serde_json::Value> {
    serde_json::from_slice(body).map_err(|_| {
        super::coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::MT5_BODY_INVALID,
            "Not a call of the MT5 terminal API.",
            Some(serde_json::json!({ "token": "json" })),
        )
    })
}

/// `GET /api/v1/mt5/config`
#[utoipa::path(
    get,
    path = "/api/v1/mt5/config",
    tag = "mt5",
    operation_id = "mt5_config",
    summary = "Configuration of the calling MT5 terminal",
    description = "Called by the terminal's Expert Advisor with its token, no user session: the symbols whose bars and ticks it sends, resolved on its catalogue, and the backfill asked. Counts as a heartbeat.",
    responses(
        (status = 200, body = pnex_core::mt5::Mt5Config),
        (status = 401, description = "`mt5-token-invalid`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn config(State(ctx): State<AppContext>, headers: HeaderMap) -> Result<Response> {
    let p = principal(&ctx, &headers).await?;
    format::json(mt5::config(&ctx.db, p).await.map_err(Error::DB)?)
}

/// `POST /api/v1/mt5/announce`
#[utoipa::path(
    post,
    path = "/api/v1/mt5/announce",
    tag = "mt5",
    operation_id = "mt5_announce",
    summary = "Announce the catalogue of the calling MT5 terminal",
    description = "Called by the terminal's Expert Advisor with its token, no user session: the broker's symbols with their description, currencies, digits, contract size and first 1-minute bar (`{\"v\": 1, \"symbols\": [...]}`), upserted into the terminal's catalogue. 4 MiB at most.",
    request_body(content = serde_json::Value, content_type = "application/json"),
    responses(
        (status = 204, description = "Kept"),
        (status = 400, description = "`mt5-body-invalid` (args `token`)", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`mt5-token-invalid`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn announce(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let p = principal(&ctx, &headers).await?;
    let body = json_body(&body)?;
    match mt5::announce(&ctx.db, p, &body).await.map_err(Error::DB)? {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(r) => refused(r),
    }
}

/// `POST /api/v1/mt5/ingest`
#[utoipa::path(
    post,
    path = "/api/v1/mt5/ingest",
    tag = "mt5",
    operation_id = "mt5_ingest",
    summary = "Send bars and ticks from the calling MT5 terminal",
    description = "Called by the terminal's Expert Advisor with its token, no user session: closed 1-minute bars and ticks, times in UTC (`{\"v\": 1, \"broker\", \"bars\": [...], \"ticks\": [...]}`), kept for the tables `mt5_ohlcv_1m` and `mt5_ticks`. A call without bars nor ticks is the heartbeat. 1 MiB at most.",
    request_body(content = serde_json::Value, content_type = "application/json"),
    responses(
        (status = 202, description = "Kept"),
        (status = 400, description = "`mt5-body-invalid` (args `token`)", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`mt5-token-invalid`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`mt5-inbox-full`: too many calls of the org's terminals waiting", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn ingest(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let p = principal(&ctx, &headers).await?;
    let body = json_body(&body)?;
    match mt5::ingest(&ctx.db, p, body).await.map_err(Error::DB)? {
        Ok(()) => Ok(StatusCode::ACCEPTED.into_response()),
        Err(r) => refused(r),
    }
}
