//! Notifications (D49–D54) — CRUD org-scoped des canaux et templates,
//! aperçu de rendu, tests d'envoi et journal des livraisons.
//!
//! École `viz_widgets.rs` : scoping org (404 masqué cross-org), écriture
//! gated `can_write()`, 400 champ-par-champ. **Divergence assumée** : le
//! conflit de nom unique répond **409** (`conflict()`, école `flows.rs`)
//! et non 400-champ — le nom est une clé naturelle et l'UI peut proposer
//! « renommer » ; cf. tests.
//!
//! Secrets (D54) : jamais rendus — `mask_config` neutralise les champs
//! `secret` au GET et `secrets_set` expose leur seule présence ; au PUT,
//! `merge_config` traite absent/`null` comme « inchangé » (le front envoie
//! null tant que l'utilisateur n'a pas cliqué « remplacer »). Aucun secret
//! n'apparaît dans les messages d'erreur ni les logs.
//!
//! Vault (secrets.md lot S4): stored `secret` fields are references
//! `{"secret_id"}` to `org_secrets`; a typed value lands in the channel's
//! dedicated secret, sends resolve the references in memory.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{notify_channels, notify_templates};
use crate::models::{notify_channels::NotifyChannels, notify_templates::NotifyTemplates};
use crate::services::settings::NotifySettings;
use crate::services::{notify, notify_journal, secrets};
use pnex_core::{err_codes, NotifyChannel, NotifyTemplate, TemplateVar};

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn conflict(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 400 for a template that fails to render: machine code
/// `notify-template-render` + canonical English description; the minijinja
/// detail travels verbatim in `errors.args.detail` (resolved by the
/// frontend as `err-notify-template-render`).
fn template_render_error(e: &pnex_notify::NotifyError) -> Error {
    let detail = match e {
        pnex_notify::NotifyError::Render(detail) => detail.clone(),
        other => other.to_string(),
    };
    Error::CustomError(
        StatusCode::BAD_REQUEST,
        loco_rs::controller::ErrorDetail {
            error: Some(err_codes::NOTIFY_TEMPLATE_RENDER.to_string()),
            description: Some(format!("Template rendering failed: {detail}")),
            errors: Some(serde_json::json!({ "args": { "detail": detail } })),
        },
    )
}

fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

// Submodules (pure code moves from the former single-file controller).
mod channels;
mod deliveries;
mod helpers;
mod templates;
mod testing;

// Private globs: bring every submodule's visible items into this module's
// namespace so `routes()` resolves its handlers and sibling submodules share
// helpers through `use super::*;`.
use channels::*;
use deliveries::*;
use helpers::*;
use templates::*;
use testing::*;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/notify")
        .add("/kinds", get(kinds))
        // Static avant param : axum donne priorité au segment statique
        // (`test-draft` n'est jamais mangé par `{id}`).
        .add("/channels", get(list_channels).post(create_channel))
        .add("/channels/test-draft", post(test_draft))
        .add(
            "/channels/{id}",
            get(channel_detail)
                .put(update_channel)
                .delete(delete_channel),
        )
        .add("/channels/{id}/test", post(test_channel))
        .add("/deliveries", get(list_deliveries))
        .add("/templates", get(list_templates).post(create_template))
        .add(
            "/templates/{id}",
            get(template_detail)
                .put(update_template)
                .delete(delete_template),
        )
        .add("/templates/{id}/preview", post(preview_template))
}
