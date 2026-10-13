//! Vision Lab training runs (media-vision-studio.md D204) — model wrapper.

pub use super::_entities::lab_training_runs::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
