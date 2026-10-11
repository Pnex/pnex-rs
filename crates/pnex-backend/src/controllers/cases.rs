//! Cases (docs/architecture/cases.md): a case is an ontology object
//! (`case`) whose members are `in_case` links, both written through
//! `/api/v1/ontology/*`. This controller adds the opening from an event
//! (window and first members filled in), the computed timeline, the
//! hypotheses board, the snapshot at an instant and the export.

use axum::http::StatusCode;
use axum::routing::{get, post};
use loco_rs::prelude::*;
use pnex_core::ontology::api::{CaseExportQuery, CaseOpenInput, TimelineQuery};

use crate::auth::OrgContext;
use crate::controllers::ontology::{actor, o2, onto_error, parse_uuid, require_write};
use crate::services::ontology::{cases, timeline};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/cases")
        .add("/", post(open_case))
        .add("/{id}/timeline", post(case_timeline))
        .add("/{id}/board", get(case_board))
        .add("/{id}/snapshot", get(case_snapshot))
        .add("/{id}/export", post(case_export))
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn open_case(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<CaseOpenInput>,
) -> Result<Response> {
    require_write(&org)?;
    match cases::open(&ctx.db, org.org.id, &actor(&org), &input).await {
        Ok(v) => Ok((StatusCode::CREATED, format::json(v)).into_response()),
        Err(e) => onto_error(e),
    }
}

/// Read only: any org member, the org from the principal (R1).
async fn case_timeline(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(q): Json<TimelineQuery>,
) -> Result<Response> {
    match timeline::for_case(&ctx.db, o2(&ctx).as_ref(), org.org.id, parse_uuid(&id)?, &q).await {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}

/// Read only: any org member, the org from the principal (R1).
async fn case_board(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    match cases::board(&ctx.db, org.org.id, parse_uuid(&id)?).await {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}

/// Read only: any org member, the org from the principal (R1).
async fn case_snapshot(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<CaseExportQuery>,
) -> Result<Response> {
    match cases::snapshot(&ctx.db, org.org.id, parse_uuid(&id)?, &q).await {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}

/// Read only (an export writes nothing): any org member, as for the
/// timeline it bundles (R1).
async fn case_export(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(q): Json<CaseExportQuery>,
) -> Result<Response> {
    match cases::export(&ctx.db, o2(&ctx).as_ref(), org.org.id, parse_uuid(&id)?, &q).await {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}
