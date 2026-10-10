//! Live editing of a page — `GET /ws/doc/{id}?ticket=<one-time ticket>`
//! (pages.md P4). Binary y-protocols messages (sync + awareness), handled
//! by `services::pages::room`.
//!
//! Auth: one-time ticket from `POST /api/v1/ws-ticket` (ws_notify school);
//! the org is the ticket's, membership and role re-read from the database.
//! A viewer joins read-only: the server drops its updates (R2). Close
//! codes: 4002 no ticket, 4001 bad ticket, 4006 not a member, 4004 page
//! not found in the org, 4400 unreadable message, 4413 too large, 4422
//! content refused by the schema, 1011 not saved.

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::models::_entities::organization_members;
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::pages::room::{self, Outcome};

const HEARTBEAT_SECS: u64 = 30;
const WATCHDOG_SECS: u64 = 60;

#[derive(Debug, Deserialize)]
pub struct DocWsQuery {
    ticket: Option<String>,
}

pub fn routes() -> Routes {
    Routes::new().prefix("/ws").add("/doc/{id}", get(ws_doc))
}

async fn ws_doc(
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
    Query(q): Query<DocWsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(ticket) = q.ticket.as_deref().filter(|t| !t.trim().is_empty()) else {
        return reject(ws, 4002, "No ticket provided");
    };
    let Some(owner) = crate::services::ws_ticket::redeem(&ctx.config, ticket).await else {
        return reject(ws, 4001, "Invalid ticket");
    };
    let member = organization_members::Entity::find()
        .filter(organization_members::Column::UserId.eq(owner.user_id))
        .filter(organization_members::Column::OrgId.eq(owner.org_id))
        .one(&ctx.db)
        .await;
    let Ok(Some(member)) = member else {
        return reject(ws, 4006, "Not a member of this org");
    };
    let can_write = !matches!(member.role, OrgMemberRole::Viewer);
    if crate::controllers::docs::find_doc(&ctx.db, owner.org_id, id)
        .await
        .is_err()
    {
        return reject(ws, 4004, "Page not found");
    }
    let valkey_url = crate::services::last_cache::ValkeySettings::from_config(&ctx.config)
        .and_then(|s| s.url)
        .filter(|u| !u.trim().is_empty());
    let db = ctx.db.clone();
    ws.on_upgrade(move |socket| async move {
        let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let session = Uuid::new_v4();
        let room = match room::join(&db, valkey_url, id, session, tx).await {
            Ok(room) => room,
            Err(e) => {
                tracing::warn!(doc_id = %id, error = %e, "pages: room not opened");
                close(socket, 1011, "Page unavailable").await;
                return;
            }
        };
        session_loop(socket, rx, &db, &room, session, owner.user_id, can_write).await;
        room::leave(&db, &room, session);
    })
    .into_response()
}

fn reject(ws: WebSocketUpgrade, code: u16, reason: &'static str) -> Response {
    ws.on_upgrade(move |socket| close(socket, code, reason))
        .into_response()
}

async fn close(mut socket: WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

async fn session_loop(
    mut socket: WebSocket,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    db: &DatabaseConnection,
    room: &room::Room,
    session: Uuid,
    user_id: i64,
    can_write: bool,
) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECS));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                if socket.send(Message::Binary(frame.into())).await.is_err() {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
            incoming = tokio::time::timeout(Duration::from_secs(WATCHDOG_SECS), socket.recv()) => {
                let raw = match incoming {
                    Ok(Some(Ok(Message::Binary(raw)))) => raw,
                    Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
                    Ok(Some(Ok(_))) => continue,
                };
                if let Outcome::Close(code, reason) = room.handle(db, session, user_id, can_write, &raw).await {
                    close(socket, code, reason).await;
                    return;
                }
            }
        }
    }
}
