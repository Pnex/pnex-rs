//! Bus WS des notifications — `GET /ws/notify?ticket=<one-time ticket>`
//! (D51, canal `websocket`).
//!
//! Auth: a one-time ticket from `POST /api/v1/ws-ticket` (a browser cannot
//! set a header on `new WebSocket()`, and a JWT in the URL would reach the
//! access logs); the org is the ticket's, membership re-checked. Close
//! codes: 4002 no ticket, 4001 unknown/used/expired ticket, 4006 no longer
//! a member of the org.
//!
//! Downlink : frames texte JSON `pnex_core::NotifyItem` poussées par le
//! broker (`services::notify::publish`, alimenté par l'endpoint interne et
//! le bouton Test). Uplink ignoré (Pong/Autobahn gérés par axum) — le
//! serveur pingue toutes les 30 s et ferme après 60 s de silence total
//! (navigateur mort, TCP half-open — leçon ws_device 2026-09-02).

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, ExprTrait, QueryFilter};
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::models::_entities::organization_members;
use crate::services::notify::{self, SessionGuard};

/// Heartbeat : ping protocole toutes les 30 s (le navigateur répond en
/// Pong automatiquement).
const HEARTBEAT_SECS: u64 = 30;
/// Silence total (aucune frame entrante, Pong compris) → fermeture.
const WATCHDOG_SECS: u64 = 60;

#[derive(Debug, Deserialize)]
pub struct NotifyWsQuery {
    ticket: Option<String>,
}

pub fn routes() -> Routes {
    Routes::new().prefix("/ws").add("/notify", get(ws_notify))
}

async fn ws_notify(
    State(ctx): State<AppContext>,
    Query(q): Query<NotifyWsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(ticket) = q.ticket.as_deref().filter(|t| !t.trim().is_empty()) else {
        return reject(ws, 4002, "No ticket provided");
    };
    let Some(owner) = crate::services::ws_ticket::redeem(&ctx.config, ticket).await else {
        return reject(ws, 4001, "Invalid ticket");
    };
    let org_id = owner.org_id;
    let member = organization_members::Entity::find()
        .filter(
            organization_members::Column::UserId
                .eq(owner.user_id)
                .and(organization_members::Column::OrgId.eq(org_id)),
        )
        .one(&ctx.db)
        .await;
    if !matches!(member, Ok(Some(_))) {
        return reject(ws, 4006, "Not a member of this org");
    }

    let (tx, rx) = mpsc::unbounded_channel::<String>();
    let session = Uuid::new_v4();
    notify::register_session(org_id, session, tx);
    let guard = SessionGuard::new(org_id, session);

    ws.on_upgrade(move |socket| session_loop(socket, rx, guard, org_id))
        .into_response()
}

fn reject(ws: WebSocketUpgrade, code: u16, reason: &'static str) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            })))
            .await;
    })
    .into_response()
}

/// Boucle de session : downlink (frames du broker) + heartbeat + watchdog.
/// Sortie = déconnexion (guard drop → désenregistrement).
async fn session_loop(
    mut socket: WebSocket,
    mut rx: mpsc::UnboundedReceiver<String>,
    _guard: SessionGuard,
    org_id: i64,
) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECS));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            // ── Downlink : frame notify à pousser ──
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                if socket.send(Message::Text(frame.into())).await.is_err() {
                    break;
                }
            }
            // ── Heartbeat protocole ──
            _ = heartbeat.tick() => {
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
            // ── Uplink : ignoré (Pong/Text), sous watchdog anti-zombie ──
            incoming = tokio::time::timeout(Duration::from_secs(WATCHDOG_SECS), socket.recv()) => {
                match incoming {
                    Ok(Some(Ok(_))) => {}
                    Ok(Some(Err(_))) | Ok(None) => break,
                    Err(_) => {
                        tracing::debug!(org_id, "ws/notify : session muette > {WATCHDOG_SECS} s");
                        break;
                    }
                }
            }
        }
    }
}
