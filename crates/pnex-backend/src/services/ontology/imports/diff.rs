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

use super::super::{OntologyError, OrgSchema, Result};

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
    defaults: Map<String, Value>,
    location: Option<(f64, f64)>,
    create: bool,
    update: bool,
    missing: pnex_core::ontology::import::Missing,
    /// Objects it needs (indexes in the run's objects): written only when
    /// one of them resolves.
    needs: Vec<usize>,
    /// Set by [`Planned::delta`]: an existing object gets only what changed in
    /// the source. A creation still takes the whole document.
    changed: Option<Map<String, Value>>,
    /// The source changed the title (always, without a delta).
    title_changed: bool,
    /// Set by [`Planned::delta`]: nothing to write. Absent from the ontology, a
    /// person took it out, and it stays out.
    unchanged: bool,
}

/// How a run names an object it planned, in `object_import_runs.skipped_keys`.
fn skipped_key(type_key: &str, key: &str) -> String {
    format!("{type_key}\u{1f}{key}")
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

/// A link of the run (`from` and `to`: indexes of the deduplicated objects).
struct Pair {
    link_type: String,
    from: usize,
    to: usize,
    valid_from: Option<String>,
    attributes: Map<String, Value>,
    entry: usize,
    /// Set by [`Planned::delta`]: an open link gets only what changed in the
    /// source. Opening one still takes all its attributes.
    changed: Option<Map<String, Value>>,
}

/// What a run's records ask for, deduplicated: objects on (type, key), links on
/// their pair of objects. Sibling rows are merged here, so two plans compare by
/// entity and not by row (data-pipelines.md §9.7 ter).
#[derive(Default)]
pub struct Planned {
    objs: Vec<Agg>,
    by_key: HashMap<(String, String), usize>,
    pairs: Vec<Pair>,
    seen_pairs: HashMap<(String, usize, usize), usize>,
    /// (link type, object, it is the source) of rows that left the other end
    /// out by its key.
    held: Vec<(String, usize, bool)>,
    planned: usize,
    pub rows: usize,
    pub errors: Vec<RowIssue>,
    pub warnings: Vec<RowIssue>,
}

/// What the source no longer holds, by key.
#[derive(Default)]
pub struct Gone {
    /// (type, key property, key).
    objects: Vec<(String, String, String)>,
    /// (link type, source type, source key property, source key, same for the target).
    links: Vec<(String, GoneEnd, GoneEnd)>,
}

type GoneEnd = (String, String, String);

/// What a run closes in snapshot mode.
pub enum Closing {
    None,
    /// What this import wrote and the run did not see: needs every key.
    NotSeen,
    /// From a delta, which sees only what changed.
    Gone(Gone),
}

impl Planned {
    /// Plans one record into the run. `max`: objects planned before refusing.
    pub fn push(
        &mut self,
        m: &Mapping,
        schema: &Schema,
        row_no: usize,
        row: &Value,
        max: usize,
    ) -> Result<()> {
        self.rows += 1;
        let plan = match plan_row(m, schema, row_no, row) {
            Ok(p) => p,
            Err(issues) => {
                self.errors.extend(issues);
                return Ok(());
            }
        };
        self.warnings.extend(plan.warnings);
        self.planned += plan.objects.len();
        if self.planned > max {
            return Err(OntologyError::Import {
                code: pnex_core::err_codes::IMPORT_SOURCE_TOO_LARGE,
                args: None,
            });
        }
        let mut local: Vec<usize> = Vec::with_capacity(plan.objects.len());
        for o in plan.objects {
            let needs: Vec<usize> = o.needs.iter().map(|n| local[*n]).collect();
            let k = (o.type_key.clone(), o.key.clone());
            let idx = match self.by_key.get(&k) {
                Some(&idx) => {
                    // A later record completes or overrides the first one.
                    let a = &mut self.objs[idx];
                    for (p, v) in o.properties {
                        if a.properties.get(&p).is_some_and(|old| *old != v) {
                            self.warnings.push(RowIssue {
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
                    for (p, v) in o.defaults {
                        a.defaults.entry(p).or_insert(v);
                    }
                    a.location = a.location.or(o.location);
                    a.create |= o.create;
                    a.update |= o.update;
                    a.needs.extend(needs);
                    idx
                }
                None => {
                    self.by_key.insert(k, self.objs.len());
                    self.objs.push(Agg {
                        row: row_no,
                        entry: o.entry,
                        type_key: o.type_key,
                        key_prop: o.key_prop,
                        key: o.key,
                        title: o.title,
                        titled: o.titled,
                        properties: o.properties,
                        defaults: o.defaults,
                        location: o.location,
                        create: o.create,
                        update: o.update,
                        missing: o.missing,
                        needs,
                        changed: None,
                        title_changed: true,
                        unchanged: false,
                    });
                    self.objs.len() - 1
                }
            };
            local.push(idx);
        }
        self.held.extend(
            plan.held
                .into_iter()
                .map(|h| (h.link_type, local[h.end], h.is_source)),
        );
        for l in plan.links {
            let (from, to) = (local[l.from], local[l.to]);
            let Some(&at) = self.seen_pairs.get(&(l.link_type.clone(), from, to)) else {
                self.seen_pairs
                    .insert((l.link_type.clone(), from, to), self.pairs.len());
                self.pairs.push(Pair {
                    link_type: l.link_type,
                    from,
                    to,
                    valid_from: l.valid_from,
                    attributes: l.attributes,
                    entry: l.entry,
                    changed: None,
                });
                continue;
            };
            // The same pair again (two SIRET of one SIREN): merge its attributes.
            let rule = m.links.get(l.entry).map(|lm| &lm.on_conflict);
            let first = &mut self.pairs[at].attributes;
            for (k, v) in l.attributes {
                match first.get(&k) {
                    None => {
                        first.insert(k, v);
                    }
                    Some(old) if *old == v => {}
                    Some(_) => match rule.and_then(|r| r.get(&k)) {
                        Some(merged) => {
                            first.insert(k, merged.clone());
                        }
                        None => self.warnings.push(RowIssue {
                            row: row_no,
                            column: k,
                            token: "conflict".into(),
                        }),
                    },
                }
            }
        }
        Ok(())
    }

    fn pair_key(&self, p: &Pair) -> (String, (String, String), (String, String)) {
        let end = |i: usize| (self.objs[i].type_key.clone(), self.objs[i].key.clone());
        (p.link_type.clone(), end(p.from), end(p.to))
    }

    /// Keeps what changed since `base`, the plan of the transactions last
    /// applied (D577, Q-P6): an edit in the ontology survives until the source
    /// changes that value. Objects left only as link ends or needs write nothing.
    /// Returns what `base` held and this plan no longer does.
    ///
    /// A plan depends on the source, but what it could write also depends on the
    /// ontology: `skipped` names what the reference run planned and left out for
    /// lack of an object it needs. Those are not applied, whatever `base` says.
    pub fn delta(&mut self, m: &Mapping, base: &Planned, skipped: &HashSet<String>) -> Gone {
        let mut keep = vec![false; self.objs.len()];
        let mut retried = vec![false; self.objs.len()];
        for (i, a) in self.objs.iter_mut().enumerate() {
            retried[i] = skipped.contains(&skipped_key(&a.type_key, &a.key));
            let found = base.by_key.get(&(a.type_key.clone(), a.key.clone()));
            let Some(&b) = found.filter(|_| !retried[i]) else {
                keep[i] = true;
                continue;
            };
            let old = &base.objs[b];
            let changed: Map<String, Value> = a
                .properties
                .iter()
                .filter(|(k, v)| old.properties.get(*k) != Some(*v))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            a.title_changed = a.titled && (!old.titled || old.title != a.title);
            if changed.is_empty() && !a.title_changed {
                // Unchanged: closed or archived in the ontology, it stays so.
                a.create = false;
                a.update = false;
                a.missing = pnex_core::ontology::import::Missing::Skip;
                a.unchanged = true;
            } else {
                keep[i] = true;
            }
            a.changed = Some(changed);
        }
        let base_pairs: HashMap<_, &Pair> =
            base.pairs.iter().map(|p| (base.pair_key(p), p)).collect();
        let keys: Vec<_> = self.pairs.iter().map(|p| self.pair_key(p)).collect();
        let current: HashSet<_> = keys.iter().cloned().collect();
        let mut pairs = Vec::new();
        for (mut p, key) in std::mem::take(&mut self.pairs).into_iter().zip(keys) {
            let retry = retried[p.from] || retried[p.to];
            if let Some(old) = base_pairs.get(&key).filter(|_| !retry) {
                let changed: Map<String, Value> = p
                    .attributes
                    .iter()
                    .filter(|(k, v)| old.attributes.get(*k) != Some(*v))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if changed.is_empty() {
                    continue;
                }
                p.changed = Some(changed);
            }
            keep[p.from] = true;
            keep[p.to] = true;
            pairs.push(p);
        }
        // Held ends decide which links stay open: the closing reads them.
        for (_, at, _) in &self.held {
            keep[*at] = true;
        }
        // What a kept object needs decides whether it is written.
        for i in (0..self.objs.len()).rev() {
            if keep[i] {
                for n in &self.objs[i].needs {
                    keep[*n] = true;
                }
            }
        }
        let gone = Gone {
            objects: base
                .objs
                .iter()
                .filter(|o| {
                    !self
                        .by_key
                        .contains_key(&(o.type_key.clone(), o.key.clone()))
                        && m.objects
                            .get(o.entry)
                            .is_some_and(|e| e.close_missing && e.create)
                })
                .map(|o| (o.type_key.clone(), o.key_prop.clone(), o.key.clone()))
                .collect(),
            links: base
                .pairs
                .iter()
                .filter(|p| {
                    !current.contains(&base.pair_key(p))
                        && m.links.get(p.entry).is_some_and(|l| l.close_missing)
                })
                .map(|p| {
                    let end = |i: usize| {
                        let o = &base.objs[i];
                        (o.type_key.clone(), o.key_prop.clone(), o.key.clone())
                    };
                    (p.link_type.clone(), end(p.from), end(p.to))
                })
                .collect(),
        };
        // Indexes change: only what is kept reaches the database.
        let mut at = vec![usize::MAX; self.objs.len()];
        let mut next = 0;
        for (i, k) in keep.iter().enumerate() {
            if *k {
                at[i] = next;
                next += 1;
            }
        }
        let mut i = 0;
        self.objs.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        for a in &mut self.objs {
            for n in &mut a.needs {
                *n = at[*n];
            }
        }
        for p in &mut pairs {
            p.from = at[p.from];
            p.to = at[p.to];
        }
        self.pairs = pairs;
        for h in &mut self.held {
            h.1 = at[h.1];
        }
        gone
    }
}

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
    let mut planned = Planned::default();
    for (i, row) in rows.iter().enumerate() {
        planned.push(
            m,
            &schema,
            first_row + i,
            row,
            pnex_core::ontology::import::PLANNED_MAX,
        )?;
    }
    let observations = match &m.observations {
        Some(_) => pnex_core::markets::observations(rows),
        None => Vec::new(),
    };
    let closing = if snapshot {
        Closing::NotSeen
    } else {
        Closing::None
    };
    let (mut diff, _) = against(db, org_id, import_id, s, m, planned, closing).await?;
    diff.observations = observations;
    Ok(diff)
}

/// Diffs a plan against the org's ontology. Also returns what it planned and
/// could not write, for lack of an object it needs.
pub async fn against<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    s: &OrgSchema,
    m: &Mapping,
    planned: Planned,
    closing: Closing,
) -> Result<(ImportDiff, Vec<String>)> {
    let Planned {
        mut objs,
        pairs: planned_links,
        held,
        rows,
        errors,
        warnings,
        ..
    } = planned;
    let mut diff = ImportDiff {
        rows,
        errors,
        warnings,
        ..Default::default()
    };

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
    // Consumed: a creation takes the planned document, not a copy.
    for a in objs.iter_mut() {
        let def = s.object_type(&a.type_key);
        let label = format!("{} {}", a.type_key, a.key);
        // Needs an object that resolved to none (a skipped lookup): left out.
        // A need first seen later than this object counts as resolved.
        if !a.needs.is_empty()
            && a.needs
                .iter()
                .all(|n| matches!(ends.get(*n), Some(End::Missing)))
        {
            ends.push(End::Missing);
            continue;
        }
        match live.get(&(a.type_key.clone(), a.key.clone())) {
            Some(o) => {
                ends.push(End::Existing(o.id, o.title.clone()));
                if !a.update {
                    continue;
                }
                let before = o.properties.as_object().cloned().unwrap_or_default();
                let ch = changes(&before, a.changed.as_ref().unwrap_or(&a.properties));
                let new_title =
                    (a.titled && a.title_changed && a.title != o.title).then(|| a.title.clone());
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
                let mut doc = std::mem::take(&mut a.defaults);
                doc.extend(std::mem::take(&mut a.properties));
                let full = def
                    .map(|d| check_values(&d.properties, &doc))
                    .unwrap_or(Ok(Map::new()));
                let pin = a.type_key == pnex_core::resources::KIND_MAP_PIN;
                // A register site without coordinates is left out, its row
                // kept (network-check.md D406): a warning, not an error.
                if pin && full.is_ok() && a.location.is_none() {
                    ends.push(End::Missing);
                    diff.warnings.push(RowIssue {
                        row: a.row,
                        column: "location".into(),
                        token: FIELD_REQUIRED.into(),
                    });
                    continue;
                }
                if let Err((column, token)) = &full {
                    let (column, token) = (column.clone(), token.clone());
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
                    properties: doc,
                    location: a.location.map(|(lat, lon)| [lat, lon]),
                });
            }
        }
    }

    let skipped: Vec<String> = objs
        .iter()
        .zip(&ends)
        .filter(|(a, e)| !a.unchanged && matches!(e, End::Missing))
        .map(|(a, _)| skipped_key(&a.type_key, &a.key))
        .collect();
    drop((objs, live));

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
    for pair in planned_links {
        let Pair {
            link_type,
            from,
            to,
            valid_from,
            attributes,
            entry,
            changed,
        } = pair;
        let (Some(f), Some(t)) = (end_of(&ends[from]), end_of(&ends[to])) else {
            continue;
        };
        if let (End::Existing(fid, fl), End::Existing(tid, tl)) = (&ends[from], &ends[to]) {
            if let Some(e) = open.get(&(link_type.clone(), *fid, *tid)) {
                kept.insert(e.id);
                let before = e.attributes.as_object().cloned().unwrap_or_default();
                let after = changed.unwrap_or(attributes);
                let ch = changes(&before, &after);
                if !ch.is_empty() {
                    let mut merged = before;
                    merged.extend(after);
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
        // `create: false`: this import only completes open links.
        if m.links.get(entry).is_some_and(|l| !l.create) {
            continue;
        }
        diff.links_opened.push(DiffLink {
            link_type,
            from: f,
            to: t,
            valid_from,
            attributes,
            propose: m.links.get(entry).is_some_and(|l| l.propose),
        });
    }

    drop(open);
    if matches!(closing, Closing::None) {
        return Ok((diff, skipped));
    }
    let (edges, others) = held_links(db, org_id, import_id, &held, &ends).await?;
    kept.extend(edges);
    match closing {
        Closing::Gone(gone) => {
            close_gone(db, org_id, import_id, gone, &others, &kept, &mut diff).await?;
        }
        _ => close_missing(db, org_id, import_id, m, &ends, &others, &kept, &mut diff).await?,
    }
    Ok((diff, skipped))
}

/// Ids of live objects by (type, key).
async fn ids_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    ends: &[&GoneEnd],
) -> Result<HashMap<(String, String), Uuid>> {
    let mut groups: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
    for (type_key, key_prop, key) in ends {
        groups
            .entry((type_key, key_prop))
            .or_default()
            .push(key.clone());
    }
    let mut out = HashMap::new();
    for ((type_key, key_prop), mut keys) in groups {
        keys.sort_unstable();
        keys.dedup();
        for chunk in keys.chunks(BATCH) {
            let rows = db
                .query_all_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT id, properties ->> $3 AS k FROM objects \
                     WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL \
                       AND properties ->> $3 = ANY($4)",
                    [
                        org_id.into(),
                        type_key.into(),
                        key_prop.into(),
                        chunk.to_vec().into(),
                    ],
                ))
                .await?;
            for r in rows {
                out.insert(
                    (type_key.to_string(), r.try_get::<String>("", "k")?),
                    r.try_get::<Uuid>("", "id")?,
                );
            }
        }
    }
    Ok(out)
}

/// Snapshot from a delta: what this import wrote and the source dropped since
/// the transactions last applied. `held_objects` and `kept` stay open.
async fn close_gone<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    gone: Gone,
    held_objects: &[Uuid],
    kept: &HashSet<i64>,
    diff: &mut ImportDiff,
) -> Result<()> {
    let like = source_like(import_id);
    let ends: Vec<&GoneEnd> = gone
        .objects
        .iter()
        .chain(gone.links.iter().flat_map(|(_, a, b)| [a, b]))
        .collect();
    let ids = ids_of(db, org_id, &ends).await?;
    let id_of = |e: &GoneEnd| ids.get(&(e.0.clone(), e.2.clone())).copied();
    let objects: Vec<Uuid> = gone
        .objects
        .iter()
        .filter_map(id_of)
        .filter(|id| !held_objects.contains(id))
        .collect();
    for chunk in objects.chunks(BATCH) {
        let rows = GoneRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id::text AS id, type_key, title AS label FROM objects \
             WHERE org_id = $1 AND valid_to IS NULL AND source_ref LIKE $2 AND id = ANY($3)",
            [org_id.into(), like.clone().into(), chunk.to_vec().into()],
        ))
        .all(db)
        .await?;
        diff.closed.extend(rows.into_iter().map(|g| DiffClose {
            kind: "object".into(),
            id: g.id,
            type_key: g.type_key,
            label: g.label,
        }));
    }
    let mut by_relation: BTreeMap<&str, (Vec<Uuid>, Vec<Uuid>)> = BTreeMap::new();
    for (relation, from, to) in &gone.links {
        if let (Some(f), Some(t)) = (id_of(from), id_of(to)) {
            let e = by_relation.entry(relation).or_default();
            e.0.push(f);
            e.1.push(t);
        }
    }
    let kept: Vec<i64> = kept.iter().copied().collect();
    for (relation, (from, to)) in by_relation {
        for (f, t) in from.chunks(BATCH).zip(to.chunks(BATCH)) {
            let rows = GoneRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT e.id::text AS id, e.relation AS type_key, \
                        coalesce(s.title, '') || ' → ' || coalesce(t.title, '') AS label \
                 FROM unnest($4::uuid[], $5::uuid[]) AS p(a, b) \
                 JOIN resource_edges e ON e.source_object_id = p.a AND e.target_object_id = p.b \
                 LEFT JOIN objects s ON s.id = e.source_object_id \
                 LEFT JOIN objects t ON t.id = e.target_object_id \
                 WHERE e.org_id = $1 AND e.valid_to IS NULL AND e.relation = $2 \
                   AND e.source_ref LIKE $3 AND NOT (e.id = ANY($6))",
                [
                    org_id.into(),
                    relation.into(),
                    like.clone().into(),
                    f.to_vec().into(),
                    t.to_vec().into(),
                    kept.clone().into(),
                ],
            ))
            .all(db)
            .await?;
            diff.closed.extend(rows.into_iter().map(|g| DiffClose {
                kind: "link".into(),
                id: g.id,
                type_key: g.type_key,
                label: g.label,
            }));
        }
    }
    Ok(())
}

#[derive(Debug, FromQueryResult)]
struct GoneRow {
    id: String,
    type_key: String,
    label: String,
}

/// Links this import wrote on the ends rows still know when they left the
/// other end out by its key (`invalid: skip`), and those other ends: the
/// run cannot tell them gone.
async fn held_links<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    held: &[(String, usize, bool)],
    ends: &[End],
) -> Result<(Vec<i64>, Vec<Uuid>)> {
    let mut groups: BTreeMap<(&str, bool), Vec<Uuid>> = BTreeMap::new();
    for (link_type, at, is_source) in held {
        if let Some(End::Existing(id, _)) = ends.get(*at) {
            groups
                .entry((link_type.as_str(), *is_source))
                .or_default()
                .push(*id);
        }
    }
    let (mut edges, mut others) = (Vec::new(), Vec::new());
    for ((relation, is_source), mut ids) in groups {
        ids.sort_unstable();
        ids.dedup();
        let (known, other) = if is_source {
            ("source_object_id", "target_object_id")
        } else {
            ("target_object_id", "source_object_id")
        };
        for chunk in ids.chunks(BATCH) {
            let rows = db
                .query_all_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    format!(
                        "SELECT id, {other} AS other FROM resource_edges \
                         WHERE org_id = $1 AND valid_to IS NULL AND relation = $2 \
                           AND source_ref LIKE $3 AND {known} = ANY($4)"
                    ),
                    [
                        org_id.into(),
                        relation.into(),
                        source_like(import_id).into(),
                        chunk.to_vec().into(),
                    ],
                ))
                .await?;
            for r in rows {
                edges.push(r.try_get::<i64>("", "id")?);
                others.extend(r.try_get::<Option<Uuid>>("", "other")?);
            }
        }
    }
    Ok((edges, others))
}

/// Snapshot: what this import wrote before and the run no longer holds
/// (`held`: objects kept whatever the run holds).
#[allow(clippy::too_many_arguments)]
async fn close_missing<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    m: &Mapping,
    ends: &[End],
    held: &[Uuid],
    kept: &HashSet<i64>,
    diff: &mut ImportDiff,
) -> Result<()> {
    let seen: Vec<Uuid> = ends
        .iter()
        .filter_map(|e| match e {
            End::Existing(id, _) => Some(*id),
            _ => None,
        })
        .chain(held.iter().copied())
        .collect();
    let types: Vec<String> = m
        .objects
        .iter()
        .filter(|o| o.close_missing && o.create)
        .map(|o| o.type_key.clone())
        .collect();
    if !types.is_empty() {
        let gone = GoneRow::find_by_statement(Statement::from_sql_and_values(
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
        let gone = GoneRow::find_by_statement(Statement::from_sql_and_values(
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
