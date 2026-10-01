//! Couche d'organisation transverse (D42) — le **moteur** (labels /
//! containment / edges / folders) + le **registre des kinds**.
//!
//! École `services/pois.rs` : contrôleurs (et futurs outils IA) passent par
//! ici, jamais par les entités directement. Le moteur ne fait **jamais** de
//! `match` sur les kinds : existence+tenancy passent par le registre
//! ([`registry`]), la validité relation×kinds est déclarée par kind
//! (`pnex_core::resources::KindSpec`).
//!
//! Sous-modules :
//! - [`registry`] — kinds vivants + résolveurs (1 entrée par concept ;
//!   un concept futur = 1 fonction `*_entry()` + 1 ligne dans `build()`) ;
//! - [`labels`] — classification requêtable (GIN `@>` sur PG) + labels
//!   effectifs (héritage ancêtres, résolu **au read**) ;
//! - [`containment`] — arbre à parent unique (cycle → `CycleDetected`) ;
//!   détacher ne supprime **jamais** une entité ;
//! - [`edges`] — liens croisés many-to-many + `placement` opaque ;
//! - [`folders`] — CRUD du kind `folder` ;
//! - [`purge_for`] — cascade **symétrique** au delete d'une entité :
//!   labels + sous-arbre (re-rooté, jamais supprimé) + arêtes (2 bouts).

pub mod containment;
pub mod edges;
pub mod folders;
pub mod labels;
pub mod registry;

use sea_orm::DatabaseConnection;

// ─────────────────────────── erreurs ───────────────────────────

#[derive(Debug)]
pub enum ResourceError {
    /// kind hors registre.
    KindInvalid,
    /// Ressource introuvable dans l'org (→ 404 masqué côté contrôleur).
    ResourceUnknown,
    /// Labels invalides (message prêt à afficher).
    LabelInvalid(String),
    /// Attachement créerait un cycle (une ressource ne peut pas être son
    /// propre ancêtre).
    CycleDetected,
    /// Relation inconnue pour ce kind source (spec).
    RelationInvalid,
    /// Cible hors spec du kind source (kind×relation×kind).
    TargetInvalid,
    /// Arête déjà existante pour cette paire (index unique).
    Conflict,
    /// `placement` invalide (pas un objet ou trop volumineux).
    PlacementInvalid(&'static str),
    Db,
}

// ─────────────────────────── purge symétrique ───────────────────────────

/// Purge app-level de **toute** trace d'organisation d'une ressource —
/// appelée (1 ligne) par chaque contrôleur `delete` d'entité. Cascade
/// **symétrique** (l'incohérence viz_links disparaît) : arêtes purgées des
/// deux bouts ; le sous-arbre est **re-rooté** (détaché), jamais supprimé —
/// la couche d'organisation ne détruit jamais une entité métier.
pub async fn purge_for(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    use crate::models::_entities::{resource_containment, resource_edges, resource_labels};

    // 1. Doc de labels.
    resource_labels::Entity::delete_many()
        .filter(resource_labels::Column::OrgId.eq(org_id))
        .filter(resource_labels::Column::ResourceKind.eq(kind))
        .filter(resource_labels::Column::ResourceId.eq(id))
        .exec(db)
        .await?;

    // 2. Containment : la ligne de la ressource + celles de tout son
    //    sous-arbre (BFS org-scopé, ensembles bornés) → descendants re-rootés.
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

    // 3. Arêtes — les deux bouts (cascade symétrique, D42).
    resource_edges::Entity::delete_many()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::SourceKind.eq(kind))
        .filter(resource_edges::Column::SourceId.eq(id))
        .exec(db)
        .await?;
    resource_edges::Entity::delete_many()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::TargetKind.eq(kind))
        .filter(resource_edges::Column::TargetId.eq(id))
        .exec(db)
        .await?;

    // 4. Le kind `folder` est lui-même une entité — sa ligne part avec lui.
    if kind == pnex_core::resources::KIND_FOLDER {
        if let Ok(numeric) = id.parse::<i64>() {
            crate::models::_entities::resource_folders::Entity::delete_by_id(numeric)
                .exec(db)
                .await?;
        }
    }
    Ok(())
}
