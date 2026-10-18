use super::*;
use crate::services::media_runs;

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
