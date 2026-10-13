//! Vision Lab (media-vision-studio.md D198–D200) — model wrapper.

pub use super::_entities::lab_dataset_versions::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
