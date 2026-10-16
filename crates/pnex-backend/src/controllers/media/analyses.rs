use super::*;
use crate::services::{doc_search, media_analyses};

// ─────────── File analyses (media-import-pipeline.md D213) ───────────

fn db_error(e: sea_orm::DbErr) -> Error {
    tracing::error!(error = %e, "media analyses query failed");
    Error::InternalServerError
}

/// Offers, step states and index state of the current version.
async fn state_json(
    ctx: &AppContext,
    org_id: i64,
    asset: &media_assets::Model,
) -> Result<serde_json::Value> {
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
    Ok(serde_json::json!({ "offers": offers, "analyses": analyses, "index": index }))
}

/// `GET /api/v1/media/{id}/analyses` — every member.
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
