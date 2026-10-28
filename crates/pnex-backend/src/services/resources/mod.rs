//! Cross-cutting organisation layer (D42) — the **engine** (labels /
//! containment / edges / folders) + the **kinds registry**.
//!
//! `services/pois.rs` pattern: controllers (and future AI tools) go through
//! here, never through the entities directly. The engine **never** does a
//! `match` on kinds: existence+tenancy go through the registry
//! ([`registry`]), containment and relation validity come from the type
//! definitions (`pnex_core::ontology`).
//!
//! Submodules:
//! - [`registry`] — registry derived from the system types + one resolver
//!   per type;
//! - [`labels`] — queryable classification (GIN `@>` on PG) + effective
//!   labels (ancestor inheritance, resolved **at read**);
//! - [`containment`] — single-parent tree (cycle → `CycleDetected`);
//!   detaching **never** deletes an entity;
//! - [`edges`] — many-to-many cross links + opaque `placement`;
//! - [`folders`] — CRUD of the `folder` kind;
//! - [`purge_for`] — **symmetric** cascade on an entity delete:
//!   labels + subtree (re-rooted, never deleted) + edges (both ends).

pub mod containment;
pub mod edges;
pub mod folders;
pub mod labels;
pub mod location;
pub mod registry;

// ─────────────────────────── errors ───────────────────────────

#[derive(Debug)]
pub enum ResourceError {
    /// kind outside the registry.
    KindInvalid,
    /// Resource not found in the org (→ masked 404 in the controller).
    ResourceUnknown,
    /// Invalid label set (too many, bad name or value): field token
    /// `invalid` on `labels`.
    LabelInvalid,
    /// Invalid folder name: field token (`required`, `max_length:255`).
    NameInvalid(String),
    /// Attachment would create a cycle (a resource cannot be its own
    /// ancestor).
    CycleDetected,
    /// Unknown relation for this source kind (spec).
    RelationInvalid,
    /// Target outside the source kind's spec (kind×relation×kind).
    TargetInvalid,
    /// Edge already exists for this pair (unique index).
    Conflict,
    /// Invalid `placement` (not an object, or too large).
    PlacementInvalid,
    Db,
}

// ─────────────────────────── symmetric purge ───────────────────────────

/// App-level purge of **every** organisation trace of a resource —
/// called (1 line) by each entity `delete` controller. **Symmetric**
/// cascade (the viz_links inconsistency is gone): edges purged from both
/// ends; the subtree is **re-rooted** (detached), never deleted — the
/// organisation layer never destroys a business entity.
pub async fn purge_for<C: sea_orm::ConnectionTrait>(
    db: &C,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    use crate::models::_entities::{
        map_pins, resource_containment, resource_edges, resource_labels,
    };

    // 1. Doc de labels.
    resource_labels::Entity::delete_many()
        .filter(resource_labels::Column::OrgId.eq(org_id))
        .filter(resource_labels::Column::ResourceKind.eq(kind))
        .filter(resource_labels::Column::ResourceId.eq(id))
        .exec(db)
        .await?;

    // 2. Containment: the resource's row + those of its whole subtree
    //    (org-scoped BFS, bounded sets) → descendants re-rooted.
    let mut frontier = vec![(kind.to_string(), id.to_string())];
    while let Some((k, i)) = frontier.pop() {
        let children = resource_containment::Entity::find()
            .filter(resource_containment::Column::OrgId.eq(org_id))
            .filter(resource_containment::Column::ParentKind.eq(&k))
            .filter(resource_containment::Column::ParentId.eq(&i))
            .all(db)
            .await?;
        for c in children {
            frontier.push((c.child_kind, c.child_id));
        }
        resource_containment::Entity::delete_many()
            .filter(resource_containment::Column::OrgId.eq(org_id))
            .filter(resource_containment::Column::ParentKind.eq(&k))
            .filter(resource_containment::Column::ParentId.eq(&i))
            .exec(db)
            .await?;
    }
    resource_containment::Entity::delete_many()
        .filter(resource_containment::Column::OrgId.eq(org_id))
        .filter(resource_containment::Column::ChildKind.eq(kind))
        .filter(resource_containment::Column::ChildId.eq(id))
        .exec(db)
        .await?;

    // 3. Edges, both ends (symmetric, D42): closed, never deleted — the
    //    link history outlives the entity like its identity does (D179).
    edges::close_edges(
        db,
        sea_orm::Condition::all()
            .add(resource_edges::Column::OrgId.eq(org_id))
            .add(
                sea_orm::Condition::any()
                    .add(
                        sea_orm::Condition::all()
                            .add(resource_edges::Column::SourceKind.eq(kind))
                            .add(resource_edges::Column::SourceId.eq(id)),
                    )
                    .add(
                        sea_orm::Condition::all()
                            .add(resource_edges::Column::TargetKind.eq(kind))
                            .add(resource_edges::Column::TargetId.eq(id)),
                    ),
            ),
    )
    .await?;

    // 4. A POI whose pinned preview was this resource drops the pin, otherwise
    //    the drawer keeps opening a viewer on a deleted object (not_found).
    map_pins::Entity::update_many()
        .col_expr(
            map_pins::Column::PreviewKind,
            sea_orm::sea_query::Expr::value(Option::<String>::None),
        )
        .col_expr(
            map_pins::Column::PreviewId,
            sea_orm::sea_query::Expr::value(Option::<String>::None),
        )
        .filter(map_pins::Column::OrgId.eq(org_id))
        .filter(map_pins::Column::PreviewKind.eq(kind))
        .filter(map_pins::Column::PreviewId.eq(id))
        .exec(db)
        .await?;

    // 5. The `folder` kind is itself an entity — its row goes with it.
    if kind == pnex_core::resources::KIND_FOLDER {
        if let Ok(numeric) = id.parse::<i64>() {
            crate::models::_entities::resource_folders::Entity::delete_by_id(numeric)
                .exec(db)
                .await?;
        }
    }
    Ok(())
}
