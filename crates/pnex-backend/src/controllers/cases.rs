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
use crate::controllers::ontology::{actor, o2, onto_error, paged, parse_uuid, PageQuery};
use crate::services::ontology::cases;
use axum::extract::Query;

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
        .add("/{id}/signals", get(signals))
        .add("/{id}/tracks", get(tracks))
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
        .add("/{id}/graph/query", post(expand))
        .add("/{id}/layout", get(layout).put(save_layout))
        .add("/{id}/snapshots/{seq}", get(snapshot))
        .add("/{id}/pages", post(add_page))
        .add("/{id}/purge", get(purge_candidates).post(purge))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        open_case,
        import_case,
        workspace,
        create_event,
        attach,
        detach,
        signals,
        tracks,
        import_track,
        track_imports,
        delete_track_import,
        detect,
        proposals,
        retain,
        reject,
        board,
        freeze,
        snapshots,
        snapshot,
        reads,
        relations,
        expand,
        layout,
        save_layout,
        export,
        page_templates,
        add_page,
        purge_candidates,
        purge
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
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
#[utoipa::path(
    post,
    path = "/api/v1/cases",
    tag = "cases",
    operation_id = "cases_open",
    summary = "Open a case",
    description = "Creates a case (an ontology object of type `case`). With `from` (a dated notification or event), its first event is that alert, with the flow that raised it and the flow's neighbours attached as context. With `parent_id` (a case of the org), it is opened as a sub-case of that case and takes its legal basis. Needs a writer role.",
    request_body = pnex_core::ontology::api::CaseOpenInput,
    responses(
        (status = 201, body = pnex_core::ontology::api::ObjectView),
        (status = 400, description = "Invalid field (field → token; `parent_id`: not a case of the org)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (flow of `from`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn open_case(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<CaseOpenInput>,
) -> Result<Response> {
    created(cases::open(&ctx.db, org.org.id, &actor(&org), &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1); objects
/// of another org are never attached (they read as missing).
#[utoipa::path(
    post,
    path = "/api/v1/cases/import",
    tag = "cases",
    operation_id = "cases_import",
    summary = "Re-create a case from an export",
    description = "Creates a new case in the org from a `pnex.case/1` export document: its events and their attachments. Objects that no longer exist (or live in another org) are listed in `missing`, never re-created. Needs a writer role.",
    request_body = pnex_core::ontology::api::CaseExport,
    responses(
        (status = 201, body = pnex_core::ontology::api::CaseImported),
        (status = 400, description = "Unknown `schema` or invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`case-legal-basis-required`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn import_case(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(doc): Json<CaseExport>,
) -> Result<Response> {
    created(cases::import_case(&ctx.db, org.org.id, &actor(&org), &doc).await)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/workspace",
    tag = "cases",
    operation_id = "cases_workspace",
    summary = "Get the workspace of a case",
    description = "The case with its events (oldest first) and their attached objects with roles, its undated pieces, the areas and places of its members for the map, its enclosing cases (nearest first) and its sub-cases with the members they share with it. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseWorkspace),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn workspace(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::workspace(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/events",
    tag = "cases",
    operation_id = "cases_events_create",
    summary = "Add an event to a case",
    description = "Creates a dated event (`occurred` is a fuzzy time: from, to, precision) in the case with its first attachments, in one transaction. Returns the updated workspace. Needs a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::CaseEventInput,
    responses(
        (status = 201, body = pnex_core::ontology::api::CaseWorkspace),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (case or attached object)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`case-legal-basis-required` (attaching a person before the processing basis is declared)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_event(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseEventInput>,
) -> Result<Response> {
    created(cases::create_event(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/attach",
    tag = "cases",
    operation_id = "cases_attach",
    summary = "Attach an object to a case",
    description = "Attaches an object to an event of the case (`event_id`) with a role (defaulted by object type), or adds it to the undated pieces (no `event_id`). Returns the updated workspace. Needs a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::CaseAttachInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseWorkspace),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (case, event or object)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`case-legal-basis-required`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn attach(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseAttachInput>,
) -> Result<Response> {
    reply(cases::attach(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/detach",
    tag = "cases",
    operation_id = "cases_detach",
    summary = "Detach an object from a case",
    description = "With `event_id`: detaches the object from that event (it stays in the case, undated if nothing else dates it). Without: removes it from the case (for an event, its pieces stay). Returns the updated workspace. Needs a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::CaseDetachInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseWorkspace),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detach(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseDetachInput>,
) -> Result<Response> {
    reply(cases::detach(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/signals",
    tag = "cases",
    operation_id = "cases_signals",
    summary = "Read the raw signals around a case",
    description = "Link openings/closings, changes, time ranges, notifications and fuzzy dates concerning the case members over an RFC 3339 window (default: the span of the case events, else the last 30 days), oldest first, computed on read. `truncated` = a source hit its cap. Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Case object id (UUID)"),
        pnex_core::ontology::api::SignalsQuery
    ),
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseSignals),
        (status = 400, description = "Invalid window (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn signals(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<SignalsQuery>,
) -> Result<Response> {
    reply(cases::signals(&ctx.db, o2(&ctx).as_ref(), org.org.id, parse_uuid(&id)?, &q).await)
}

/// Read only: any org member, the org from the principal (R1); device ids
/// are resolved in the org's registry only.
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/tracks",
    tag = "cases",
    operation_id = "cases_tracks",
    summary = "Read the position tracks of the case members",
    description = "Gap-free track segments of each located member over the window, generalised to `resolution_ms` per timeline pixel; points are `[lon, lat, alt|null, dt_ms]` relative to `t0_ms`. Side effect: the read of persons' tracks is logged (see `GET /reads`). `available` = false when the position history cannot be read. Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Case object id (UUID)"),
        pnex_core::ontology::api::CaseTracksQuery
    ),
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseTracks),
        (status = 400, description = "Invalid window (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn tracks(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<CaseTracksQuery>,
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
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/tracks/import",
    tag = "cases",
    operation_id = "cases_tracks_import",
    summary = "Import a GPX or CSV track onto a case member",
    description = "Reads a GPX or CSV file (sent as JSON text, up to 20 MiB) and stores its clean points as the position history of a case member, kept as case evidence with its SHA-256. `dry_run=true` only previews (nothing written). Times without offset need `time_zone` (IANA). Needs a writer role, even for a dry run.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::TrackImportInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::TrackImportPreview),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (case or member)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`case-legal-basis-required`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "Body over the limit"),
        (status = 422, description = "`track-import-timezone-required`, `track-import-empty`, `track-import-unreadable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn import_track(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<TrackImportInput>,
) -> Result<Response> {
    reply(cases::import(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/tracks/imports",
    tag = "cases",
    operation_id = "cases_tracks_imports_list",
    summary = "List the imported tracks of a case",
    description = "Every track file imported into the case, with its member, fingerprint, point counts and time span. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::TrackImport>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn track_imports(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = cases::imports(&ctx.db, org.org.id, parse_uuid(&id)?).await;
    paged(&format!("/api/v1/cases/{id}/tracks/imports"), &[], &q, rows)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    delete,
    path = "/api/v1/cases/{id}/tracks/imports/{iid}",
    tag = "cases",
    operation_id = "cases_tracks_imports_delete",
    summary = "Delete an imported track",
    description = "Deletes an imported track and the positions it brought. Needs a writer role. 204 with no body.",
    params(
        ("id" = String, Path, format = "uuid", description = "Case object id (UUID)"),
        ("iid" = String, Path, format = "uuid", description = "Import id (UUID)")
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_track_import(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, iid)): Path<(String, String)>,
) -> Result<Response> {
    let (id, iid) = (parse_uuid(&id)?, parse_uuid(&iid)?);
    match cases::delete_import(&ctx.db, org.org.id, id, iid).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

/// Write (it records proposals): role guard first (R2), the org from the
/// principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/detect",
    tag = "cases",
    operation_id = "cases_detect",
    summary = "Run the detectors on a case",
    description = "Runs the co-presence, stop and signal-loss detectors over the members' tracks in an RFC 3339 window (default: the span of the events, else the last 24 h) and records the new proposals for review (a rejected one is never proposed again). Logs the read of persons' tracks. Needs a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::DetectQuery,
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseDetected),
        (status = 400, description = "Invalid window (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detect(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(q): Json<DetectQuery>,
) -> Result<Response> {
    let (id, user) = (parse_uuid(&id)?, actor(&org).user_id);
    reply(cases::detect(&ctx.db, o2(&ctx).as_ref(), org.org.id, user, id, &q).await)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/proposals",
    tag = "cases",
    operation_id = "cases_proposals_list",
    summary = "List the detection proposals of a case",
    description = "Every detection proposed to the case: still proposed, retained (with the event it became) or rejected (with the reason). Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::CaseProposal>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn proposals(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = cases::proposals(&ctx.db, org.org.id, parse_uuid(&id)?).await;
    paged(&format!("/api/v1/cases/{id}/proposals"), &[], &q, rows)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/proposals/{pid}/retain",
    tag = "cases",
    operation_id = "cases_proposals_retain",
    summary = "Retain a detection proposal",
    description = "Turns a proposal into an event of the case titled `title`, with the members it concerns attached. Returns the updated workspace. Needs a writer role.",
    params(
        ("id" = String, Path, format = "uuid", description = "Case object id (UUID)"),
        ("pid" = String, Path, format = "uuid", description = "Proposal id (UUID)")
    ),
    request_body = pnex_core::ontology::api::ProposalRetain,
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseWorkspace),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`case-proposal-stale` (already rejected), `case-legal-basis-required`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn retain(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, pid)): Path<(String, String)>,
    Json(input): Json<ProposalRetain>,
) -> Result<Response> {
    let (id, pid) = (parse_uuid(&id)?, parse_uuid(&pid)?);
    reply(cases::retain(&ctx.db, org.org.id, &actor(&org), id, pid, &input).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/proposals/{pid}/reject",
    tag = "cases",
    operation_id = "cases_proposals_reject",
    summary = "Reject a detection proposal",
    description = "Marks a proposal rejected with an optional reason; it is kept and never proposed again. Returns the rejected proposal. Needs a writer role.",
    params(
        ("id" = String, Path, format = "uuid", description = "Case object id (UUID)"),
        ("pid" = String, Path, format = "uuid", description = "Proposal id (UUID)")
    ),
    request_body = pnex_core::ontology::api::ProposalReject,
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseProposal),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`case-proposal-stale` (already retained)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn reject(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, pid)): Path<(String, String)>,
    Json(input): Json<ProposalReject>,
) -> Result<Response> {
    let (id, pid) = (parse_uuid(&id)?, parse_uuid(&pid)?);
    let pid_s = pid.to_string();
    let rejected = cases::reject(&ctx.db, org.org.id, &actor(&org), id, pid, &input)
        .await
        .and_then(|all| {
            all.into_iter()
                .find(|p| p.id == pid_s)
                .ok_or(crate::services::ontology::OntologyError::NotFound)
        });
    reply(rejected)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/board",
    tag = "cases",
    operation_id = "cases_board",
    summary = "Get the hypotheses board of a case",
    description = "The case's hypotheses with the claims supporting and contradicting them, plus the member claims needing attention (unsourced or contradicted). No score: the human concludes. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseBoard),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn board(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::board(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/snapshots",
    tag = "cases",
    operation_id = "cases_snapshots_create",
    summary = "Freeze the current state of a case",
    description = "Stores an immutable, SHA-256 fingerprinted snapshot of the case: its export, reviewed detections and generalised tracks (left out over a size cap). Logs the read of persons' tracks. Needs a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::CaseSnapshotInput,
    responses(
        (status = 201, body = pnex_core::ontology::api::CaseSnapshot),
        (status = 400, description = "Invalid `label` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn freeze(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseSnapshotInput>,
) -> Result<Response> {
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
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/snapshots",
    tag = "cases",
    operation_id = "cases_snapshots_list",
    summary = "List the frozen states of a case",
    description = "Every snapshot of the case (sequence, label, author, fingerprint, size), without content. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::CaseSnapshot>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn snapshots(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = cases::snapshots(&ctx.db, org.org.id, parse_uuid(&id)?).await;
    paged(&format!("/api/v1/cases/{id}/snapshots"), &[], &q, rows)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/snapshots/{seq}",
    tag = "cases",
    operation_id = "cases_snapshots_get",
    summary = "Get a frozen state of a case",
    description = "One snapshot with its content: the canonical JSON string that was fingerprinted, byte for byte (hash it to verify `sha256`). Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Case object id (UUID)"),
        ("seq" = i32, Path, description = "Snapshot sequence number")
    ),
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseSnapshotFull),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn snapshot(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, seq)): Path<(String, i32)>,
) -> Result<Response> {
    reply(cases::snapshot(&ctx.db, org.org.id, parse_uuid(&id)?, seq).await)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/reads",
    tag = "cases",
    operation_id = "cases_reads",
    summary = "List the logged reads of persons' tracks",
    description = "Who read the tracks of the case's persons, for what (`tracks`, `detect`, `snapshot`) and over which window, over the last year, newest first. Paginated (`limit`/`offset`). 503 `case-reads-unavailable` when the read log cannot be read. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::CaseRead>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`case-reads-unavailable`: the read log cannot be read", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn reads(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = q.page();
    let uuid = parse_uuid(&id)?;
    match cases::reads(&ctx.db, o2(&ctx).as_ref(), org.org.id, uuid, page).await {
        Ok(Some((count, rows))) => format::json(crate::controllers::pagination::envelope(
            &format!("/api/v1/cases/{id}/reads"),
            &[],
            page,
            count,
            rows,
        )),
        Ok(None) => Err(crate::controllers::coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            pnex_core::err_codes::CASE_READS_UNAVAILABLE,
            "The read log of the case cannot be read.",
            None,
        )),
        Err(e) => onto_error(e),
    }
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/relations",
    tag = "cases",
    operation_id = "cases_relations",
    summary = "List the links among the case members",
    description = "Every link ever opened between two members of the case, closed ones included (the edges of the case graph; structural case links excluded, at most 2000). Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::LinkView>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn relations(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = cases::relations(&ctx.db, org.org.id, parse_uuid(&id)?).await;
    paged(&format!("/api/v1/cases/{id}/relations"), &[], &q, rows)
}

/// Read only (it adds nothing to the case): any org member (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/graph/query",
    tag = "cases",
    operation_id = "cases_expand",
    summary = "Expand a node of the case graph",
    description = "Read-only query (POST for its body: the excluded nodes and picked facets; adds nothing to the case). Without `pick`: the neighbours of `object_id` counted by link type and object type (`facets`). With `pick`: those neighbours and their links, minus the `exclude`d nodes; `truncated` = more than returned. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::ExpandInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::ExpandResult),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn expand(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<ExpandInput>,
) -> Result<Response> {
    reply(cases::expand(&ctx.db, org.org.id, parse_uuid(&id)?, &input).await)
}

/// Read only: any org member, the org from the principal (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/layout",
    tag = "cases",
    operation_id = "cases_layout_get",
    summary = "Get the hand-made layout of the case graph",
    description = "Node positions `[x, y]` by object id, as the team arranged them; empty = automatic layout. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseLayout),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn layout(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::layout(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    put,
    path = "/api/v1/cases/{id}/layout",
    tag = "cases",
    operation_id = "cases_layout_save",
    summary = "Save the layout of the case graph",
    description = "Replaces the node positions of the case graph (empty map = back to the automatic layout). Needs a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::CaseLayout,
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseLayout),
        (status = 400, description = "Invalid `positions` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn save_layout(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CaseLayout>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    reply(cases::save_layout(&ctx.db, org.org.id, &actor(&org), id, &input).await)
}

/// Read only (an export writes nothing): any org member (R1).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/export",
    tag = "cases",
    operation_id = "cases_export",
    summary = "Export a case",
    description = "The whole case in one `pnex.case/1` JSON document: workspace, hypotheses board and pieces manifest (with fingerprints), as they are now. `persons` counts person members (personal data). Re-importable with `POST /api/v1/cases/import`. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    responses(
        (status = 200, body = pnex_core::ontology::api::CaseExport),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn export(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(cases::export(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

/// Read only: the page templates of the packs the org installed.
#[utoipa::path(
    get,
    path = "/api/v1/cases/templates",
    tag = "cases",
    operation_id = "cases_templates",
    summary = "List the page templates",
    description = "Page templates shipped by the packs the org installed, usable when adding a page to a case. Any org member.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::PageTemplate>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn page_templates(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = cases::templates(&ctx.db, org.org.id).await;
    paged("/api/v1/cases/templates", &[], &q, rows)
}

/// Write: role guard first (R2), the org from the principal (R1).
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/pages",
    tag = "cases",
    operation_id = "cases_pages_create",
    summary = "Add a page to a case",
    description = "Creates a page (collaborative document), blank or from a `template` of an installed pack, and puts it among the case's pieces. Needs a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::CasePageInput,
    responses(
        (status = 201, body = pnex_core::pages::DocSummary),
        (status = 400, description = "Invalid `title` or unknown `template` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn add_page(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CasePageInput>,
) -> Result<Response> {
    created(cases::add_page(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

/// Read only: the personal objects only this case holds (K16).
#[utoipa::path(
    get,
    path = "/api/v1/cases/{id}/purge",
    tag = "cases",
    operation_id = "cases_purge_candidates",
    summary = "List the purge candidates of a case",
    description = "Read-only. The personal objects (persons, identifiers) that only this case holds: no other case and no open link outside the case. These are what a retention purge would delete. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::ObjectRef>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn purge_candidates(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = cases::candidates(&ctx.db, org.org.id, parse_uuid(&id)?).await;
    paged(&format!("/api/v1/cases/{id}/purge"), &[], &q, rows)
}

/// Write: role guard first (R2), the org from the principal (R1); the
/// candidates are re-computed server side.
#[utoipa::path(
    post,
    path = "/api/v1/cases/{id}/purge",
    tag = "cases",
    operation_id = "cases_purge",
    summary = "Purge personal data of an archived case",
    description = "DESTRUCTIVE, irreversible: permanently deletes the given objects among the purge candidates of an archived case (case `status` must be `archived`), with their links, ranges, labels, containment and the snapshots that mention them. Ids that are not candidates (re-computed server side) are skipped. Returns how many were purged. Needs a writer role and the write role of each purged type.",
    params(("id" = String, Path, format = "uuid", description = "Case object id (UUID)")),
    request_body = pnex_core::ontology::api::CasePurgeInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::CasePurged),
        (status = 400, description = "The case is not archived (`status` → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn purge(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<CasePurgeInput>,
) -> Result<Response> {
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
