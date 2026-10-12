//! Cases (docs/architecture/cases-workspace.md): a case is an ontology object
//! (`case`) holding dated events and undated pieces; objects are attached to
//! events with a role. This controller serves the workspace and its
//! gestures, the raw signals, the tracks, the detections and their review,
//! the hypotheses board, the pages, the frozen states,
//! the retention purge and the export. Other edits (an event's title or
//! date, a role) go through `/api/v1/ontology/*`.

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use loco_rs::prelude::*;
use pnex_core::ontology::api::{
    CaseAttachInput, CaseDetachInput, CaseEventInput, CaseExport, CaseLayout, CaseOpenInput,
    CasePageInput, CasePurgeInput, CasePurged, CaseSnapshotInput, CaseTracksQuery, DetectQuery,
    ExpandInput, ProposalReject, ProposalRetain, SignalsQuery, TrackImportInput,
};

use crate::auth::OrgContext;
use crate::controllers::ontology::{actor, o2, onto_error, parse_uuid, require_write};
use crate::services::ontology::cases;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/cases")
        .add("/", post(open_case))
        .add("/templates", get(page_templates))
        .add("/import", post(import_case))
        .add("/{id}/workspace", get(workspace))
        .add("/{id}/events", post(create_event))
        .add("/{id}/attach", post(attach))
        .add("/{id}/detach", post(detach))
        .add("/{id}/signals", post(signals))
        .add("/{id}/tracks", post(tracks))
        .add(
            "/{id}/tracks/import",
            // The file travels as JSON text: a little over its own cap.
            post(import_track).layer(DefaultBodyLimit::max(cases::IMPORT_MAX_BYTES + 1024 * 1024)),
        )
        .add("/{id}/tracks/imports", get(track_imports))
        .add("/{id}/tracks/imports/{iid}", delete(delete_track_import))
        .add("/{id}/detect", post(detect))
        .add("/{id}/proposals", get(proposals))
        .add("/{id}/proposals/{pid}/retain", post(retain))
        .add("/{id}/proposals/{pid}/reject", post(reject))
        .add("/{id}/board", get(board))
        .add("/{id}/export", get(export))
        .add("/{id}/snapshots", get(snapshots).post(freeze))
        .add("/{id}/reads", get(reads))
        .add("/{id}/relations", get(relations))
        .add("/{id}/expand", post(expand))
        .add("/{id}/layout", get(layout).put(save_layout))
        .add("/{id}/snapshots/{seq}", get(snapshot))
        .add("/{id}/pages", post(add_page))
        .add("/{id}/purge", get(purge_candidates).post(purge))
}

fn reply<T: serde::Serialize>(r: crate::services::ontology::Result<T>) -> Result<Response> {
    match r {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}

fn created<T: serde::Serialize>(r: crate::services::ontology::Result<T>) -> Result<Response> {
    match r {
        Ok(v) => Ok((StatusCode::CREATED, format::json(v)).into_response()),
        Err(e) => onto_error(e),
    }
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn open_case(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<CaseOpenInput>,
) -> Result<Response> {
    require_write(&org)?;
    created(cases::open(&ctx.db, org.org.id, &actor(&org), &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1); objects
/// of another org are never attached (they read as missing).
async fn import_case(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(doc): Json<CaseExport>,
) -> Result<Response> {
    require_write(&org)?;
    created(cases::import_case(&ctx.db, org.org.id, &actor(&org), &doc).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn workspace(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::workspace(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn create_event(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseEventInput>,
) -> Result<Response> {
    require_write(&org)?;
    created(cases::create_event(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn attach(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseAttachInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(cases::attach(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn detach(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseDetachInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(cases::detach(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn signals(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(q): Json<SignalsQuery>,
) -> Result<Response> {
    reply(cases::signals(&ctx.db, o2(&ctx).as_ref(), org.org.id, parse_uuid(&id)?, &q).await)
}

/// Read only: any org member, the org from the principal (R1); device ids
/// are resolved in the org's registry only.
async fn tracks(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(q): Json<CaseTracksQuery>,
) -> Result<Response> {
    let reader = cases::Reader {
        user_id: actor(&org).user_id,
        purpose: "tracks",
    };
    let id = parse_uuid(&id)?;
    reply(cases::tracks(&ctx.db, o2(&ctx).as_ref(), org.org.id, reader, id, &q).await)
}

/// Write (a dry run writes nothing but stays a write gesture): role guard
/// first (R2), the org from the principal (R1), K38 for a person.
async fn import_track(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<TrackImportInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(cases::import(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn track_imports(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::imports(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn delete_track_import(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, iid)): Path<(String, String)>,
) -> Result<Response> {
    require_write(&org)?;
    let (id, iid) = (parse_uuid(&id)?, parse_uuid(&iid)?);
    match cases::delete_import(&ctx.db, org.org.id, id, iid).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

/// Write (it records proposals): role guard first (R2), the org from the
/// principal (R1).
async fn detect(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(q): Json<DetectQuery>,
) -> Result<Response> {
    require_write(&org)?;
    let (id, user) = (parse_uuid(&id)?, actor(&org).user_id);
    reply(cases::detect(&ctx.db, o2(&ctx).as_ref(), org.org.id, user, id, &q).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn proposals(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::proposals(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn retain(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, pid)): Path<(String, String)>,
    Json(input): Json<ProposalRetain>,
) -> Result<Response> {
    require_write(&org)?;
    let (id, pid) = (parse_uuid(&id)?, parse_uuid(&pid)?);
    reply(cases::retain(&ctx.db, org.org.id, &actor(&org), id, pid, &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn reject(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, pid)): Path<(String, String)>,
    Json(input): Json<ProposalReject>,
) -> Result<Response> {
    require_write(&org)?;
    let (id, pid) = (parse_uuid(&id)?, parse_uuid(&pid)?);
    reply(cases::reject(&ctx.db, org.org.id, &actor(&org), id, pid, &input).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn board(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::board(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn freeze(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseSnapshotInput>,
) -> Result<Response> {
    require_write(&org)?;
    let id = parse_uuid(&id)?;
    created(
        cases::freeze(
            &ctx.db,
            o2(&ctx).as_ref(),
            org.org.id,
            &actor(&org),
            id,
            &input,
        )
        .await,
    )
}

/// Read only: any org member, the org from the principal (R1).
async fn snapshots(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::snapshots(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn snapshot(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, seq)): Path<(String, i32)>,
) -> Result<Response> {
    reply(cases::snapshot(&ctx.db, org.org.id, parse_uuid(&id)?, seq).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn reads(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(
        cases::reads(
            &ctx.db,
            o2(&ctx).as_ref(),
            org.org.id,
            parse_uuid(&id)?,
            200,
        )
        .await,
    )
}

/// Read only: any org member, the org from the principal (R1).
async fn relations(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::relations(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Read only (it adds nothing to the case): any org member (R1).
async fn expand(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<ExpandInput>,
) -> Result<Response> {
    reply(cases::expand(&ctx.db, org.org.id, parse_uuid(&id)?, &input).await)
}

/// Read only: any org member, the org from the principal (R1).
async fn layout(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::layout(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn save_layout(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseLayout>,
) -> Result<Response> {
    require_write(&org)?;
    let id = parse_uuid(&id)?;
    reply(cases::save_layout(&ctx.db, org.org.id, &actor(&org), id, &input).await)
}

/// Read only (an export writes nothing): any org member (R1).
async fn export(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::export(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Read only: the page templates of the packs the org installed.
async fn page_templates(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    reply(cases::templates(&ctx.db, org.org.id).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
async fn add_page(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CasePageInput>,
) -> Result<Response> {
    require_write(&org)?;
    created(cases::add_page(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Read only: the personal objects only this case holds (K16).
async fn purge_candidates(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::candidates(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1); the
/// candidates are re-computed server side.
async fn purge(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CasePurgeInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(
        cases::purge(
            &ctx.db,
            org.org.id,
            &actor(&org),
            parse_uuid(&id)?,
            &input.ids,
        )
        .await
        .map(|purged| CasePurged { purged }),
    )
}
