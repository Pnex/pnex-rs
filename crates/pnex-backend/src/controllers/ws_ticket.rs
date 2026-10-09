//! `POST /api/v1/ws-ticket` — trades the caller's bearer token for a
//! one-time ticket that opens a browser websocket (`services::ws_ticket`).
//! Any member of the org (a viewer reads notifications and live cameras);
//! the org comes from the principal (`X-Org-Id` checked by `OrgContext`).

use axum::http::StatusCode;
use loco_rs::prelude::*;

use crate::auth::OrgContext;

async fn issue(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match crate::services::ws_ticket::issue(&ctx.config, org.auth.user.id, org.org.id).await {
        Some(ticket) => format::json(serde_json::json!({
            "ticket": ticket,
            "expires_in": crate::services::ws_ticket::TTL_SECS,
        })),
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
