use super::*;
use crate::controllers::{coded_error, field_error};
use crate::services::{media_attributions, media_runs};
use pnex_core::media_review::{AttributionInput, AttributionPatch, SpeechAttribution};

// ─────────── Review of a media file (media-review.md D271–D280) ───────────

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct TranscriptQuery {
    /// Run to read (from `runs`); the default run otherwise.
    #[param(value_type = Option<String>, format = "uuid")]
    run: Option<Uuid>,
}

/// `GET /api/v1/media/{id}/transcript` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/transcript",
    tag = "media",
    operation_id = "media_transcript_get",
    summary = "Get the transcript of a media asset",
    description = "Transcription runs of the asset (PNeX transcription of a version, or subtitles given by the platform it comes from), and the segments of one run on the media clock: words with their stream-local speaker label and speaker turns. Without `run`, the latest PNeX run of the current version, else the latest run. `unavailable` is true when the text store cannot be read. Any member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Media asset UUID"),
        TranscriptQuery
    ),
    responses(
        (status = 200, body = pnex_core::media_review::MediaTranscript),
        (status = 404, description = "Unknown asset, asset of another organisation, or unknown run", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn transcript(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<TranscriptQuery>,
) -> Result<Response> {
    match media_runs::transcript(&ctx, org.org.id, id, q.run).await {
        Ok(t) => format::json(t),
        Err(media_runs::RunError::NotFound) => Err(Error::NotFound),
        Err(e) => {
            tracing::error!(asset = %id, error = %e, "media transcript failed");
            Err(Error::InternalServerError)
        }
    }
}

fn attribution_error(e: media_attributions::AttributionError) -> Result<Response> {
    use media_attributions::AttributionError as E;
    match e {
        E::NotFound => Err(Error::NotFound),
        E::NotPerson => Ok(field_error("object_id", "invalid")),
        E::Spans => Ok(field_error("spans", "invalid")),
        E::Conflict => Err(coded_error(
            axum::http::StatusCode::CONFLICT,
            pnex_core::err_codes::MEDIA_ATTRIBUTION_CONFLICT,
            "The attribution changed meanwhile: reload it",
            None,
        )),
        E::Db(e) => {
            tracing::error!(error = %e, "media attribution query failed");
            Err(Error::InternalServerError)
        }
    }
}

fn write_forbidden() -> Error {
    forbidden(
        "media-write-forbidden",
        "Media writes are restricted to owner and admin roles",
    )
}

/// `GET /api/v1/media/{id}/attributions` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/attributions",
    tag = "media",
    operation_id = "media_attributions_list",
    summary = "List the speech attributions of a media asset",
    description = "Spans of the media (ms on the media clock) attributed to persons of the ontology, with their origin (manual, voice, transcript, grid), confidence and status (proposed, confirmed, rejected), in media order. Paginated (`limit` default 10, max 100; `offset`). Any member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Media asset UUID"),
        VersionsPage
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<SpeechAttribution>),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn attributions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<VersionsPage>,
) -> Result<Response> {
    if find_asset(&ctx.db, &org, id).await?.is_none() {
        return Err(Error::NotFound);
    }
    let all = match media_attributions::list(&ctx.db, org.org.id, id).await {
        Ok(all) => all,
        Err(e) => return attribution_error(media_attributions::AttributionError::Db(e)),
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (from, to) = page.slice(all.len());
    format::json(pagination::envelope(
        &format!("/api/v1/media/{id}/attributions"),
        &[],
        page,
        all.len() as i64,
        all[from..to].to_vec(),
    ))
}

/// `POST /api/v1/media/{id}/attributions` — owner/admin.
#[utoipa::path(
    post,
    path = "/api/v1/media/{id}/attributions",
    tag = "media",
    operation_id = "media_attributions_create",
    summary = "Attribute spans of a media asset to a person",
    description = "Attributes one or several spans of the media (ms on the media clock, at most 500) to a person of the ontology, confirmed at once, without any voice recognition. Confirmed spans of the same person that overlap are merged. Returns every attribution of the asset. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    request_body = AttributionInput,
    responses(
        (status = 201, body = pnex_core::Paginated<SpeechAttribution>),
        (status = 400, description = "Not a person of the organisation (`object_id`), or invalid spans (`spans`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn create_attribution(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AttributionInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    match media_attributions::create(&ctx.db, org.org.id, org.auth.user.id, id, &input).await {
        Ok(all) => {
            let count = all.len() as i64;
            let body = pagination::envelope(
                &format!("/api/v1/media/{id}/attributions"),
                &[],
                pagination::PageParams::from(Some("100"), None),
                count,
                all,
            );
            Ok((axum::http::StatusCode::CREATED, axum::Json(body)).into_response())
        }
        Err(e) => attribution_error(e),
    }
}

/// `PATCH /api/v1/media/{id}/attributions/{aid}` — owner/admin.
#[utoipa::path(
    patch,
    path = "/api/v1/media/{id}/attributions/{aid}",
    tag = "media",
    operation_id = "media_attributions_update",
    summary = "Confirm, reject or move a speech attribution",
    description = "Changes the status (a proposal is confirmed or rejected; a rejected proposal is kept so it is not made again) or the span of an attribution, written at `expected_version`. Owner/admin only.",
    params(
        ("id" = String, Path, format = "uuid", description = "Media asset UUID"),
        ("aid" = String, Path, format = "uuid", description = "Attribution UUID")
    ),
    request_body = AttributionPatch,
    responses(
        (status = 200, body = SpeechAttribution),
        (status = 400, description = "Invalid span (`spans`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or attribution", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Changed since `expected_version` (`media-attribution-conflict`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn update_attribution(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, aid)): Path<(Uuid, Uuid)>,
    Json(input): Json<AttributionPatch>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    match media_attributions::patch(&ctx.db, org.org.id, id, aid, &input).await {
        Ok(a) => format::json(a),
        Err(e) => attribution_error(e),
    }
}

/// `DELETE /api/v1/media/{id}/attributions/{aid}` — owner/admin.
#[utoipa::path(
    delete,
    path = "/api/v1/media/{id}/attributions/{aid}",
    tag = "media",
    operation_id = "media_attributions_delete",
    summary = "Remove a speech attribution",
    description = "Removes an attribution of the media. Owner/admin only.",
    params(
        ("id" = String, Path, format = "uuid", description = "Media asset UUID"),
        ("aid" = String, Path, format = "uuid", description = "Attribution UUID")
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or attribution", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn delete_attribution(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, aid)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    match media_attributions::delete(&ctx.db, org.org.id, id, aid).await {
        Ok(()) => Ok(axum::http::StatusCode::NO_CONTENT.into_response()),
        Err(e) => attribution_error(e),
    }
}
