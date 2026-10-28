pub use super::_entities::regulator_configs::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
pub type RegulatorConfigs = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

// Projection materialization: no business logic here — the life
// cycle (full recompute, cast) lives in services/regulator.rs.
impl Model {}
impl ActiveModel {}
impl Entity {}
