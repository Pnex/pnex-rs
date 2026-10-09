//! Location of a resource in the sites tree (breadcrumb "Site › folder ›
//! folder"): containment chain up to a site (`map_pin`), then the site
//! links that do not go through a folder — `placed_on` edges (media, tour,
//! dashboard, several sites possible) and the D43 device placement.

use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::models::_entities::{device_placements, map_pins};
use crate::services::resources::{containment, edges};
use pnex_core::resources::{
    LocationNode, ResourceLocation, KIND_DEVICE, KIND_FOLDER, KIND_MAP_PIN, REL_PLACED_ON,
};

/// Every site path of `(kind, id)` in `org_id`, sites deduplicated (the
/// folder path wins over a direct link to the same site).
pub async fn locations_of(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Vec<ResourceLocation>, DbErr> {
    // (site id, folder ids root → leaf)
    let mut paths: Vec<(String, Vec<i64>)> = Vec::new();

    let chain = containment::path_of(db, org_id, kind, id).await?;
    if let Some((root_kind, root_id)) = chain.last() {
        if root_kind == KIND_MAP_PIN {
            let folders: Vec<i64> = chain
                .iter()
                .rev()
                .filter(|(k, _)| k == KIND_FOLDER)
                .filter_map(|(_, i)| i.parse().ok())
                .collect();
            paths.push((root_id.clone(), folders));
        }
    }
    let mut push_site = |site: String| {
        if !paths.iter().any(|(s, _)| *s == site) {
            paths.push((site, Vec::new()));
        }
    };
    for edge in edges::list_edges(db, org_id, Some(REL_PLACED_ON), None, Some((kind, id))).await? {
        if edge.source_kind == KIND_MAP_PIN {
            push_site(edge.source_id);
        }
    }
    if kind == KIND_DEVICE {
        if let Ok(pk) = id.parse::<i64>() {
            if let Some(p) = device_placements::Entity::find()
                .filter(device_placements::Column::OrgId.eq(org_id))
                .filter(device_placements::Column::DeviceRegistryId.eq(pk))
                .one(db)
                .await?
            {
                push_site(p.pin_id.to_string());
            }
        }
    }
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    // Names, batched, org-scoped.
    let site_ids: Vec<Uuid> = paths.iter().filter_map(|(s, _)| s.parse().ok()).collect();
    let sites = map_pins::Entity::find()
        .filter(map_pins::Column::OrgId.eq(org_id))
        .filter(map_pins::Column::Id.is_in(site_ids))
        .all(db)
        .await?;
    let folder_ids: Vec<i64> = paths.iter().flat_map(|(_, f)| f.iter().copied()).collect();
    let folder_names = containment::folder_names(db, org_id, &folder_ids).await?;

    Ok(paths
        .into_iter()
        .filter_map(|(site_id, folders)| {
            let site = sites.iter().find(|s| s.id.to_string() == site_id)?;
            Some(ResourceLocation {
                site: LocationNode {
                    kind: KIND_MAP_PIN.into(),
                    id: site_id,
                    name: site.label.clone(),
                    emoji: site.emoji.clone(),
                },
                folders: folders
                    .into_iter()
                    .filter_map(|f| {
                        let (name, emoji) = folder_names.get(&f)?.clone();
                        Some(LocationNode {
                            kind: KIND_FOLDER.into(),
                            id: f.to_string(),
                            name,
                            emoji,
                        })
                    })
                    .collect(),
            })
        })
        .collect())
}
