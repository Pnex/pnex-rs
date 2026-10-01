//! Vision model registry (camera-video.md D81) — model wrapper.

pub use super::_entities::ml_models::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
