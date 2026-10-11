//! Cases (docs/architecture/cases.md): a case is an ontology object
//! (`case`) whose members are `in_case` links, both written through
//! `/api/v1/ontology/*`. This controller only adds the computed timeline.

use axum::routing::post;
use loco_rs::prelude::*;
use pnex_core::ontology::api::TimelineQuery;

use crate::auth::OrgContext;
use crate::controllers::ontology::{onto_error, parse_uuid};
use crate::services::ontology::timeline;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/cases")
        .add("/{id}/timeline", post(case_timeline))
}

/// Read only: any org member, the org from the principal (R1).
async fn case_timeline(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(q): Json<TimelineQuery>,
) -> Result<Response> {
    match timeline::for_case(&ctx, org.org.id, parse_uuid(&id)?, &q).await {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}
