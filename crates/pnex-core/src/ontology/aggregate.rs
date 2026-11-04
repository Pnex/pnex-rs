//! Read-only aggregates of an ontology query (object-aggregates.md D579–D581):
//! the request, its answer, and the pure part of the computation. Pure,
//! wasm-safe. No `flatten` and no untagged enum (serde_json
//! `arbitrary_precision`).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::api::{Hop, OntologyQuery, PropertyFilter};
use super::schema::valid_key;

pub const GROUP_KEYS_MAX: usize = 2;
pub const HOPS_MAX: usize = 2;
pub const METRICS_MAX: usize = 8;
pub const LIMIT_DEFAULT: u32 = 100;
pub const LIMIT_MAX: u32 = 1000;
/// Groups read from the database: past it the answer says `truncated`.
pub const GROUPS_READ_MAX: u32 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DateBucket {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl DateBucket {
    /// The unit of `date_trunc`.
    pub fn sql(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Quarter => "quarter",
            Self::Year => "year",
        }
    }
}

/// One way to split: by `property` (with `bucket` on a date), or by the
/// object reached through `hops`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GroupKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<DateBucket>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hops: Vec<Hop>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MetricOp {
    Count,
    CountDistinct,
    Sum,
    Avg,
    Min,
    Max,
}

impl MetricOp {
    /// Needs a numeric property.
    pub fn numeric(self) -> bool {
        matches!(self, Self::Sum | Self::Avg | Self::Min | Self::Max)
    }
}

/// `op` over the objects meeting `where`, or `ratio`: `[a, b]`, metric `a`
/// divided by metric `b` of the same group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<MetricOp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<PropertyFilter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ratio: Vec<String>,
    /// A ratio as a percentage.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub percent: bool,
}

/// The query selects the objects, the rest computes on them. `$today±n` is
/// accepted in the filters (crm.md D451).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct AggregateQuery {
    #[serde(default)]
    pub query: OntologyQuery,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<GroupKey>,
    pub metrics: Vec<Metric>,
    /// Groups returned, the others summed in `others`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One value: a number, or one number per currency for a money property
/// (D580: currencies are never added together).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AggregateValue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_currency: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AggregateGroup {
    /// One stable value per group key (enum key, date, object id); `null`
    /// for the objects without one.
    pub key: Vec<Option<String>>,
    /// Title of the object of a key by link, already masked by the server.
    pub labels: Vec<Option<String>>,
    pub values: BTreeMap<String, AggregateValue>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AggregateResult {
    pub groups: Vec<AggregateGroup>,
    /// What `limit` left out. A sum, count, minimum or maximum stays right;
    /// an average or a distinct count cannot be merged: absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub others: Option<AggregateGroup>,
    /// More groups than could be read: `others` is incomplete.
    pub truncated: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub computed_at: String,
}

/// A widget reading an aggregate of objects (D583). `query` on a dashboard,
/// `object_set` on a screen of an application: never both.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ObjectsSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<OntologyQuery>,
    /// Parameter of the set → variable that fills it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<GroupKey>,
    pub metrics: Vec<Metric>,
    /// The metric shown.
    pub show: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ObjectsSource {
    /// The request, over `query`.
    pub fn request(&self, query: OntologyQuery) -> AggregateQuery {
        AggregateQuery {
            query,
            group_by: self.group_by.clone(),
            metrics: self.metrics.clone(),
            limit: self.limit,
        }
    }
}

/// `POST /apps/{id}/object-sets/{name}/aggregate/query`: the set selects the
/// objects, `params` fills it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SetAggregateQuery {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<GroupKey>,
    pub metrics: Vec<Metric>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The object a type dashboard shows (D583): an entry of `ids` or a filter
/// value, filled by the page once an object is picked.
pub const VALUE_OBJECT: &str = "$object";

/// Whether the query waits for the shown object.
pub fn reads_object(q: &OntologyQuery) -> bool {
    q.ids.iter().any(|i| i == VALUE_OBJECT)
        || q.filters
            .iter()
            .chain(&q.reached)
            .any(|f| f.value == VALUE_OBJECT)
}

/// `$object` replaced by the shown object's id.
pub fn bind_object(q: &mut OntologyQuery, id: &str) {
    for i in q.ids.iter_mut().filter(|i| *i == VALUE_OBJECT) {
        *i = id.to_string();
    }
    let filters = q.filters.iter_mut().chain(&mut q.reached);
    for f in filters.filter(|f| f.value == VALUE_OBJECT) {
        f.value = id.into();
    }
}

/// `$today±n` and `$now` of the query's filters (crm.md D451).
pub fn resolve_tokens(q: &mut OntologyQuery, today: &str, now: &str) {
    for f in q.filters.iter_mut().chain(&mut q.reached) {
        f.value = super::flow::resolve_value(&f.value, today, now, &|_| None);
    }
}

/// First problem of a request: `(field, token)`.
pub fn check(a: &AggregateQuery) -> Result<(), (String, &'static str)> {
    use crate::err_codes::FIELD_INVALID;
    let bad = |f: String| Err((f, FIELD_INVALID));
    if a.group_by.len() > GROUP_KEYS_MAX {
        return bad("group_by".into());
    }
    for (i, g) in a.group_by.iter().enumerate() {
        let by_property = g.property.as_deref().is_some_and(valid_key);
        let by_hops = !g.hops.is_empty() && g.hops.len() <= HOPS_MAX;
        if by_property == by_hops || (g.bucket.is_some() && !by_property) {
            return bad(format!("group_by.{i}"));
        }
    }
    if a.metrics.is_empty() || a.metrics.len() > METRICS_MAX {
        return bad("metrics".into());
    }
    if a.limit.is_some_and(|l| l == 0 || l > LIMIT_MAX) {
        return bad("limit".into());
    }
    let mut seen = HashMap::new();
    for (i, m) in a.metrics.iter().enumerate() {
        if !valid_key(&m.key) || seen.insert(m.key.as_str(), m).is_some() {
            return bad(format!("metrics.{i}.key"));
        }
    }
    for (i, m) in a.metrics.iter().enumerate() {
        let shape = match (m.op, m.ratio.len()) {
            // A ratio of ratios or of money has no single meaning here.
            (None, 2) => {
                m.property.is_none()
                    && m.filter.is_none()
                    && m.ratio.iter().all(|r| {
                        seen.get(r.as_str())
                            .is_some_and(|o| o.op.is_some() && o.key != m.key)
                    })
            }
            (Some(op), 0) => {
                let property = m.property.as_deref();
                !m.percent
                    && property.is_none_or(valid_key)
                    && (property.is_some() || !op.numeric())
                    && (op != MetricOp::Count || property.is_none())
            }
            _ => false,
        };
        if !shape {
            return bad(format!("metrics.{i}"));
        }
    }
    Ok(())
}

/// Fills the ratios, a null denominator or zero leaving no value (D580).
pub fn fill_ratios(metrics: &[Metric], g: &mut AggregateGroup) {
    for m in metrics.iter().filter(|m| m.op.is_none()) {
        let of = |k: &str| g.values.get(k).and_then(|v| v.value);
        let value = match (of(&m.ratio[0]), of(&m.ratio[1])) {
            (Some(a), Some(b)) if b != 0.0 => Some(a / b * if m.percent { 100.0 } else { 1.0 }),
            _ => None,
        };
        g.values.insert(
            m.key.clone(),
            AggregateValue {
                value,
                ..Default::default()
            },
        );
    }
}

/// Keeps the first `limit` groups, merges the rest into `others`.
pub fn split_others(
    metrics: &[Metric],
    mut groups: Vec<AggregateGroup>,
    limit: usize,
) -> (Vec<AggregateGroup>, Option<AggregateGroup>) {
    if groups.len() <= limit {
        return (groups, None);
    }
    let rest = groups.split_off(limit);
    let mut merged = AggregateGroup::default();
    for m in metrics {
        let Some(op) = m.op else { continue };
        let merge: fn(f64, f64) -> f64 = match op {
            MetricOp::Count | MetricOp::Sum => |a, b| a + b,
            MetricOp::Min => f64::min,
            MetricOp::Max => f64::max,
            MetricOp::Avg | MetricOp::CountDistinct => continue,
        };
        let mut out = AggregateValue::default();
        for v in rest.iter().filter_map(|g| g.values.get(&m.key)) {
            if let Some(x) = v.value {
                out.value = Some(out.value.map_or(x, |a| merge(a, x)));
            }
            for (c, x) in &v.by_currency {
                out.by_currency
                    .entry(c.clone())
                    .and_modify(|a| *a = merge(*a, *x))
                    .or_insert(*x);
            }
        }
        merged.values.insert(m.key.clone(), out);
    }
    fill_ratios(metrics, &mut merged);
    (groups, Some(merged))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shown_object_fills_ids_and_filter_values() {
        use crate::ontology::api::{FilterOp, OntologyQuery, PropertyFilter};
        let mut q = OntologyQuery {
            ids: vec![super::VALUE_OBJECT.into()],
            filters: vec![PropertyFilter {
                key: "owner".into(),
                op: FilterOp::Eq,
                value: super::VALUE_OBJECT.into(),
            }],
            ..Default::default()
        };
        assert!(super::reads_object(&q));
        super::bind_object(&mut q, "abc");
        assert!(!super::reads_object(&q));
        assert_eq!(q.ids, ["abc"]);
        assert_eq!(q.filters[0].value, "abc");
    }

    use super::*;
    use serde_json::json;

    fn req(v: serde_json::Value) -> AggregateQuery {
        serde_json::from_value(v).unwrap()
    }

    fn count(key: &str) -> Metric {
        Metric {
            key: key.into(),
            op: Some(MetricOp::Count),
            property: None,
            filter: None,
            ratio: vec![],
            percent: false,
        }
    }

    fn ratio(key: &str, a: &str, b: &str) -> Metric {
        Metric {
            key: key.into(),
            op: None,
            property: None,
            filter: None,
            ratio: vec![a.into(), b.into()],
            percent: false,
        }
    }

    fn group(values: &[(&str, f64)]) -> AggregateGroup {
        AggregateGroup {
            values: values
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        AggregateValue {
                            value: Some(*v),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_request_is_checked_before_any_sql() {
        let ok = |v| check(&req(v)).is_ok();
        assert!(ok(json!({"metrics": [{"key": "n", "op": "count"}]})));
        assert!(ok(json!({
            "group_by": [{"property": "day", "bucket": "month"}, {"hops": [{"link_type": "l"}]}],
            "metrics": [
                {"key": "won", "op": "count", "where": {"key": "s", "op": "eq", "value": "w"}},
                {"key": "all", "op": "count"},
                {"key": "rate", "ratio": ["won", "all"]},
                {"key": "total", "op": "sum", "property": "amount"}
            ]
        })));
        for (bad, field) in [
            (json!({"metrics": []}), "metrics"),
            (json!({"metrics": [{"key": "n", "op": "sum"}]}), "metrics.0"),
            (
                json!({"metrics": [{"key": "n", "op": "count", "property": "p"}]}),
                "metrics.0",
            ),
            (
                json!({"metrics": [{"key": "bad key", "op": "count"}]}),
                "metrics.0.key",
            ),
            (
                json!({"metrics": [{"key": "n", "op": "count"}, {"key": "n", "op": "count"}]}),
                "metrics.1.key",
            ),
            (
                json!({"metrics": [{"key": "r", "ratio": ["r", "r"]}]}),
                "metrics.0",
            ),
            (
                json!({"metrics": [{"key": "a", "op": "count"}, {"key": "r", "ratio": ["a", "x"]}]}),
                "metrics.1",
            ),
            (
                json!({"group_by": [{"property": "p", "hops": [{"link_type": "l"}]}], "metrics": [{"key": "n", "op": "count"}]}),
                "group_by.0",
            ),
            (
                json!({"group_by": [{"hops": [{"link_type": "l"}], "bucket": "day"}], "metrics": [{"key": "n", "op": "count"}]}),
                "group_by.0",
            ),
            (
                json!({"group_by": [{"property": "a'; DROP TABLE objects"}], "metrics": [{"key": "n", "op": "count"}]}),
                "group_by.0",
            ),
            (
                json!({"limit": 0, "metrics": [{"key": "n", "op": "count"}]}),
                "limit",
            ),
        ] {
            assert_eq!(check(&req(bad)).unwrap_err().0, field);
        }
    }

    #[test]
    fn a_ratio_without_a_denominator_has_no_value() {
        let metrics = [count("a"), count("b"), ratio("r", "a", "b")];
        let mut g = group(&[("a", 1.0), ("b", 4.0)]);
        fill_ratios(&metrics, &mut g);
        assert_eq!(g.values["r"].value, Some(0.25));
        let mut zero = group(&[("a", 1.0), ("b", 0.0)]);
        fill_ratios(&metrics, &mut zero);
        assert_eq!(zero.values["r"].value, None);
        let mut pct = ratio("r", "a", "b");
        pct.percent = true;
        let mut g = group(&[("a", 1.0), ("b", 4.0)]);
        fill_ratios(&[count("a"), count("b"), pct], &mut g);
        assert_eq!(g.values["r"].value, Some(25.0));
    }

    #[test]
    fn the_groups_past_the_limit_add_up_in_others() {
        let mut sum = count("s");
        sum.op = Some(MetricOp::Sum);
        let mut avg = count("m");
        avg.op = Some(MetricOp::Avg);
        let metrics = [count("a"), count("b"), sum, avg, ratio("r", "a", "b")];
        let mut money = group(&[("a", 1.0), ("b", 2.0), ("m", 5.0)]);
        money.values.insert(
            "s".into(),
            AggregateValue {
                by_currency: [("EUR".to_string(), 10.0), ("USD".to_string(), 1.0)].into(),
                ..Default::default()
            },
        );
        let mut other = group(&[("a", 3.0), ("b", 6.0), ("m", 7.0)]);
        other.values.insert(
            "s".into(),
            AggregateValue {
                by_currency: [("EUR".to_string(), 5.0)].into(),
                ..Default::default()
            },
        );
        let first = group(&[("a", 100.0), ("b", 100.0)]);
        let (kept, others) = split_others(&metrics, vec![first.clone(), money, other], 1);
        assert_eq!(kept, vec![first]);
        let others = others.unwrap();
        assert_eq!(others.values["a"].value, Some(4.0));
        assert_eq!(others.values["s"].by_currency["EUR"], 15.0);
        assert_eq!(others.values["s"].by_currency["USD"], 1.0);
        assert!(
            !others.values.contains_key("m"),
            "an average cannot be merged"
        );
        assert_eq!(others.values["r"].value, Some(0.5));

        assert_eq!(split_others(&metrics, vec![group(&[])], 1).1, None);
    }
}
