pub use super::_entities::ai_conversations::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
pub type AiConversations = Entity;

// Timestamps are set explicitly by `services::ai::conversations` (one clock
// for `last_message_at`, `updated_at` and the busy lease).
impl ActiveModelBehavior for ActiveModel {}
