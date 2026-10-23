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

use pnex_core::ontology::api::{
    LinkTypeInput, ObjectTypeInput, Pack, PackSettingView, PackSettingsInput, PackView,
    PageTemplate,
};
use pnex_core::ontology::{LinkTypeDef, ObjectTypeDef, PropertyDef, TypeSet};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set,
};
use serde_json::{Map, Value};
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
    include_str!("../../../packs/entreprises.yaml"),
    include_str!("../../../packs/entreprises_fr.yaml"),
    include_str!("../../../packs/presence_web.yaml"),
    include_str!("../../../packs/quali_watch.yaml"),
    include_str!("../../../packs/prospection_b2b.yaml"),
    include_str!("../../../packs/prospection_fr.yaml"),
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
    let layers = layers(db, org_id).await?;
    let mut out = Vec::new();
    for p in shipped() {
        let installed_version = installed
            .iter()
            .find(|i| i.key == p.key)
            .map(|i| i.version.clone());
        let (settings, kept) = match installed_version {
            Some(_) => (
                settings_view(&p, layers.get(&p.key).unwrap_or(&Value::Null)),
                edited(db, org_id, &p).await?,
            ),
            None => (Vec::new(), Vec::new()),
        };
        out.push(PackView {
            descriptions: p.descriptions.clone(),
            names: p.names.clone(),
            installed_version,
            settings,
            kept,
            key: p.key,
            name: p.name,
            version: p.version,
            description: p.description,
        });
    }
    Ok(out)
}

/// The pack's functions and flows the org edited since the pack wrote them
/// (D368): they keep the org's version at an upgrade. One of the same name
/// the pack never wrote is the org's own, not listed.
async fn edited(db: &DatabaseConnection, org_id: i64, pack: &Pack) -> Result<Vec<String>> {
    if pack.functions.is_empty() && pack.flows.is_empty() {
        return Ok(Vec::new());
    }
    let names = |it: Vec<&str>| Value::from(it.into_iter().map(str::trim).collect::<Vec<_>>());
    let note = format!("pack {} ", pack.key);
    let rows = db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT f.name, l.code FROM functions f \
             JOIN LATERAL (SELECT code, note FROM function_versions v WHERE v.function_id = f.id \
                           ORDER BY version_number DESC LIMIT 1) l ON true \
             WHERE f.org_id = $1 AND f.name IN (SELECT jsonb_array_elements_text($2)) \
               AND left(coalesce(l.note, ''), length($3)) <> $3 \
               AND EXISTS (SELECT 1 FROM function_versions v WHERE v.function_id = f.id \
                           AND left(v.note, length($3)) = $3)",
            vec![
                org_id.into(),
                names(pack.functions.iter().map(|f| f.name.as_str()).collect()).into(),
                note.into(),
            ],
        ))
        .await?;
    let mut out: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            let name: String = r.try_get("", "name").ok()?;
            let code: String = r.try_get("", "code").ok()?;
            let f = pack.functions.iter().find(|f| f.name.trim() == name)?;
            (f.code != code).then_some(name)
        })
        .collect();
    let rows = db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT f.name FROM flows f \
             JOIN LATERAL (SELECT author FROM flow_versions v WHERE v.flow_id = f.id \
                           ORDER BY version_number DESC LIMIT 1) l ON true \
             WHERE f.org_id = $1 AND f.name IN (SELECT jsonb_array_elements_text($2)) \
               AND l.author IS DISTINCT FROM $3 \
               AND EXISTS (SELECT 1 FROM flow_versions v WHERE v.flow_id = f.id AND v.author = $3)",
            vec![
                org_id.into(),
                names(pack.flows.iter().map(|f| f.name.as_str()).collect()).into(),
                format!("pack:{}", pack.key).into(),
            ],
        ))
        .await?;
    out.extend(rows.iter().filter_map(|r| r.try_get("", "name").ok()));
    Ok(out)
}

/// The pack's version of the org's edited functions and flows `names`
/// (D368): a new version over the org's, which stays in the history.
pub async fn take(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    key: &str,
    input: &pnex_core::ontology::api::PackTakeInput,
) -> Result<PackView> {
    let pack = shipped()
        .into_iter()
        .find(|p| p.key == key)
        .ok_or(OntologyError::NotFound)?;
    let installed = packs::Entity::find()
        .filter(packs::Column::OrgId.eq(org_id))
        .filter(packs::Column::Key.eq(key))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)?;
    // Taking an older pack's code would undo the upgrade it never had.
    if installed.version != pack.version {
        return Err(invalid("version", FIELD_INVALID));
    }
    let edited = edited(db, org_id, &pack).await?;
    for (i, n) in input.names.iter().enumerate() {
        if !edited.contains(n) {
            return Err(invalid(format!("names.{i}"), FIELD_INVALID));
        }
    }
    install_automation(db, org_id, user_id, &pack, &input.names).await?;
    list(db, org_id)
        .await?
        .into_iter()
        .find(|p| p.key == key)
        .ok_or(OntologyError::NotFound)
}

/// The org's layer of each installed pack (D368): its own setting values.
async fn layers(db: &DatabaseConnection, org_id: i64) -> Result<Map<String, Value>> {
    Ok(db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT key, settings FROM packs WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| Some((r.try_get("", "key").ok()?, r.try_get("", "settings").ok()?)))
        .collect())
}

/// The pack's settings with the value in force: the org's, else the pack's.
pub fn settings_view(pack: &Pack, layer: &Value) -> Vec<PackSettingView> {
    pack.settings
        .iter()
        .map(|s| {
            let own = layer.get(&s.key).filter(|v| !v.is_null());
            PackSettingView {
                def: s.clone(),
                value: own.cloned().unwrap_or_else(|| s.default.clone()),
                overridden: own.is_some(),
            }
        })
        .collect()
}

/// Every setting in force for the org, keyed `<pack>.<key>`: what pack
/// functions read with `pack_setting` (stamped at deploy).
pub async fn org_settings(db: &DatabaseConnection, org_id: i64) -> Result<Map<String, Value>> {
    let layers = layers(db, org_id).await?;
    let mut out = Map::new();
    for p in shipped() {
        let Some(layer) = layers.get(&p.key) else {
            continue;
        };
        for s in settings_view(&p, layer) {
            out.insert(format!("{}.{}", p.key, s.def.key), s.value);
        }
    }
    Ok(out)
}

/// The shipped definition of a property `pack` owns in `scope`
/// (`type:<key>`, its extensions included, or `link:<key>`).
fn shipped_property(pack: &Pack, scope: &str, prop: &str) -> Option<PropertyDef> {
    let find = |props: &[PropertyDef]| props.iter().find(|p| p.key == prop).cloned();
    if let Some(t) = scope.strip_prefix("type:") {
        pack.types
            .iter()
            .find(|d| d.key == t)
            .and_then(|d| find(&d.properties))
            .or_else(|| {
                pack.extends
                    .iter()
                    .find(|e| e.type_key == t)
                    .and_then(|e| find(&e.properties))
            })
    } else {
        let l = scope.strip_prefix("link:")?;
        pack.link_types
            .iter()
            .find(|d| d.key == l)
            .and_then(|d| find(&d.attributes))
    }
}

/// What the org changed of a shipped enum property's labels and colours
/// (D370): the entries that differ from the pack's, `null` for one the org
/// removed; `None` when nothing.
fn display_diff(org: &PropertyDef, pack: &PropertyDef) -> Option<Value> {
    let mut labels: Map<String, Value> = org
        .labels
        .iter()
        .filter(|(v, n)| pack.labels.get(*v) != Some(*n))
        .map(|(v, n)| (v.clone(), serde_json::to_value(n).unwrap_or_default()))
        .collect();
    labels.extend(
        pack.labels
            .keys()
            .filter(|v| !org.labels.contains_key(*v))
            .map(|v| (v.clone(), Value::Null)),
    );
    let mut colors: Map<String, Value> = org
        .colors
        .iter()
        .filter(|(v, c)| pack.colors.get(*v) != Some(*c))
        .map(|(v, c)| (v.clone(), Value::String(c.clone())))
        .collect();
    colors.extend(
        pack.colors
            .keys()
            .filter(|v| !org.colors.contains_key(*v))
            .map(|v| (v.clone(), Value::Null)),
    );
    (!labels.is_empty() || !colors.is_empty())
        .then(|| serde_json::json!({ "labels": labels, "colors": colors }))
}

/// Records in the owning pack's org layer the labels and colours the org
/// set on the enum properties of `scope` (D370), so that an upgrade of the
/// pack keeps them. Called by every write of a type or link type; a
/// property no installed pack ships is the org's own and needs nothing.
pub async fn record_display(
    db: &DatabaseConnection,
    org_id: i64,
    scope: &str,
    owner: Option<&str>,
    props: &[PropertyDef],
) -> Result<()> {
    let mut by_pack: std::collections::BTreeMap<String, Vec<&PropertyDef>> = Default::default();
    for p in props {
        if let Some(k) = p.pack.as_deref().or(owner) {
            by_pack.entry(k.to_string()).or_default().push(p);
        }
    }
    for (key, props) in by_pack {
        let Some(pack) = shipped().into_iter().find(|p| p.key == key) else {
            continue;
        };
        let row = db
            .query_one_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT display FROM packs WHERE org_id = $1 AND key = $2",
                vec![org_id.into(), key.clone().into()],
            ))
            .await?;
        let Some(mut layer) = row
            .and_then(|r| r.try_get::<Value>("", "display").ok())
            .map(|v| v.as_object().cloned().unwrap_or_default())
        else {
            continue;
        };
        let before = layer.clone();
        for p in props {
            let Some(shipped) = shipped_property(&pack, scope, &p.key) else {
                continue;
            };
            let at = format!("{scope}/{}", p.key);
            match display_diff(p, &shipped) {
                Some(d) => layer.insert(at, d),
                None => layer.remove(&at),
            };
        }
        if layer != before {
            db.execute_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "UPDATE packs SET display = $3, updated_at = now() WHERE org_id = $1 AND key = $2",
                vec![org_id.into(), key.into(), Value::Object(layer).into()],
            ))
            .await?;
        }
    }
    Ok(())
}

/// The org's labels and colours laid over the pack's (D370), `null`
/// removing the pack's; a value the pack no longer has is dropped.
fn apply_display(layer: &Value, scope: &str, props: &mut [PropertyDef]) {
    for p in props.iter_mut() {
        let Some(d) = layer.get(format!("{scope}/{}", p.key)) else {
            continue;
        };
        let pnex_core::ontology::PropertyKind::Enum { values } = &p.kind else {
            continue;
        };
        let values = values.clone();
        for (v, n) in d
            .get("labels")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if !values.contains(v) {
                continue;
            }
            if n.is_null() {
                p.labels.remove(v);
            } else if let Ok(n) = serde_json::from_value(n.clone()) {
                p.labels.insert(v.clone(), n);
            }
        }
        for (v, c) in d
            .get("colors")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if !values.contains(v) {
                continue;
            }
            match c.as_str() {
                Some(c) => p.colors.insert(v.clone(), c.to_string()),
                None => p.colors.remove(v),
            };
        }
    }
}

/// The org's display layer of `pack_key` (D370), `{}` when not installed.
async fn display_layer(db: &DatabaseConnection, org_id: i64, pack_key: &str) -> Result<Value> {
    Ok(db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT display FROM packs WHERE org_id = $1 AND key = $2",
            vec![org_id.into(), pack_key.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<Value>("", "display").ok())
        .unwrap_or_else(|| serde_json::json!({})))
}

/// Sets the org's values for settings of an installed pack; `null` returns
/// one to the pack's default. Unknown keys and wrong values are refused.
pub async fn set_settings(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
    input: &PackSettingsInput,
) -> Result<PackView> {
    let pack = shipped()
        .into_iter()
        .find(|p| p.key == key)
        .ok_or(OntologyError::NotFound)?;
    let mut layer = match layers(db, org_id).await?.remove(key) {
        Some(Value::Object(m)) => m,
        Some(_) => Map::new(),
        None => return Err(OntologyError::NotFound),
    };
    for (k, v) in &input.values {
        let Some(def) = pack.settings.iter().find(|s| &s.key == k) else {
            return Err(invalid(format!("values.{k}"), FIELD_INVALID));
        };
        if v.is_null() {
            layer.remove(k);
            continue;
        }
        if let Some(code) = def.check(v) {
            return Err(invalid(format!("values.{k}"), code));
        }
        layer.insert(k.clone(), v.clone());
    }
    let layer = Value::Object(layer);
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE packs SET settings = $3, updated_at = now() WHERE org_id = $1 AND key = $2",
        vec![org_id.into(), key.into(), layer.clone().into()],
    ))
    .await?;
    list(db, org_id)
        .await?
        .into_iter()
        .find(|p| p.key == key)
        .ok_or(OntologyError::NotFound)
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
            settings: Vec::new(),
            kept: Vec::new(),
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
    // A type of a pack that now builds on this one is taken over (D363).
    let owned_by_other = |key: &str, pk: Option<&String>| {
        s.knows(key)
            && pk.map(String::as_str) != Some(pack.key.as_str())
            && !pk.is_some_and(|o| hands_over(o, &pack.key))
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
    let display = display_layer(db, org_id, &pack.key).await?;
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
        let owner = s
            .types
            .iter()
            .find(|(d, _, _)| d.key == t.key)
            .and_then(|(_, _, p)| p.clone());
        if owner.is_some_and(|o| o != pack.key) {
            types::set_pack(db, org_id, &t.key, &pack.key).await?;
        }
        let current = types::get(db, org_id, &t.key).await?.version;
        let mut def = match s.object_type(&t.key) {
            Some(local) => keep_org_additions(t, local),
            None => t.clone(),
        };
        apply_display(&display, &format!("type:{}", t.key), &mut def.properties);
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
    install_automation(db, org_id, user_id, pack, &[]).await?;
    let kept = edited(db, org_id, pack).await?;
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
        settings: Vec::new(),
        kept,
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
    let display = display_layer(db, org_id, &pack.key).await?;
    for (i, e) in pack.extends.iter().enumerate() {
        let mut props = extended_properties(&s, pack, i)?;
        apply_display(&display, &format!("type:{}", e.type_key), &mut props);
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

/// Whether `owner` gives a key up to `taker` on upgrade: the shipped
/// `owner` now builds on `taker`, which took the key over (account-mapping.md
/// D343: `entreprises_fr` handing its shared fields to `entreprises`).
fn hands_over(owner: &str, taker: &str) -> bool {
    shipped()
        .iter()
        .find(|p| p.key == owner)
        .is_some_and(|p| requires_closure(p).iter().any(|k| k == taker))
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
    props.retain(|p| match p.pack.as_deref() {
        Some(o) if o == pack.key => false,
        Some(o) => !(hands_over(o, &pack.key) && e.properties.iter().any(|q| q.key == p.key)),
        None => true,
    });
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
/// none of that name. On an upgrade, one the org never edited (its latest
/// version is still the pack's) takes the new pack version, as do those
/// named in `take`; one the org edited is left alone (see `edited`). Flows
/// are drafts:
/// deploying them stays a human act (D123), so an upgraded flow runs at its
/// next deploy.
async fn install_automation(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    pack: &Pack,
    take: &[String],
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
        let note = format!("pack {} {}", pack.key, pack.version);
        let (id, version) = match existing {
            Some(row) => {
                let v = crate::services::functions::latest_version(db, row.id).await?;
                let pristine = v.as_ref().is_some_and(|v| {
                    v.code != f.code
                        && (pack_wrote(v.note.as_deref(), &pack.key) || take.contains(&f.name))
                });
                if !pristine {
                    (row.id, v)
                } else {
                    let id = row.id;
                    let (_, saved) = crate::services::functions::save_new_version(
                        db,
                        row,
                        &pnex_core::SaveFunctionVersion {
                            expected_version_number: v.map_or(0, |v| v.version_number),
                            code: Some(f.code.clone()),
                            name: None,
                            description: None,
                            note: Some(note),
                        },
                    )
                    .await
                    .map_err(|e| {
                        tracing::warn!(pack = %pack.key, function = %f.name, error = %e, "pack function upgrade refused");
                        invalid(format!("functions.{i}"), FIELD_INVALID)
                    })?;
                    (id, saved)
                }
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
                        note: Some(note),
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
    // name → id of the org's imports (ingestion nodes, D322).
    let imports: std::collections::HashMap<String, String> = db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT id::text AS id, name FROM object_imports WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| Some((r.try_get("", "name").ok()?, r.try_get("", "id").ok()?)))
        .collect();
    let author = format!("pack:{}", pack.key);
    let writer = crate::services::secrets::flow::GraphWriter {
        ring: None,
        user_id,
        can_write_secrets: false,
    };
    for (i, f) in pack.flows.iter().enumerate() {
        let existing = flows::Entity::find()
            .filter(flows::Column::OrgId.eq(org_id))
            .filter(flows::Column::Name.eq(f.name.trim()))
            .one(db)
            .await?;
        let latest = match &existing {
            Some(row) => {
                crate::models::_entities::flow_versions::Entity::find()
                    .filter(crate::models::_entities::flow_versions::Column::FlowId.eq(row.id))
                    .order_by_desc(crate::models::_entities::flow_versions::Column::VersionNumber)
                    .one(db)
                    .await?
            }
            None => None,
        };
        // An existing flow is upgraded only while its latest version is
        // still the pack's own (a human save takes its author away).
        if existing.is_some()
            && !take.contains(&f.name)
            && !latest
                .as_ref()
                .is_some_and(|v| v.author.as_deref() == Some(author.as_str()))
        {
            continue;
        }
        let mut graph = f.graph.clone();
        resolve_functions(&mut graph, |name| {
            known
                .get(name)
                .map(|(id, v, inputs, outputs)| (*id, *v, inputs.clone(), outputs.clone()))
        });
        resolve_imports(&mut graph, |name| imports.get(name).cloned());
        let graph: pnex_core::FlowGraph = serde_json::from_value(graph)
            .map_err(|_| invalid(format!("flows.{i}.graph"), FIELD_INVALID))?;
        let note = Some(f.note.clone()).filter(|n| !n.is_empty());
        let written = match (existing, latest) {
            (Some(row), Some(v)) => {
                if serde_json::to_value(&graph).ok().as_ref() == Some(&v.graph) {
                    continue;
                }
                crate::services::flow::append_version(
                    db,
                    &row,
                    v.version_number,
                    &graph,
                    None,
                    Some(author.clone()),
                    note,
                    &writer,
                )
                .await
            }
            _ => {
                crate::services::flow::create_flow(
                    db,
                    org_id,
                    &f.name,
                    &graph,
                    None,
                    Some(author.clone()),
                    note,
                    &writer,
                )
                .await
            }
        };
        written.map_err(|e| {
            tracing::warn!(pack = %pack.key, flow = %f.name, error = ?e, "pack flow refused");
            invalid(format!("flows.{i}"), FIELD_INVALID)
        })?;
    }
    Ok(())
}

/// Whether a version note was written by `key`'s install (`pack <key> <v>`).
fn pack_wrote(note: Option<&str>, key: &str) -> bool {
    note.and_then(|n| n.strip_prefix("pack "))
        .and_then(|n| n.strip_prefix(key))
        .is_some_and(|rest| rest.starts_with(' '))
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

/// Ingestion nodes naming an import (`import_name`, no id) get the id of
/// the org's import of that name.
pub fn resolve_imports(graph: &mut serde_json::Value, lookup: impl Fn(&str) -> Option<String>) {
    let Some(nodes) = graph.get_mut("nodes").and_then(|n| n.as_array_mut()) else {
        return;
    };
    for n in nodes.iter_mut().filter(|n| n["kind"] == "ingest") {
        let Some(c) = n.get_mut("config").and_then(|c| c.as_object_mut()) else {
            continue;
        };
        let Some(name) = c.remove("import_name") else {
            continue;
        };
        if let Some(id) = name.as_str().and_then(&lookup) {
            c.insert("import_id".into(), id.into());
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
        Some((local, v, Some(pk))) if pk == pack_key || hands_over(pk, pack_key) => {
            let mut def = keep_org_link_additions(l, local);
            let display = display_layer(db, org_id, pack_key).await?;
            apply_display(&display, &format!("link:{}", l.key), &mut def.attributes);
            link_types::update(
                db,
                org_id,
                &l.key,
                &LinkTypeInput {
                    def,
                    expected_version: Some(*v),
                },
            )
            .await?;
            if pk != pack_key {
                link_types::set_pack(db, org_id, &l.key, pack_key).await?;
            }
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
        settings: Vec::new(),
    };
    serde_saphyr::to_string(&pack)
        .map_err(|e| OntologyError::Db(sea_orm::DbErr::Custom(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_org_removal_of_a_pack_colour_survives_the_upgrade() {
        let def = |v: Value| -> PropertyDef { serde_json::from_value(v).unwrap() };
        let pack = def(
            serde_json::json!({"key": "etat", "kind": "enum", "values": ["ok", "ko"],
            "colors": {"ok": "green", "ko": "red"}, "labels": {"ok": {"fr": "Bon"}}}),
        );
        let org = def(
            serde_json::json!({"key": "etat", "kind": "enum", "values": ["ok", "ko"],
            "colors": {"ko": "amber"}}),
        );
        let d = display_diff(&org, &pack).unwrap();
        let layer = serde_json::json!({ "type:t/etat": d });
        let mut props = vec![pack];
        apply_display(&layer, "type:t", &mut props);
        assert_eq!(props[0].colors, org.colors);
        assert!(props[0].labels.is_empty());
    }

    #[test]
    fn shipped_packs_are_valid() {
        let all = shipped();
        // A key belongs to one pack: two packs defining it cannot both install.
        let mut owner = std::collections::HashMap::new();
        for p in &all {
            let keys = p.types.iter().map(|t| &t.key);
            for k in keys.chain(p.link_types.iter().map(|l| &l.key)) {
                if let Some(other) = owner.insert(k.clone(), &p.key) {
                    panic!("{k}: defined by {other} and {}", p.key);
                }
            }
        }
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
            // A template filled from an object links back to it (D340).
            for t in &p.page_templates {
                if let Some(l) = &t.link {
                    let subject = t
                        .subject
                        .as_deref()
                        .expect("a linked template has a subject");
                    let def = deps
                        .iter()
                        .flat_map(|d| d.link_types.iter())
                        .chain(p.link_types.iter())
                        .find(|d| &d.key == l)
                        .unwrap_or_else(|| panic!("{}: template {} link {l}", p.key, t.key));
                    assert!(
                        def.allows(l, "doc", subject),
                        "{}: {l} doc → {subject}",
                        p.key
                    );
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
    fn only_the_packs_own_note_counts_as_unedited() {
        assert!(super::pack_wrote(
            Some("pack presence_web 1.2.0"),
            "presence_web"
        ));
        assert!(!super::pack_wrote(
            Some("pack presence_webx 1.0.0"),
            "presence_web"
        ));
        assert!(!super::pack_wrote(Some("my fix"), "presence_web"));
        assert!(!super::pack_wrote(None, "presence_web"));
    }

    #[test]
    fn export_round_trips_through_parse() {
        let p = shipped().remove(0);
        let yaml = serde_saphyr::to_string(&p).unwrap();
        assert_eq!(parse(&yaml).unwrap(), p);
    }
}
