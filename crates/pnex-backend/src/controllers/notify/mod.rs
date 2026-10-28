//! Notifications (D49–D54) — org-scoped CRUD of channels and templates,
//! render preview, send tests and delivery log.
//!
//! `viz_widgets.rs` school: org scoping (cross-org 404 masked), write
//! gated by its IAM permission, 400 field by field. **Deliberate divergence**: the
//! unique name conflict answers **409** (`conflict()`, `flows.rs` school)
//! and not a field 400 — the name is a natural key and the UI can offer
//! "rename"; see tests.
//!
//! Secrets (D54): never returned — `mask_config` neutralizes the `secret`
//! fields on GET and `secrets_set` exposes only their presence; on PUT,
//! `merge_config` treats absent/`null` as "unchanged" (the front sends
//! null as long as the user has not clicked "replace"). No secret
//! appears in error messages or logs.
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
    crate::controllers::field_error(field, msg)
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
        // Static before param: axum gives priority to the static segment
        // (`test-draft` is never swallowed by `{id}`).
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

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        kinds,
        list_channels,
        create_channel,
        test_draft,
        channel_detail,
        update_channel,
        delete_channel,
        test_channel,
        list_deliveries,
        list_templates,
        create_template,
        template_detail,
        update_template,
        delete_template,
        preview_template
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
