//! Objects (D177): org-type objects live entirely in `objects`; system
//! objects have their identity row (written by triggers) and carry the
//! org's extra properties there (D176), their native content staying in
//! place. Every write is validated against the current type definition,
//! locked by the type's write role (D188) and journaled (D184).

use std::collections::HashMap;

use pnex_core::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::ontology::api::{ObjectInput, ObjectView, TITLE_MAX};
use pnex_core::ontology::schema::check_values;
use pnex_core::ontology::{
    ObjectTypeDef, PropertyKind, CASE_CONCLUDED, CASE_CONCLUSION_PAGE, CASE_STATUS, KIND_CASE,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set, TransactionTrait,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::changes::{self, Change};
use super::{role_allows, schema, Actor, OntologyError, OrgSchema, Result};
use crate::models::_entities::objects;

pub fn view(o: &objects::Model, s: &OrgSchema) -> ObjectView {
    ObjectView {
        id: o.id.to_string(),
        type_key: o.type_key.clone(),
        title: o.title.clone(),
        native_id: o.native_id.clone(),
        system: s.object_type(&o.type_key).is_some_and(|t| t.native()),
        properties: o.properties.as_object().cloned().unwrap_or_default(),
        type_version: o.type_version,
        version: o.version,
        source_ref: o.source_ref.clone(),
        valid_from: o.valid_from.to_rfc3339(),
        valid_to: o.valid_to.map(|t| t.to_rfc3339()),
        created_at: o.created_at.to_rfc3339(),
        updated_at: o.updated_at.to_rfc3339(),
    }
}

/// An object of the org by id (archived included).
pub async fn find<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<objects::Model> {
    objects::Entity::find_by_id(id)
        .filter(objects::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)
}

/// The identity of a system or org object addressed the D42 way.
pub async fn find_by_native<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_key: &str,
    native_id: &str,
) -> Result<objects::Model> {
    objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::TypeKey.eq(type_key))
        .filter(objects::Column::NativeId.eq(native_id))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)
}

pub async fn get(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<ObjectView> {
    let s = schema(db, org_id).await?;
    Ok(view(&find(db, org_id, id).await?, &s))
}

fn check_title(title: &str) -> Result<String> {
    let t = title.trim();
    if t.is_empty() {
        return Err(super::invalid("title", FIELD_REQUIRED));
    }
    if t.chars().count() > TITLE_MAX {
        return Err(super::invalid(
            "title",
            format!("{FIELD_MAX_LENGTH}:{TITLE_MAX}"),
        ));
    }
    Ok(t.to_string())
}

/// Validates the property document, then that every `ref` names a live
/// object of the expected type in the org (R1: never another org's).
pub(super) async fn check_properties<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    def: &ObjectTypeDef,
    props: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let out = check_values(&def.properties, props)?;
    for p in &def.properties {
        let PropertyKind::Ref { to_type } = &p.kind else {
            continue;
        };
        let Some(id) = out.get(&p.key).and_then(Value::as_str) else {
            continue;
        };
        let ok = match Uuid::parse_str(id) {
            Ok(id) => find(db, org_id, id)
                .await
                .is_ok_and(|o| &o.type_key == to_type && o.valid_to.is_none()),
            Err(_) => false,
        };
        if !ok {
            return Err(super::invalid(p.key.clone(), FIELD_INVALID));
        }
    }
    Ok(out)
}

/// D302: a unique property names at most one live object per org × type
/// × value; `except` is the object being updated.
// shortcut: checked before the write, not by an index: two concurrent
// writers of the same new key can both pass; imports serialize per org.
pub(super) async fn check_unique<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    def: &ObjectTypeDef,
    props: &Map<String, Value>,
    except: Option<Uuid>,
) -> Result<()> {
    for p in def.properties.iter().filter(|p| p.unique) {
        let Some(v) = props.get(&p.key) else {
            continue;
        };
        if find_by_key(db, org_id, &def.key, &p.key, v)
            .await?
            .is_some_and(|o| Some(o.id) != except)
        {
            return Err(super::invalid(p.key.clone(), "duplicate"));
        }
    }
    Ok(())
}

/// The live object of `type_key` whose unique property `key` is `value`.
pub async fn find_by_key<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_key: &str,
    key: &str,
    value: &Value,
) -> Result<Option<objects::Model>> {
    let text = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    // Containment uses the GIN index (`->>` alone scans every object of the
    // type); the string and the scalar spelling of the text are both tried,
    // `->>` keeps the match exact.
    let as_string = serde_json::json!({ key: text });
    let as_scalar = match serde_json::from_str::<Value>(&text) {
        Ok(v @ (Value::Number(_) | Value::Bool(_))) => serde_json::json!({ key: v }),
        _ => as_string.clone(),
    };
    Ok(objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::TypeKey.eq(type_key))
        .filter(objects::Column::ValidTo.is_null())
        .filter(Expr::cust_with_values(
            "(properties @> $1 OR properties @> $2)",
            [as_string, as_scalar],
        ))
        .filter(Expr::cust_with_values(
            "properties ->> $1 = $2",
            [key.to_string(), text],
        ))
        .one(db)
        .await?)
}

/// Investigation pack (cases.md K7): `claim.status = corroborated` needs
/// at least two `sourced_by` pieces of distinct origins (a piece's source
/// URL, else the object it wraps, else the piece itself), unless a
/// `corroboration_note` justifies the exception (K7: the rule can be
/// overridden, never silently).
async fn check_claim<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_key: &str,
    claim: Option<Uuid>,
    props: &Map<String, Value>,
) -> Result<()> {
    if type_key != CLAIM || props.get("status").and_then(Value::as_str) != Some(CORROBORATED) {
        return Ok(());
    }
    let Some(claim) = claim else {
        return match claim_note(props) {
            Some(_) => Ok(()),
            None => Err(OntologyError::ClaimUnsourced),
        };
    };
    let row = db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT count(DISTINCT coalesce(p.properties ->> 'source_url', w.target_object_id::text, p.id::text)) AS n \
             FROM resource_edges s JOIN objects p ON p.id = s.target_object_id \
             LEFT JOIN resource_edges w ON w.source_object_id = p.id AND w.relation = 'wraps' AND w.valid_to IS NULL \
             WHERE s.org_id = $1 AND s.relation = 'sourced_by' AND s.valid_to IS NULL \
               AND s.source_object_id = $2 AND p.valid_to IS NULL",
            [org_id.into(), claim.into()],
        ))
        .await?;
    let n: i64 = row.and_then(|r| r.try_get("", "n").ok()).unwrap_or(0);
    if n < 2 && claim_note(props).is_none() {
        return Err(OntologyError::ClaimUnsourced);
    }
    Ok(())
}

/// The written justification of a claim corroborated below two origins.
pub(crate) fn claim_note(props: &Map<String, Value>) -> Option<String> {
    props
        .get(CLAIM_NOTE)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

const CLAIM: &str = "claim";
const CLAIM_NOTE: &str = "corroboration_note";
const CORROBORATED: &str = "corroborated";

fn require_role(def: &ObjectTypeDef, actor: &Actor) -> Result<()> {
    if role_allows(&actor.role, def.write_role) {
        Ok(())
    } else {
        Err(OntologyError::WriteForbidden)
    }
}

/// Creates an org-type object. System objects are created by their own
/// pages (device wizard, media upload…): the identity follows. Works
/// inside a transaction (the journal entry is queued at once).
pub async fn create<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    input: &ObjectInput,
) -> Result<ObjectView> {
    create_with(db, &schema(db, org_id).await?, org_id, actor, input).await
}

/// `create` on a schema already loaded (a bulk import loads it once).
pub async fn create_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    input: &ObjectInput,
) -> Result<ObjectView> {
    let Some(def) = s.object_type(&input.type_key) else {
        return Err(super::invalid("type_key", FIELD_INVALID));
    };
    if def.native() {
        return Err(OntologyError::SystemReadOnly);
    }
    require_role(def, actor)?;
    let title = check_title(&input.title)?;
    let props = check_properties(db, org_id, def, &input.properties).await?;
    check_unique(db, org_id, def, &props, None).await?;
    check_claim(db, org_id, &def.key, None, &props).await?;
    let id = Uuid::new_v4();
    let row = objects::ActiveModel {
        id: Set(id),
        org_id: Set(org_id),
        type_key: Set(def.key.clone()),
        type_version: Set(s.type_version(&def.key)),
        title: Set(title),
        native_id: Set(id.to_string()),
        properties: Set(Value::Object(props)),
        source_ref: Set(Some(actor.source_ref.clone())),
        version: Set(1),
        ..Default::default()
    }
    .insert(db)
    .await?;
    let v = view(&row, s);
    super::recurrence::refresh_row_logged(db, &row, def).await;
    changes::submit(Change::object(
        org_id,
        "object.create",
        &actor.source_ref,
        Value::Null,
        &v,
    ));
    Ok(v)
}

/// Updates title and properties on `expected_version` (409 otherwise).
/// For a system object only the extra properties are written.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    input: &ObjectInput,
) -> Result<ObjectView> {
    crate::services::ontology::changes::deferred(async {
        // One transaction: a failure after the write leaves nothing behind its error.
        let txn = db.begin().await?;
        let (view, conclusion) = update_in(&txn, org_id, actor, id, input).await?;
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        if let Some(page) = conclusion {
            super::cases::on_concluded(db, org_id, actor, &page).await;
        }
        Ok(view)
    })
    .await
}

/// `update` inside a transaction; returns the conclusion page of a case
/// that just concluded (its versioning runs after the commit).
pub async fn update_in<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    input: &ObjectInput,
) -> Result<(ObjectView, Option<String>)> {
    update_in_with(db, &schema(db, org_id).await?, org_id, actor, id, input).await
}

/// `update_in` on a schema already loaded (a bulk import loads it once).
pub async fn update_in_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    input: &ObjectInput,
) -> Result<(ObjectView, Option<String>)> {
    let row = find(db, org_id, id).await?;
    if row.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
    let Some(def) = s.object_type(&row.type_key) else {
        return Err(OntologyError::NotFound);
    };
    require_role(def, actor)?;
    let Some(expected) = input.expected_version else {
        return Err(super::invalid("expected_version", FIELD_REQUIRED));
    };
    let title = if def.native() {
        row.title.clone()
    } else {
        check_title(&input.title)?
    };
    let props = check_properties(db, org_id, def, &input.properties).await?;
    check_unique(db, org_id, def, &props, Some(id)).await?;
    // Checked when the status changes, not on every later edit.
    let status_changed = row.properties.get("status") != props.get("status");
    let concluded = status_changed
        && def.key == KIND_CASE
        && props.get(CASE_STATUS).and_then(Value::as_str) == Some(CASE_CONCLUDED);
    let conclusion = props
        .get(CASE_CONCLUSION_PAGE)
        .and_then(Value::as_str)
        .filter(|_| concluded)
        .map(String::from);
    if status_changed {
        check_claim(db, org_id, &def.key, Some(id), &props).await?;
    }
    let now = sea_orm::prelude::DateTimeWithTimeZone::from(chrono::Utc::now());
    let done = objects::Entity::update_many()
        .col_expr(objects::Column::Title, Expr::value(title))
        .col_expr(
            objects::Column::Properties,
            Expr::value(Value::Object(props)),
        )
        .col_expr(
            objects::Column::TypeVersion,
            Expr::value(s.type_version(&def.key)),
        )
        .col_expr(
            objects::Column::SourceRef,
            Expr::value(actor.source_ref.clone()),
        )
        .col_expr(objects::Column::Version, Expr::value(expected + 1))
        .col_expr(objects::Column::UpdatedAt, Expr::value(now))
        .filter(objects::Column::Id.eq(id))
        .filter(objects::Column::Version.eq(expected))
        .exec(db)
        .await?;
    if done.rows_affected != 1 {
        return Err(OntologyError::VersionConflict {
            current: row.version,
        });
    }
    let before = view(&row, s);
    let updated = find(db, org_id, id).await?;
    let after = view(&updated, s);
    super::recurrence::refresh_row_logged(db, &updated, def).await;
    changes::submit(Change::object(
        org_id,
        "object.update",
        &actor.source_ref,
        serde_json::to_value(&before).unwrap_or_default(),
        &after,
    ));
    Ok((after, conclusion))
}

/// Archives an org-type object: its validity closes, its open links close
/// with it (D179), nothing is deleted. System objects are deleted by their
/// own pages, which closes their identity.
pub async fn archive<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
) -> Result<()> {
    let s = schema(db, org_id).await?;
    let row = find(db, org_id, id).await?;
    if row.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
    let Some(def) = s.object_type(&row.type_key) else {
        return Err(OntologyError::NotFound);
    };
    if def.native() {
        return Err(OntologyError::SystemReadOnly);
    }
    require_role(def, actor)?;
    let now = sea_orm::prelude::DateTimeWithTimeZone::from(chrono::Utc::now());
    objects::Entity::update_many()
        .col_expr(objects::Column::ValidTo, Expr::value(now))
        .col_expr(objects::Column::UpdatedAt, Expr::value(now))
        .col_expr(
            objects::Column::SourceRef,
            Expr::value(actor.source_ref.clone()),
        )
        .filter(objects::Column::Id.eq(id))
        .exec(db)
        .await?;
    // Labels and containment go like for any deleted resource (D42 purge,
    // links closed rather than deleted).
    crate::services::resources::purge_for(db, org_id, &row.type_key, &row.native_id).await?;
    super::recurrence::refresh_logged(db, org_id, id).await;
    let before = view(&row, &s);
    changes::submit(Change {
        org_id,
        action: "object.archive".into(),
        object_id: row.id.to_string(),
        type_key: row.type_key.clone(),
        source_ref: actor.source_ref.clone(),
        before: changes::Doc::of(&before),
        after: Value::Null.into(),
    });
    Ok(())
}

// ─────────────────────────── bulk (data-pipelines.md D347) ───────────────────────────

/// Rows per bulk statement (one JSON parameter each).
pub(super) const BULK_CHUNK: usize = 5_000;

fn bulk_stmt(sql: &str, values: Vec<sea_orm::Value>) -> sea_orm::Statement {
    sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

/// Text of a unique value, as `find_by_key` compares it.
fn key_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `check_properties` for many documents: every `ref` checked with one
/// query per target type and batch.
async fn check_refs_many<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    items: &[(&ObjectTypeDef, &Map<String, Value>)],
) -> Result<()> {
    let mut wanted: std::collections::BTreeMap<&str, Vec<(Uuid, &str)>> = Default::default();
    for (def, props) in items {
        for p in &def.properties {
            let PropertyKind::Ref { to_type } = &p.kind else {
                continue;
            };
            let Some(raw) = props.get(&p.key).and_then(Value::as_str) else {
                continue;
            };
            let id =
                Uuid::parse_str(raw).map_err(|_| super::invalid(p.key.clone(), FIELD_INVALID))?;
            wanted
                .entry(to_type)
                .or_default()
                .push((id, p.key.as_str()));
        }
    }
    for (to_type, refs) in wanted {
        for chunk in refs.chunks(BULK_CHUNK) {
            let ids: Vec<Uuid> = chunk.iter().map(|(id, _)| *id).collect();
            let found: std::collections::HashSet<Uuid> = db
                .query_all_raw(bulk_stmt(
                    "SELECT id FROM objects WHERE org_id = $1 AND type_key = $2 \
                       AND valid_to IS NULL AND id = ANY($3)",
                    vec![org_id.into(), to_type.into(), ids.into()],
                ))
                .await?
                .iter()
                .filter_map(|r| r.try_get::<Uuid>("", "id").ok())
                .collect();
            if let Some((_, key)) = chunk.iter().find(|(id, _)| !found.contains(id)) {
                return Err(super::invalid(key.to_string(), FIELD_INVALID));
            }
        }
    }
    Ok(())
}

/// `check_unique` for many documents (`except` = the object updated):
/// twice in the batch or held by another live object = `duplicate`.
async fn check_unique_many<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    items: &[(&ObjectTypeDef, &Map<String, Value>, Option<Uuid>)],
) -> Result<()> {
    let mut groups: std::collections::BTreeMap<(&str, &str), HashMap<String, Option<Uuid>>> =
        Default::default();
    for (def, props, except) in items {
        for p in def.properties.iter().filter(|p| p.unique) {
            let Some(v) = props.get(&p.key) else {
                continue;
            };
            let g = groups
                .entry((def.key.as_str(), p.key.as_str()))
                .or_default();
            if g.insert(key_text(v), *except).is_some() {
                return Err(super::invalid(p.key.clone(), "duplicate"));
            }
        }
    }
    for ((type_key, key), values) in groups {
        let texts: Vec<String> = values.keys().cloned().collect();
        for chunk in texts.chunks(1_000) {
            let rows = db
                .query_all_raw(bulk_stmt(
                    "SELECT id, properties ->> $3 AS k FROM objects WHERE org_id = $1 \
                       AND type_key = $2 AND valid_to IS NULL AND properties ->> $3 = ANY($4)",
                    vec![
                        org_id.into(),
                        type_key.into(),
                        key.into(),
                        chunk.to_vec().into(),
                    ],
                ))
                .await?;
            for r in rows {
                let (Ok(id), Ok(k)) = (r.try_get::<Uuid>("", "id"), r.try_get::<String>("", "k"))
                else {
                    continue;
                };
                if values.get(&k).is_some_and(|except| *except != Some(id)) {
                    return Err(super::invalid(key.to_string(), "duplicate"));
                }
            }
        }
    }
    Ok(())
}

/// Recurrence occurrences only exist for types with a recurrence property.
// shortcut: an updated object of a type that lost its recurrence property
// keeps its old occurrences until the daily sweep; per-row refresh if it matters.
async fn refresh_recurrences<C: ConnectionTrait>(db: &C, s: &OrgSchema, rows: &[objects::Model]) {
    for row in rows {
        if let Some(def) = s.object_type(&row.type_key) {
            if !super::recurrence::recurrence_keys(def).is_empty() {
                super::recurrence::refresh_row_logged(db, row, def).await;
            }
        }
    }
}

/// `create_with` for many objects (imports): the same checks, a few
/// queries per type and batch instead of several per object. Claims keep
/// the one-by-one path (their rule reads links). Ids in input order.
pub async fn create_many_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    inputs: &[ObjectInput],
) -> Result<Vec<Uuid>> {
    let mut ids = vec![Uuid::nil(); inputs.len()];
    let mut checked: Vec<(usize, &ObjectTypeDef, String, Map<String, Value>)> = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let Some(def) = s.object_type(&input.type_key) else {
            return Err(super::invalid("type_key", FIELD_INVALID));
        };
        if def.native() {
            return Err(OntologyError::SystemReadOnly);
        }
        require_role(def, actor)?;
        if def.key == CLAIM {
            let v = create_with(db, s, org_id, actor, input).await?;
            ids[i] = Uuid::parse_str(&v.id).map_err(|_| OntologyError::NotFound)?;
            continue;
        }
        let title = check_title(&input.title)?;
        let props = check_values(&def.properties, &input.properties)?;
        checked.push((i, def, title, props));
    }
    let docs: Vec<_> = checked.iter().map(|(_, d, _, p)| (*d, p)).collect();
    check_refs_many(db, org_id, &docs).await?;
    let docs: Vec<_> = checked.iter().map(|(_, d, _, p)| (*d, p, None)).collect();
    check_unique_many(db, org_id, &docs).await?;
    for chunk in checked.chunks(BULK_CHUNK) {
        let mut recs = Vec::with_capacity(chunk.len());
        for (i, def, title, props) in chunk {
            ids[*i] = Uuid::new_v4();
            recs.push(serde_json::json!({
                "id": ids[*i],
                "type_key": def.key,
                "type_version": s.type_version(&def.key),
                "title": title,
                "properties": props,
            }));
        }
        let rows = objects::Entity::find()
            .from_raw_sql(bulk_stmt(
                "INSERT INTO objects (id, org_id, type_key, type_version, title, native_id, \
                   properties, source_ref, version) \
                 SELECT u.id, $1, u.type_key, u.type_version, u.title, u.id::text, u.properties, $2, 1 \
                 FROM jsonb_to_recordset($3::jsonb) AS u(id uuid, type_key text, type_version int, \
                   title text, properties jsonb) \
                 RETURNING *",
                vec![
                    org_id.into(),
                    actor.source_ref.clone().into(),
                    Value::Array(recs).into(),
                ],
            ))
            .all(db)
            .await?;
        refresh_recurrences(db, s, &rows).await;
        for row in &rows {
            changes::submit(Change::object(
                org_id,
                "object.create",
                &actor.source_ref,
                Value::Null,
                &view(row, s),
            ));
        }
    }
    Ok(ids)
}

/// `update_in_with` for many objects on their `expected_version`
/// (imports): one statement per batch. Claims and cases keep the
/// one-by-one path (status rules, conclusion).
pub async fn update_many_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    inputs: &[(Uuid, ObjectInput)],
) -> Result<()> {
    let mut rows: HashMap<Uuid, objects::Model> = HashMap::with_capacity(inputs.len());
    for chunk in inputs.chunks(BULK_CHUNK) {
        rows.extend(super::links::refs(db, org_id, chunk.iter().map(|(id, _)| *id)).await?);
    }
    let mut checked: Vec<(
        &objects::Model,
        &ObjectTypeDef,
        i32,
        String,
        Map<String, Value>,
    )> = Vec::new();
    for (id, input) in inputs {
        let row = rows.get(id).ok_or(OntologyError::NotFound)?;
        if row.valid_to.is_some() {
            return Err(OntologyError::NotFound);
        }
        let Some(def) = s.object_type(&row.type_key) else {
            return Err(OntologyError::NotFound);
        };
        require_role(def, actor)?;
        let Some(expected) = input.expected_version else {
            return Err(super::invalid("expected_version", FIELD_REQUIRED));
        };
        if def.key == CLAIM || def.key == KIND_CASE {
            update_in_with(db, s, org_id, actor, *id, input).await?;
            continue;
        }
        if row.version != expected {
            return Err(OntologyError::VersionConflict {
                current: row.version,
            });
        }
        let title = if def.native() {
            row.title.clone()
        } else {
            check_title(&input.title)?
        };
        let props = check_values(&def.properties, &input.properties)?;
        checked.push((row, def, expected, title, props));
    }
    let docs: Vec<_> = checked.iter().map(|(_, d, _, _, p)| (*d, p)).collect();
    check_refs_many(db, org_id, &docs).await?;
    let docs: Vec<_> = checked
        .iter()
        .map(|(r, d, _, _, p)| (*d, p, Some(r.id)))
        .collect();
    check_unique_many(db, org_id, &docs).await?;
    for chunk in checked.chunks(BULK_CHUNK) {
        let recs: Vec<Value> = chunk
            .iter()
            .map(|(row, def, expected, title, props)| {
                serde_json::json!({
                    "id": row.id,
                    "version": expected,
                    "title": title,
                    "properties": props,
                    "type_version": s.type_version(&def.key),
                })
            })
            .collect();
        let updated = objects::Entity::find()
            .from_raw_sql(bulk_stmt(
                "UPDATE objects o SET title = u.title, properties = u.properties, \
                   type_version = u.type_version, source_ref = $2, version = u.version + 1, \
                   updated_at = now() \
                 FROM jsonb_to_recordset($3::jsonb) AS u(id uuid, version int, title text, \
                   properties jsonb, type_version int) \
                 WHERE o.org_id = $1 AND o.id = u.id AND o.version = u.version \
                 RETURNING o.*",
                vec![
                    org_id.into(),
                    actor.source_ref.clone().into(),
                    Value::Array(recs).into(),
                ],
            ))
            .all(db)
            .await?;
        if updated.len() != chunk.len() {
            let done: std::collections::HashSet<Uuid> = updated.iter().map(|o| o.id).collect();
            let row = chunk
                .iter()
                .find(|(r, ..)| !done.contains(&r.id))
                .map(|c| c.0);
            return Err(OntologyError::VersionConflict {
                current: row.map_or(0, |r| r.version),
            });
        }
        refresh_recurrences(db, s, &updated).await;
        for after in &updated {
            let before = rows.get(&after.id).map(|b| view(b, s));
            changes::submit(Change::object(
                org_id,
                "object.update",
                &actor.source_ref,
                serde_json::to_value(&before).unwrap_or_default(),
                &view(after, s),
            ));
        }
    }
    Ok(())
}

/// Colour of an object: its first coloured enum property that holds a
/// value (account-mapping.md D339).
pub fn color_of(s: &OrgSchema, o: &objects::Model) -> Option<String> {
    s.object_type(&o.type_key)?
        .properties
        .iter()
        .filter(|p| !p.colors.is_empty())
        .find_map(|p| p.color_of(o.properties.get(&p.key)?))
        .map(str::to_string)
}

/// Colour of each system object of `type_key` (by native id). Nothing is
/// read when the type has no coloured property.
pub async fn colors_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_key: &str,
    native_ids: &[String],
) -> Result<HashMap<String, String>> {
    if native_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let s = schema(db, org_id).await?;
    let colored = s
        .object_type(type_key)
        .is_some_and(|d| d.properties.iter().any(|p| !p.colors.is_empty()));
    if !colored {
        return Ok(HashMap::new());
    }
    let rows = objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::TypeKey.eq(type_key))
        .filter(objects::Column::NativeId.is_in(native_ids.to_vec()))
        .filter(objects::Column::ValidTo.is_null())
        .all(db)
        .await?;
    Ok(rows
        .iter()
        .filter_map(|o| Some((o.native_id.clone(), color_of(&s, o)?)))
        .collect())
}
