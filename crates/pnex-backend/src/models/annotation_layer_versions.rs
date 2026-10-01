pub use super::_entities::annotation_layer_versions::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

pub type AnnotationLayerVersions = Entity;

// Versions append-only : jamais mises à jour (l'historique est immuable,
// école tour_versions).
#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}
