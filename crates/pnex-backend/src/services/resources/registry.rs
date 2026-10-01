//! Registre des kinds vivants (D42) — **le** point d'extension de la couche.
//!
//! Ajouter un concept = 1 fonction `<kind>_entry()` (spec + résolveur) et
//! 1 ligne dans [`build`] : additif, local, aucun `match` à modifier dans le
//! moteur. La validité relation×kinds vit dans la spec du kind
//! (`pnex_core::resources`), jamais dans le moteur.
//!
//! Découplage device (D17) : le résolveur device **lit** `device_registries`
//! (existence org), n'y écrit jamais — taguer/organiser n'a aucun effet sur
//! le firmware ni sur le contrat `pnex_api_contract::CONTRACT`.

use std::collections::HashMap;
use std::sync::LazyLock;

use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::models::_entities::{
    dashboards, device_registries, flows, map_pins, media_assets, resource_folders, tours,
};

use pnex_core::resources::{
    KindSpec, KIND_DASHBOARD, KIND_DEVICE, KIND_FLOW, KIND_FOLDER, KIND_MAP_PIN, KIND_MEDIA_ASSET,
    KIND_TOUR, REL_PLACED_ON,
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

// ─────────────────────────── entrées de registre ───────────────────────────

/// Spec de containment — `None` = interdit, `Some(&[])` = sans restriction.
const fn contained_in_folder_only() -> pnex_core::resources::ContainmentRules {
    pnex_core::resources::ContainmentRules {
        may_contain: None,
        may_be_contained_in: Some(&[KIND_FOLDER]),
    }
}

fn device_entry() -> KindEntry {
    KindEntry {
        spec: KindSpec {
            kind: KIND_DEVICE,
            containment: contained_in_folder_only(),
            relations: vec![],
        },
        resolver: std::sync::Arc::new(DeviceKind),
    }
}

fn media_asset_entry() -> KindEntry {
    KindEntry {
        spec: KindSpec {
            kind: KIND_MEDIA_ASSET,
            containment: contained_in_folder_only(),
            relations: vec![],
        },
        resolver: std::sync::Arc::new(MediaAssetKind),
    }
}

fn dashboard_entry() -> KindEntry {
    KindEntry {
        spec: KindSpec {
            kind: KIND_DASHBOARD,
            containment: contained_in_folder_only(),
            relations: vec![],
        },
        resolver: std::sync::Arc::new(DashboardKind),
    }
}

fn tour_entry() -> KindEntry {
    KindEntry {
        spec: KindSpec {
            kind: KIND_TOUR,
            containment: contained_in_folder_only(),
            relations: vec![],
        },
        resolver: std::sync::Arc::new(TourKind),
    }
}

/// D39→D42 : le POI pose des médias/dashboards sur la carte (`placed_on`),
/// placement = hotspot coords — opaque pour le moteur.
/// Drawer POI : le POI contient aussi des **dossiers** de rangement
/// (containment `map_pin → folder`, objets rangés dedans).
fn map_pin_entry() -> KindEntry {
    KindEntry {
        spec: KindSpec {
            kind: KIND_MAP_PIN,
            containment: pnex_core::resources::ContainmentRules {
                may_contain: Some(&[KIND_FOLDER]),
                may_be_contained_in: Some(&[KIND_FOLDER]),
            },
            relations: vec![pnex_core::resources::RelationSpec {
                relation: REL_PLACED_ON,
                source_kind: KIND_MAP_PIN,
                target_kinds: &[KIND_MEDIA_ASSET, KIND_DASHBOARD, KIND_TOUR],
            }],
        },
        resolver: std::sync::Arc::new(MapPinKind),
    }
}

fn flow_entry() -> KindEntry {
    KindEntry {
        spec: KindSpec {
            kind: KIND_FLOW,
            containment: contained_in_folder_only(),
            relations: vec![],
        },
        resolver: std::sync::Arc::new(FlowKind),
    }
}

/// Le conteneur d'organisation pur : peut contenir **tous** les kinds
/// (`Some(&[])`), n'être contenu que dans un autre folder.
fn folder_entry() -> KindEntry {
    KindEntry {
        spec: KindSpec {
            kind: KIND_FOLDER,
            containment: pnex_core::resources::ContainmentRules {
                may_contain: Some(&[]),
                may_be_contained_in: Some(&[KIND_FOLDER]),
            },
            relations: vec![],
        },
        resolver: std::sync::Arc::new(FolderKind),
    }
}

// ─────────────────────────── registre ───────────────────────────

pub struct KindEntry {
    pub spec: KindSpec,
    pub resolver: std::sync::Arc<dyn KindResolver>,
}

pub struct Registry {
    kinds: HashMap<&'static str, KindEntry>,
}

impl Registry {
    /// Spec + résolveur d'un kind, sinon `None` (kind inconnu).
    pub fn entry(&self, kind: &str) -> Option<&KindEntry> {
        self.kinds.get(kind)
    }

    /// Kinds enregistrés (triés, pour les listes API).
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut ks: Vec<&'static str> = self.kinds.keys().copied().collect();
        ks.sort_unstable();
        ks
    }

    /// La règle « child_kind peut vivre dans parent_kind ? » — chemin unique
    /// par lequel le moteur interroge la validité (jamais de match).
    pub fn allows_containment(&self, parent_kind: &str, child_kind: &str) -> bool {
        let Some(entry) = self.kinds.get(parent_kind) else {
            return false;
        };
        match entry.spec.containment.may_contain {
            // None = le parent ne contient rien.
            None => false,
            // Some(&[]) = wildcard.
            Some(list) => list.is_empty() || list.contains(&child_kind),
        }
    }

    pub fn allows_detach(&self, child_kind: &str) -> bool {
        let Some(entry) = self.kinds.get(child_kind) else {
            return false;
        };
        entry.spec.containment.may_be_contained_in.is_some()
    }

    /// La règle « relation(source → target) est déclarée ? » — spec du kind
    /// source uniquement (validité déclarée par kind, école D42).
    pub fn allows_relation(&self, relation: &str, source_kind: &str, target_kind: &str) -> bool {
        let Some(entry) = self.kinds.get(source_kind) else {
            return false;
        };
        entry.spec.relations.iter().any(|r| {
            r.relation == relation
                && r.source_kind == source_kind
                // Wildcard = liste vide.
                && (r.target_kinds.is_empty() || r.target_kinds.contains(&target_kind))
        })
    }
}

/// Registre global — les kinds vivants (un concept futur ajoute sa ligne ici
/// et sa fonction `*_entry()` au-dessus, rien d'autre).
pub fn global() -> &'static Registry {
    static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry {
        kinds: HashMap::from([
            (KIND_DEVICE, device_entry()),
            (KIND_MEDIA_ASSET, media_asset_entry()),
            (KIND_DASHBOARD, dashboard_entry()),
            (KIND_TOUR, tour_entry()),
            (KIND_MAP_PIN, map_pin_entry()),
            (KIND_FLOW, flow_entry()),
            (KIND_FOLDER, folder_entry()),
        ]),
    });
    &REGISTRY
}
