//! Website candidates waiting for a human (web-presence.md D366): the open
//! `a_pour_site` links a flow proposed, and the decision. Org from the
//! principal (R1); reading under `ontology:read`, deciding under
//! `ontology:write` (R2), through the shared link service (same checks as
//! the ontology pages). `valide_par` is the user's display name, never an
//! identifier (R4).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::web_visit::{SiteCandidate, SiteDecision, SiteDecisionKind};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::auth::OrgContext;
use crate::controllers::ontology::{actor, onto_error};
use crate::controllers::pagination::{self, PageParams};
use crate::models::_entities::resource_edges;
use crate::services::ontology::{self as onto, changes, links};

const LINK: &str = "a_pour_site";
/// Candidates listed at most (best scores first).
const LISTED_MAX: i64 = 500;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/web-presence/candidates", get(list))
        .add("/web-presence/candidates/{id}/decision", post(decide))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// `propose` (default) or `confirme`.
    status: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/web-presence/candidates",
    tag = "web-presence",
    operation_id = "web_presence_candidates_list",
    summary = "List website candidates",
    description = "Open `a_pour_site` links of a status (`propose` by default, or `confirme`), best score first, with what proved them (SIREN, declaration number), where they came from and the organization. Any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<SiteCandidate>),
        (status = 400, description = "Unknown `status` (`field-invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let status = q.status.as_deref().unwrap_or("propose");
    if !matches!(status, "propose" | "confirme") {
        return Ok(super::field_error(
            "status",
            pnex_core::err_codes::FIELD_INVALID,
        ));
    }
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = ctx
        .db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT e.id, e.attributes, e.valid_from, o.id AS org_id, o.title AS org_title, \
               s.id AS site_id, s.title AS site_title, \
               coalesce(s.properties ->> 'domaine', '') AS domain, \
               coalesce(s.properties ->> 'url_racine', '') AS url \
             FROM resource_edges e \
             JOIN objects o ON o.id = e.source_object_id AND o.valid_to IS NULL \
             JOIN objects s ON s.id = e.target_object_id AND s.valid_to IS NULL \
             WHERE e.org_id = $1 AND e.relation = $2 AND e.valid_to IS NULL \
               AND e.attributes ->> 'statut' = $3 \
             ORDER BY (e.attributes ->> 'score')::double precision DESC NULLS LAST, e.id \
             LIMIT $4",
            [
                org.org.id.into(),
                LINK.into(),
                status.into(),
                LISTED_MAX.into(),
            ],
        ))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "website candidates database error");
            Error::InternalServerError
        })?;
    let all: Vec<SiteCandidate> = rows
        .iter()
        .filter_map(|r| {
            let a: Value = r.try_get("", "attributes").ok()?;
            let text = |k: &str| a[k].as_str().unwrap_or_default().to_string();
            let since: sea_orm::prelude::DateTimeWithTimeZone = r.try_get("", "valid_from").ok()?;
            Some(SiteCandidate {
                link_id: r.try_get("", "id").ok()?,
                organization_id: r.try_get::<uuid::Uuid>("", "org_id").ok()?.to_string(),
                organization: r.try_get("", "org_title").ok()?,
                site_id: r.try_get::<uuid::Uuid>("", "site_id").ok()?.to_string(),
                domain: r.try_get("", "domain").ok()?,
                url: r.try_get("", "url").ok()?,
                status: text("statut"),
                score: a["score"].as_f64(),
                proof_siren: a["preuve_siren"].as_bool().unwrap_or(false),
                proof_nda: a["preuve_nda"].as_bool().unwrap_or(false),
                source: text("source"),
                query: text("requete"),
                rank: a["rang"].as_f64(),
                opinion: text("avis_llm"),
                since: since.to_rfc3339(),
            })
        })
        .collect();
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    let results: Vec<SiteCandidate> = all.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(
        "/api/v1/web-presence/candidates",
        &[],
        page,
        count,
        results,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/web-presence/candidates/{id}/decision",
    tag = "web-presence",
    operation_id = "web_presence_candidates_decide",
    summary = "Confirm or reject a website candidate",
    description = "`confirme` marks the open `a_pour_site` link confirmed (the site may then be crawled); `rejete` marks it rejected and closes it, so a flow never proposes it again. `valide_par` takes the user's display name. Needs the right to write the ontology.",
    params(("id" = i64, Path, description = "Link id")),
    request_body = SiteDecision,
    responses(
        (status = 200, body = pnex_core::ontology::api::LinkView),
        (status = 400, description = "Malformed body (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`, or `ontology-write-forbidden` by the type's write role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`: no open website link with this id", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn decide(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(body): Json<SiteDecision>,
) -> Result<Response> {
    let who = org
        .auth
        .user
        .full_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "—".into());
    let r: onto::Result<_> = changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let edge = resource_edges::Entity::find_by_id(id)
            .filter(resource_edges::Column::OrgId.eq(org.org.id))
            .filter(resource_edges::Column::Relation.eq(LINK))
            .filter(resource_edges::Column::ValidTo.is_null())
            .one(&txn)
            .await?
            .ok_or(onto::OntologyError::NotFound)?;
        let mut attrs: Map<String, Value> =
            edge.attributes.as_object().cloned().unwrap_or_default();
        let status = match body.decision {
            SiteDecisionKind::Confirme => "confirme",
            SiteDecisionKind::Rejete => "rejete",
        };
        attrs.insert("statut".into(), json!(status));
        attrs.insert("valide_par".into(), json!(who));
        let mut v = links::update(&txn, org.org.id, &actor(&org), id, &attrs).await?;
        if body.decision == SiteDecisionKind::Rejete {
            v = links::close(&txn, org.org.id, &actor(&org), id, None).await?;
        }
        txn.commit().await?;
        changes::flush();
        Ok(v)
    })
    .await;
    match r {
        Ok(v) => Ok((StatusCode::OK, format::json(v)).into_response()),
        Err(e) => onto_error(e),
    }
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, decide),
        components(schemas(SiteCandidate, SiteDecision, SiteDecisionKind))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
