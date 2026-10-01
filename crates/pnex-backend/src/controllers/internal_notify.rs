//! Endpoint interne de livraison websocket — `POST /internal/notify/deliver`.
//!
//! Consommateurs : le runtime de flows (nœud `pnex-notify`, canal
//! `websocket` — le backend ne liant jamais le moteur, le push transite par
//! HTTP interne, école N6 partiellement tirée) et le bouton Test (loopback,
//! même chemin d'envoi).
//!
//! Auth : jeton de service dans `x-pnex-internal-token` comparé à
//! `settings.notifications.internal_token` (env `PNEX_NOTIFY_INTERNAL_TOKEN`)
//! — absent/vide ⇒ 401 systématique (fail-closed, warn au boot). Le token
//! n'apparaît jamais dans les logs.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

use crate::models::_entities::notify_channels;
use crate::services::settings::NotifySettings;
use crate::services::{notify, notify_journal};

const INTERNAL_TOKEN_HEADER: &str = "x-pnex-internal-token";

#[derive(Debug, Deserialize)]
pub struct DeliverInput {
    pub org_id: i64,
    pub channel_id: Uuid,
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub meta: serde_json::Value,
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/internal/notify/deliver", post(deliver))
        .add("/internal/notify/journal", post(journal))
}

/// Service token check shared by both routes (`None` = authorized).
fn unauthorized(ctx: &AppContext, headers: &HeaderMap) -> Option<Response> {
    let settings = NotifySettings::from_config(&ctx.config);
    let Some(expected) = settings.internal_token.as_deref().filter(|t| !t.is_empty()) else {
        // Fail-closed : pas de token configuré ⇒ aucune livraison externe.
        return Some(StatusCode::UNAUTHORIZED.into_response());
    };
    // Deux vecteurs acceptés : `x-pnex-internal-token` (clients internes) ou
    // `Authorization: Bearer` (le canal websocket de pnex-notify envoie son
    // token en Bearer) — même valeur settings attendue ; jamais loggés.
    let supplied = headers
        .get(INTERNAL_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .unwrap_or_default();
    if !crate::auth::service_token::matches(supplied, expected) {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    }
    None
}

/// `POST /internal/notify/journal` — one delivery attempt of the flow node
/// on a non-websocket channel (D86); journaled to O2 in the background.
async fn journal(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: axum::Json<pnex_core::NotifyDeliveryEntry>,
) -> Result<Response> {
    if let Some(denied) = unauthorized(&ctx, &headers) {
        return Ok(denied);
    }
    let mut entry = body.0;
    // The channel must belong to the org (the token is org-agnostic).
    let channel = notify_channels::Entity::find()
        .filter(notify_channels::Column::Id.eq(entry.channel_id))
        .filter(notify_channels::Column::OrgId.eq(entry.org_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let Some(channel) = channel else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if entry.channel_kind.is_empty() {
        entry.channel_kind = channel.kind;
    }
    notify_journal::submit(entry);
    Ok(StatusCode::ACCEPTED.into_response())
}

async fn deliver(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: axum::Json<DeliverInput>,
) -> Result<Response> {
    if let Some(denied) = unauthorized(&ctx, &headers) {
        return Ok(denied);
    }
    // Fencing (D106): a flow worker that no longer owns the org never
    // delivers (the draft test path sends no worker header).
    let fence = crate::services::flow_cluster::fence_header(&headers);
    if !crate::services::flow_cluster::fence_ok(&ctx.db, body.org_id, fence).await {
        tracing::warn!(
            org = body.org_id,
            fence,
            "notify delivery refused: worker fenced"
        );
        return Ok((
            axum::http::StatusCode::CONFLICT,
            axum::Json(serde_json::json!({ "code": "fenced" })),
        )
            .into_response());
    }

    // Draft test (`POST /api/v1/notify/channels/test-draft`): the channel
    // has no row yet, so there is nothing to look up or journal — publish
    // straight to the org bus.
    // The nil UUID is the reserved draft marker and never matches a
    // persisted channel.
    if body.channel_id.is_nil() {
        let msg = pnex_notify::Message {
            subject: body.subject.clone(),
            body: body.body.clone(),
            meta: body.meta.clone(),
        };
        let item = pnex_core::NotifyItem {
            id: 0,
            subject: msg.subject.clone(),
            body: msg.body.clone(),
            meta: msg.meta.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let frame = serde_json::to_string(&item).map_err(|_| Error::InternalServerError)?;
        let delivered = notify::publish(body.org_id, frame);
        tracing::debug!(
            org = body.org_id,
            "deliver : draft test, {delivered} ws session(s)"
        );
        return format::json(serde_json::json!({ "delivered": delivered }));
    }

    // Hygiene: the channel must exist, belong to the org and be of the
    // websocket kind.
    let channel = notify_channels::Entity::find()
        .filter(notify_channels::Column::Id.eq(body.channel_id))
        .filter(notify_channels::Column::OrgId.eq(body.org_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let Some(channel) = channel else {
        tracing::error!(
            "deliver : canal {} org {} introuvable",
            body.channel_id,
            body.org_id
        );
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if channel.kind != "websocket" {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }

    let msg = pnex_notify::Message {
        subject: body.subject.clone(),
        body: body.body.clone(),
        meta: body.meta.clone(),
    };
    // Context from the message meta: the flow node stamps `flow`/`node`/
    // `template`,
    // the Test loopback stamps `test: true`.
    let is_test = body.meta.get("test").and_then(|v| v.as_bool()) == Some(true);
    let entry = pnex_core::NotifyDeliveryEntry {
        org_id: body.org_id,
        channel_id: body.channel_id,
        source: if is_test { "test" } else { "flow" }.into(),
        flow_id: body
            .meta
            .get("flow")
            .and_then(|v| v.as_i64())
            .filter(|f| *f > 0),
        node_id: body
            .meta
            .get("node")
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
            .map(str::to_string),
        template_id: body
            .meta
            .get("template")
            .and_then(|v| v.as_str())
            .and_then(|t| Uuid::parse_str(t).ok()),
        ..Default::default()
    };
    let delivered = notify::deliver_in_app(entry, &msg);
    format::json(serde_json::json!({ "delivered": delivered }))
}
