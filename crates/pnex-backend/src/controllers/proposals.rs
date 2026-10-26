//! Proposals inbox (proposals.md D374): what machines proposed to write,
//! and the human decision. Org from the principal (R1); reading under
//! `proposals:read`, deciding under `proposals:decide` (R2), and the write
//! itself checks the role allowed on the type (D188).

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::ontology::proposal::{
    DecisionKind, ProposalDecision, ProposalKind, ProposalStatus, ProposalView,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::ontology::{actor, onto_error, parse_uuid};
use crate::controllers::pagination::{self, PageParams};
use crate::services::ontology::proposals::{self, Filter};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/proposals", get(list))
        .add("/proposals/{id}", get(detail))
        .add("/proposals/{id}/decision", post(decide))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// `proposed` (default), `accepted`, `rejected` or `superseded`.
    status: Option<String>,
    /// Origin prefix: `import:`, `flow:`, `detector:`, `llm:`.
    origin: Option<String>,
    /// Only the proposals about this object (UUID).
    object_id: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/proposals",
    tag = "proposals",
    operation_id = "proposals_list",
    summary = "List proposals",
    description = "What imports, flows and detectors proposed to write and a human has to decide, newest first, by status (proposed by default), origin prefix or object concerned. Any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<ProposalView>),
        (status = 400, description = "Unknown `status`, or `object_id` not a UUID (`field-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let status = match q.status.as_deref() {
        None => None,
        Some(s) => match ProposalStatus::parse(s) {
            Some(s) => Some(s),
            None => {
                return Ok(super::field_error(
                    "status",
                    pnex_core::err_codes::FIELD_INVALID,
                ))
            }
        },
    };
    let object = match q.object_id.as_deref() {
        None => None,
        Some(o) => match Uuid::parse_str(o) {
            Ok(u) => Some(u),
            Err(_) => {
                return Ok(super::field_error(
                    "object_id",
                    pnex_core::err_codes::FIELD_INVALID_UUID,
                ))
            }
        },
    };
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let filter = Filter {
        status,
        origin: q.origin.filter(|o| !o.trim().is_empty()),
        object,
    };
    match proposals::list(
        &ctx.db,
        org.org.id,
        &filter,
        page.limit as u64,
        page.offset as u64,
    )
    .await
    {
        Ok((count, mut rows)) => {
            match super::ontology::masking(&ctx, &org).await {
                Ok(Some(super::ontology::Masking(m, s))) => {
                    rows.iter_mut().for_each(|p| m.refs(&s, &mut p.refs))
                }
                Ok(None) => {}
                Err(e) => return super::ontology::onto_error(e),
            }
            format::json(pagination::envelope(
                "/api/v1/proposals",
                &[],
                page,
                count,
                rows,
            ))
        }
        Err(e) => onto_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/proposals/{id}",
    tag = "proposals",
    operation_id = "proposals_get",
    summary = "Get a proposal",
    description = "One proposal with its values, evidence, origin and decision. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Proposal id")),
    responses(
        (status = 200, body = ProposalView),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    match proposals::get(&ctx.db, org.org.id, id).await {
        Ok(mut v) => {
            match super::ontology::masking(&ctx, &org).await {
                Ok(Some(super::ontology::Masking(m, s))) => m.refs(&s, &mut v.refs),
                Ok(None) => {}
                Err(e) => return super::ontology::onto_error(e),
            }
            format::json(v)
        }
        Err(e) => onto_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/proposals/{id}/decision",
    tag = "proposals",
    operation_id = "proposals_decide",
    summary = "Accept or reject a proposal",
    description = "`accept` writes what the proposal says through the same checks as the ontology pages (optionally with corrected `values`), provenance `proposal:<id>`; `reject` keeps it with its reason so the same values are never proposed again. Accepting an erasure (`object_erase`, from the retention queue) erases the object's personal data (`POST /ontology/objects/{id}/erase`) and needs a person: never an org rule. Repeating the same decision is harmless. Needs the right to decide proposals and to write the type concerned.",
    params(("id" = String, Path, format = "uuid", description = "Proposal id")),
    request_body = ProposalDecision,
    responses(
        (status = 200, body = ProposalView),
        (status = 400, description = "Invalid corrected values (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`, or `ontology-write-forbidden` by the type's write role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`proposal-decided` (decided the other way or superseded), `ontology-version-conflict` (the object changed since it was read), `ontology-link-cardinality`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn decide(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(body): Json<ProposalDecision>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    match crate::services::ontology::erase::decide(&ctx, org.org.id, &actor(&org), id, &body).await
    {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, detail, decide),
        components(schemas(
            ProposalView,
            ProposalDecision,
            DecisionKind,
            ProposalKind,
            ProposalStatus
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
