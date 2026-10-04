pub use super::_entities::ai_messages::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
pub type AiMessages = Entity;

impl ActiveModelBehavior for ActiveModel {}
