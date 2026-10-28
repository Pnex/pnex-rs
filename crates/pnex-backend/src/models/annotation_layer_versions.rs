pub use super::_entities::annotation_layer_versions::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

pub type AnnotationLayerVersions = Entity;

// Append-only versions: never updated (history is immutable,
// tour_versions pattern).
#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}
