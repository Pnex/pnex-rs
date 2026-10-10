//! Declarative ontology query (D185): a start set (type, ids, title,
//! property filters, effective labels, containment), then up to
//! [`MAX_HOPS`] link traversals, all evaluated "as of" one instant, then a
//! page. One parameterized SQL statement per call: values are always bound,
//! keys are checked against the type charset before entering the text.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::api::{
    Direction, FilterOp, OntologyQuery, QueryResult, QueryRow, MAX_HOPS, QUERY_LIMIT_MAX,
};
use pnex_core::ontology::schema::valid_key;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseConnection, FromQueryResult, Statement, Value as DbValue};
use serde_json::Value;

use super::objects::view;
use super::{invalid, parse_instant, schema, Result};
use crate::models::_entities::objects;

const DEFAULT_LIMIT: u64 = 50;

struct Sql {
    params: Vec<DbValue>,
}

impl Sql {
    /// Binds a value, returns its placeholder.
    fn bind(&mut self, v: impl Into<DbValue>) -> String {
        self.params.push(v.into());
        format!("${}", self.params.len())
    }
}

/// Objects: alive at `at` unless archived before it. Their `valid_from` is
/// when they were recorded, not when the real thing started, so it does
/// not filter (a link may start before its ends were recorded).
fn object_validity(at: &str, archived: bool) -> String {
    if archived {
        "TRUE".into()
    } else {
        format!("(o.valid_to IS NULL OR o.valid_to > {at})")
    }
}

fn validity(alias: &str, at: &str, archived: bool) -> String {
    if archived {
        format!("{alias}.valid_from <= {at}")
    } else {
        format!(
            "{alias}.valid_from <= {at} AND ({alias}.valid_to IS NULL OR {alias}.valid_to > {at})"
        )
    }
}

fn filter_sql(
    sql: &mut Sql,
    f: &pnex_core::ontology::api::PropertyFilter,
    i: usize,
) -> Result<String> {
    let field = |n: &str| format!("filters.{i}.{n}");
    if !valid_key(&f.key) {
        return Err(invalid(field("key"), FIELD_INVALID));
    }
    let k = &f.key;
    let text = |v: &Value| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    };
    Ok(match f.op {
        FilterOp::Exists => format!("o.properties ? '{k}'"),
        FilterOp::Eq | FilterOp::Ne => {
            let doc = sql.bind(serde_json::json!({ k: f.value }).to_string());
            let p = format!("o.properties @> {doc}::jsonb");
            if f.op == FilterOp::Eq {
                p
            } else {
                format!("NOT ({p})")
            }
        }
        FilterOp::Contains => {
            let pat = text(&f.value)
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let p = sql.bind(format!("%{pat}%"));
            format!("(o.properties ->> '{k}') ILIKE {p}")
        }
        FilterOp::Lt | FilterOp::Lte | FilterOp::Gt | FilterOp::Gte => {
            let op = match f.op {
                FilterOp::Lt => "<",
                FilterOp::Lte => "<=",
                FilterOp::Gt => ">",
                _ => ">=",
            };
            match f.value.as_f64() {
                Some(n) => {
                    let p = sql.bind(n);
                    format!(
                        "(jsonb_typeof(o.properties -> '{k}') = 'number' \
                         AND (o.properties ->> '{k}')::double precision {op} {p})"
                    )
                }
                // Dates and texts compare as text (ISO dates sort right).
                None => {
                    let p = sql.bind(text(&f.value));
                    format!("(o.properties ->> '{k}') {op} {p}")
                }
            }
        }
    })
}

pub async fn run(
    db: &DatabaseConnection,
    client: Option<&crate::services::openobserve::client::Client>,
    org_id: i64,
    q: &OntologyQuery,
) -> Result<QueryResult> {
    let s = schema(db, org_id).await?;
    if q.traverse.len() > MAX_HOPS {
        return Err(invalid(
            "traverse",
            format!("{}:{MAX_HOPS}", pnex_core::err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let mut sql = Sql { params: Vec::new() };
    let org = sql.bind(org_id);
    let at_ts: DateTimeWithTimeZone =
        parse_instant("as_of", q.as_of.as_deref())?.unwrap_or_else(|| chrono::Utc::now().into());
    let at = sql.bind(at_ts);
    let archived = q.include_archived && q.as_of.is_none();
    let mut start = vec![format!("o.org_id = {org}"), object_validity(&at, archived)];

    if let Some(t) = q.type_key.as_deref().filter(|t| !t.is_empty()) {
        if !s.knows(t) {
            return Err(invalid("type_key", FIELD_INVALID));
        }
        let p = sql.bind(t.to_string());
        start.push(format!("o.type_key = {p}"));
    }
    if !q.ids.is_empty() {
        let ids: Vec<uuid::Uuid> = q
            .ids
            .iter()
            .map(|i| {
                uuid::Uuid::parse_str(i)
                    .map_err(|_| invalid("ids", pnex_core::err_codes::FIELD_INVALID_UUID))
            })
            .collect::<Result<_>>()?;
        let p = sql.bind(ids);
        start.push(format!("o.id = ANY({p})"));
    }
    if let Some(text) = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let pat = text
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let p = sql.bind(format!("%{pat}%"));
        start.push(format!("o.title ILIKE {p}"));
    }
    for (i, f) in q.filters.iter().enumerate() {
        start.push(filter_sql(&mut sql, f, i)?);
    }
    if let Some(raw) = q.label.as_deref().filter(|l| !l.trim().is_empty()) {
        let (filter, _) = pnex_core::resources::parse_label_filter(raw);
        let filter = filter.ok_or_else(|| invalid("label", FIELD_INVALID))?;
        let hits = crate::services::resources::labels::ids_with_effective_label(
            db,
            org_id,
            &filter,
            q.type_key.as_deref(),
        )
        .await?;
        let kinds = sql.bind(hits.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>());
        let ids = sql.bind(hits.into_iter().map(|(_, i)| i).collect::<Vec<_>>());
        start.push(format!(
            "(o.type_key, o.native_id) IN (SELECT * FROM unnest({kinds}::text[], {ids}::text[]))"
        ));
    }
    if let Some(within) = q.within.as_deref().filter(|w| !w.is_empty()) {
        let id = uuid::Uuid::parse_str(within)
            .map_err(|_| invalid("within", pnex_core::err_codes::FIELD_INVALID_UUID))?;
        let root = super::objects::find(db, org_id, id).await?;
        let rk = sql.bind(root.type_key);
        let ri = sql.bind(root.native_id);
        start.push(format!(
            "(o.type_key, o.native_id) IN (\
               WITH RECURSIVE sub(kind, id, depth) AS (\
                 SELECT child_kind, child_id, 1 FROM resource_containments \
                   WHERE org_id = {org} AND parent_kind = {rk} AND parent_id = {ri} \
                 UNION ALL \
                 SELECT c.child_kind, c.child_id, sub.depth + 1 FROM resource_containments c \
                   JOIN sub ON c.parent_kind = sub.kind AND c.parent_id = sub.id \
                   WHERE c.org_id = {org} AND sub.depth < 32) \
               SELECT kind, id FROM sub)"
        ));
    }

    let mut ctes = vec![format!(
        "h0 AS (SELECT o.* FROM objects o WHERE {})",
        start.join(" AND ")
    )];
    for (i, hop) in q.traverse.iter().enumerate() {
        if s.link_type(&hop.link_type).is_none() {
            return Err(invalid(format!("traverse.{i}.link_type"), FIELD_INVALID));
        }
        let rel = sql.bind(hop.link_type.clone());
        let (from, to) = match hop.direction {
            Direction::Out => ("source_object_id", "target_object_id"),
            Direction::In => ("target_object_id", "source_object_id"),
        };
        ctes.push(format!(
            "h{n} AS (SELECT DISTINCT o.* FROM h{i} h \
               JOIN resource_edges e ON e.{from} = h.id AND e.org_id = {org} AND e.relation = {rel} \
                 AND {ev} \
               JOIN objects o ON o.id = e.{to} AND {ov})",
            n = i + 1,
            ev = validity("e", &at, false),
            ov = object_validity(&at, archived),
        ));
    }
    let last = format!("h{}", q.traverse.len());
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, QUERY_LIMIT_MAX);
    let offset = q.offset.unwrap_or(0);
    let with = ctes.join(", ");
    let backend = sea_orm::DatabaseBackend::Postgres;

    let count = db
        .query_one_raw(Statement::from_sql_and_values(
            backend,
            format!("WITH {with} SELECT count(*)::bigint AS n FROM {last}"),
            sql.params.clone(),
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    let lim = sql.bind(limit as i64);
    let off = sql.bind(offset as i64);
    let rows = objects::Model::find_by_statement(Statement::from_sql_and_values(
        backend,
        format!("WITH {with} SELECT * FROM {last} ORDER BY title, id LIMIT {lim} OFFSET {off}"),
        sql.params,
    ))
    .all(db)
    .await?;

    let mut out: Vec<QueryRow> = rows
        .iter()
        .map(|o| QueryRow {
            object: view(o, &s),
            latest: Default::default(),
        })
        .collect();
    if !q.latest.is_empty() {
        super::series::join_latest(db, client, org_id, &s, &q.latest, &mut out).await?;
    }
    Ok(QueryResult { count, rows: out })
}
