//! Document indexing (doc-search.md P1): extraction, chunking and lexical
//! index of one `document` / `table` media version. The work lives in
//! [`crate::services::doc_search::index_version`]; a failure is recorded on
//! `media_text_index` (status + machine code), never replayed.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDocumentArgs {
    pub media_version_id: Uuid,
    pub org_id: i64,
}

pub struct IndexDocumentWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<IndexDocumentArgs> for IndexDocumentWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: IndexDocumentArgs) -> Result<()> {
        crate::services::doc_search::index_version(&self.ctx, args.org_id, args.media_version_id)
            .await
            .map_err(|e| Error::string(&e.to_string()))
    }
}
