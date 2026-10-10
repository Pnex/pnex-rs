//! Labels de la couche d'organisation (D42) — classification petit et
//! requêtable. 1 doc JSONB par ressource (`resource_labels`, unique), GIN
//! `jsonb_path_ops` sur PG pour le filtrage `@>`.
//!
//! **Labels effectifs résolus au read** : propres + hérités des ancêtres
//! (containment) — jamais dénormalisés. Fusion = « le plus proche gagne »
//! (les propres écrasent l'héritage, l'ancêtre proche écrase le lointain) :
//! on fusionne racine → feuille puis les propres en dernier.
//!
//! `WITH RECURSIVE` + `@>` / `?` JSONB (GIN).

use std::collections::{BTreeMap, HashMap, HashSet};

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, Set,
};
use serde_json::Value;

use crate::models::_entities::resource_containment as containment;
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

/// Labels effectifs = propres ⊕ hérités (ancêtres containment), le plus
/// proche gagne. Renvoie aussi le détail de l'héritage (feuille → racine)
/// pour l'affichage front (« hérité de Serre »).
pub async fn effective(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<(LabelSet, Vec<(String, String, LabelSet)>), DbErr> {
    let own = get_labels(db, org_id, kind, id).await?.unwrap_or_default();
    let chain = ancestor_chain(db, org_id, kind, id).await?;
    // Racine → feuille, puis les propres en dernier : le plus proche gagne.
    let mut merged = LabelSet::new();
    let mut inherited_detail = Vec::new();
    for (k, i) in chain.iter().rev() {
        if let Some(labels) = get_labels(db, org_id, k, i).await? {
            if !labels.is_empty() {
                for (n, v) in &labels {
                    merged.insert(n.clone(), v.clone());
                }
                inherited_detail.push((k.clone(), i.clone(), labels));
            }
        }
    }
    for (n, v) in &own {
        merged.insert(n.clone(), v.clone());
    }
    Ok((merged, inherited_detail))
}

/// Chaîne des ancêtres (feuille → racine), résolue par containment.
async fn ancestor_chain(
    db: &DatabaseConnection,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Vec<(String, String)>, DbErr> {
    let mut chain = Vec::new();
    let mut current = (kind.to_string(), id.to_string());
    loop {
        let row = containment::Entity::find()
            .filter(containment::Column::OrgId.eq(org_id))
            .filter(containment::Column::ChildKind.eq(&current.0))
            .filter(containment::Column::ChildId.eq(&current.1))
            .one(db)
            .await?;
        let Some(row) = row else { break };
        let next = (row.parent_kind.clone(), row.parent_id.clone());
        if chain.contains(&next) {
            break; // défense en profondeur (cycle impossible par construction)
        }
        chain.push(next.clone());
        current = next;
    }
    Ok(chain)
}

// ─────────────────────────── filtrage effectif ───────────────────────────

/// Toutes les ressources de l'org portant le label **effectivement**
/// (propres OU hérité d'un ancêtre) : `(kind, id)`.
///
/// Sémantique « le plus proche gagne » : un descendant qui **surcharge**
/// localement la clé ne sort PAS du filtre pour l'ancienne valeur — les
/// candidats (propres + descendants des porteurs) sont donc re-vérifiés
/// via la fusion [`effective`] (résolution au read, même règle partout).
///
/// Two recursive CTEs (tagged → descendants) + GIN for discovery.
pub async fn ids_with_effective_label(
    db: &DatabaseConnection,
    org_id: i64,
    filter: &(String, Option<String>),
    kind: Option<&str>,
) -> Result<Vec<(String, String)>, DbErr> {
    let (name, value) = filter;

    let candidates: Vec<(String, String)> = {
        // tagged = porte le label (valeur exacte ou tag nu `?`) ;
        // down   = tagged + tous leurs descendants (containment).
        let (predicate, params) = match value {
            Some(v) => (
                "labels @> jsonb_build_object($2::text, $3::text)",
                vec![org_id.into(), name.clone().into(), v.clone().into()],
            ),
            None => (
                "labels ? $2::text",
                vec![org_id.into(), name.clone().into()],
            ),
        };
        let sql = format!(
            "WITH RECURSIVE tagged AS (\
                 SELECT resource_kind, resource_id FROM resource_labels \
                  WHERE org_id = $1 AND {predicate}\
             ), down AS (\
                 SELECT resource_kind, resource_id FROM tagged \
                 UNION ALL \
                 SELECT c.child_kind, c.child_id FROM resource_containments c \
                 JOIN down d ON c.parent_kind = d.resource_kind AND c.parent_id = d.resource_id \
                 WHERE c.org_id = $1\
             ) \
             SELECT resource_kind, resource_id FROM down",
            predicate = predicate
        );
        let stmt = sea_orm::Statement::from_sql_and_values(db.get_database_backend(), &sql, params);
        let mut out = Vec::new();
        for row in db.query_all_raw(stmt).await? {
            let k: String = row.try_get("", "resource_kind")?;
            let id: String = row.try_get("", "resource_id")?;
            if kind.is_none_or(|want| want == k) {
                out.push((k, id));
            }
        }
        out
    };

    // ── Post-filtre « le plus proche gagne » : la fusion effective décide ──
    let mut out = Vec::new();
    for (k, id) in candidates {
        let (merged, _) = effective(db, org_id, &k, &id).await?;
        let hit = match value {
            Some(want) => merged.get(name).and_then(|v| v.as_deref()) == Some(want.as_str()),
            None => merged.contains_key(name),
        };
        if hit {
            out.push((k, id));
        }
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

/// List filter `label=name` / `label=name:value` (D42) for one kind:
/// `None` without filter, else the ids (stringified PK) of the org's
/// resources of `kind` carrying the label, inherited labels included.
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
    let ids = ids_with_effective_label(db, org_id, &filter, Some(kind))
        .await
        .map_err(ListLabelFilterError::Db)?;
    Ok(Some(ids.into_iter().map(|(_, id)| id).collect()))
}
