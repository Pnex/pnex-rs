//! Change feed of the ontology (external-apps.md D435) under
//! `/api/v1/ontology/changes`: what changed in the org since a cursor, for
//! applications that keep a copy. Org from the principal (R1); reading
//! needs `ontology:read`, service accounts included.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::ontology::api::FeedHead;
use serde::Deserialize;

use super::coded_error;
use crate::auth::OrgContext;
use crate::services::ontology::feed::{self, Cursor, FeedError, Filter};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/ontology/changes")
        .add("/", get(list))
        .add("/head", get(head))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, head))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct FeedQuery {
    /// Cursor of the last entry applied (`next` of the previous page); `0`
    /// or absent = from the start, a full copy.
    since: Option<String>,
    /// Entries per page, 1 to 1000 (default 500).
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    /// Comma-separated object types to keep (default every type).
    types: Option<String>,
    /// Comma-separated link types to keep (default every link type).
    link_types: Option<String>,
}

/// A comma-separated list; absent or empty = no restriction.
fn list_of(raw: Option<&str>) -> Option<Vec<String>> {
    let list: Vec<String> = raw
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (!list.is_empty()).then_some(list)
}

/// What an application asked, kept to what its scope allows (D433).
fn within(asked: Option<Vec<String>>, allowed: &[String]) -> Option<Vec<String>> {
    Some(match asked {
        None => allowed.to_vec(),
        Some(a) => a.into_iter().filter(|k| allowed.contains(k)).collect(),
    })
}

fn feed_error(e: FeedError) -> Result<Response> {
    match e {
        FeedError::InvalidCursor => Ok(super::field_error("since", err_codes::FIELD_INVALID)),
        FeedError::Expired => Err(coded_error(
            StatusCode::GONE,
            err_codes::CHANGES_CURSOR_EXPIRED,
            "The cursor is older than the feed keeps: copy again from 0.",
            None,
        )),
        FeedError::Db(e) => Err(e.into()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/changes",
    tag = "ontology",
    operation_id = "ontology_changes_list",
    summary = "List the ontology changes since a cursor",
    description = "Objects and links created, updated, closed or deleted in the org after `since`, oldest first, each once per page with its current state. Pass `next` as `since` to resume; `more` = ask again now. `since=0` is a full copy (the log keeps the latest change of every object and link). Order never skips a transaction that commits late. 410 `changes-cursor-expired` when `since` is older than the retention of closes and deletes (30 days): copy again from 0. For an application's service account, only the types and link types of its scope. Any org member, service accounts included.",
    params(FeedQuery),
    responses(
        (status = 200, body = pnex_core::ontology::api::ChangeFeed),
        (status = 400, description = "`since` is not a cursor", body = crate::openapi::ErrorDetail),
        (status = 410, description = "`changes-cursor-expired`: copy again from 0", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<FeedQuery>,
) -> Result<Response> {
    let since = match Cursor::parse(q.since.as_deref().unwrap_or_default()) {
        Ok(c) => c,
        Err(e) => return feed_error(e),
    };
    let limit = q
        .limit
        .as_deref()
        .and_then(|l| l.parse::<u64>().ok())
        .unwrap_or(500);
    let mut filter = Filter {
        types: list_of(q.types.as_deref()),
        link_types: list_of(q.link_types.as_deref()),
    };
    // An application reads its scope, nothing else (D433).
    if let Some(sp) = org.auth.service.as_ref() {
        match crate::services::apps::of_service_account(&ctx.db, org.org.id, sp.id).await {
            Ok(Some(app)) => {
                filter.types = within(filter.types, &app.scope.types);
                filter.link_types = within(filter.link_types, &app.scope.link_types);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "application scope not read");
                return Err(loco_rs::Error::InternalServerError);
            }
        }
    }
    match feed::page(&ctx.db, org.org.id, since, &filter, limit).await {
        Ok(mut page) => {
            // Masked by classification like every ontology read (crm.md D443).
            match super::ontology::masking(&ctx, &org).await {
                Ok(Some(super::ontology::Masking(m, s))) => m.feed(&s, &mut page),
                Ok(None) => {}
                Err(e) => return super::ontology::onto_error(e),
            }
            format::json(page)
        }
        Err(e) => feed_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/changes/head",
    tag = "ontology",
    operation_id = "ontology_changes_head",
    summary = "Get the newest position of the change feed",
    description = "The cursor of the newest change of the org (`0` when none): an application that copied the org by queries starts following the feed from it. Any org member, service accounts included.",
    responses((status = 200, body = FeedHead)),
    security(("bearer" = [], "org" = []))
)]
async fn head(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match feed::head(&ctx.db, org.org.id).await {
        Ok(c) => format::json(FeedHead {
            cursor: c.to_string(),
        }),
        Err(e) => feed_error(e),
    }
}
