//! Registre des kinds vivants (D42) — **le** point d'extension de la couche.
//!
//! Rules are derived from the type definitions in `pnex_core::ontology`
//! (D176, lot L0); this module only adds a resolver per type. No `match` on
//! kinds in the engine.
//!
//! Découplage device (D17) : le résolveur device **lit** `device_registries`
//! (existence org), n'y écrit jamais — taguer/organiser n'a aucun effet sur
//! le firmware ni sur le contrat `pnex_api_contract::CONTRACT`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::models::_entities::{
    dashboards, device_registries, flows, map_pins, media_assets, objects, resource_folders, tours,
};

use pnex_core::ontology::{LinkTypeDef, ObjectTypeDef};
use pnex_core::resources::{
    KIND_DASHBOARD, KIND_DEVICE, KIND_FLOW, KIND_FOLDER, KIND_MAP_PIN, KIND_MEDIA_ASSET, KIND_TOUR,
};

// ─────────────────────────── résolveurs ───────────────────────────

/// Existence+tenancy d'une ressource (une table par kind — école viz_links).
#[async_trait]
pub trait KindResolver: Send + Sync {
    async fn resolve(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<bool, sea_orm::DbErr>;

    /// Nom d'affichage (labels d'arbre du drawer POI) — `None` = inconnu.
    /// Redondant avec [`Self::resolve`] (même ligne lue) : accepté, les
    /// listes d'arbre sont courtes.
    async fn display_name(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<Option<String>, sea_orm::DbErr> {
        let _ = (db, org_id, id);
        Ok(None)
    }
}

macro_rules! uuid_resolver {
    ($name:ident, $entity:ident, $title_col:ident) => {
        struct $name;
        #[async_trait]
        impl KindResolver for $name {
            async fn resolve(
                &self,
                db: &DatabaseConnection,
                org_id: i64,
                id: &str,
            ) -> Result<bool, sea_orm::DbErr> {
                let Ok(id) = Uuid::parse_str(id) else {
                    return Ok(false);
                };
                $entity::Entity::find_by_id(id)
                    .filter($entity::Column::OrgId.eq(org_id))
                    .one(db)
                    .await
                    .map(|row| row.is_some())
            }

            async fn display_name(
                &self,
                db: &DatabaseConnection,
                org_id: i64,
                id: &str,
            ) -> Result<Option<String>, sea_orm::DbErr> {
                let Ok(id) = Uuid::parse_str(id) else {
                    return Ok(None);
                };
                Ok($entity::Entity::find_by_id(id)
                    .filter($entity::Column::OrgId.eq(org_id))
                    .one(db)
                    .await?
                    .map(|row| row.$title_col))
            }
        }
    };
}

uuid_resolver!(MediaAssetKind, media_assets, name);
uuid_resolver!(DashboardKind, dashboards, name);
uuid_resolver!(TourKind, tours, name);
uuid_resolver!(MapPinKind, map_pins, label);

struct DeviceKind;
#[async_trait]
impl KindResolver for DeviceKind {
    async fn resolve(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<bool, sea_orm::DbErr> {
        device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org_id))
            .filter(device_registries::Column::Id.eq(id.parse::<i64>().unwrap_or(0)))
            .one(db)
            .await
            .map(|row| row.is_some())
    }

    async fn display_name(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<Option<String>, sea_orm::DbErr> {
        Ok(device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org_id))
            .filter(device_registries::Column::Id.eq(id.parse::<i64>().unwrap_or(0)))
            .one(db)
            .await?
            .map(|row| row.device_id))
    }
}

struct FlowKind;
#[async_trait]
impl KindResolver for FlowKind {
    async fn resolve(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<bool, sea_orm::DbErr> {
        flows::Entity::find()
            .filter(flows::Column::OrgId.eq(org_id))
            .filter(flows::Column::Id.eq(id.parse::<i64>().unwrap_or(0)))
            .one(db)
            .await
            .map(|row| row.is_some())
    }

    async fn display_name(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<Option<String>, sea_orm::DbErr> {
        Ok(flows::Entity::find()
            .filter(flows::Column::OrgId.eq(org_id))
            .filter(flows::Column::Id.eq(id.parse::<i64>().unwrap_or(0)))
            .one(db)
            .await?
            .map(|row| row.name))
    }
}

/// Resolver of org-type objects: identity rows of `objects` (D177).
struct ObjectKind(String);
#[async_trait]
impl KindResolver for ObjectKind {
    async fn resolve(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<bool, sea_orm::DbErr> {
        Ok(self.display_name(db, org_id, id).await?.is_some())
    }

    async fn display_name(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<Option<String>, sea_orm::DbErr> {
        Ok(objects::Entity::find()
            .filter(objects::Column::OrgId.eq(org_id))
            .filter(objects::Column::TypeKey.eq(self.0.as_str()))
            .filter(objects::Column::NativeId.eq(id))
            .filter(objects::Column::ValidTo.is_null())
            .one(db)
            .await?
            .map(|o| o.title))
    }
}

struct FolderKind;
#[async_trait]
impl KindResolver for FolderKind {
    async fn resolve(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<bool, sea_orm::DbErr> {
        resource_folders::Entity::find()
            .filter(resource_folders::Column::OrgId.eq(org_id))
            .filter(resource_folders::Column::Id.eq(id.parse::<i64>().unwrap_or(0)))
            .one(db)
            .await
            .map(|row| row.is_some())
    }

    async fn display_name(
        &self,
        db: &DatabaseConnection,
        org_id: i64,
        id: &str,
    ) -> Result<Option<String>, sea_orm::DbErr> {
        Ok(resource_folders::Entity::find()
            .filter(resource_folders::Column::OrgId.eq(org_id))
            .filter(resource_folders::Column::Id.eq(id.parse::<i64>().unwrap_or(0)))
            .one(db)
            .await?
            .map(|row| row.name))
    }
}

// ─────────────────────────── registre ───────────────────────────

pub struct KindEntry {
    /// Type definition, data from `pnex_core::ontology` (D176).
    pub def: ObjectTypeDef,
    pub resolver: Arc<dyn KindResolver>,
}

pub struct Registry {
    kinds: HashMap<String, KindEntry>,
    links: Vec<LinkTypeDef>,
}

impl Registry {
    /// Derives the registry from type definitions; each type needs a
    /// resolver (system types are native tables behind an adapter, D177).
    fn derive(
        types: Vec<ObjectTypeDef>,
        links: Vec<LinkTypeDef>,
        mut resolvers: HashMap<&str, Arc<dyn KindResolver>>,
    ) -> Self {
        let kinds = types
            .into_iter()
            .map(|def| {
                let resolver = resolvers
                    .remove(def.key.as_str())
                    .unwrap_or_else(|| panic!("no resolver for type {}", def.key));
                (def.key.clone(), KindEntry { def, resolver })
            })
            .collect();
        Self { kinds, links }
    }

    /// Spec + résolveur d'un kind, sinon `None` (kind inconnu).
    pub fn entry(&self, kind: &str) -> Option<&KindEntry> {
        self.kinds.get(kind)
    }

    /// Kinds enregistrés (triés, pour les listes API).
    pub fn kinds(&self) -> Vec<&str> {
        let mut ks: Vec<&str> = self.kinds.keys().map(String::as_str).collect();
        ks.sort_unstable();
        ks
    }

    /// La règle « child_kind peut vivre dans parent_kind ? » — chemin unique
    /// par lequel le moteur interroge la validité (jamais de match).
    pub fn allows_containment(&self, parent_kind: &str, child_kind: &str) -> bool {
        self.kinds.get(parent_kind).is_some_and(|e| {
            e.def
                .may_contain
                .as_ref()
                .is_some_and(|set| set.contains(child_kind))
        })
    }

    pub fn allows_detach(&self, child_kind: &str) -> bool {
        self.kinds
            .get(child_kind)
            .is_some_and(|e| e.def.may_be_contained_in.is_some())
    }

    /// Is `relation(source → target)` a declared link type?
    pub fn allows_relation(&self, relation: &str, source_kind: &str, target_kind: &str) -> bool {
        self.links
            .iter()
            .any(|l| l.allows(relation, source_kind, target_kind))
    }
}

fn system_resolvers() -> HashMap<&'static str, Arc<dyn KindResolver>> {
    HashMap::from([
        (KIND_DEVICE, Arc::new(DeviceKind) as Arc<dyn KindResolver>),
        (KIND_MEDIA_ASSET, Arc::new(MediaAssetKind)),
        (KIND_DASHBOARD, Arc::new(DashboardKind)),
        (KIND_TOUR, Arc::new(TourKind)),
        (KIND_MAP_PIN, Arc::new(MapPinKind)),
        (KIND_FLOW, Arc::new(FlowKind)),
        (KIND_FOLDER, Arc::new(FolderKind)),
    ])
}

/// Registry of an org: the system types plus the org's types (D176), whose
/// objects resolve through their identity rows.
pub async fn for_org(db: &DatabaseConnection, org_id: i64) -> Result<Registry, sea_orm::DbErr> {
    let s = crate::services::ontology::schema(db, org_id).await?;
    let mut resolvers: HashMap<&str, Arc<dyn KindResolver>> = system_resolvers();
    for (t, _, _) in &s.types {
        if !t.system {
            resolvers.insert(t.key.as_str(), Arc::new(ObjectKind(t.key.clone())));
        }
    }
    let types = s.types.iter().map(|(t, _, _)| t.clone()).collect();
    let links = s.links.iter().map(|(l, _, _)| l.clone()).collect();
    Ok(Registry::derive(types, links, resolvers))
}

/// Registry of the system types alone (the D42 truth table of the tests).
#[cfg(test)]
pub fn global() -> &'static Registry {
    use pnex_core::ontology::{system_link_types, system_object_types};
    use std::sync::LazyLock;
    static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
        let resolvers = system_resolvers();
        Registry::derive(system_object_types(), system_link_types(), resolvers)
    });
    &REGISTRY
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::resources::{KINDS, REL_PLACED_ON};

    /// L0 exit criterion: the derived registry answers exactly like the
    /// hand-written D42 registry it replaces (truth table frozen below).
    #[test]
    fn derived_registry_matches_d42_rules() {
        let reg = global();
        assert_eq!(reg.kinds().len(), KINDS.len());
        for parent in KINDS {
            for child in KINDS {
                let expected =
                    parent == KIND_FOLDER || (parent == KIND_MAP_PIN && child == KIND_FOLDER);
                assert_eq!(
                    reg.allows_containment(parent, child),
                    expected,
                    "{parent} ⊃ {child}"
                );
            }
        }
        assert!(KINDS.iter().all(|k| reg.allows_detach(k)));
        assert!(!reg.allows_detach("unknown"));
        assert!(!reg.allows_containment("unknown", KIND_FOLDER));
        for source in KINDS {
            for target in KINDS {
                let expected = source == KIND_MAP_PIN
                    && [KIND_MEDIA_ASSET, KIND_DASHBOARD, KIND_TOUR].contains(&target);
                assert_eq!(
                    reg.allows_relation(REL_PLACED_ON, source, target),
                    expected,
                    "{source} placed_on {target}"
                );
                assert!(!reg.allows_relation("unknown", source, target));
            }
        }
    }
}
