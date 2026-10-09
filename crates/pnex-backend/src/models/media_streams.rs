//! Media ingest (media-ingest.md P2.13) — model wrapper.

pub use super::_entities::media_streams::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
