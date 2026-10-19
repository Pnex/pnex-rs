//! Packs (D190): YAML bundles of object and link types. The server ships
//! reference packs; an org installs them (types created with `pack_key`),
//! upgrades them (a newer version appends type versions), and exports or
//! imports its own schema as a pack (as-code, D186).
//!
//! Installed types are the org's: editable locally (decision on question 4,
//! 2026-10-11); an upgrade appends the pack definition as a new version,
//! the local history staying readable. What the org added (properties,
//! actions, link attributes, source/target types) survives the upgrade
//! (2026-10-18): the pack wins on what it declares, the org keeps the rest.

use pnex_core::ontology::api::{LinkTypeInput, ObjectTypeInput, Pack, PackView, PageTemplate};
use pnex_core::ontology::{LinkTypeDef, ObjectTypeDef, TypeSet};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use pnex_core::err_codes::FIELD_INVALID;

use super::{invalid, link_types, schema, types, OntologyError, Result};
use crate::models::_entities::packs;

/// YAML documents are bounded before parsing.
pub const YAML_MAX_BYTES: usize = 256 * 1024;

const SHIPPED: &[&str] = &[
    include_str!("../../../packs/maintenance.yaml"),
    include_str!("../../../packs/investigation.yaml"),
    include_str!("../../../packs/media_coverage.yaml"),
    include_str!("../../../packs/entreprises_fr.yaml"),
    include_str!("../../../packs/quali_watch.yaml"),
];

pub fn shipped() -> Vec<Pack> {
    SHIPPED
        .iter()
        .map(|y| serde_saphyr::from_str(y).expect("shipped pack is valid YAML"))
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
    let pack: Pack = serde_saphyr::from_str(yaml)
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
            descriptions: p.descriptions.clone(),
            names: p.names.clone(),
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

/// Page templates of the shipped packs the org installed.
pub async fn page_templates(db: &DatabaseConnection, org_id: i64) -> Result<Vec<PageTemplate>> {
    let installed: Vec<String> = packs::Entity::find()
        .filter(packs::Column::OrgId.eq(org_id))
        .all(db)
        .await?
        .into_iter()
        .map(|p| p.key)
        .collect();
    Ok(shipped()
        .into_iter()
        .filter(|p| installed.contains(&p.key))
        .flat_map(|p| p.page_templates)
        .collect())
}

/// Hierarchies (D305) of the shipped packs the org installed.
pub async fn hierarchies(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<pnex_core::ontology::api::Hierarchy>> {
    let installed: Vec<String> = packs::Entity::find()
        .filter(packs::Column::OrgId.eq(org_id))
        .all(db)
        .await?
        .into_iter()
        .map(|p| p.key)
        .collect();
    Ok(shipped()
        .into_iter()
        .filter(|p| installed.contains(&p.key))
        .flat_map(|p| p.hierarchies)
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
    for i in 0..pack.extends.len() {
        extended_properties(s, pack, i)?;
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
    // One install at a time per org: two at once raced on the type keys
    // (409 for the loser) although the second had nothing to do.
    use crate::services::db_lock::{ns, TenantLock};
    let lock = TenantLock::acquire(
        db,
        ns::ONTOLOGY_PACK,
        org_id,
        std::time::Duration::from_secs(30),
    )
    .await?;
    let out = install_locked(db, org_id, user_id, pack).await;
    lock.release().await;
    out
}

async fn install_locked(
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
            descriptions: pack.descriptions.clone(),
            names: pack.names.clone(),
            key: pack.key.clone(),
            name: pack.name.clone(),
            version: pack.version.clone(),
            description: pack.description.clone(),
            installed_version: Some(pack.version.clone()),
        });
    }
    for key in &pack.requires {
        let present = packs::Entity::find()
            .filter(packs::Column::OrgId.eq(org_id))
            .filter(packs::Column::Key.eq(key))
            .one(db)
            .await?
            .is_some();
        if !present {
            let Some(dep) = shipped().into_iter().find(|p| &p.key == key) else {
                return Err(invalid("requires", pnex_core::err_codes::FIELD_INVALID));
            };
            Box::pin(install_locked(db, org_id, user_id, &dep)).await?;
        }
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
                actions: Vec::new(),
                ..t.clone()
            };
            types::create(db, org_id, user_id, &stub, Some(&pack.key)).await?;
        }
    }
    for t in &pack.types {
        let current = types::get(db, org_id, &t.key).await?.version;
        let def = match s.object_type(&t.key) {
            Some(local) => keep_org_additions(t, local),
            None => t.clone(),
        };
        types::update(
            db,
            org_id,
            &t.key,
            user_id,
            &ObjectTypeInput {
                def,
                expected_version: Some(current),
            },
        )
        .await?;
    }
    install_extensions(db, org_id, user_id, pack).await?;
    for l in &pack.link_types {
        install_link(db, org_id, &s, &pack.key, l).await?;
    }
    super::imports::install_from_pack(db, org_id, user_id, pack).await?;
    install_automation(db, org_id, user_id, pack).await?;
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
        descriptions: pack.descriptions.clone(),
        names: pack.names.clone(),
        key: pack.key.clone(),
        name: pack.name.clone(),
        version: pack.version.clone(),
        description: pack.description.clone(),
        installed_version: Some(pack.version.clone()),
    })
}

/// D306: adds the pack's optional properties to a system type or to a type
/// of a required pack. The properties this pack added before are replaced
/// (upgrade); a key the type already has otherwise refuses the install.
async fn install_extensions(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    pack: &Pack,
) -> Result<()> {
    if pack.extends.is_empty() {
        return Ok(());
    }
    let s = schema(db, org_id).await?;
    for (i, e) in pack.extends.iter().enumerate() {
        let props = extended_properties(&s, pack, i)?;
        let Some((target, version, _)) = s.types.iter().find(|(t, _, _)| t.key == e.type_key)
        else {
            return Err(invalid(format!("extends.{i}.type"), FIELD_INVALID));
        };
        types::update(
            db,
            org_id,
            &e.type_key,
            user_id,
            &ObjectTypeInput {
                def: ObjectTypeDef {
                    properties: props,
                    ..target.clone()
                },
                expected_version: Some(*version),
            },
        )
        .await?;
    }
    Ok(())
}

/// Keys of the shipped packs `pack` builds on, directly or not.
pub fn requires_closure(pack: &Pack) -> Vec<String> {
    let all = shipped();
    let mut keys = pack.requires.clone();
    let mut i = 0;
    while i < keys.len() {
        if let Some(dep) = all.iter().find(|p| p.key == keys[i]) {
            for k in &dep.requires {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
        }
        i += 1;
    }
    keys
}

/// The properties of the type extended by `pack.extends[i]` once extended,
/// or the field error refusing the extension.
fn extended_properties(
    s: &super::OrgSchema,
    pack: &Pack,
    i: usize,
) -> Result<Vec<pnex_core::ontology::PropertyDef>> {
    let e = &pack.extends[i];
    let at = |f: &str| format!("extends.{i}.{f}");
    let Some((target, _, owner)) = s.types.iter().find(|(t, _, _)| t.key == e.type_key) else {
        return Err(invalid(at("type"), FIELD_INVALID));
    };
    let owned_by_dep = owner
        .as_ref()
        .is_some_and(|o| requires_closure(pack).contains(o));
    if !(target.system || owned_by_dep) {
        return Err(invalid(at("type"), FIELD_INVALID));
    }
    pnex_core::ontology::schema::check_defs(&e.properties, &at("properties"), &|k| s.knows(k))?;
    let mut props = target.properties.clone();
    props.retain(|p| p.pack.as_deref() != Some(pack.key.as_str()));
    for (j, p) in e.properties.iter().enumerate() {
        if p.required {
            return Err(invalid(
                at(&format!("properties.{j}.required")),
                FIELD_INVALID,
            ));
        }
        if props.iter().any(|q| q.key == p.key) {
            return Err(invalid(at(&format!("properties.{j}.key")), "duplicate"));
        }
        props.push(pnex_core::ontology::PropertyDef {
            pack: Some(pack.key.clone()),
            ..p.clone()
        });
    }
    Ok(props)
}

/// D316: the pack's functions and flow models, created when the org has
/// none of that name (never overwritten: the org may have edited them).
/// Flows are drafts: deploying them stays a human act (D123).
async fn install_automation(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    pack: &Pack,
) -> Result<()> {
    use crate::models::_entities::{flows, functions};
    if pack.functions.is_empty() && pack.flows.is_empty() {
        return Ok(());
    }
    // name → (id, version, inputs, outputs) of the org's functions.
    let mut known: std::collections::HashMap<
        String,
        (i64, i64, serde_json::Value, serde_json::Value),
    > = std::collections::HashMap::new();
    for (i, f) in pack.functions.iter().enumerate() {
        let existing = functions::Entity::find()
            .filter(functions::Column::OrgId.eq(org_id))
            .filter(functions::Column::Name.eq(f.name.trim()))
            .one(db)
            .await?;
        let (id, version) = match existing {
            Some(row) => {
                let v = crate::services::functions::latest_version(db, row.id).await?;
                (row.id, v)
            }
            None => {
                let (row, v) = crate::services::functions::create_function(
                    db,
                    org_id,
                    &pnex_core::CreateFunction {
                        name: f.name.clone(),
                        language: f.language,
                        description: Some(f.description.clone()).filter(|d| !d.is_empty()),
                        code: f.code.clone(),
                        note: Some(format!("pack {} {}", pack.key, pack.version)),
                    },
                )
                .await
                .map_err(|e| {
                    tracing::warn!(pack = %pack.key, function = %f.name, error = %e, "pack function refused");
                    invalid(format!("functions.{i}"), FIELD_INVALID)
                })?;
                (row.id, Some(v))
            }
        };
        if let Some(v) = version {
            known.insert(f.name.clone(), (id, v.version_number, v.inputs, v.outputs));
        }
    }
    for (i, f) in pack.flows.iter().enumerate() {
        let exists = flows::Entity::find()
            .filter(flows::Column::OrgId.eq(org_id))
            .filter(flows::Column::Name.eq(f.name.trim()))
            .one(db)
            .await?
            .is_some();
        if exists {
            continue;
        }
        let mut graph = f.graph.clone();
        resolve_functions(&mut graph, |name| {
            known
                .get(name)
                .map(|(id, v, inputs, outputs)| (*id, *v, inputs.clone(), outputs.clone()))
        });
        let graph: pnex_core::FlowGraph = serde_json::from_value(graph)
            .map_err(|_| invalid(format!("flows.{i}.graph"), FIELD_INVALID))?;
        crate::services::flow::create_flow(
            db,
            org_id,
            &f.name,
            &graph,
            None,
            Some(format!("pack:{}", pack.key)),
            Some(f.note.clone()).filter(|n| !n.is_empty()),
            &crate::services::secrets::flow::GraphWriter {
                ring: None,
                user_id,
                can_write_secrets: false,
            },
        )
        .await
        .map_err(|e| {
            tracing::warn!(pack = %pack.key, flow = %f.name, error = ?e, "pack flow refused");
            invalid(format!("flows.{i}"), FIELD_INVALID)
        })?;
    }
    Ok(())
}

/// Function nodes naming a function (`function_name`, no id) get the id,
/// version and declared rows of the org's function of that name.
pub fn resolve_functions(
    graph: &mut serde_json::Value,
    lookup: impl Fn(&str) -> Option<(i64, i64, serde_json::Value, serde_json::Value)>,
) {
    let Some(nodes) = graph.get_mut("nodes").and_then(|n| n.as_array_mut()) else {
        return;
    };
    for n in nodes {
        if n["kind"] != "pnex_function" {
            continue;
        }
        let name = n["config"]["function_name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if let Some((id, v, inputs, outputs)) = lookup(&name) {
            let c = &mut n["config"];
            c["function_id"] = id.into();
            c["version_number"] = v.into();
            c["inputs"] = inputs;
            c["outputs"] = outputs;
        }
    }
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
        Some((local, v, Some(pk))) if pk == pack_key => {
            link_types::update(
                db,
                org_id,
                &l.key,
                &LinkTypeInput {
                    def: keep_org_link_additions(l, local),
                    expected_version: Some(*v),
                },
            )
            .await?;
        }
        Some(_) => return Err(OntologyError::KeyTaken),
    }
    Ok(())
}

/// The pack definition plus the properties and actions the org added
/// locally (keys the pack does not declare).
fn keep_org_additions(pack: &ObjectTypeDef, local: &ObjectTypeDef) -> ObjectTypeDef {
    let mut def = pack.clone();
    def.properties.extend(
        local
            .properties
            .iter()
            .filter(|p| !pack.properties.iter().any(|q| q.key == p.key))
            .cloned(),
    );
    def.actions.extend(
        local
            .actions
            .iter()
            .filter(|a| !pack.actions.iter().any(|b| b.key == a.key))
            .cloned(),
    );
    def
}

/// The pack link type plus the attributes and source/target types the org
/// added locally.
fn keep_org_link_additions(pack: &LinkTypeDef, local: &LinkTypeDef) -> LinkTypeDef {
    let mut def = pack.clone();
    def.attributes.extend(
        local
            .attributes
            .iter()
            .filter(|a| !pack.attributes.iter().any(|b| b.key == a.key))
            .cloned(),
    );
    def.from_types = union(&pack.from_types, &local.from_types);
    def.to_types = union(&pack.to_types, &local.to_types);
    def
}

fn union(pack: &TypeSet, local: &TypeSet) -> TypeSet {
    match (pack, local) {
        (TypeSet::Only(p), TypeSet::Only(l)) => {
            let mut keys = p.clone();
            keys.extend(l.iter().filter(|k| !p.contains(k)).cloned());
            TypeSet::Only(keys)
        }
        _ => TypeSet::Any,
    }
}

/// The org's own schema as a pack (types and link types, system ones and
/// system extensions excluded).
pub async fn export(db: &DatabaseConnection, org_id: i64) -> Result<String> {
    let s = schema(db, org_id).await?;
    let pack = Pack {
        descriptions: Default::default(),
        names: Default::default(),
        key: "org".into(),
        name: "Organization schema".into(),
        version: chrono::Utc::now().format("%Y%m%d%H%M%S").to_string(),
        requires: Vec::new(),
        description: String::new(),
        types: s.org_types(),
        link_types: s.org_links(),
        page_templates: Vec::new(),
        extends: Vec::new(),
        hierarchies: Vec::new(),
        imports: Vec::new(),
        functions: Vec::new(),
        flows: Vec::new(),
    };
    serde_saphyr::to_string(&pack)
        .map_err(|e| OntologyError::Db(sea_orm::DbErr::Custom(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_packs_are_valid() {
        let all = shipped();
        for p in &all {
            // A pack sees its own types and those of the packs it requires,
            // directly or not.
            let mut keys = p.requires.clone();
            let mut i = 0;
            while i < keys.len() {
                let dep = all.iter().find(|d| d.key == keys[i]);
                let dep = dep.unwrap_or_else(|| panic!("{}: unknown required pack", p.key));
                keys.extend(
                    dep.requires
                        .iter()
                        .filter(|k| !keys.contains(k))
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                i += 1;
            }
            let deps: Vec<&Pack> = all.iter().filter(|d| keys.contains(&d.key)).collect();
            let known = |k: &str| {
                pnex_core::ontology::system_object_types()
                    .iter()
                    .any(|t| t.key == k)
                    || p.types.iter().any(|t| t.key == k)
                    || deps.iter().any(|d| d.types.iter().any(|t| t.key == k))
            };
            for t in &p.types {
                t.check(&known)
                    .unwrap_or_else(|e| panic!("{}: {e:?}", t.key));
            }
            for l in &p.link_types {
                l.check(&known)
                    .unwrap_or_else(|e| panic!("{}: {e:?}", l.key));
            }
            for e in &p.extends {
                let in_dep = deps
                    .iter()
                    .any(|d| d.types.iter().any(|t| t.key == e.type_key));
                let system = pnex_core::ontology::system_object_types()
                    .iter()
                    .any(|t| t.key == e.type_key);
                assert!(in_dep || system, "{}: extends {}", p.key, e.type_key);
                pnex_core::ontology::schema::check_defs(&e.properties, "extends", &known)
                    .unwrap_or_else(|err| panic!("{}: {err:?}", e.type_key));
                assert!(e.properties.iter().all(|q| !q.required));
            }
            let links: Vec<&str> = deps
                .iter()
                .flat_map(|d| d.link_types.iter())
                .chain(p.link_types.iter())
                .map(|l| l.key.as_str())
                .collect();
            for h in &p.hierarchies {
                for s in &h.steps {
                    assert!(links.contains(&s.link.as_str()), "{}: {}", h.key, s.link);
                }
            }
            // Shipped mappings fit the schema the pack installs: system,
            // required and own types, extended by the whole chain.
            let chain: Vec<&Pack> = deps.iter().copied().chain([p]).collect();
            let mut types: Vec<ObjectTypeDef> = pnex_core::ontology::system_object_types();
            types.extend(chain.iter().flat_map(|d| d.types.iter().cloned()));
            for e in chain.iter().flat_map(|d| d.extends.iter()) {
                if let Some(t) = types.iter_mut().find(|t| t.key == e.type_key) {
                    t.properties.extend(e.properties.iter().cloned());
                }
            }
            let mut link_defs = pnex_core::ontology::system_link_types();
            link_defs.extend(chain.iter().flat_map(|d| d.link_types.iter().cloned()));
            // Flow models are valid once their functions are resolved, and
            // every function of the pack declares well-formed rows.
            for f in &p.functions {
                pnex_core::parse_directives(&f.code)
                    .unwrap_or_else(|e| panic!("{}: function {}: {e:?}", p.key, f.name));
            }
            for f in &p.flows {
                let mut g = f.graph.clone();
                resolve_functions(&mut g, |name| {
                    p.functions.iter().find(|x| x.name == name).map(|x| {
                        let sig = pnex_core::parse_directives(&x.code).unwrap();
                        (
                            1,
                            1,
                            serde_json::to_value(sig.inputs).unwrap(),
                            serde_json::to_value(sig.outputs).unwrap(),
                        )
                    })
                });
                let g: pnex_core::FlowGraph = serde_json::from_value(g)
                    .unwrap_or_else(|e| panic!("{}: flow {}: {e}", p.key, f.name));
                let v = pnex_core::validate_graph(&g);
                assert!(v.is_empty(), "{}: flow {}: {v:?}", p.key, f.name);
            }
            for i in &p.imports {
                pnex_core::ontology::import::check(
                    &i.mapping,
                    &pnex_core::ontology::import::Schema {
                        types: &types,
                        links: &link_defs,
                    },
                )
                .unwrap_or_else(|e| panic!("{}: import {}: {e:?}", p.key, i.name));
            }
        }
    }

    #[test]
    fn export_round_trips_through_parse() {
        let p = shipped().remove(0);
        let yaml = serde_saphyr::to_string(&p).unwrap();
        assert_eq!(parse(&yaml).unwrap(), p);
    }
}
