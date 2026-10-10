//! Packs (D190): YAML bundles of object and link types. The server ships
//! reference packs; an org installs them (types created with `pack_key`),
//! upgrades them (a newer version appends type versions), and exports or
//! imports its own schema as a pack (as-code, D186).
//!
//! Installed types are the org's: editable locally (decision on question 4,
//! 2026-10-11); an upgrade appends the pack definition as a new version,
//! the local history staying readable.

use pnex_core::ontology::api::{LinkTypeInput, ObjectTypeInput, Pack, PackView};
use pnex_core::ontology::{LinkTypeDef, ObjectTypeDef};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use super::{invalid, link_types, schema, types, OntologyError, Result};
use crate::models::_entities::packs;

/// YAML documents are bounded before parsing.
pub const YAML_MAX_BYTES: usize = 256 * 1024;

const SHIPPED: &[&str] = &[include_str!("../../../packs/maintenance.yaml")];

pub fn shipped() -> Vec<Pack> {
    SHIPPED
        .iter()
        .map(|y| serde_yaml::from_str(y).expect("shipped pack is valid YAML"))
        .collect()
}

pub fn parse(yaml: &str) -> Result<Pack> {
    if yaml.len() > YAML_MAX_BYTES {
        return Err(invalid(
            "yaml",
            format!(
                "{}:{YAML_MAX_BYTES}",
                pnex_core::err_codes::FIELD_MAX_LENGTH
            ),
        ));
    }
    let pack: Pack = serde_yaml::from_str(yaml)
        .map_err(|_| invalid("yaml", pnex_core::err_codes::FIELD_INVALID))?;
    if !pnex_core::ontology::schema::valid_key(&pack.key)
        || pack.version.trim().is_empty()
        || pack.version.len() > 32
    {
        return Err(invalid("key", pnex_core::err_codes::FIELD_INVALID));
    }
    Ok(pack)
}

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<PackView>> {
    let installed = packs::Entity::find()
        .filter(packs::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    Ok(shipped()
        .into_iter()
        .map(|p| PackView {
            installed_version: installed
                .iter()
                .find(|i| i.key == p.key)
                .map(|i| i.version.clone()),
            key: p.key,
            name: p.name,
            version: p.version,
            description: p.description,
        })
        .collect())
}

/// Checks the whole pack against the org schema plus its own types before
/// any write, so a pack does not stop halfway on a validation error.
fn check(s: &super::OrgSchema, pack: &Pack) -> Result<()> {
    let known = |k: &str| s.knows(k) || pack.types.iter().any(|t| t.key == k);
    for (i, t) in pack.types.iter().enumerate() {
        t.check(&known)
            .map_err(|(f, tok)| invalid(format!("types.{i}.{f}"), tok))?;
    }
    for (i, l) in pack.link_types.iter().enumerate() {
        l.check(&known)
            .map_err(|(f, tok)| invalid(format!("link_types.{i}.{f}"), tok))?;
    }
    Ok(())
}

/// Installs or upgrades a pack in the org. Types and link types the org
/// already has under the same key are versioned when they come from this
/// pack, refused (409) when they are the org's own.
pub async fn install(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    pack: &Pack,
) -> Result<PackView> {
    // Same version already installed: nothing to do (no duplicate versions).
    let installed = packs::Entity::find()
        .filter(packs::Column::OrgId.eq(org_id))
        .filter(packs::Column::Key.eq(&pack.key))
        .one(db)
        .await?;
    if installed.is_some_and(|p| p.version == pack.version) {
        return Ok(PackView {
            key: pack.key.clone(),
            name: pack.name.clone(),
            version: pack.version.clone(),
            description: pack.description.clone(),
            installed_version: Some(pack.version.clone()),
        });
    }
    let s = schema(db, org_id).await?;
    check(&s, pack)?;
    let owned_by_other = |key: &str, pk: Option<&String>| {
        s.knows(key) && pk.map(String::as_str) != Some(pack.key.as_str())
    };
    for t in &pack.types {
        let pk = s
            .types
            .iter()
            .find(|(d, _, _)| d.key == t.key)
            .and_then(|(_, _, p)| p.as_ref());
        if owned_by_other(&t.key, pk) {
            return Err(OntologyError::KeyTaken);
        }
    }
    // Create the missing types first with an empty schema so that types of
    // the pack may reference each other, then write every definition.
    for t in &pack.types {
        if s.object_type(&t.key).is_none() {
            let stub = ObjectTypeDef {
                may_contain: None,
                may_be_contained_in: None,
                properties: Vec::new(),
                ..t.clone()
            };
            types::create(db, org_id, user_id, &stub, Some(&pack.key)).await?;
        }
    }
    for t in &pack.types {
        let current = types::get(db, org_id, &t.key).await?.version;
        types::update(
            db,
            org_id,
            &t.key,
            user_id,
            &ObjectTypeInput {
                def: t.clone(),
                expected_version: Some(current),
            },
        )
        .await?;
    }
    for l in &pack.link_types {
        install_link(db, org_id, &s, &pack.key, l).await?;
    }
    let row = packs::Entity::find()
        .filter(packs::Column::OrgId.eq(org_id))
        .filter(packs::Column::Key.eq(&pack.key))
        .one(db)
        .await?;
    match row {
        Some(r) => {
            let mut am: packs::ActiveModel = r.into();
            am.version = Set(pack.version.clone());
            am.installed_by = Set(user_id);
            am.updated_at = Set(chrono::Utc::now().into());
            am.update(db).await?;
        }
        None => {
            packs::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(org_id),
                key: Set(pack.key.clone()),
                version: Set(pack.version.clone()),
                installed_by: Set(user_id),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
    }
    Ok(PackView {
        key: pack.key.clone(),
        name: pack.name.clone(),
        version: pack.version.clone(),
        description: pack.description.clone(),
        installed_version: Some(pack.version.clone()),
    })
}

async fn install_link(
    db: &DatabaseConnection,
    org_id: i64,
    s: &super::OrgSchema,
    pack_key: &str,
    l: &LinkTypeDef,
) -> Result<()> {
    match s.links.iter().find(|(d, _, _)| d.key == l.key) {
        None => {
            link_types::create(db, org_id, l, Some(pack_key)).await?;
        }
        Some((_, v, Some(pk))) if pk == pack_key => {
            link_types::update(
                db,
                org_id,
                &l.key,
                &LinkTypeInput {
                    def: l.clone(),
                    expected_version: Some(*v),
                },
            )
            .await?;
        }
        Some(_) => return Err(OntologyError::KeyTaken),
    }
    Ok(())
}

/// The org's own schema as a pack (types and link types, system ones and
/// system extensions excluded).
pub async fn export(db: &DatabaseConnection, org_id: i64) -> Result<String> {
    let s = schema(db, org_id).await?;
    let pack = Pack {
        key: "org".into(),
        name: "Organization schema".into(),
        version: chrono::Utc::now().format("%Y%m%d%H%M%S").to_string(),
        description: String::new(),
        types: s.org_types(),
        link_types: s.org_links(),
    };
    serde_yaml::to_string(&pack)
        .map_err(|e| OntologyError::Db(sea_orm::DbErr::Custom(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_packs_are_valid() {
        for p in shipped() {
            let known = |k: &str| {
                pnex_core::ontology::system_object_types()
                    .iter()
                    .any(|t| t.key == k)
                    || p.types.iter().any(|t| t.key == k)
            };
            for t in &p.types {
                t.check(&known)
                    .unwrap_or_else(|e| panic!("{}: {e:?}", t.key));
            }
            for l in &p.link_types {
                l.check(&known)
                    .unwrap_or_else(|e| panic!("{}: {e:?}", l.key));
            }
        }
    }

    #[test]
    fn export_round_trips_through_parse() {
        let p = shipped().remove(0);
        let yaml = serde_yaml::to_string(&p).unwrap();
        assert_eq!(parse(&yaml).unwrap(), p);
    }
}
