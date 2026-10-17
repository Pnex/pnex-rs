//! Devices: org-scoped registry (D2) + shared global catalogue.
//!
//! Contracts (pagination D14/D240, errors D239):
//! - `GET /api/v1/devices`: filters `device_type` ("all" = no-op),
//!   `capability`, `device_id` (exact), `active` (true|false), `search`,
//!   `label` + `limit`/`offset` → `Paginated<Device>`;
//! - `POST`: implicit reactivation of a known inactive device (200) or 400
//!   `device-already-active`, tier quota per type, otherwise inactive
//!   creation + auto DeviceToken (urlsafe 32-byte token + base64 ChaCha20
//!   key) → 201 with the full DTO;
//! - `PUT /{id}/board`, `PUT /{id}/peripherals`: replace the board variant
//!   or the screen choice;
//! - `DELETE /{id}`: cleans build_records + device_token → 204.
//!
//! Access rules (multi-tenant D2 + deny by default):
//! - **org** scoping (`X-Org-Id`), never per-user;
//! - writes (create/update/delete) for owner/admin/member, reads for every
//!   member;
//! - `DELETE` answers 204 **with no body** (a body on 204 is unreadable in
//!   the browser; the cleaned-records count goes to the logs);
//! - `predefined-devices` catalog requires authentication;
//! - the `revision` filter matches the `revision` field;
//! - lists are paginated in SQL (catalogue and org registry alike).

use axum::extract::{Path, Query, RawQuery, State};
use axum::http::StatusCode;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use loco_rs::prelude::*;
use sea_orm::sea_query::extension::postgres::PgExpr;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, QueryTrait, Set, TransactionTrait,
};
use serde::Deserialize;
use std::collections::HashMap;

use super::pagination;
use crate::auth::{AuthUser, OrgContext};
use crate::models::_entities::{
    build_records, device_capabilities, device_registries, device_tokens, device_types, mcu_boards,
    ota_assignments, predefined_device_capabilities, predefined_devices,
    sea_orm_active_enums::CapabilityMode, subscription_tiers,
};
use pnex_core::{boards::ScreenChoice, err_codes};

// ─────────────────────────── Helpers ───────────────────────────

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

use crate::controllers::field_error;

// Submodules (pure code moves from the former single-file controller).
mod board;
mod catalogue;
mod crud;
mod dto;
mod peripherals;
mod token;

// Private globs: bring every submodule's visible items into this module's
// namespace so `routes()` resolves its handlers and sibling submodules share
// helpers through `use super::*;`.
use board::*;
use catalogue::*;
use crud::*;
use dto::*;
use peripherals::*;
use token::*;

// Disambiguate our `delete` handler from `loco_rs::prelude::delete`
// (an explicit binding shadows glob imports).
use crud::delete;

// Historical crate-level paths, preserved for external consumers.
pub(crate) use crud::{tier_limit_for, tiers_enforced};
pub(crate) use dto::capabilities_of;
pub use dto::capability_mode_str;
pub(crate) use token::{generate_device_key, generate_token};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/devices")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).delete(delete))
        .add("/{id}/peripherals", put(update_peripherals))
        .add("/{id}/board", put(update_board))
}

/// Routes of the global catalogue (shared /api/v1 prefix).
pub fn catalogue_routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/device-capabilities", get(capabilities))
        .add("/predefined-devices", get(predefined_list))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        create,
        detail,
        delete,
        update_board,
        update_peripherals,
        capabilities,
        predefined_list
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
