use super::*;
use crate::services::doc_search;

// ─────────────── Document search (doc-search.md P1) ───────────────

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    q: String,
    kind: Option<String>,
    k: Option<u64>,
}

fn db_error(e: sea_orm::DbErr) -> Error {
    tracing::error!(error = %e, "document search query failed");
    Error::InternalServerError
}

/// `GET /api/v1/media/search?q=&kind=&k=` — lexical search over the
/// current versions of the org's documents and tables.
pub(super) async fn search(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<SearchQuery>,
) -> Result<Response> {
    let kind = q.kind.as_deref().filter(|k| doc_search::is_indexed_kind(k));
    let hits = doc_search::search(&ctx.db, org.org.id, &q.q, kind, q.k.unwrap_or(20))
        .await
        .map_err(db_error)?;
    format::json(serde_json::json!({ "hits": hits }))
}

#[derive(Deserialize)]
pub(super) struct ChunkQuery {
    context: Option<u32>,
}

/// `GET /api/v1/media/chunks/{chunk_id}?context=` — a chunk and its
/// neighbours (404 when absent or of another org).
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
    format::json(serde_json::json!({ "chunks": chunks }))
}

/// `GET /api/v1/media/{id}/index` — index state of the current version
/// (`null` when never indexed).
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
    format::json(serde_json::json!({ "index": state }))
}

/// `POST /api/v1/media/{id}/index` — (re)index the current version (F4).
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
