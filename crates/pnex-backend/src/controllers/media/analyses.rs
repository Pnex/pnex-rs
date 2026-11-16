use super::*;
use crate::services::{doc_search, media_analyses};

// ─────────── File analyses (media-import-pipeline.md D213) ───────────

/// Analysis offers, step states and index state of an asset's current version.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub(super) struct MediaAnalysesState {
    offers: Vec<pnex_core::media_analysis::StepOffer>,
    analyses: Vec<pnex_core::media_analysis::MediaAnalysis>,
    index: Option<doc_search::IndexState>,
}

/// Offers, step states and index state of the current version.
async fn state_json(
    ctx: &AppContext,
    org_id: i64,
    asset: &media_assets::Model,
) -> Result<MediaAnalysesState> {
    let offers = media_analyses::offers(ctx, org_id, &asset.kind, asset.current_version_id)
        .await
        .map_err(crate::controllers::db_error)?;
    let analyses = match asset.current_version_id {
        Some(v) => media_analyses::list(&ctx.db, org_id, v)
            .await
            .map_err(crate::controllers::db_error)?,
        None => Vec::new(),
    };
    let index = doc_search::state_of(&ctx.db, org_id, asset.id)
        .await
        .map_err(crate::controllers::db_error)?;
    Ok(MediaAnalysesState {
        offers,
        analyses,
        index,
    })
}

/// `GET /api/v1/media/{id}/analyses` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/analyses",
    tag = "media",
    operation_id = "media_analyses_get",
    summary = "Get the analyses of a media asset",
    description = "Analysis steps offered for the current version (with a blocking error code when unavailable), the state of each launched step, and the search index state.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    responses(
        (status = 200, body = MediaAnalysesState),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn analyses(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    format::json(state_json(&ctx, org.org.id, &asset).await?)
}

/// `POST /api/v1/media/{id}/analyses` — owner/admin: (re)launch steps on
/// the current version; 202 with the new state.
#[utoipa::path(
    post,
    path = "/api/v1/media/{id}/analyses",
    tag = "media",
    operation_id = "media_analyses_start",
    summary = "Start analyses on a media asset",
    description = "(Re)launches the given steps on the current version, asynchronously: `index` (text search) or `summarize` (summary written to a page by the organisation's LLM, which consumes LLM quota). Steps over the large-call threshold need `confirm_large`. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    request_body = pnex_core::media_analysis::StartAnalyses,
    responses(
        (status = 202, description = "Steps queued; returns the new state", body = MediaAnalysesState),
        (status = 400, description = "Step not offered for this kind, or large step not confirmed (`media-analysis-step-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or no current version", body = crate::openapi::ErrorDetail),
        (status = 409, description = "No LLM configured for the organisation (`media-analysis-no-llm`), no valid transcription profile (`media-analysis-no-asr`), no ready voice profile (`media-analysis-no-voice-profile`), no valid detection model (`media-analysis-no-detector`), or a step already running, or another transcription of the organisation (`media-analysis-busy`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn start_analyses(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<pnex_core::media_analysis::StartAnalyses>,
) -> Result<Response> {
    let Some(asset) = find_asset(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version_id) = asset.current_version_id else {
        return Err(Error::NotFound);
    };
    if let Err(e) = media_analyses::start(
        &ctx,
        org.org.id,
        org.auth.user.id,
        &asset.kind,
        version_id,
        &input.steps,
        &input.locale,
        input.confirm_large,
        input.voice_scope,
        &input.focus,
    )
    .await
    {
        let status = match e {
            media_analyses::AnalysisError::StepInvalid => StatusCode::BAD_REQUEST,
            media_analyses::AnalysisError::NoLlm
            | media_analyses::AnalysisError::NoAsr
            | media_analyses::AnalysisError::NoVoiceProfile
            | media_analyses::AnalysisError::NoDetector
            | media_analyses::AnalysisError::Busy => StatusCode::CONFLICT,
            media_analyses::AnalysisError::Db(e) => return Err(crate::controllers::db_error(e)),
        };
        return Err(Error::CustomError(
            status,
            loco_rs::controller::ErrorDetail::new(e.code(), e.to_string()),
        ));
    }
    let body = state_json(&ctx, org.org.id, &asset).await?;
    Ok((StatusCode::ACCEPTED, format::json(body)).into_response())
}

/// `POST /api/v1/media/analyses` — the same steps on many media at once
/// (voice-id.md D983, D985): each media goes through the very start of
/// `POST /media/{id}/analyses`, with its checks; one refusal never stops
/// the others.
#[utoipa::path(
    post,
    path = "/api/v1/media/analyses",
    tag = "media",
    operation_id = "media_analyses_start_batch",
    summary = "Start analyses on many media assets",
    description = "(Re)launches the given steps on the current version of each media asset listed (at most 200), as `POST /media/{id}/analyses` does for one: for example `identify` on a selection to regenerate the voice recognition with the profiles of the moment. Returns one result per asset, in order: started, or the machine code of the refusal (`media-not-found`, `media-analysis-busy`, `media-analysis-no-voice-profile`…). Owner, admin or member.",
    request_body = pnex_core::media_analysis::StartAnalysesBatch,
    responses(
        (status = 200, description = "One result per asset", body = pnex_core::media_analysis::BatchStartResult),
        (status = 400, description = "Empty list, more than 200 assets or an id that is not a UUID (`field-invalid` on `asset_ids`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn start_analyses_batch(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<pnex_core::media_analysis::StartAnalysesBatch>,
) -> Result<Response> {
    use pnex_core::media_analysis::{BatchStartItem, BatchStartResult, BATCH_MAX};
    let ids: Option<Vec<Uuid>> = input
        .asset_ids
        .iter()
        .map(|s| Uuid::parse_str(s.trim()).ok())
        .collect();
    let Some(ids) = ids.filter(|v| !v.is_empty() && v.len() <= BATCH_MAX) else {
        return Ok(crate::controllers::field_error(
            "asset_ids",
            pnex_core::err_codes::FIELD_INVALID,
        ));
    };
    let s = &input.start;
    let mut results = Vec::with_capacity(ids.len());
    for id in ids {
        let asset = find_asset(&ctx.db, &org, id).await?;
        let error = match asset
            .as_ref()
            .and_then(|a| a.current_version_id.map(|v| (a, v)))
        {
            None => Some(pnex_core::err_codes::MEDIA_NOT_FOUND.to_string()),
            Some((asset, version_id)) => match media_analyses::start(
                &ctx,
                org.org.id,
                org.auth.user.id,
                &asset.kind,
                version_id,
                &s.steps,
                &s.locale,
                s.confirm_large,
                s.voice_scope,
                &s.focus,
            )
            .await
            {
                Ok(_) => None,
                Err(media_analyses::AnalysisError::Db(e)) => {
                    return Err(crate::controllers::db_error(e))
                }
                Err(e) => Some(e.code().to_string()),
            },
        };
        results.push(BatchStartItem {
            asset_id: id.to_string(),
            started: error.is_none(),
            error,
        });
    }
    format::json(BatchStartResult { results })
}
