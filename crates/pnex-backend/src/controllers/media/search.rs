use super::*;
use crate::services::doc_search;

// ─────────────── Document search (doc-search.md P1) ───────────────

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct SearchQuery {
    q: String,
    kind: Option<String>,
    k: Option<u64>,
}

/// Ranked search hits.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub(super) struct MediaSearchHits {
    hits: Vec<doc_search::Hit>,
}

/// A chunk and its neighbours, in document order.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub(super) struct MediaChunks {
    chunks: Vec<doc_search::ChunkText>,
}

/// Index state of the current version (`null` when never indexed).
#[derive(serde::Serialize, utoipa::ToSchema)]
pub(super) struct MediaIndexState {
    index: Option<doc_search::IndexState>,
}

fn db_error(e: sea_orm::DbErr) -> Error {
    tracing::error!(error = %e, "document search query failed");
    Error::InternalServerError
}

/// `GET /api/v1/media/search?q=&kind=&k=` — lexical search over the
/// current versions of the org's documents and tables.
#[utoipa::path(
    get,
    path = "/api/v1/media/search",
    tag = "media",
    operation_id = "media_search",
    summary = "Search the text of the organisation's documents",
    description = "Lexical full-text search (with trigram fallback for near-miss codes) over the current versions of indexed documents and tables. Snippets mark matches between `⟦` and `⟧`.",
    params(SearchQuery),
    responses(
        (status = 200, body = MediaSearchHits)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn search(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<SearchQuery>,
) -> Result<Response> {
    let kind = q.kind.as_deref().filter(|k| doc_search::is_indexed_kind(k));
    let hits = doc_search::search(&ctx.db, org.org.id, &q.q, kind, q.k.unwrap_or(20))
        .await
        .map_err(db_error)?;
    format::json(MediaSearchHits { hits })
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ChunkQuery {
    context: Option<u32>,
}

/// `GET /api/v1/media/chunks/{chunk_id}?context=` — a chunk and its
/// neighbours (404 when absent or of another org).
#[utoipa::path(
    get,
    path = "/api/v1/media/chunks/{chunk_id}",
    tag = "media",
    operation_id = "media_chunks_get",
    summary = "Read an indexed text chunk with its neighbours",
    description = "Returns the chunk and `context` neighbours on each side, in document order.",
    params(("chunk_id" = String, Path, format = "uuid", description = "Chunk UUID (from a search hit)"), ChunkQuery),
    responses(
        (status = 200, body = MediaChunks),
        (status = 404, description = "Unknown chunk or chunk of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn chunk(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(chunk_id): Path<Uuid>,
    Query(q): Query<ChunkQuery>,
) -> Result<Response> {
    let chunks = doc_search::read_chunk(&ctx.db, org.org.id, chunk_id, q.context.unwrap_or(0))
        .await
        .map_err(db_error)?;
    if chunks.is_empty() {
        return Err(Error::NotFound);
    }
    format::json(MediaChunks { chunks })
}

/// `GET /api/v1/media/{id}/index` — index state of the current version
/// (`null` when never indexed).
#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/index",
    tag = "media",
    operation_id = "media_index_get",
    summary = "Get the search index state of a media asset",
    description = "Index state of the current version; `index` is `null` when never indexed.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    responses(
        (status = 200, body = MediaIndexState),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn index_state(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if find_asset(&ctx.db, &org, id).await?.is_none() {
        return Err(Error::NotFound);
    }
    let state = doc_search::state_of(&ctx.db, org.org.id, id)
        .await
        .map_err(db_error)?;
    format::json(MediaIndexState { index: state })
}

/// `POST /api/v1/media/{id}/index` — (re)index the current version (F4).
#[utoipa::path(
    post,
    path = "/api/v1/media/{id}/index",
    tag = "media",
    operation_id = "media_index_rebuild",
    summary = "Re-index a document for search",
    description = "Queues text extraction and indexing of the current version (documents and tables only). Asynchronous: poll `GET /api/v1/media/{id}/index`. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset UUID")),
    responses(
        (status = 202, description = "Indexing queued"),
        (status = 400, description = "Not a document or table, or no version (`media-index-not-a-document`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`media-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown asset or asset of another organisation", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn reindex(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
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
    let Some(version_id) = asset
        .current_version_id
        .filter(|_| doc_search::is_indexed_kind(&asset.kind))
    else {
        return Err(Error::CustomError(
            StatusCode::BAD_REQUEST,
            loco_rs::controller::ErrorDetail::new(
                pnex_core::err_codes::MEDIA_INDEX_NOT_A_DOCUMENT,
                "Only documents and tables are indexed for search".to_string(),
            ),
        ));
    };
    doc_search::enqueue(&ctx, org.org.id, version_id)
        .await
        .map_err(db_error)?;
    Ok(StatusCode::ACCEPTED.into_response())
}
