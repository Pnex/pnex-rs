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
use crate::services::ontology::proposals::{self, ExtraLink, Filter};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/proposals", get(list).post(file))
        .add("/proposals/{id}", get(detail))
        .add("/proposals/{id}/decision", post(decide))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// `proposed` (default), `accepted`, `rejected` or `superseded`.
    status: Option<String>,
    /// Origin prefix: `import:`, `flow:`, `detector:`, `llm:`, `agent:<agent id>`.
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

/// One extra link of a filed proposal, from the new object.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct FileLink {
    link_type: String,
    /// Object linked to (UUID), a photo's media for a field note.
    #[schema(format = "uuid")]
    target_id: String,
}

/// `POST /proposals`: a person proposes a new object.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct FileProposal {
    /// Object type of the new object.
    type_key: String,
    title: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    values: serde_json::Map<String, serde_json::Value>,
    /// Link type from the new object to `link_target_id` (the object a note is on).
    #[serde(default)]
    link_type: Option<String>,
    #[serde(default)]
    #[schema(format = "uuid")]
    link_target_id: Option<String>,
    /// Other links made on acceptance, at most 20.
    #[serde(default)]
    links: Vec<FileLink>,
}

const FILE_LINKS_MAX: usize = 20;

#[utoipa::path(
    post,
    path = "/api/v1/proposals",
    tag = "proposals",
    operation_id = "proposals_file",
    summary = "Propose a new object",
    description = "A person proposes a new object (a field note on an equipment, with its photos): it waits in the inbox until another person accepts it, never its author. On acceptance the object is created with its links. Filing the same proposal again answers the one already filed (an offline queue may send twice). Any member.",
    request_body = FileProposal,
    responses(
        (status = 201, body = ProposalView),
        (status = 400, description = "Unknown type or link type, empty title, more than 20 links (`field-invalid`, `field-required`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "An object linked to is not in the organization (`ontology-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn file(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<FileProposal>,
) -> Result<Response> {
    if body.title.trim().is_empty() {
        return Ok(super::field_error(
            "title",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    }
    if body.links.len() > FILE_LINKS_MAX {
        return Ok(super::field_error(
            "links",
            &format!(
                "{}:{FILE_LINKS_MAX}",
                pnex_core::err_codes::FIELD_MAX_LENGTH
            ),
        ));
    }
    if body.link_type.is_some() != body.link_target_id.is_some() {
        return Ok(super::field_error(
            "link_target_id",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    }
    let known = proposals::types_known(
        &ctx.db,
        org.org.id,
        &body.type_key,
        body.link_type
            .iter()
            .chain(body.links.iter().map(|l| &l.link_type)),
    )
    .await;
    match known {
        Ok(None) => {}
        Ok(Some(field)) => {
            return Ok(super::field_error(
                field,
                pnex_core::err_codes::FIELD_INVALID,
            ))
        }
        Err(e) => return onto_error(e),
    }
    // Every object linked to is one of the org (R1).
    for target in body
        .link_target_id
        .iter()
        .chain(body.links.iter().map(|l| &l.target_id))
    {
        let id = parse_uuid(target)?;
        if let Err(e) = crate::services::ontology::objects::find(&ctx.db, org.org.id, id).await {
            return onto_error(e);
        }
    }
    let input = pnex_core::ontology::proposal::ProposalInput {
        kind: Some(ProposalKind::ObjectCreate),
        type_key: body.type_key,
        title: Some(body.title.trim().to_string()),
        values: body.values,
        link_type: body.link_type,
        link_target_id: body.link_target_id,
        evidence: serde_json::Value::Null,
        origin: "member".into(),
        ..Default::default()
    };
    let extra: Vec<ExtraLink> = body
        .links
        .into_iter()
        .map(|l| ExtraLink {
            link_type: l.link_type,
            target_id: l.target_id,
        })
        .collect();
    match proposals::file(&ctx.db, org.org.id, org.auth.user.id, &input, &extra).await {
        Ok(v) => Ok((axum::http::StatusCode::CREATED, Json(v)).into_response()),
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
    description = "`accept` writes what the proposal says through the same checks as the ontology pages (optionally with corrected `values`), provenance `proposal:<id>`; `reject` keeps it with its reason so the same values are never proposed again. Accepting an erasure (`object_erase`, from the retention queue) erases the object's personal data (`POST /ontology/objects/{id}/erase`) and needs a person: never an org rule. Accepting a `tool_call` (a write of an internal agent, D785) runs its exact arguments with the decider's rights; a refusal of the tool keeps it open (409 with the tool's code, or `proposal-tool-failed`). Accepting a `dataset_mark` (personal data detected in a table) marks the table and needs `governance:write`. Repeating the same decision is harmless. Needs the right to decide proposals and to write the type concerned.",
    params(("id" = String, Path, format = "uuid", description = "Proposal id")),
    request_body = ProposalDecision,
    responses(
        (status = 200, body = ProposalView),
        (status = 400, description = "Invalid corrected values (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied` (also when the type's pack reserves decisions to `<pack>:validate`, as `maintenance:validate`), `ontology-write-forbidden` by the type's write role, or `proposal-own` (the author accepts their own proposal)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`proposal-decided` (decided the other way or superseded), `ontology-version-conflict` (the object changed since it was read), `ontology-link-cardinality`, `proposal-tool-failed` (args `tool`) or the refusal code of the tool of a `tool_call`", body = crate::openapi::ErrorDetail)
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
    let kind = match proposals::get(&ctx.db, org.org.id, id).await {
        Ok(p) => p.kind,
        Err(e) => return onto_error(e),
    };
    // A pack may reserve its decisions to validators (`<pack>:validate`, parts-docs.md PD1d).
    let pack = match proposals::pack_of(&ctx.db, org.org.id, id).await {
        Ok(p) => p,
        Err(e) => return onto_error(e),
    };
    if let Some(perm) = pack.and_then(|p| validate_permission(&p)) {
        if !org.can(perm) {
            return Err(crate::auth::access_denied(perm));
        }
    }
    if kind == ProposalKind::ToolCall && body.decision == DecisionKind::Accept {
        return accept_tool_call(&ctx, &org, id, &body).await;
    }
    // Marking a table is a governance decision (governance.md D804).
    if kind == ProposalKind::DatasetMark && body.decision == DecisionKind::Accept {
        if !org.can("governance:write") {
            return Err(crate::auth::access_denied("governance:write"));
        }
        let accepted = crate::services::governance::accept_dataset_mark(
            &ctx.db,
            org.org.id,
            &actor(&org),
            id,
            &body,
        )
        .await;
        return match accepted {
            Ok(v) => format::json(v),
            Err(e) => onto_error(e),
        };
    }
    match crate::services::ontology::erase::decide(&ctx, org.org.id, &actor(&org), id, &body).await
    {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}

/// The `<pack>:validate` permission, when the registry declares one for the pack.
fn validate_permission(pack: &str) -> Option<&'static str> {
    pnex_api_contract::iam::PERMISSIONS
        .iter()
        .map(|(p, _)| *p)
        .find(|p| p.strip_suffix(":validate") == Some(pack))
}

/// An internal agent's write (D785): its exact arguments, run with the decider's rights.
async fn accept_tool_call(
    ctx: &AppContext,
    org: &OrgContext,
    id: Uuid,
    body: &ProposalDecision,
) -> Result<Response> {
    use crate::services::internal_agents::{self, AcceptError, Decider};
    // The arguments are the ones the agent proposed: corrected values would be a new write.
    if body.values.is_some() {
        return Ok(super::field_error(
            "values",
            pnex_core::err_codes::FIELD_INVALID,
        ));
    }
    let reason = body
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| {
            r.chars()
                .take(pnex_core::ontology::proposal::REASON_MAX)
                .collect()
        });
    let decider = Decider {
        user_id: org.auth.user.id,
        email: &org.auth.user.email,
        role: org.role,
        grants: org.grants.clone(),
    };
    match internal_agents::accept_tool_call(ctx, org.org.id, decider, id, reason).await {
        Ok(v) => format::json(v),
        Err(AcceptError::NotFound) => {
            onto_error(crate::services::ontology::OntologyError::NotFound)
        }
        Err(AcceptError::Decided) => {
            onto_error(crate::services::ontology::OntologyError::ProposalDecided)
        }
        Err(AcceptError::Refused {
            code,
            args,
            message,
        }) => Err(super::coded_error(
            axum::http::StatusCode::CONFLICT,
            code,
            message,
            args,
        )),
        Err(AcceptError::Db(e)) => Err(crate::controllers::db_error(e)),
    }
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, file, detail, decide),
        components(schemas(
            FileProposal,
            FileLink,
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
