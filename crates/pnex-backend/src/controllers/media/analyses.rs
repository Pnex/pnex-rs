use super::*;
use crate::services::{doc_search, media_analyses};

// ─────────── File analyses (media-import-pipeline.md D213) ───────────

fn db_error(e: sea_orm::DbErr) -> Error {
    tracing::error!(error = %e, "media analyses query failed");
    Error::InternalServerError
}

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
        .map_err(db_error)?;
    let analyses = match asset.current_version_id {
        Some(v) => media_analyses::list(&ctx.db, org_id, v)
            .await
            .map_err(db_error)?,
        None => Vec::new(),
    };
    let index = doc_search::state_of(&ctx.db, org_id, asset.id)
        .await
        .map_err(db_error)?;
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
    description = "(Re)launches the given steps on the current version, asynchronously: `index` (text search) or `summarize` (summary written to a page by the organisation's LLM, which consumes LLM quota). Steps over the large-call threshold need `confirm_large`. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    request_body = pnex_core::media_analysis::StartAnalyses,
    responses(
        (status = 202, description = "Steps queued; returns the new state", body = MediaAnalysesState),
        (status = 400, description = "Step not offered for this kind, or large step not confirmed (`media-analysis-step-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or no current version", body = crate::openapi::ErrorDetail),
        (status = 409, description = "No LLM configured for the organisation (`media-analysis-no-llm`), or a step already running (`media-analysis-busy`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn start_analyses(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<pnex_core::media_analysis::StartAnalyses>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "media-write-forbidden",
            "Media writes are restricted to owner and admin roles",
        ));
    }
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
    )
    .await
    {
        let status = match e {
            media_analyses::AnalysisError::StepInvalid => StatusCode::BAD_REQUEST,
            media_analyses::AnalysisError::NoLlm | media_analyses::AnalysisError::Busy => {
                StatusCode::CONFLICT
            }
            media_analyses::AnalysisError::Db(e) => return Err(db_error(e)),
        };
        return Err(Error::CustomError(
            status,
            loco_rs::controller::ErrorDetail::new(e.code(), e.to_string()),
        ));
    }
    let body = state_json(&ctx, org.org.id, &asset).await?;
    Ok((StatusCode::ACCEPTED, format::json(body)).into_response())
}
