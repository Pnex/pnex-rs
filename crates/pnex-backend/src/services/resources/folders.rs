//! Kind `folder` (D42) — conteneur d'organisation pur (nom + emoji).
//! L'arbre lui-même vit dans `resource_containment` ; ce module ne gère que
//! l'entité (CRUD) et son purge hook (le delete d'un folder purge sa trace
//! d'organisation — le sous-arbre est re-rooté, jamais supprimé).

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::models::_entities::resource_folders;
use crate::services::resources::ResourceError;

/// Nom de folder valide (trim, 1..=255 chars).
pub fn validate_name(name: &str) -> Result<String, ResourceError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ResourceError::LabelInvalid(
            "nom de dossier invalide".into(),
        ));
    }
    Ok(name.to_string())
}

pub async fn list_folders(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<resource_folders::Model>, DbErr> {
    resource_folders::Entity::find()
        .filter(resource_folders::Column::OrgId.eq(org_id))
        .order_by_asc(resource_folders::Column::Name)
        .all(db)
        .await
}

pub async fn find_folder(
    db: &DatabaseConnection,
    org_id: i64,
    id: i64,
) -> Result<Option<resource_folders::Model>, DbErr> {
    resource_folders::Entity::find_by_id(id)
        .filter(resource_folders::Column::OrgId.eq(org_id))
        .one(db)
        .await
}

pub async fn create_folder(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    emoji: Option<&str>,
    created_by: Option<i64>,
) -> Result<resource_folders::Model, ResourceError> {
    let name = validate_name(name)?;
    let emoji = emoji
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    let am = resource_folders::ActiveModel {
        org_id: Set(org_id),
        name: Set(name),
        emoji: Set(emoji),
        created_by: Set(created_by),
        ..Default::default()
    };
    am.insert(db).await.map_err(|_| ResourceError::Db)
}

/// PATCH partiel : `Option<Option<T>>` (école pois).
pub struct FolderPatch<'a> {
    pub name: Option<&'a str>,
    pub emoji: Option<Option<&'a str>>,
}

pub async fn update_folder(
    db: &DatabaseConnection,
    org_id: i64,
    id: i64,
    p: FolderPatch<'_>,
) -> Result<Option<resource_folders::Model>, ResourceError> {
    let Some(row) = find_folder(db, org_id, id)
        .await
        .map_err(|_| ResourceError::Db)?
    else {
        return Ok(None);
    };
    let mut am: resource_folders::ActiveModel = row.into();
    if let Some(name) = p.name {
        am.name = Set(validate_name(name)?);
    }
    if let Some(emoji) = p.emoji {
        am.emoji = Set(emoji
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_string));
    }
    am.updated_at = Set(chrono::Utc::now().into());
    Ok(Some(am.update(db).await.map_err(|_| ResourceError::Db)?))
}

/// Delete = purge d'organisation (sous-arbre re-rooté) + ligne entité.
pub async fn delete_folder(db: &DatabaseConnection, org_id: i64, id: i64) -> Result<bool, DbErr> {
    let Some(row) = find_folder(db, org_id, id).await? else {
        return Ok(false);
    };
    crate::services::resources::purge_for(
        db,
        org_id,
        pnex_core::resources::KIND_FOLDER,
        &row.id.to_string(),
    )
    .await?;
    Ok(true)
}
