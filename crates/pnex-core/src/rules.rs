//! Rules (analysis.md D829–D830): a condition on the objects of a type or on a
//! series, evaluated every few minutes, notifying when it turns true, and replayed
//! on the past. DTOs and the pure evaluation, wasm-safe.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ontology::api::{FilterOp, PropertyFilter};

pub const NAME_MAX: usize = 120;
pub const FILTERS_MAX: usize = 20;
/// Longest a series must hold its condition, in minutes.
pub const HOLD_MAX_MINUTES: u32 = 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum RuleKind {
    /// An object of a type starts matching the filters.
    #[default]
    Objects,
    /// A series stays past a threshold for a while.
    Series,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Cmp {
    #[default]
    Gt,
    Gte,
    Lt,
    Lte,
}

impl Cmp {
    pub fn holds(self, v: f64, threshold: f64) -> bool {
        match self {
            Self::Gt => v > threshold,
            Self::Gte => v >= threshold,
            Self::Lt => v < threshold,
            Self::Lte => v <= threshold,
        }
    }
}

/// What a rule watches; only the fields of its kind are read.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuleCondition {
    #[serde(default)]
    pub kind: RuleKind,
    // ── objects ──
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub type_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<PropertyFilter>,
    // ── series ──
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub metric: String,
    #[serde(default)]
    pub cmp: Cmp,
    #[serde(default)]
    pub threshold: f64,
    /// Minutes the series must hold the condition before the rule fires.
    #[serde(default)]
    pub hold_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuleInput {
    pub name: String,
    pub condition: RuleCondition,
    /// A notification channel of the org; absent = in-app.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The version read, on an update.
    #[serde(default)]
    pub expected_version: Option<i32>,
}

fn yes() -> bool {
    true
}

/// A key of the ontology or of telemetry: letters, digits, `_`, `-`, `.`.
fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 128
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

impl RuleInput {
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if self.name.trim().is_empty() {
            return Some(("name", "required".into()));
        }
        if self.name.chars().count() > NAME_MAX {
            return Some(("name", format!("max_length:{NAME_MAX}")));
        }
        let c = &self.condition;
        match c.kind {
            RuleKind::Objects => {
                if !key_ok(&c.type_key) {
                    return Some(("condition.type_key", "required".into()));
                }
                if c.filters.len() > FILTERS_MAX || c.filters.iter().any(|f| !key_ok(&f.key)) {
                    return Some(("condition.filters", "invalid".into()));
                }
            }
            RuleKind::Series => {
                if !key_ok(&c.device_id) {
                    return Some(("condition.device_id", "required".into()));
                }
                if !key_ok(&c.metric) {
                    return Some(("condition.metric", "required".into()));
                }
                if !c.threshold.is_finite() {
                    return Some(("condition.threshold", "invalid".into()));
                }
                if c.hold_minutes > HOLD_MAX_MINUTES {
                    return Some((
                        "condition.hold_minutes",
                        format!("range:0:{HOLD_MAX_MINUTES}"),
                    ));
                }
            }
        }
        None
    }
}

/// A rule as read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Rule {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub condition: RuleCondition,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub channel_id: Option<String>,
    pub enabled: bool,
    /// The condition held at the last pass.
    pub firing: bool,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub last_fired_at: Option<String>,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

/// One instant a rule fired, or would have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuleEvent {
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub at: String,
    /// The object that started matching (objects rules).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    /// The value that crossed (series rules).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuleReplay {
    pub events: Vec<RuleEvent>,
    /// False when the history store did not answer for the whole window.
    pub complete: bool,
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One filter on the properties of an object, as the query of D185 reads it:
/// numbers compare as numbers when both sides are, text otherwise.
pub fn filter_matches(f: &PropertyFilter, props: &Map<String, Value>) -> bool {
    let got = props.get(&f.key).filter(|v| !v.is_null());
    let order = |want: fn(std::cmp::Ordering) -> bool| {
        let Some(got) = got else { return false };
        match (as_number(got), as_number(&f.value)) {
            (Some(a), Some(b)) => a.partial_cmp(&b).is_some_and(want),
            _ => want(as_text(got).cmp(&as_text(&f.value))),
        }
    };
    use std::cmp::Ordering::{Equal, Greater, Less};
    match f.op {
        FilterOp::Exists => got.is_some(),
        FilterOp::Missing => got.is_none(),
        FilterOp::Eq => order(|o| o == Equal),
        FilterOp::Ne => got.is_none() || order(|o| o != Equal),
        FilterOp::Lt => order(|o| o == Less),
        FilterOp::Lte => order(|o| o != Greater),
        FilterOp::Gt => order(|o| o == Greater),
        FilterOp::Gte => order(|o| o != Less),
        FilterOp::Contains => got.is_some_and(|g| {
            as_text(g)
                .to_lowercase()
                .contains(&as_text(&f.value).to_lowercase())
        }),
        FilterOp::In => got.is_some_and(|g| {
            let g = as_text(g);
            f.value
                .as_array()
                .is_some_and(|vs| vs.iter().any(|v| as_text(v) == g))
        }),
    }
}

pub fn all_match(filters: &[PropertyFilter], props: &Map<String, Value>) -> bool {
    filters.iter().all(|f| filter_matches(f, props))
}

/// A change of one object: when, which, its properties before and after (`None`
/// when it did not exist or was archived).
pub struct ObjectChange {
    pub at_us: i64,
    pub object_id: String,
    pub before: Option<Map<String, Value>>,
    pub after: Option<Map<String, Value>>,
}

/// D830: the changes after which an object started matching.
pub fn object_events(changes: &[ObjectChange], filters: &[PropertyFilter]) -> Vec<(i64, String)> {
    let hit = |p: &Option<Map<String, Value>>| p.as_ref().is_some_and(|p| all_match(filters, p));
    changes
        .iter()
        .filter(|c| !hit(&c.before) && hit(&c.after))
        .map(|c| (c.at_us, c.object_id.clone()))
        .collect()
}

/// D830: the instants a series, sorted by time, has held the condition for
/// `hold_us`; once per stretch, again only after it stopped holding.
pub fn series_events(
    points: &[(i64, f64)],
    cmp: Cmp,
    threshold: f64,
    hold_us: i64,
) -> Vec<(i64, f64)> {
    let mut out = Vec::new();
    let (mut since, mut fired) = (None::<i64>, false);
    for &(at, v) in points {
        if cmp.holds(v, threshold) {
            let start = *since.get_or_insert(at);
            if !fired && at - start >= hold_us {
                out.push((at, v));
                fired = true;
            }
        } else {
            since = None;
            fired = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn f(key: &str, op: FilterOp, value: Value) -> PropertyFilter {
        PropertyFilter {
            key: key.into(),
            op,
            value,
        }
    }

    #[test]
    fn filters_read_numbers_and_text() {
        let p = props(json!({ "amount": "1200", "stage": "won", "name": "Acme Corp" }));
        assert!(filter_matches(&f("amount", FilterOp::Gt, json!(1000)), &p));
        assert!(!filter_matches(&f("amount", FilterOp::Lt, json!(1000)), &p));
        assert!(filter_matches(&f("stage", FilterOp::Eq, json!("won")), &p));
        assert!(filter_matches(
            &f("name", FilterOp::Contains, json!("acme")),
            &p
        ));
        assert!(filter_matches(
            &f("stage", FilterOp::In, json!(["lost", "won"])),
            &p
        ));
        assert!(filter_matches(
            &f("owner", FilterOp::Missing, Value::Null),
            &p
        ));
        assert!(!filter_matches(&f("owner", FilterOp::Gt, json!(1)), &p));
        // "9" < "10" as numbers, not as text.
        assert!(filter_matches(&f("amount", FilterOp::Gt, json!("999")), &p));
    }

    #[test]
    fn an_object_fires_when_it_starts_matching() {
        let fs = [f("stage", FilterOp::Eq, json!("won"))];
        let ch = |at, id: &str, before: Option<Value>, after: Option<Value>| ObjectChange {
            at_us: at,
            object_id: id.into(),
            before: before.map(props),
            after: after.map(props),
        };
        let changes = [
            ch(1, "a", None, Some(json!({ "stage": "open" }))),
            ch(
                2,
                "a",
                Some(json!({ "stage": "open" })),
                Some(json!({ "stage": "won" })),
            ),
            ch(
                3,
                "a",
                Some(json!({ "stage": "won" })),
                Some(json!({ "stage": "won", "x": 1 })),
            ),
            ch(4, "b", None, Some(json!({ "stage": "won" }))),
        ];
        assert_eq!(
            object_events(&changes, &fs),
            vec![(2, "a".to_string()), (4, "b".to_string())]
        );
    }

    #[test]
    fn a_series_fires_once_it_held_long_enough() {
        let pts = [
            (0, 10.0),
            (60, 31.0),
            (120, 32.0),
            (180, 33.0),
            (240, 20.0),
            (300, 35.0),
            (420, 36.0),
        ];
        // Over 30 for 120 s: fires at 180, then again only after it dropped.
        assert_eq!(
            series_events(&pts, Cmp::Gt, 30.0, 120),
            vec![(180, 33.0), (420, 36.0)]
        );
        assert_eq!(
            series_events(&pts, Cmp::Gt, 30.0, 0),
            vec![(60, 31.0), (300, 35.0)]
        );
        assert!(series_events(&pts, Cmp::Lt, 0.0, 0).is_empty());
    }

    #[test]
    fn inputs_are_checked() {
        let ok = RuleInput {
            name: "Big deals".into(),
            condition: RuleCondition {
                type_key: "deal".into(),
                filters: vec![f("amount", FilterOp::Gt, json!(1000))],
                ..Default::default()
            },
            channel_id: None,
            enabled: true,
            expected_version: None,
        };
        assert!(ok.violation().is_none());
        let series = RuleInput {
            condition: RuleCondition {
                kind: RuleKind::Series,
                ..Default::default()
            },
            ..ok.clone()
        };
        assert_eq!(series.violation().unwrap().0, "condition.device_id");
        let bad = RuleInput {
            condition: RuleCondition {
                filters: vec![f("a b", FilterOp::Eq, json!(1))],
                ..ok.condition.clone()
            },
            ..ok
        };
        assert_eq!(bad.violation().unwrap().0, "condition.filters");
    }
}
