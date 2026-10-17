//! `POST /api/v1/ws-ticket` — trades the caller's bearer token for a
//! one-time ticket that opens a browser websocket (`services::ws_ticket`).
//! Any member of the org (a viewer reads notifications and live cameras);
//! the org comes from the principal (`X-Org-Id` checked by `OrgContext`).

use axum::http::StatusCode;
use loco_rs::prelude::*;

use crate::auth::OrgContext;

/// One-time websocket ticket.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct WsTicket {
    ticket: String,
    /// Lifetime of the ticket in seconds.
    expires_in: u64,
}

#[utoipa::path(
    post,
    path = "/api/v1/ws-ticket",
    tag = "ws-ticket",
    operation_id = "ws_ticket_issue",
    summary = "Issue a one-time websocket ticket",
    description = "Trades the caller's bearer token for a short-lived, single-use ticket bound to the user and the selected org, used to open a browser websocket (notifications, live cameras, doc collaboration) since browsers cannot set an Authorization header on websockets. Any org member. Stores the ticket in Valkey.",
    responses(
        (status = 200, body = WsTicket),
        (status = 503, description = "Valkey unreachable (`ws-ticket-unavailable`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn issue(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match crate::services::ws_ticket::issue(&ctx.config, org.auth.user.id, org.org.id).await {
        Some(ticket) => format::json(WsTicket {
            ticket,
            expires_in: crate::services::ws_ticket::TTL_SECS,
        }),
        None => Err(Error::CustomError(
            StatusCode::SERVICE_UNAVAILABLE,
            loco_rs::controller::ErrorDetail::new(
                pnex_core::err_codes::WS_TICKET_UNAVAILABLE,
                "Websocket tickets are unavailable (Valkey not reachable).".to_string(),
            ),
        )),
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/ws-ticket", post(issue))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(issue))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
