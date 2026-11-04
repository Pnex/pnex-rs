//! Read-only aggregates of an ontology query (object-aggregates.md D579–D582):
//! the selection of `query::plan`, its masks included, then one `GROUP BY`.
//! Keys are checked against the schema before entering the text; values are
//! always bound.

use std::collections::HashMap;

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::aggregate::{
    check, fill_ratios, resolve_tokens, split_others, AggregateGroup, AggregateQuery,
    AggregateResult, AggregateValue, GroupKey, Metric, MetricOp, GROUPS_READ_MAX, LIMIT_DEFAULT,
};
use pnex_core::ontology::api::{Direction, Hop, OntologyQuery, PropertyFilter};
use pnex_core::ontology::{ObjectTypeDef, PropertyKind, TypeSet};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, TransactionTrait};

use super::query::{filter_sql, plan, Plan, Sql};
use super::redact::Redaction;
use super::{invalid, OntologyError, OrgSchema, Result};

/// Cap of one aggregate (D586): measured at 100 ms on 80,000 objects with two
/// hops (lot AG0), so 5 s only stops a runaway one.
const TIMEOUT_MS: u32 = 5000;

/// The types the selected objects can have.
fn end_types<'a>(s: &'a OrgSchema, q: &OntologyQuery) -> Vec<&'a ObjectTypeDef> {
    let all = || s.types.iter().map(|(t, _, _)| t).collect::<Vec<_>>();
    match q.traverse.last() {
        Some(hop) => match s.link_type(&hop.link_type).map(|l| match hop.direction {
            Direction::Out => &l.to_types,
            Direction::In => &l.from_types,
        }) {
            Some(TypeSet::Only(keys)) => keys.iter().filter_map(|k| s.object_type(k)).collect(),
            _ => all(),
        },
        None => match q.type_key.as_deref().filter(|t| !t.is_empty()) {
            Some(t) => s.object_type(t).into_iter().collect(),
            None => all(),
        },
    }
}

/// The definitions of `key` on `types`: unknown is an error, and so is one
/// the caller may not read, or a filter or a split would reveal it (D582).
fn readable_kinds<'a>(
    types: &[&'a ObjectTypeDef],
    key: &str,
    field: &str,
    hide: Redaction,
) -> Result<Vec<&'a PropertyKind>> {
    let defs: Vec<_> = types
        .iter()
        .flat_map(|t| t.properties.iter())
        .filter(|p| p.key == key)
        .collect();
    if defs.is_empty() {
        return Err(invalid(field, FIELD_INVALID));
    }
    if defs.iter().any(|p| hide.hides(p.classification)) {
        return Err(OntologyError::AggregateHidden {
            property: key.to_string(),
        });
    }
    Ok(defs.iter().map(|p| &p.kind).collect())
}

/// A hop chain ends on objects whose title the group shows.
fn check_hops(s: &OrgSchema, hops: &[Hop], field: &str, hide: Redaction) -> Result<()> {
    for hop in hops {
        if s.link_type(&hop.link_type).is_none() {
            return Err(invalid(field, FIELD_INVALID));
        }
    }
    let hidden = hide.title_hidden_types(s);
    let last = hops.last().and_then(|h| {
        let l = s.link_type(&h.link_type)?;
        Some(match h.direction {
            Direction::Out => &l.to_types,
            Direction::In => &l.from_types,
        })
    });
    let shown = |t: &String| last.is_some_and(|set| set.contains(t));
    if let Some(t) = hidden.iter().find(|t| shown(t)) {
        return Err(OntologyError::AggregateHidden {
            property: t.clone(),
        });
    }
    Ok(())
}

/// The objects reached from `o` through `hops`, at the query's instant.
fn lateral(sql: &mut Sql, org_id: i64, at: &str, i: usize, hops: &[Hop]) -> String {
    let org = sql.bind(org_id);
    let mut joins = String::new();
    let mut from = "o".to_string();
    for (n, hop) in hops.iter().enumerate() {
        let (near, far) = match hop.direction {
            Direction::Out => ("source_object_id", "target_object_id"),
            Direction::In => ("target_object_id", "source_object_id"),
        };
        let rel = sql.bind(hop.link_type.clone());
        joins.push_str(&format!(
            " JOIN resource_edges e{n} ON e{n}.{near} = {from}.id AND e{n}.org_id = {org} \
               AND e{n}.relation = {rel} AND e{n}.valid_from <= {at} \
               AND (e{n}.valid_to IS NULL OR e{n}.valid_to > {at}) \
             JOIN objects t{n} ON t{n}.id = e{n}.{far} \
               AND (t{n}.valid_to IS NULL OR t{n}.valid_to > {at})"
        ));
        from = format!("t{n}");
    }
    format!(
        "LEFT JOIN LATERAL (SELECT DISTINCT {from}.id, {from}.title FROM (SELECT o.id) o{joins}) g{i} ON TRUE"
    )
}

/// One group key: its text, the `LATERAL` it needs, its label.
struct Split {
    key: String,
    join: String,
    label: Option<String>,
}

fn split(sql: &mut Sql, org_id: i64, at: &str, i: usize, g: &GroupKey) -> Split {
    if !g.hops.is_empty() {
        return Split {
            key: format!("g{i}.id::text"),
            join: lateral(sql, org_id, at, i, &g.hops),
            label: Some(format!("max(g{i}.title)")),
        };
    }
    let k = g.property.as_deref().unwrap_or_default();
    let text = format!("(o.properties ->> '{k}')");
    let key = match g.bucket {
        // A value that is no date goes to the group without a value.
        Some(b) => format!(
            "to_char(date_trunc('{}', (CASE WHEN {text} ~ '^\\d{{4}}-\\d{{2}}-\\d{{2}}' \
             THEN left({text}, 10)::date END)::timestamp), 'YYYY-MM-DD')",
            b.sql()
        ),
        None => text,
    };
    Split {
        key,
        join: String::new(),
        label: None,
    }
}

fn number_of(k: &str) -> String {
    format!(
        "(CASE WHEN jsonb_typeof(o.properties -> '{k}') = 'number' \
         THEN (o.properties ->> '{k}')::double precision END)"
    )
}

fn op_sql(op: MetricOp, operand: &str) -> String {
    match op {
        MetricOp::Count => "count(*)".into(),
        MetricOp::CountDistinct => format!("count(DISTINCT {operand})"),
        MetricOp::Sum => format!("sum({operand})"),
        MetricOp::Avg => format!("avg({operand})"),
        MetricOp::Min => format!("min({operand})"),
        MetricOp::Max => format!("max({operand})"),
    }
}

fn filter_clause(sql: &mut Sql, f: &Option<PropertyFilter>, i: usize) -> Result<String> {
    Ok(match f {
        Some(f) => format!(" FILTER (WHERE {})", filter_sql(sql, f, i)?),
        None => String::new(),
    })
}

fn is_money(types: &[&ObjectTypeDef], key: &str) -> bool {
    types
        .iter()
        .flat_map(|t| t.properties.iter())
        .any(|p| p.key == key && matches!(p.kind, PropertyKind::Money))
}

/// What a request names exists, has the right kind, and may be read. Also
/// what a dashboard's save checks (D583).
pub fn validate(s: &OrgSchema, req: &AggregateQuery, hide: Redaction) -> Result<()> {
    check(req).map_err(|(field, token)| invalid(field, token))?;
    if let Some(t) = req.query.type_key.as_deref().filter(|t| !t.is_empty()) {
        if !s.knows(t) {
            return Err(invalid("type_key", FIELD_INVALID));
        }
    }
    let types = end_types(s, &req.query);
    for (i, g) in req.group_by.iter().enumerate() {
        let field = format!("group_by.{i}");
        match g.property.as_deref() {
            Some(k) => {
                let kinds = readable_kinds(&types, k, &field, hide)?;
                let dated =
                    |k: &&PropertyKind| matches!(k, PropertyKind::Date | PropertyKind::DateTime);
                if g.bucket.is_some() && !kinds.iter().all(dated) {
                    return Err(invalid(field, FIELD_INVALID));
                }
                if kinds.iter().any(|k| matches!(k, PropertyKind::Money)) {
                    return Err(invalid(field, FIELD_INVALID));
                }
            }
            None => check_hops(s, &g.hops, &field, hide)?,
        }
    }
    let money_metric = |m: &Metric| m.property.as_deref().is_some_and(|k| is_money(&types, k));
    for (i, m) in req.metrics.iter().enumerate() {
        if let Some(k) = m.property.as_deref() {
            let field = format!("metrics.{i}.property");
            let kinds = readable_kinds(&types, k, &field, hide)?;
            let summable =
                |k: &&PropertyKind| matches!(k, PropertyKind::Number { .. } | PropertyKind::Money);
            if m.op.is_some_and(MetricOp::numeric) && !kinds.iter().all(summable) {
                return Err(invalid(field, FIELD_INVALID));
            }
        }
        if let Some(f) = &m.filter {
            readable_kinds(&types, &f.key, &format!("metrics.{i}.where.key"), hide)?;
        }
        // A ratio has no meaning across currencies (D580).
        if m.ratio
            .iter()
            .any(|r| req.metrics.iter().any(|o| &o.key == r && money_metric(o)))
        {
            return Err(invalid(format!("metrics.{i}"), FIELD_INVALID));
        }
    }
    Ok(())
}

pub async fn run(
    db: &DatabaseConnection,
    org_id: i64,
    req: &AggregateQuery,
    hide: Redaction,
) -> Result<AggregateResult> {
    let clock = chrono::Utc::now();
    let (today, now) = (clock.format("%Y-%m-%d").to_string(), clock.to_rfc3339());
    let mut q = req.query.clone();
    resolve_tokens(&mut q, &today, &now);
    let mut metrics: Vec<Metric> = req.metrics.clone();
    for f in metrics.iter_mut().filter_map(|m| m.filter.as_mut()) {
        f.value = pnex_core::ontology::flow::resolve_value(&f.value, &today, &now, &|_| None);
    }

    let Plan {
        s,
        sql,
        with,
        last,
        at,
        ..
    } = plan(db, org_id, &q, hide).await?;
    validate(&s, req, hide)?;
    let types = end_types(&s, &q);
    let money_metric = |m: &Metric| m.property.as_deref().is_some_and(|k| is_money(&types, k));

    // Text and binds of the splits, for one statement.
    let splits = |sql: &mut Sql| -> Vec<Split> {
        req.group_by
            .iter()
            .enumerate()
            .map(|(i, g)| split(sql, org_id, &at, i, g))
            .collect()
    };
    let keys_sql = |parts: &[Split]| -> String {
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let label = p.label.as_deref().unwrap_or("NULL::text");
                format!("{} AS k{i}, {label} AS l{i}, ", p.key)
            })
            .collect()
    };
    let group_sql = |parts: &[Split], extra: &str| -> String {
        let mut cols: Vec<String> = parts.iter().map(|p| p.key.clone()).collect();
        cols.extend((!extra.is_empty()).then(|| extra.to_string()));
        if cols.is_empty() {
            String::new()
        } else {
            format!(" GROUP BY {}", cols.join(", "))
        }
    };
    let joins_sql = |parts: &[Split]| -> String {
        parts
            .iter()
            .map(|p| p.join.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };

    let mut main = sql.clone();
    let parts = splits(&mut main);
    let mut select = keys_sql(&parts);
    for (i, m) in metrics.iter().enumerate() {
        let Some(op) = m.op else { continue };
        if money_metric(m) {
            continue;
        }
        let operand = match (op, m.property.as_deref()) {
            (MetricOp::CountDistinct, Some(k)) => format!("(o.properties ->> '{k}')"),
            (MetricOp::CountDistinct, None) => "o.id".to_string(),
            (_, Some(k)) => number_of(k),
            (_, None) => String::new(),
        };
        let filter = filter_clause(&mut main, &m.filter, i)?;
        select.push_str(&format!(
            "({}{filter})::double precision AS m{i}, ",
            op_sql(op, &operand)
        ));
    }
    select.push_str("count(*) AS total");
    // Dates in their order; else the largest first, by the first plain metric.
    let dated = req.group_by.first().is_some_and(|g| g.bucket.is_some());
    let first = metrics
        .iter()
        .position(|m| m.op.is_some() && !money_metric(m));
    let order = match (parts.is_empty(), dated, first) {
        (true, ..) => String::new(),
        (_, true, _) => " ORDER BY k0 NULLS LAST".to_string(),
        (_, _, Some(i)) => format!(" ORDER BY m{i} DESC NULLS LAST, k0 NULLS LAST"),
        _ => " ORDER BY total DESC, k0 NULLS LAST".to_string(),
    };
    let statement = format!(
        "WITH {with} SELECT {select} FROM {last} o {joins}{group}{order} LIMIT {lim}",
        joins = joins_sql(&parts),
        group = group_sql(&parts, ""),
        lim = GROUPS_READ_MAX + 1,
    );

    let txn = db.begin().await?;
    txn.execute_unprepared(&format!(
        "SET TRANSACTION READ ONLY; SET LOCAL statement_timeout = {TIMEOUT_MS}"
    ))
    .await?;
    let backend = sea_orm::DatabaseBackend::Postgres;
    let timed = |e: sea_orm::DbErr| {
        if e.to_string().contains("statement timeout") {
            OntologyError::AggregateTimeout
        } else {
            OntologyError::Db(e)
        }
    };
    let rows = txn
        .query_all_raw(Statement::from_sql_and_values(
            backend,
            statement,
            main.params,
        ))
        .await
        .map_err(timed)?;
    let truncated = rows.len() > GROUPS_READ_MAX as usize;
    let key_of = |r: &sea_orm::QueryResult, prefix: &str| -> Vec<Option<String>> {
        (0..req.group_by.len())
            .map(|i| {
                r.try_get::<Option<String>>("", &format!("{prefix}{i}"))
                    .ok()
                    .flatten()
            })
            .collect()
    };
    let mut groups: Vec<AggregateGroup> = Vec::with_capacity(rows.len());
    let mut at_key: HashMap<Vec<Option<String>>, usize> = HashMap::new();
    for r in rows.iter().take(GROUPS_READ_MAX as usize) {
        let mut g = AggregateGroup {
            key: key_of(r, "k"),
            labels: key_of(r, "l"),
            values: Default::default(),
        };
        for (i, m) in metrics.iter().enumerate() {
            if m.op.is_some() && !money_metric(m) {
                let value = r
                    .try_get::<Option<f64>>("", &format!("m{i}"))
                    .ok()
                    .flatten();
                g.values.insert(
                    m.key.clone(),
                    AggregateValue {
                        value,
                        ..Default::default()
                    },
                );
            }
        }
        at_key.insert(g.key.clone(), groups.len());
        groups.push(g);
    }

    // One statement per money metric: currencies are a value of the group,
    // never a sum.
    for (i, m) in metrics.iter().enumerate() {
        let (Some(op), true) = (m.op, money_metric(m)) else {
            continue;
        };
        let k = m.property.as_deref().unwrap_or_default();
        let mut one = sql.clone();
        let parts = splits(&mut one);
        let amount = format!(
            "(CASE WHEN jsonb_typeof(o.properties -> '{k}' -> 'amount') = 'number' \
             THEN (o.properties -> '{k}' ->> 'amount')::double precision END)"
        );
        let currency = format!("(o.properties -> '{k}' ->> 'currency')");
        let filter = filter_clause(&mut one, &m.filter, i)?;
        let statement = format!(
            "WITH {with} SELECT {keys}{currency} AS cur, ({agg}{filter})::double precision AS v \
             FROM {last} o {joins} WHERE {currency} IS NOT NULL{group} LIMIT {lim}",
            keys = keys_sql(&parts),
            agg = op_sql(op, &amount),
            joins = joins_sql(&parts),
            group = group_sql(&parts, &currency),
            lim = GROUPS_READ_MAX + 1,
        );
        let rows = txn
            .query_all_raw(Statement::from_sql_and_values(
                backend, statement, one.params,
            ))
            .await
            .map_err(timed)?;
        for r in &rows {
            let (Ok(cur), Ok(Some(v))) = (
                r.try_get::<String>("", "cur"),
                r.try_get::<Option<f64>>("", "v"),
            ) else {
                continue;
            };
            let key = key_of(r, "k");
            let index = *at_key.entry(key.clone()).or_insert_with(|| {
                groups.push(AggregateGroup {
                    key,
                    labels: key_of(r, "l"),
                    values: Default::default(),
                });
                groups.len() - 1
            });
            groups[index]
                .values
                .entry(m.key.clone())
                .or_default()
                .by_currency
                .insert(cur, v);
        }
    }
    txn.commit().await?;

    for g in &mut groups {
        fill_ratios(&metrics, g);
    }
    let limit = req.limit.unwrap_or(LIMIT_DEFAULT) as usize;
    let (groups, others) = split_others(&metrics, groups, limit);
    Ok(AggregateResult {
        groups,
        others,
        truncated,
        computed_at: clock.to_rfc3339(),
    })
}
