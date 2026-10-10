//! `check_asr_model` (media-ingest.md D167): the check of an audio model
//! run on a dedicated transcription worker (`PNEX_ASR_QUEUE_TAG`), where
//! the model will actually serve. A model valid on the GPU worker can be
//! too slow on the server: each carrier keeps its own row in
//! `ml_model_checks`.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::_entities::ml_models;
use crate::services::media_ingest::asr::{self, ASR_TASK};
use crate::services::media_ingest::models::save_carrier_check;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckAsrModelArgs {
    pub model_id: Uuid,
    pub org_id: i64,
}

pub struct CheckAsrModelWorker {
    ctx: AppContext,
}

/// Name of this carrier: `PNEX_ASR_CARRIER`, else `worker:<hostname>`.
pub fn carrier_name() -> String {
    if let Some(name) = std::env::var("PNEX_ASR_CARRIER")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return name;
    }
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into());
    format!("worker:{host}")
}

#[async_trait]
impl BackgroundWorker<CheckAsrModelArgs> for CheckAsrModelWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    fn tags() -> Vec<String> {
        crate::services::media_ingest::asr_queue_tags()
    }

    async fn perform(&self, args: CheckAsrModelArgs) -> Result<()> {
        let Some(model) = ml_models::Entity::find_by_id(args.model_id)
            .filter(ml_models::Column::OrgId.eq(args.org_id))
            .filter(ml_models::Column::Task.eq(ASR_TASK))
            .one(&self.ctx.db)
            .await?
        else {
            return Ok(()); // deleted meanwhile
        };
        let report = asr::check(&self.ctx, &model).await;
        save_carrier_check(&self.ctx.db, &model, &carrier_name(), &report).await?;
        Ok(())
    }
}
