//! One file analysis step (media-import-pipeline.md D213). The work lives
//! in [`crate::services::media_analyses::run`]; a failure is recorded on
//! the `media_analyses` row (state + machine code), never replayed.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAnalysisArgs {
    pub analysis_id: Uuid,
}

pub struct MediaAnalysisWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<MediaAnalysisArgs> for MediaAnalysisWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: MediaAnalysisArgs) -> Result<()> {
        crate::services::media_analyses::run(&self.ctx, args.analysis_id)
            .await
            .map_err(|e| Error::string(&e.to_string()))
    }
}
