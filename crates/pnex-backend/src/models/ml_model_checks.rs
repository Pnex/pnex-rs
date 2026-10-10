//! Per-carrier audio model checks (media-ingest.md D167) — model wrapper.

pub use super::_entities::ml_model_checks::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
