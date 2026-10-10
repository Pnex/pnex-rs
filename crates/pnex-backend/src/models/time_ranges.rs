//! Time ranges (media-ingest.md D169) — model wrapper.

pub use super::_entities::time_ranges::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
