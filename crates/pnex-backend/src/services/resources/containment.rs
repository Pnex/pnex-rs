//! Containment of the organisation layer (D42) — a **single-parent** tree
//! (guaranteed by `uniq_resource_containment_child`). Moving = 1 UPDATE
//! (re-parent). Cycle → [`ResourceError::CycleDetected`] (ancestor walk).
//! Detaching **never** deletes an entity.
//!
//! Existence+tenancy of both ends via the **registry** (no match);
//! parent×child validity declared per kind (`ContainmentRules`).

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::models::_entities::resource_containment as containment;
use crate::services::resources::registry;
use crate::services::resources::ResourceError;

/// Parent of a resource, `None` = root (never placed or **detached**
/// — convention: row with an empty parent `("", "")`).
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

/// Direct children of a parent (order = `sort_key` then kind/id).
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

/// Full path of a resource (leaf → root), for the breadcrumb.
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
            break; // defence in depth
        }
        let edge = (pk.clone(), pi.clone());
        chain.push(edge.clone());
        current = edge;
    }
    Ok(chain)
}

/// (Re)parents: `parent = None` detaches (the resource becomes a root — its
/// subtree follows, nothing is deleted). Replaces any existing parent
/// (single parent). Optional `sort_key` (order within the parent).
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

    // The child kind must be containable (all living kinds are, barring
    // future change — single path through the spec).
    if !reg.allows_detach(child_kind) {
        return Err(ResourceError::KindInvalid);
    }
    if let Some((pk, pi)) = parent {
        if reg.entry(pk).is_none() {
            return Err(ResourceError::KindInvalid);
        }
        // Existence+tenancy of both ends (registry).
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
        // Validity declared per kind (parent spec, wildcard = Some(&[])).
        if !reg.allows_containment(pk, child_kind) {
            return Err(ResourceError::TargetInvalid);
        }
        // Anti-cycle: the parent must not live in the child's subtree
        // (a folder cannot be its own descendant).
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

/// `true` if the ancestor of (root_kind, root_id) is (kind, id).
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

/// "Detached root" convention: the row exists with an empty parent
/// (`("", "")`) — `""` is never used as a real kind. Keeps rows stable
/// (sort_key preserved) and tells "never placed" (no row) apart from
/// "detached" (row with an empty parent).
pub const DETACHED_PARENT_KIND: &str = "";

/// Explicit detach (PUT parent=null) — the subtree stays under the
/// resource, nothing is deleted.
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

/// Tree view helper: name/emoji of a folder (for display).
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
