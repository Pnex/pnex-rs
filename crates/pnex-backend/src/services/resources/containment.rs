//! Containment de la couche d'organisation (D42) — arbre à **parent
//! unique** (garantie par `uniq_resource_containment_child`). Déplacer = 1
//! UPDATE (re-parent). Cycle → [`ResourceError::CycleDetected`] (remontée
//! d'ancêtres). Détacher ne supprime **jamais** une entité.
//!
//! Existence+tenancy des deux bouts via le **registre** (pas de match) ;
//! validité parent×enfant déclarée par kind (`ContainmentRules`).

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::models::_entities::resource_containment as containment;
use crate::services::resources::registry;
use crate::services::resources::ResourceError;

/// Parent d'une ressource, `None` = racine (jamais placée ou **détachée**
/// — convention : ligne à parent vide `("", "")`).
pub async fn parent_of(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Option<(String, String)>, DbErr> {
    let row = containment::Entity::find()
        .filter(containment::Column::OrgId.eq(org_id))
        .filter(containment::Column::ChildKind.eq(kind))
        .filter(containment::Column::ChildId.eq(id))
        .one(db)
        .await?;
    Ok(row
        .filter(|r| !r.parent_kind.is_empty())
        .map(|r| (r.parent_kind, r.parent_id)))
}

/// Enfants directs d'un parent (ordre = `sort_key` puis kind/id).
pub async fn children_of(
    db: &DatabaseConnection,
    org_id: i64,
    parent_kind: &str,
    parent_id: &str,
) -> Result<Vec<(String, String, Option<String>)>, DbErr> {
    let rows = containment::Entity::find()
        .filter(containment::Column::OrgId.eq(org_id))
        .filter(containment::Column::ParentKind.eq(parent_kind))
        .filter(containment::Column::ParentId.eq(parent_id))
        .order_by_asc(containment::Column::SortKey)
        .order_by_asc(containment::Column::ChildKind)
        .order_by_asc(containment::Column::ChildId)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.child_kind, r.child_id, r.sort_key))
        .collect())
}

/// Chemin complet d'une ressource (feuille → racine), pour le breadcrumb.
pub async fn path_of(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Vec<(String, String)>, DbErr> {
    let mut chain = Vec::new();
    let mut current = (kind.to_string(), id.to_string());
    loop {
        let Some((pk, pi)) = parent_of(db, org_id, &current.0, &current.1).await? else {
            break;
        };
        if chain.contains(&(pk.clone(), pi.clone())) {
            break; // défense en profondeur
        }
        let edge = (pk.clone(), pi.clone());
        chain.push(edge.clone());
        current = edge;
    }
    Ok(chain)
}

/// (Re)parente : `parent = None` détache (la ressource devient racine — son
/// sous-arbre suit, rien n'est supprimé). Remplace l'éventuel parent existant
/// (parent unique). `sort_key` optionnel (ordre dans le parent).
pub async fn set_parent(
    db: &DatabaseConnection,
    org_id: i64,
    child_kind: &str,
    child_id: &str,
    parent: Option<(&str, &str)>,
    sort_key: Option<&str>,
) -> Result<(), ResourceError> {
    let reg = registry::for_org(db, org_id)
        .await
        .map_err(|_| ResourceError::Db)?;

    // Le kind enfant doit pouvoir être contenu (tous les kinds vivants oui,
    // sauf évolution future — chemin unique par la spec).
    if !reg.allows_detach(child_kind) {
        return Err(ResourceError::KindInvalid);
    }
    if let Some((pk, pi)) = parent {
        if reg.entry(pk).is_none() {
            return Err(ResourceError::KindInvalid);
        }
        // Existence+tenancy des deux bouts (registre).
        let child_ok = reg
            .entry(child_kind)
            .ok_or(ResourceError::KindInvalid)?
            .resolver
            .resolve(db, org_id, child_id)
            .await
            .map_err(|_| ResourceError::Db)?;
        if !child_ok {
            return Err(ResourceError::ResourceUnknown);
        }
        let parent_ok = reg
            .entry(pk)
            .ok_or(ResourceError::KindInvalid)?
            .resolver
            .resolve(db, org_id, pi)
            .await
            .map_err(|_| ResourceError::Db)?;
        if !parent_ok {
            return Err(ResourceError::ResourceUnknown);
        }
        // Validité déclarée par kind (spec du parent, wildcard = Some(&[])).
        if !reg.allows_containment(pk, child_kind) {
            return Err(ResourceError::TargetInvalid);
        }
        // Anti-cycle : le parent ne doit pas vivre dans le sous-arbre de
        // l'enfant (un folder ne peut pas être son propre descendant).
        if chain_has(db, org_id, pk, pi, child_kind, child_id)
            .await
            .map_err(|_| ResourceError::Db)?
        {
            return Err(ResourceError::CycleDetected);
        }
    }

    let existing = containment::Entity::find()
        .filter(containment::Column::OrgId.eq(org_id))
        .filter(containment::Column::ChildKind.eq(child_kind))
        .filter(containment::Column::ChildId.eq(child_id))
        .one(db)
        .await
        .map_err(|_| ResourceError::Db)?;
    match (existing, parent) {
        (None, None) => Ok(()),
        (Some(row), None) => {
            let mut am: containment::ActiveModel = row.into();
            am.parent_kind = Set(String::new());
            am.parent_id = Set(String::new());
            am.sort_key = Set(sort_key.map(str::to_string));
            am.updated_at = Set(chrono::Utc::now().into());
            am.update(db).await.map_err(|_| ResourceError::Db)?;
            Ok(())
        }
        (None, Some((pk, pi))) => {
            let am = containment::ActiveModel {
                org_id: Set(org_id),
                child_kind: Set(child_kind.to_string()),
                child_id: Set(child_id.to_string()),
                parent_kind: Set(pk.to_string()),
                parent_id: Set(pi.to_string()),
                sort_key: Set(sort_key.map(str::to_string)),
                ..Default::default()
            };
            am.insert(db).await.map_err(|_| ResourceError::Db)?;
            Ok(())
        }
        (Some(row), Some((pk, pi))) => {
            let mut am: containment::ActiveModel = row.into();
            am.parent_kind = Set(pk.to_string());
            am.parent_id = Set(pi.to_string());
            if let Some(sk) = sort_key {
                am.sort_key = Set(Some(sk.to_string()));
            }
            am.updated_at = Set(chrono::Utc::now().into());
            am.update(db).await.map_err(|_| ResourceError::Db)?;
            Ok(())
        }
    }
}

/// `true` si l'ancêtre de (root_kind, root_id) est (kind, id).
async fn chain_has(
    db: &DatabaseConnection,
    org_id: i64,
    start_kind: &str,
    start_id: &str,
    ancestor_kind: &str,
    ancestor_id: &str,
) -> Result<bool, DbErr> {
    let mut current = (start_kind.to_string(), start_id.to_string());
    loop {
        let Some((pk, pi)) = parent_of(db, org_id, &current.0, &current.1).await? else {
            return Ok(false);
        };
        if pk == ancestor_kind && pi == ancestor_id {
            return Ok(true);
        }
        current = (pk, pi);
    }
}

/// Convention « racine détachée » : la ligne existe avec parent vide
/// (`("", "")`) — on n'utilise jamais `""` comme kind réel. Garde les
/// lignes stables (sort_key conservé) et distingue « jamais placé »
/// (pas de ligne) de « détaché » (ligne à parent vide).
pub const DETACHED_PARENT_KIND: &str = "";

/// Détache explicitement (PUT parent=null) — le sous-arbre reste sous la
/// ressource, rien n'est supprimé.
pub async fn detach(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<bool, DbErr> {
    let row = containment::Entity::find()
        .filter(containment::Column::OrgId.eq(org_id))
        .filter(containment::Column::ChildKind.eq(kind))
        .filter(containment::Column::ChildId.eq(id))
        .one(db)
        .await?;
    let Some(row) = row else { return Ok(false) };
    let mut am: containment::ActiveModel = row.into();
    am.parent_kind = Set(DETACHED_PARENT_KIND.to_string());
    am.parent_id = Set(String::new());
    am.updated_at = Set(chrono::Utc::now().into());
    am.update(db).await?;
    Ok(true)
}

/// Helper de vue arbre : name/emoji d'un folder (pour l'affichage).
pub async fn folder_names(
    db: &DatabaseConnection,
    org_id: i64,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, (String, Option<String>)>, DbErr> {
    use crate::models::_entities::resource_folders;
    let mut out = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let rows = resource_folders::Entity::find()
        .filter(resource_folders::Column::OrgId.eq(org_id))
        .filter(resource_folders::Column::Id.is_in(ids.to_vec()))
        .all(db)
        .await?;
    for r in rows {
        out.insert(r.id, (r.name, r.emoji));
    }
    Ok(out)
}
