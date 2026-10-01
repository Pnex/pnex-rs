//! Camera & video (camera-video.md D76/D79) — model wrapper.

pub use super::_entities::device_cameras::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
