//! Topic taxonomies (media-ingest.md D168) — model wrapper.

pub use super::_entities::taxonomy_versions::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
