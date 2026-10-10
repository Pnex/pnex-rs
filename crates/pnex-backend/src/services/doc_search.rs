//! Document search P1 (docs/architecture/doc-search.md, decision #22):
//! indexing of `document` / `table` media versions into `text_chunks`, and
//! the lexical search shared by the HTTP controller and the assistant
//! tools (ai-assistant.md §9.3).
//!
//! Every read takes the org from the caller's principal (R1) and only sees
//! the current version of each asset. Chunk text is untrusted user content:
//! the assistant receives it as data (see `services::ai::doc_tools`).

use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use pnex_core::doc_extract::{self, ExtractError, Limits};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

use crate::models::_entities::media_versions;
use crate::services::media::MediaSettings;
use crate::workers::index_document::{IndexDocumentArgs, IndexDocumentWorker};

/// Rows inserted per statement.
const INSERT_BATCH: usize = 200;
/// Ceiling of `k` for every caller (HTTP and tools).
pub const MAX_HITS: u64 = 50;
/// Wall-clock ceiling of one extraction (a 100-page PDF takes seconds).
const EXTRACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub fn is_indexed_kind(kind: &str) -> bool {
    kind == doc_extract::KIND_DOCUMENT || kind == doc_extract::KIND_TABLE
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DbBackend::Postgres, sql, values)
}

/// Marks a version `pending` and queues its indexing. A queue failure
/// leaves the row `pending`: the manual reindex (F4) recovers it.
pub async fn enqueue(ctx: &AppContext, org_id: i64, version_id: Uuid) -> Result<(), DbErr> {
    ctx.db
        .execute_raw(stmt(
            "INSERT INTO media_text_index (media_version_id, org_id, status)
             VALUES ($1, $2, $3)
             ON CONFLICT (media_version_id) DO UPDATE
             SET status = EXCLUDED.status, error_code = NULL, updated_at = now()",
            vec![
                version_id.into(),
                org_id.into(),
                doc_extract::STATUS_PENDING.into(),
            ],
        ))
        .await?;
    let args = IndexDocumentArgs {
        media_version_id: version_id,
        org_id,
    };
    if let Err(e) = IndexDocumentWorker::perform_later(ctx, args).await {
        tracing::warn!(version = %version_id, error = %e, "document index job not queued");
    }
    Ok(())
}

async fn set_status(
    db: &DatabaseConnection,
    version_id: Uuid,
    status: &str,
    error_code: Option<&str>,
) -> Result<(), DbErr> {
    db.execute_raw(stmt(
        "UPDATE media_text_index SET status = $2, error_code = $3, updated_at = now()
         WHERE media_version_id = $1",
        vec![
            version_id.into(),
            status.into(),
            error_code.map(str::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

fn error_code(e: ExtractError) -> &'static str {
    match e {
        ExtractError::Unsupported => pnex_core::err_codes::MEDIA_INDEX_UNSUPPORTED,
        ExtractError::TooLarge => pnex_core::err_codes::MEDIA_INDEX_TOO_LARGE,
        ExtractError::Malformed | ExtractError::NeedsOcr => {
            pnex_core::err_codes::MEDIA_INDEX_MALFORMED
        }
    }
}

/// Worker body: bytes → text → chunks → `text_chunks`, replacing the
/// previous chunks of the version in one transaction.
pub async fn index_version(ctx: &AppContext, org_id: i64, version_id: Uuid) -> Result<(), DbErr> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};
    let Some(version) = media_versions::Entity::find_by_id(version_id)
        .filter(media_versions::Column::OrgId.eq(org_id))
        .one(&ctx.db)
        .await?
    else {
        return Ok(()); // deleted meanwhile
    };
    set_status(&ctx.db, version_id, doc_extract::STATUS_EXTRACTING, None).await?;
    let bytes = match MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|e| e.to_string())
    {
        Ok(store) => store
            .get(&version.storage_key)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(version = %version_id, error = %e, "document bytes unreadable");
            let code = pnex_core::err_codes::MEDIA_INDEX_UNREADABLE;
            return set_status(&ctx.db, version_id, doc_extract::STATUS_ERROR, Some(code)).await;
        }
    };
    let filename = version.filename.clone();
    // CPU-bound, and third-party parsers may panic on hostile input: a
    // panic surfaces as a JoinError and becomes `malformed`.
    let job = tokio::task::spawn_blocking(move || {
        let format = doc_extract::format_of(&filename, &bytes).ok_or(ExtractError::Unsupported)?;
        doc_extract::extract(format, &bytes, &Limits::default())
    });
    // shortcut: a timed-out parse keeps its blocking thread until it ends
    // (no cancellation of sync code); move extraction to a subprocess if
    // hostile PDFs ever pin threads in practice.
    let extracted = match tokio::time::timeout(EXTRACT_TIMEOUT, job).await {
        Ok(joined) => joined.unwrap_or(Err(ExtractError::Malformed)),
        Err(_) => Err(ExtractError::TooLarge),
    };
    let doc = match extracted {
        Ok(doc) => doc,
        Err(ExtractError::NeedsOcr) => {
            return set_status(&ctx.db, version_id, doc_extract::STATUS_NEEDS_OCR, None).await;
        }
        Err(e) => {
            let code = error_code(e);
            return set_status(&ctx.db, version_id, doc_extract::STATUS_ERROR, Some(code)).await;
        }
    };
    let chunks = doc_extract::chunk(&doc);

    let txn = ctx.db.begin().await?;
    txn.execute_raw(stmt(
        "DELETE FROM text_chunks WHERE media_version_id = $1",
        vec![version_id.into()],
    ))
    .await?;
    for batch in chunks.chunks(INSERT_BATCH) {
        let mut sql = String::from(
            "INSERT INTO text_chunks (org_id, media_version_id, ord, page, heading, content) VALUES ",
        );
        let mut values: Vec<sea_orm::Value> = vec![org_id.into(), version_id.into()];
        for (i, c) in batch.iter().enumerate() {
            let p = values.len();
            if i > 0 {
                sql.push(',');
            }
            sql.push_str(&format!(
                "($1, $2, ${}, ${}, ${}, ${})",
                p + 1,
                p + 2,
                p + 3,
                p + 4
            ));
            values.push((c.ord as i32).into());
            values.push(c.page.map(|p| p as i32).into());
            values.push(c.heading.clone().into());
            values.push(strip_nul(&c.content).into());
        }
        txn.execute_raw(stmt(&sql, values)).await?;
    }
    txn.execute_raw(stmt(
        "UPDATE media_text_index
         SET status = $2, error_code = NULL, page_count = $3, chunk_count = $4,
             indexed_at = now(), updated_at = now()
         WHERE media_version_id = $1",
        vec![
            version_id.into(),
            doc_extract::STATUS_INDEXED.into(),
            doc.page_count.map(|p| p as i32).into(),
            (chunks.len() as i32).into(),
        ],
    ))
    .await?;
    txn.commit().await
}

/// Postgres text cannot hold NUL bytes (lossy UTF-8 of odd files can).
fn strip_nul(s: &str) -> String {
    s.replace('\0', "")
}

// ─────────────────────────── Reads ───────────────────────────

#[derive(Debug, Serialize, FromQueryResult)]
pub struct IndexState {
    pub status: String,
    pub error_code: Option<String>,
    pub page_count: Option<i32>,
    pub chunk_count: i32,
}

/// Index state of the current version of an asset of the org.
pub async fn state_of(
    db: &DatabaseConnection,
    org_id: i64,
    asset_id: Uuid,
) -> Result<Option<IndexState>, DbErr> {
    IndexState::find_by_statement(stmt(
        "SELECT i.status, i.error_code, i.page_count, i.chunk_count
         FROM media_text_index i
         JOIN media_assets a ON a.current_version_id = i.media_version_id
         WHERE a.id = $1 AND a.org_id = $2 AND i.org_id = $2",
        vec![asset_id.into(), org_id.into()],
    ))
    .one(db)
    .await
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct Hit {
    pub chunk_id: Uuid,
    pub asset_id: Uuid,
    pub asset_name: String,
    pub kind: String,
    pub filename: String,
    pub page: Option<i32>,
    pub heading: Option<String>,
    /// Excerpt with matches between `⟦` and `⟧` (never found in prose, unlike French quotes).
    pub snippet: String,
    pub score: f64,
}

/// Lexical search over the current versions of the org: full text (codes
/// kept intact by `simple`, prose stemmed by `french`) or, for near-miss
/// codes, trigram word similarity.
pub async fn search(
    db: &DatabaseConnection,
    org_id: i64,
    query: &str,
    kind: Option<&str>,
    k: u64,
) -> Result<Vec<Hit>, DbErr> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    Hit::find_by_statement(stmt(
        "WITH q AS (
             SELECT websearch_to_tsquery('simple', $2) || websearch_to_tsquery('french', $2) AS tq
         )
         SELECT c.id AS chunk_id, a.id AS asset_id, a.name AS asset_name, a.kind,
                v.filename, c.page, c.heading,
                ts_headline('simple', c.content, q.tq,
                    'StartSel=⟦, StopSel=⟧, MaxWords=40, MinWords=15, MaxFragments=2') AS snippet,
                (ts_rank_cd(c.tsv, q.tq) + word_similarity($2, c.content))::float8 AS score
         FROM text_chunks c
         JOIN media_assets a ON a.current_version_id = c.media_version_id AND a.org_id = $1
         JOIN media_versions v ON v.id = c.media_version_id
         CROSS JOIN q
         WHERE c.org_id = $1
           AND (c.tsv @@ q.tq OR $2 <% c.content)
           AND ($3::text IS NULL OR a.kind = $3)
         ORDER BY score DESC, c.ord
         LIMIT $4",
        vec![
            org_id.into(),
            query.into(),
            kind.map(str::to_string).into(),
            (k.clamp(1, MAX_HITS) as i64).into(),
        ],
    ))
    .all(db)
    .await
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ChunkText {
    pub chunk_id: Uuid,
    pub asset_id: Uuid,
    pub asset_name: String,
    pub ord: i32,
    pub page: Option<i32>,
    pub heading: Option<String>,
    pub content: String,
}

const CHUNK_COLUMNS: &str = "c.id AS chunk_id, a.id AS asset_id, a.name AS asset_name,
    c.ord, c.page, c.heading, c.content";

/// A chunk and its `context` neighbours on each side, in document order.
pub async fn read_chunk(
    db: &DatabaseConnection,
    org_id: i64,
    chunk_id: Uuid,
    context: u32,
) -> Result<Vec<ChunkText>, DbErr> {
    ChunkText::find_by_statement(stmt(
        &format!(
            "SELECT {CHUNK_COLUMNS}
             FROM text_chunks t
             JOIN text_chunks c ON c.media_version_id = t.media_version_id
                  AND c.ord BETWEEN t.ord - $3 AND t.ord + $3
             JOIN media_assets a ON a.id = (SELECT asset_id FROM media_versions WHERE id = c.media_version_id)
             WHERE t.id = $1 AND t.org_id = $2 AND a.org_id = $2
             ORDER BY c.ord"
        ),
        vec![chunk_id.into(), org_id.into(), (context.min(3) as i32).into()],
    ))
    .all(db)
    .await
}

/// Every chunk of one page (PDF page, spreadsheet sheet) of the current
/// version of an asset. Overlapping chunks repeat ~15 % of their text.
pub async fn page_text(
    db: &DatabaseConnection,
    org_id: i64,
    asset_id: Uuid,
    page: i32,
) -> Result<Vec<ChunkText>, DbErr> {
    ChunkText::find_by_statement(stmt(
        &format!(
            "SELECT {CHUNK_COLUMNS}
             FROM text_chunks c
             JOIN media_assets a ON a.current_version_id = c.media_version_id
             WHERE a.id = $1 AND a.org_id = $2 AND c.org_id = $2 AND c.page = $3
             ORDER BY c.ord"
        ),
        vec![asset_id.into(), org_id.into(), page.into()],
    ))
    .all(db)
    .await
}
