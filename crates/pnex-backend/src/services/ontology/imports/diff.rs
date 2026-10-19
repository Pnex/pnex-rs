//! The difference of a run (D301): every record planned by the mapping,
//! objects deduplicated on their natural key (D302) and matched in
//! batches, links matched on their open pair, and in snapshot mode what
//! this import wrote before and the run no longer holds.

use std::collections::{BTreeMap, HashMap, HashSet};

use pnex_core::err_codes::FIELD_REQUIRED;
use pnex_core::ontology::import::{
    plan_row, DiffClose, DiffCreate, DiffEnd, DiffLink, DiffLinkUpdate, DiffUpdate, ImportDiff,
    Mapping, RowIssue, Schema, ValueChange, FIELD_UNKNOWN_KEY,
};
use pnex_core::ontology::schema::check_values;
use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::super::{OrgSchema, Result};

/// Keys looked up per query.
const BATCH: usize = 1_000;

#[derive(Debug, FromQueryResult)]
struct Live {
    id: Uuid,
    title: String,
    version: i32,
    properties: Value,
    k: String,
}

#[derive(Debug, FromQueryResult)]
struct Edge {
    id: i64,
    relation: String,
    source_object_id: Uuid,
    target_object_id: Uuid,
    attributes: Value,
}

/// An object of the run, deduplicated on (type, key).
struct Agg {
    row: usize,
    entry: usize,
    type_key: String,
    key_prop: String,
    key: String,
    title: String,
    titled: bool,
    properties: Map<String, Value>,
    location: Option<(f64, f64)>,
    create: bool,
    update: bool,
    missing: pnex_core::ontology::import::Missing,
}

enum End {
    Existing(Uuid, String),
    Created(usize, String),
    Missing,
}

fn source_like(import_id: Uuid) -> String {
    format!("import:{import_id}@%")
}

fn changes(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> BTreeMap<String, ValueChange> {
    after
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(*v))
        .map(|(k, v)| {
            (
                k.clone(),
                ValueChange {
                    from: before.get(k).cloned().unwrap_or(Value::Null),
                    to: v.clone(),
                },
            )
        })
        .collect()
}

/// A link of the run: type, from and to (indexes of the deduplicated
/// objects), start, attributes, row.
type PlannedPair = (
    String,
    usize,
    usize,
    Option<String>,
    Map<String, Value>,
    usize,
);

/// Plans `rows` with `m` and diffs them against the org's ontology.
/// `first_row` is the number of the first record as the user sees it (2 in
/// a sheet with a header line).
#[allow(clippy::too_many_arguments)]
pub async fn compute<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    s: &OrgSchema,
    m: &Mapping,
    snapshot: bool,
    rows: &[Value],
    first_row: usize,
) -> Result<ImportDiff> {
    let types = s
        .types
        .iter()
        .map(|(t, _, _)| t.clone())
        .collect::<Vec<_>>();
    let links = s
        .links
        .iter()
        .map(|(l, _, _)| l.clone())
        .collect::<Vec<_>>();
    let schema = Schema {
        types: &types,
        links: &links,
    };
    let mut diff = ImportDiff {
        rows: rows.len(),
        ..Default::default()
    };
    let mut objs: Vec<Agg> = Vec::new();
    let mut by_key: HashMap<(String, String), usize> = HashMap::new();
    let mut planned_links: Vec<PlannedPair> = Vec::new();
    let mut seen_links: HashSet<(String, usize, usize)> = HashSet::new();
    for (i, row) in rows.iter().enumerate() {
        let row_no = first_row + i;
        let plan = match plan_row(m, &schema, row_no, row) {
            Ok(p) => p,
            Err(issues) => {
                diff.errors.extend(issues);
                continue;
            }
        };
        let mut local = Vec::with_capacity(plan.objects.len());
        for o in plan.objects {
            let k = (o.type_key.clone(), o.key.clone());
            let idx = match by_key.get(&k) {
                Some(&idx) => {
                    // A later record completes or overrides the first one.
                    let a = &mut objs[idx];
                    for (p, v) in o.properties {
                        if a.properties.get(&p).is_some_and(|old| *old != v) {
                            diff.warnings.push(RowIssue {
                                row: row_no,
                                column: p.clone(),
                                token: "conflict".into(),
                            });
                        }
                        a.properties.insert(p, v);
                    }
                    if o.titled && !a.titled {
                        a.title = o.title;
                        a.titled = true;
                    }
                    a.location = a.location.or(o.location);
                    a.create |= o.create;
                    a.update |= o.update;
                    idx
                }
                None => {
                    by_key.insert(k, objs.len());
                    objs.push(Agg {
                        row: row_no,
                        entry: o.entry,
                        type_key: o.type_key,
                        key_prop: o.key_prop,
                        key: o.key,
                        title: o.title,
                        titled: o.titled,
                        properties: o.properties,
                        location: o.location,
                        create: o.create,
                        update: o.update,
                        missing: o.missing,
                    });
                    objs.len() - 1
                }
            };
            local.push(idx);
        }
        for l in plan.links {
            let (from, to) = (local[l.from], local[l.to]);
            if seen_links.insert((l.link_type.clone(), from, to)) {
                planned_links.push((l.link_type, from, to, l.valid_from, l.attributes, row_no));
            }
        }
    }

    // Existing objects, one query per type and batch of keys.
    let mut live: HashMap<(String, String), Live> = HashMap::new();
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for a in &objs {
        groups
            .entry((a.type_key.clone(), a.key_prop.clone()))
            .or_default()
            .push(a.key.clone());
    }
    for ((type_key, key_prop), keys) in &groups {
        for chunk in keys.chunks(BATCH) {
            let found = Live::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT id, title, version, properties, properties ->> $3 AS k FROM objects \
                 WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL \
                   AND properties ->> $3 = ANY($4)",
                [
                    org_id.into(),
                    type_key.clone().into(),
                    key_prop.clone().into(),
                    chunk.to_vec().into(),
                ],
            ))
            .all(db)
            .await?;
            for o in found {
                live.insert((type_key.clone(), o.k.clone()), o);
            }
        }
    }

    let mut ends: Vec<End> = Vec::with_capacity(objs.len());
    for a in &objs {
        let def = s.object_type(&a.type_key);
        let label = format!("{} {}", a.type_key, a.key);
        match live.get(&(a.type_key.clone(), a.key.clone())) {
            Some(o) => {
                ends.push(End::Existing(o.id, o.title.clone()));
                if !a.update {
                    continue;
                }
                let before = o.properties.as_object().cloned().unwrap_or_default();
                let ch = changes(&before, &a.properties);
                let new_title = (a.titled && a.title != o.title).then(|| a.title.clone());
                if !ch.is_empty() || new_title.is_some() {
                    diff.updated.push(DiffUpdate {
                        id: o.id.to_string(),
                        type_key: a.type_key.clone(),
                        key: a.key.clone(),
                        title: o.title.clone(),
                        version: o.version,
                        new_title,
                        changes: ch,
                    });
                }
            }
            None if !a.create && a.missing == pnex_core::ontology::import::Missing::Skip => {
                ends.push(End::Missing);
            }
            None if !a.create => {
                ends.push(End::Missing);
                let column = m.objects[a.entry].properties[&a.key_prop]
                    .from
                    .clone()
                    .unwrap_or_else(|| a.key_prop.clone());
                diff.errors.push(RowIssue {
                    row: a.row,
                    column,
                    token: FIELD_UNKNOWN_KEY.into(),
                });
            }
            None => {
                // A creation carries the whole document: `required` now.
                let full = def
                    .map(|d| check_values(&d.properties, &a.properties))
                    .unwrap_or(Ok(Map::new()));
                let pin = a.type_key == pnex_core::resources::KIND_MAP_PIN;
                let issue = match (&full, pin && a.location.is_none()) {
                    (Err((k, t)), _) => Some((k.clone(), t.clone())),
                    (_, true) => Some(("location".to_string(), FIELD_REQUIRED.to_string())),
                    _ => None,
                };
                if let Some((column, token)) = issue {
                    ends.push(End::Missing);
                    diff.errors.push(RowIssue {
                        row: a.row,
                        column,
                        token,
                    });
                    continue;
                }
                ends.push(End::Created(diff.created.len(), label));
                diff.created.push(DiffCreate {
                    type_key: a.type_key.clone(),
                    key_prop: a.key_prop.clone(),
                    key: a.key.clone(),
                    title: a.title.clone(),
                    properties: a.properties.clone(),
                    location: a.location.map(|(lat, lon)| [lat, lon]),
                });
            }
        }
    }

    // Open links among the existing ends of the run.
    let existing: Vec<Uuid> = ends
        .iter()
        .filter_map(|e| match e {
            End::Existing(id, _) => Some(*id),
            _ => None,
        })
        .collect();
    let relations: Vec<String> = m.links.iter().map(|l| l.link_type.clone()).collect();
    let mut open: HashMap<(String, Uuid, Uuid), Edge> = HashMap::new();
    for chunk in existing.chunks(BATCH) {
        let found = Edge::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id, relation, source_object_id, target_object_id, attributes FROM resource_edges \
             WHERE org_id = $1 AND valid_to IS NULL AND relation = ANY($2) \
               AND source_object_id = ANY($3) AND target_object_id IS NOT NULL",
            [
                org_id.into(),
                relations.clone().into(),
                chunk.to_vec().into(),
            ],
        ))
        .all(db)
        .await?;
        for e in found {
            open.insert(
                (e.relation.clone(), e.source_object_id, e.target_object_id),
                e,
            );
        }
    }
    let mut kept: HashSet<i64> = HashSet::new();
    let end_of = |e: &End| match e {
        End::Existing(id, label) => Some(DiffEnd {
            id: Some(id.to_string()),
            created: None,
            label: label.clone(),
        }),
        End::Created(i, label) => Some(DiffEnd {
            id: None,
            created: Some(*i),
            label: label.clone(),
        }),
        End::Missing => None,
    };
    for (link_type, from, to, valid_from, attributes, _row) in planned_links {
        let (Some(f), Some(t)) = (end_of(&ends[from]), end_of(&ends[to])) else {
            continue;
        };
        if let (End::Existing(fid, fl), End::Existing(tid, tl)) = (&ends[from], &ends[to]) {
            if let Some(e) = open.get(&(link_type.clone(), *fid, *tid)) {
                kept.insert(e.id);
                let before = e.attributes.as_object().cloned().unwrap_or_default();
                let ch = changes(&before, &attributes);
                if !ch.is_empty() {
                    let mut merged = before;
                    merged.extend(attributes);
                    diff.links_updated.push(DiffLinkUpdate {
                        id: e.id,
                        link_type,
                        label: format!("{fl} → {tl}"),
                        attributes: merged,
                        changes: ch,
                    });
                }
                continue;
            }
        }
        diff.links_opened.push(DiffLink {
            link_type,
            from: f,
            to: t,
            valid_from,
            attributes,
        });
    }

    if snapshot {
        close_missing(db, org_id, import_id, m, &ends, &kept, &mut diff).await?;
    }
    Ok(diff)
}

#[derive(Debug, FromQueryResult)]
struct Gone {
    id: String,
    type_key: String,
    label: String,
}

/// Snapshot: what this import wrote before and the run no longer holds.
async fn close_missing<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    m: &Mapping,
    ends: &[End],
    kept: &HashSet<i64>,
    diff: &mut ImportDiff,
) -> Result<()> {
    let seen: Vec<Uuid> = ends
        .iter()
        .filter_map(|e| match e {
            End::Existing(id, _) => Some(*id),
            _ => None,
        })
        .collect();
    let types: Vec<String> = m
        .objects
        .iter()
        .filter(|o| o.close_missing && o.create)
        .map(|o| o.type_key.clone())
        .collect();
    if !types.is_empty() {
        let gone = Gone::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id::text AS id, type_key, title AS label FROM objects \
             WHERE org_id = $1 AND valid_to IS NULL AND type_key = ANY($2) \
               AND source_ref LIKE $3 AND NOT (id = ANY($4))",
            [
                org_id.into(),
                types.into(),
                source_like(import_id).into(),
                seen.into(),
            ],
        ))
        .all(db)
        .await?;
        diff.closed.extend(gone.into_iter().map(|g| DiffClose {
            kind: "object".into(),
            id: g.id,
            type_key: g.type_key,
            label: g.label,
        }));
    }
    let relations: Vec<String> = m
        .links
        .iter()
        .filter(|l| l.close_missing)
        .map(|l| l.link_type.clone())
        .collect();
    if !relations.is_empty() {
        let kept: Vec<i64> = kept.iter().copied().collect();
        let gone = Gone::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT e.id::text AS id, e.relation AS type_key, \
                    coalesce(s.title, '') || ' → ' || coalesce(t.title, '') AS label \
             FROM resource_edges e \
             LEFT JOIN objects s ON s.id = e.source_object_id \
             LEFT JOIN objects t ON t.id = e.target_object_id \
             WHERE e.org_id = $1 AND e.valid_to IS NULL AND e.relation = ANY($2) \
               AND e.source_ref LIKE $3 AND NOT (e.id = ANY($4))",
            [
                org_id.into(),
                relations.into(),
                source_like(import_id).into(),
                kept.into(),
            ],
        ))
        .all(db)
        .await?;
        diff.closed.extend(gone.into_iter().map(|g| DiffClose {
            kind: "link".into(),
            id: g.id,
            type_key: g.type_key,
            label: g.label,
        }));
    }
    Ok(())
}
