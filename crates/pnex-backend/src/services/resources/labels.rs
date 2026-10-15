//! Labels de la couche d'organisation (D42) — classification petit et
//! requêtable. 1 doc JSONB par ressource (`resource_labels`, unique), GIN
//! `jsonb_path_ops` sur PG pour le filtrage `@>`.
//!
//! Labels are a plain search aid (D218): an object carries its own labels,
//! nothing is inherited through folders or links. Filtering is one `@>` /
//! `?` JSONB query (GIN).

use std::collections::{BTreeMap, HashMap, HashSet};

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, Set,
};
use serde_json::Value;

use crate::models::_entities::resource_labels;
use crate::services::resources::ResourceError;
use pnex_core::resources::{valid_label_name, valid_label_value, LabelSet, LABELS_MAX};

// ─────────────────────────── helpers ───────────────────────────

fn doc_to_set(v: &Value) -> LabelSet {
    let mut out = LabelSet::new();
    if let Value::Object(map) = v {
        for (k, val) in map {
            out.insert(k.clone(), val.as_str().map(str::to_string));
        }
    }
    out
}

fn set_to_doc(labels: &LabelSet) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in labels {
        map.insert(
            k.clone(),
            v.as_deref()
                .map(|s| Value::String(s.to_string()))
                .unwrap_or(Value::Null),
        );
    }
    Value::Object(map)
}

/// Validation complète d'un doc de labels (nom normalisé, valeurs bornées,
/// cap [`LABELS_MAX`]) — l'écriture est tout-ou-rien.
pub fn validate_set(labels: &LabelSet) -> Result<(), ResourceError> {
    if labels.len() > LABELS_MAX {
        return Err(ResourceError::LabelInvalid(format!(
            "trop de labels ({}) — cap {}",
            labels.len(),
            LABELS_MAX
        )));
    }
    for (name, value) in labels {
        if !valid_label_name(name) {
            return Err(ResourceError::LabelInvalid(format!(
                "nom de label invalide : {name}"
            )));
        }
        if let Some(v) = value {
            if !valid_label_value(v) {
                return Err(ResourceError::LabelInvalid(format!(
                    "valeur de label invalide pour « {name} »"
                )));
            }
        }
    }
    Ok(())
}

// ─────────────────────────── CRUD ───────────────────────────

/// Doc de labels d'une ressource, `None` = jamais étiquetée (pas de ligne).
pub async fn get_labels(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Option<LabelSet>, DbErr> {
    let row = resource_labels::Entity::find()
        .filter(resource_labels::Column::OrgId.eq(org_id))
        .filter(resource_labels::Column::ResourceKind.eq(kind))
        .filter(resource_labels::Column::ResourceId.eq(id))
        .one(db)
        .await?;
    Ok(row.and_then(|r| r.labels).map(|v| doc_to_set(&v)))
}

/// Remplace **tout** le doc (PUT) — upsert atomique, tout-ou-rien.
pub async fn put_labels(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
    labels: &LabelSet,
    updated_by: Option<i64>,
) -> Result<LabelSet, ResourceError> {
    validate_set(labels)?;
    let doc = set_to_doc(labels);
    let row_id = find_row_id(db, org_id, kind, id)
        .await
        .map_err(|_| ResourceError::Db)?;
    match row_id {
        Some(row_id) => {
            let mut am = resource_labels::ActiveModel {
                id: Set(row_id),
                ..Default::default()
            };
            am.labels = Set(Some(doc));
            am.updated_by = Set(updated_by);
            am.updated_at = Set(chrono::Utc::now().into());
            am.update(db).await.map_err(|_| ResourceError::Db)?;
        }
        None => {
            let am = resource_labels::ActiveModel {
                org_id: Set(org_id),
                resource_kind: Set(kind.to_string()),
                resource_id: Set(id.to_string()),
                labels: Set(Some(doc)),
                updated_by: Set(updated_by),
                ..Default::default()
            };
            am.insert(db).await.map_err(|_| ResourceError::Db)?;
        }
    }
    Ok(labels.clone())
}

async fn find_row_id(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Option<i64>, DbErr> {
    Ok(resource_labels::Entity::find()
        .filter(resource_labels::Column::OrgId.eq(org_id))
        .filter(resource_labels::Column::ResourceKind.eq(kind))
        .filter(resource_labels::Column::ResourceId.eq(id))
        .one(db)
        .await?
        .map(|r| r.id))
}
// ─────────────────────────── label-only kinds ───────────────────────────

/// (table, title column) of a label-only kind (D218); `None` for any
/// other kind. Static map: names never come from the request.
fn label_only_table(kind: &str) -> Option<(&'static str, &'static str)> {
    use pnex_core::resources as r;
    Some(match kind {
        k if k == r::KIND_FUNCTION => ("functions", "name"),
        k if k == r::KIND_FIRMWARE_PROJECT => ("firmware_projects", "name"),
        k if k == r::KIND_CONTROL => ("controls", "label"),
        k if k == r::KIND_MEDIA_STREAM => ("media_streams", "name"),
        k if k == r::KIND_ML_MODEL => ("ml_models", "name"),
        k if k == r::KIND_NOTIFY_CHANNEL => ("notify_channels", "name"),
        k if k == r::KIND_NOTIFY_TEMPLATE => ("notify_templates", "name"),
        k if k == r::KIND_ANNOTATION_LAYER => ("annotation_layers", "name"),
        _ => return None,
    })
}

/// Whether `id` is a row of the org for a label-only kind: `None` when the
/// kind is not label-only (the caller checks the ontology registry).
pub async fn label_only_exists(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Option<bool>, DbErr> {
    let Some((table, _)) = label_only_table(kind) else {
        return Ok(None);
    };
    let sql = format!("SELECT 1 AS hit FROM {table} WHERE org_id = $1 AND id::text = $2 LIMIT 1");
    let stmt = sea_orm::Statement::from_sql_and_values(
        db.get_database_backend(),
        &sql,
        vec![org_id.into(), id.to_string().into()],
    );
    Ok(Some(db.query_one_raw(stmt).await?.is_some()))
}

/// Display name of a label-only row: `Ok(None)` when the kind is not
/// label-only or the row is gone.
pub async fn label_only_name(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Option<String>, DbErr> {
    let Some((table, title)) = label_only_table(kind) else {
        return Ok(None);
    };
    let sql =
        format!("SELECT {title}::text AS title FROM {table} WHERE org_id = $1 AND id::text = $2");
    let stmt = sea_orm::Statement::from_sql_and_values(
        db.get_database_backend(),
        &sql,
        vec![org_id.into(), id.to_string().into()],
    );
    match db.query_one_raw(stmt).await? {
        Some(row) => Ok(Some(row.try_get("", "title")?)),
        None => Ok(None),
    }
}

/// Labelled resources whose label name or value matches (global search,
/// D218): `exact` compares whole names/values case-insensitively (a `#`
/// query), otherwise a contained text; `name:value` matches the pair.
pub async fn search_labeled(
    db: &DatabaseConnection,
    org_id: i64,
    term: &str,
    exact: bool,
    limit: u64,
) -> Result<Vec<(String, String, LabelSet)>, DbErr> {
    let escape = |t: &str| {
        t.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    };
    let (cond, params): (&str, Vec<sea_orm::Value>) = match (exact, term.split_once(':')) {
        (true, Some((k, v))) => (
            "lower(e.key) = lower($2) AND lower(e.value) = lower($3)",
            vec![
                org_id.into(),
                k.trim().to_string().into(),
                v.trim().to_string().into(),
            ],
        ),
        (true, None) => (
            "lower(e.key) = lower($2) OR lower(e.value) = lower($2)",
            vec![org_id.into(), term.to_string().into()],
        ),
        (false, _) => (
            "e.key ILIKE $2 OR e.value ILIKE $2",
            vec![org_id.into(), format!("%{}%", escape(term)).into()],
        ),
    };
    let sql = format!(
        "SELECT resource_kind, resource_id, labels FROM resource_labels r \
         WHERE org_id = $1 AND EXISTS (SELECT 1 FROM jsonb_each_text(r.labels) e WHERE {cond}) \
         ORDER BY r.updated_at DESC LIMIT {limit}"
    );
    let stmt = sea_orm::Statement::from_sql_and_values(db.get_database_backend(), &sql, params);
    let mut out = Vec::new();
    for row in db.query_all_raw(stmt).await? {
        let kind: String = row.try_get("", "resource_kind")?;
        let id: String = row.try_get("", "resource_id")?;
        let doc: Option<Value> = row.try_get("", "labels")?;
        out.push((kind, id, doc.as_ref().map(doc_to_set).unwrap_or_default()));
    }
    Ok(out)
}

// ─────────────────────────── filtering ───────────────────────────

/// Every resource of the org carrying the label (exact value, or the bare
/// name for `name` alone): `(kind, id)`, optionally for one kind.
pub async fn ids_with_label(
    db: &DatabaseConnection,
    org_id: i64,
    filter: &(String, Option<String>),
    kind: Option<&str>,
) -> Result<Vec<(String, String)>, DbErr> {
    let (name, value) = filter;
    let mut params: Vec<sea_orm::Value> = vec![org_id.into(), name.clone().into()];
    let predicate = match value {
        Some(v) => {
            params.push(v.clone().into());
            "labels @> jsonb_build_object($2::text, $3::text)".to_string()
        }
        None => "labels ? $2::text".to_string(),
    };
    let kind_clause = match kind {
        Some(k) => {
            params.push(k.to_string().into());
            format!(" AND resource_kind = ${}", params.len())
        }
        None => String::new(),
    };
    let sql = format!(
        "SELECT resource_kind, resource_id FROM resource_labels \
         WHERE org_id = $1 AND {predicate}{kind_clause}"
    );
    let stmt = sea_orm::Statement::from_sql_and_values(db.get_database_backend(), &sql, params);
    let mut out = Vec::new();
    for row in db.query_all_raw(stmt).await? {
        let k: String = row.try_get("", "resource_kind")?;
        let id: String = row.try_get("", "resource_id")?;
        out.push((k, id));
    }
    Ok(out)
}

/// Catalogue pour l'autocomplétion front : `{name → {values, count}}`,
/// tous kinds confondus, org-scopé.
pub async fn catalog(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<(String, Vec<String>, i64)>, DbErr> {
    let rows = resource_labels::Entity::find()
        .filter(resource_labels::Column::OrgId.eq(org_id))
        .order_by_asc(resource_labels::Column::ResourceKind)
        .all(db)
        .await?;
    let mut acc: BTreeMap<String, (Vec<String>, HashSet<String>, i64)> = BTreeMap::new();
    for r in rows {
        let Some(doc) = r.labels else { continue };
        for (name, value) in doc_to_set(&doc) {
            let e = acc.entry(name).or_default();
            e.2 += 1;
            if let Some(v) = value {
                if e.1.insert(v.clone()) {
                    e.0.push(v);
                }
            }
        }
    }
    Ok(acc
        .into_iter()
        .map(|(name, (values, _, count))| (name, values, count))
        .collect())
}

// ─────────────────────────── hydratation batch ───────────────────────────

/// Labels d'un lot de ressources (1 requête, pas de N+1 — école media list).
pub async fn labels_for_many(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    ids: &[String],
) -> Result<HashMap<String, LabelSet>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = resource_labels::Entity::find()
        .filter(resource_labels::Column::OrgId.eq(org_id))
        .filter(resource_labels::Column::ResourceKind.eq(kind))
        .filter(resource_labels::Column::ResourceId.is_in(ids.to_vec()))
        .all(db)
        .await?;
    let mut out = HashMap::new();
    for r in rows {
        if let Some(doc) = r.labels {
            out.insert(r.resource_id, doc_to_set(&doc));
        }
    }
    Ok(out)
}

/// Outcome of a list filter `label=` that cannot be applied.
#[derive(Debug)]
pub enum ListLabelFilterError {
    /// Malformed filter: reason for the `label` field.
    Invalid(String),
    Db(DbErr),
}

/// List filter `label=name` / `label=name:value` (D218) for one kind:
/// `None` without filter, else the ids (stringified PK) of the org's
/// resources of `kind` carrying the label.
pub async fn list_filter(
    db: &DatabaseConnection,
    org_id: i64,
    raw: Option<&str>,
    kind: &str,
) -> Result<Option<HashSet<String>>, ListLabelFilterError> {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let (filter, err) = pnex_core::resources::parse_label_filter(raw);
    let Some(filter) = filter else {
        return Err(ListLabelFilterError::Invalid(
            err.unwrap_or_else(|| "invalid label filter".into()),
        ));
    };
    let ids = ids_with_label(db, org_id, &filter, Some(kind))
        .await
        .map_err(ListLabelFilterError::Db)?;
    Ok(Some(ids.into_iter().map(|(_, id)| id).collect()))
}

#[cfg(test)]
mod tests {
    use super::label_only_table;

    #[test]
    fn every_label_only_kind_has_its_table() {
        for kind in pnex_core::resources::LABEL_ONLY_KINDS {
            assert!(label_only_table(kind).is_some(), "{kind}");
        }
        assert!(label_only_table(pnex_core::resources::KIND_DEVICE).is_none());
    }
}
