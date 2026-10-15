//! Live editing of a page — `GET /ws/doc/{id}?ticket=<one-time ticket>`
//! (pages.md P4). Binary y-protocols messages (sync + awareness), handled
//! by `services::pages::room`.
//!
//! Auth: one-time ticket from `POST /api/v1/ws-ticket` (ws_notify school);
//! the org is the ticket's, membership and role re-read from the database.
//! A viewer joins read-only: the server drops its updates (R2); the role is
//! re-read at every heartbeat, so a demotion or removal applies within
//! 30 s (SEC-29). Close
//! codes: 4002 no ticket, 4001 bad ticket, 4006 not a member, 4004 page
//! not found in the org, 4400 unreadable message, 4413 too large, 4422
//! content refused by the schema, 4429 too many messages, 1011 not saved.

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
/// Per-connection rate (SEC-28): bursts of 200 messages, 50 per second
/// sustained — fast typing with presence stays far below.
const RATE_BURST: f64 = 200.0;
const RATE_PER_SEC: f64 = 50.0;
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
    let Ok(Some(can_write)) = member_can_write(&ctx.db, owner.user_id, owner.org_id).await else {
        return reject(ws, 4006, "Not a member of this org");
    };
    if crate::controllers::docs::find_doc(&ctx.db, owner.org_id, id)
        .await
        .is_err()
    {
        return reject(ws, 4004, "Page not found");
    }
    let valkey_url = crate::services::pages::valkey_url(&ctx.config);
    let db = ctx.db.clone();
    // Frames over the update limit are refused by axum before buffering.
    let ws = ws.max_message_size(pnex_core::pages::MAX_UPDATE_BYTES + 1024);
    ws.on_upgrade(move |socket| async move {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(room::DOWNLINK_CAPACITY);
        let session = Uuid::new_v4();
        let room = match room::join(&db, valkey_url, id, session, tx).await {
            Ok(room) => room,
            Err(e) => {
                tracing::warn!(doc_id = %id, error = %e, "pages: room not opened");
                close(socket, 1011, "Page unavailable").await;
                return;
            }
        };
        // Leaves the room on every exit path, a panic included.
        let _guard = LeaveGuard {
            db: db.clone(),
            room: room.clone(),
            session,
        };
        let who = (owner.user_id, owner.org_id);
        session_loop(socket, rx, &db, &room, session, who, can_write).await;
    })
    .into_response()
}

/// Write right of a member (`Ok(None)` = not a member).
async fn member_can_write(
    db: &DatabaseConnection,
    user_id: i64,
    org_id: i64,
) -> Result<Option<bool>, sea_orm::DbErr> {
    let member = organization_members::Entity::find()
        .filter(organization_members::Column::UserId.eq(user_id))
        .filter(organization_members::Column::OrgId.eq(org_id))
        .one(db)
        .await?;
    Ok(member.map(|m| !matches!(m.role, OrgMemberRole::Viewer)))
}

struct LeaveGuard {
    db: DatabaseConnection,
    room: std::sync::Arc<room::Room>,
    session: Uuid,
}

impl Drop for LeaveGuard {
    fn drop(&mut self) {
        room::leave(&self.db, &self.room, self.session);
    }
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
    mut rx: mpsc::Receiver<Vec<u8>>,
    db: &DatabaseConnection,
    room: &room::Room,
    session: Uuid,
    (user_id, org_id): (i64, i64),
    mut can_write: bool,
) {
    let mut tokens = RATE_BURST;
    let mut refilled = std::time::Instant::now();
    // Last inbound frame (Pong included): the heartbeat tick closes a
    // session silent for longer than the watchdog (half-open socket).
    let mut last_rx = tokio::time::Instant::now();
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
                if last_rx.elapsed() > Duration::from_secs(WATCHDOG_SECS) {
                    tracing::debug!(%session, "ws/doc: silent session closed");
                    break;
                }
                // Role re-read (SEC-29): removed → closed, demoted → read-only.
                // A failed lookup keeps the current right (no mass close on a DB blip).
                match member_can_write(db, user_id, org_id).await {
                    Ok(Some(w)) => can_write = w,
                    Ok(None) => {
                        close(socket, 4006, "Not a member of this org").await;
                        return;
                    }
                    Err(_) => {}
                }
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                last_rx = tokio::time::Instant::now();
                let raw = match incoming {
                    Some(Ok(Message::Binary(raw))) => raw,
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => continue,
                };
                tokens = (tokens + refilled.elapsed().as_secs_f64() * RATE_PER_SEC).min(RATE_BURST);
                refilled = std::time::Instant::now();
                if tokens < 1.0 {
                    close(socket, 4429, "Too many messages").await;
                    return;
                }
                tokens -= 1.0;
                if let Outcome::Close(code, reason) = room.handle(db, session, user_id, can_write, &raw).await {
                    close(socket, code, reason).await;
                    return;
                }
            }
        }
    }
}
