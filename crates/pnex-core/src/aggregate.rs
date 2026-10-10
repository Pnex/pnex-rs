//! Aggregation by bucket (ontology.md D182, media-ingest.md D171): one value
//! of a series per time range of a scope, or per time-of-day slice repeated
//! over the days of a window. Generic read primitive (a show, a shift, a
//! batch), served by `POST /api/v1/telemetry/aggregate`. Pure, wasm-safe.
//!
//! - DTOs: [`AggregateRequest`], [`AggregateResponse`];
//! - [`parse_bound`], [`check_slice_bounds`]: `HH:MM` slice bounds;
//! - [`AggregateWidgetOptions`]: the `range_bars` dashboard widget.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Max ranges resolved for one request.
pub const RANGES_MAX: usize = 200;
/// Max slice bounds (= slices per day).
pub const BOUNDS_MAX: usize = 24;
/// Max window of a request, in days.
pub const WINDOW_DAYS_MAX: i64 = 31;
/// Widget windows relative to now (key → seconds).
pub const WIDGET_WINDOWS: &[(&str, i64)] = &[("24h", 86_400), ("7d", 604_800)];
/// Timezone of a slice widget when none is set.
pub const DEFAULT_TIMEZONE: &str = "Europe/Paris";

/// How the values of one bucket are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateOp {
    #[default]
    Sum,
    Avg,
    Max,
    Min,
    /// Growth of a counter (`media_mentions_total`) over the bucket.
    Increase,
}

impl AggregateOp {
    pub const ALL: [AggregateOp; 5] = [Self::Sum, Self::Avg, Self::Max, Self::Min, Self::Increase];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Max => "max",
            Self::Min => "min",
            Self::Increase => "increase",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.wire() == s)
    }
}

/// Buckets = the time ranges of a scope met by `[from, to)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeBuckets {
    pub scope_kind: String,
    #[serde(default)]
    pub scope_id: String,
    pub from: String,
    pub to: String,
}

/// Buckets = time-of-day slices (`bounds`, local to `timezone`) repeated
/// over the days of `[from, to)`. The last slice wraps to the first bound
/// of the next day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceBuckets {
    pub from: String,
    pub to: String,
    pub bounds: Vec<String>,
    pub timezone: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateBuckets {
    Ranges(RangeBuckets),
    Slices(SliceBuckets),
}

/// Body of `POST /api/v1/telemetry/aggregate`; the series selector follows
/// the `series-batch` rules (device and/or free labels, D171).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateRequest {
    pub metric: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub op: AggregateOp,
    pub buckets: AggregateBuckets,
}

/// Which time a bucket was computed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BucketBasis {
    /// Realigned time of a range.
    Actual,
    /// Announced time of a range (no realigned pair).
    Planned,
    /// Time-of-day slice.
    Slice,
}

/// One bucket. Ranges: `start`/`end` RFC 3339 of the basis used. Slices:
/// `start`/`end` are the local `HH:MM` bounds and `days` counts the days
/// of the window that had data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateRow {
    pub label: String,
    pub start: String,
    pub end: String,
    pub basis: BucketBasis,
    /// `None` = no data in the bucket (or not read: see `available`).
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
}

/// `available: false` = O2 not configured, unreachable or past the budget:
/// the rows keep their shape, unread values stay `None`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AggregateResponse {
    pub available: bool,
    pub rows: Vec<AggregateRow>,
}

/// `HH:MM` (00:00..=23:59) → minutes after midnight.
pub fn parse_bound(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 || !(h.bytes().chain(m.bytes())).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Slice bounds rule: 1..=[`BOUNDS_MAX`] valid `HH:MM`, strictly
/// increasing. `Some(token)` = the field error token.
pub fn check_slice_bounds(bounds: &[String]) -> Option<String> {
    if bounds.is_empty() {
        return Some(crate::err_codes::FIELD_REQUIRED.to_string());
    }
    if bounds.len() > BOUNDS_MAX {
        return Some(format!(
            "{}:{BOUNDS_MAX}",
            crate::err_codes::FIELD_MAX_LENGTH
        ));
    }
    let mut prev: Option<u32> = None;
    for b in bounds {
        match parse_bound(b) {
            Some(m) if prev.is_none_or(|p| m > p) => prev = Some(m),
            _ => return Some(crate::err_codes::FIELD_INVALID.to_string()),
        }
    }
    None
}

/// IANA-like timezone name shape (`Europe/Paris`, `UTC`): the server
/// resolves it, this only bounds what a layout may store.
pub fn valid_timezone_shape(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() <= 64
        && tz
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
}

/// Bucket mode of the `range_bars` widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WidgetBuckets {
    /// Time ranges of one media stream of the org.
    Ranges { stream_id: String },
    /// Time-of-day slices.
    Slices {
        bounds: Vec<String>,
        #[serde(default)]
        timezone: Option<String>,
    },
}

/// Options of the `range_bars` widget (per range / per time slice bars).
/// The series is the widget's single source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateWidgetOptions {
    #[serde(default)]
    pub op: AggregateOp,
    /// Key of [`WIDGET_WINDOWS`], relative to now.
    pub window: String,
    pub buckets: WidgetBuckets,
}

impl Default for AggregateWidgetOptions {
    fn default() -> Self {
        Self {
            op: AggregateOp::Sum,
            window: "24h".into(),
            buckets: WidgetBuckets::Slices {
                bounds: [
                    "00:00", "06:00", "09:00", "12:00", "14:00", "18:00", "20:00",
                ]
                .map(String::from)
                .to_vec(),
                timezone: None,
            },
        }
    }
}

impl AggregateWidgetOptions {
    /// Seconds of the widget window, `None` when unknown.
    pub fn window_secs(&self) -> Option<i64> {
        WIDGET_WINDOWS
            .iter()
            .find(|(k, _)| *k == self.window)
            .map(|(_, s)| *s)
    }

    /// `(code, message)` of the first rule broken, `None` when valid.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.window_secs().is_none() {
            return Some(("aggregate_bad_window", "window must be 24h or 7d".into()));
        }
        match &self.buckets {
            WidgetBuckets::Ranges { stream_id } => {
                if stream_id.trim().is_empty() || stream_id.len() > 64 {
                    return Some(("aggregate_stream_missing", "pick a stream".into()));
                }
            }
            WidgetBuckets::Slices { bounds, timezone } => {
                if check_slice_bounds(bounds).is_some() {
                    return Some((
                        "aggregate_bad_bounds",
                        "bounds: 1 to 24 increasing HH:MM".into(),
                    ));
                }
                if timezone
                    .as_deref()
                    .is_some_and(|tz| !valid_timezone_shape(tz))
                {
                    return Some(("aggregate_bad_timezone", "invalid timezone".into()));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bounds_parse_and_order() {
        assert_eq!(parse_bound("00:00"), Some(0));
        assert_eq!(parse_bound("23:59"), Some(1439));
        for bad in ["24:00", "9:00", "09:60", "0900", "ab:cd", "09:00 ", ""] {
            assert_eq!(parse_bound(bad), None, "{bad:?}");
        }
        assert_eq!(check_slice_bounds(&b(&["00:00", "06:00", "09:00"])), None);
        assert_eq!(check_slice_bounds(&b(&["06:00"])), None);
        assert_eq!(check_slice_bounds(&b(&[])).as_deref(), Some("required"));
        assert_eq!(
            check_slice_bounds(&b(&["06:00", "06:00"])).as_deref(),
            Some("invalid")
        );
        assert_eq!(
            check_slice_bounds(&b(&["09:00", "06:00"])).as_deref(),
            Some("invalid")
        );
        let many: Vec<String> = (0..25).map(|i| format!("{:02}:00", i % 24)).collect();
        assert_eq!(check_slice_bounds(&many).as_deref(), Some("max_length:24"));
    }

    #[test]
    fn request_wire_shape() {
        let req: AggregateRequest = serde_json::from_value(serde_json::json!({
            "metric": "etl_mentions",
            "labels": {"stream": "inter"},
            "op": "increase",
            "buckets": {"slices": {"from": "a", "to": "b", "bounds": ["00:00"], "timezone": "UTC"}}
        }))
        .unwrap();
        assert_eq!(req.op, AggregateOp::Increase);
        assert!(req.device_id.is_empty());
        assert!(matches!(req.buckets, AggregateBuckets::Slices(_)));
        assert!(
            serde_json::from_value::<AggregateRequest>(serde_json::json!({
                "metric": "m", "op": "median",
                "buckets": {"ranges": {"scope_kind": "org", "from": "a", "to": "b"}}
            }))
            .is_err()
        );
    }

    #[test]
    fn range_bars_widget_validation() {
        use crate::viz::{validate_widget, SourceRef, WidgetOptions};
        let src = SourceRef {
            object_property: None,
            role: "primary".into(),
            metric: "etl_mentions".into(),
            device_id: String::new(),
            window: "24h".into(),
            memory: None,
            labels: [("stream".to_string(), "inter".to_string())].into(),
        };
        let opts = WidgetOptions {
            aggregate: Some(AggregateWidgetOptions::default()),
            ..Default::default()
        };
        let codes = |ty: &str, source: &[SourceRef], o: &WidgetOptions| {
            let mut v = Vec::new();
            validate_widget("w", ty, source, o, &mut v);
            v.into_iter().map(|x| x.code).collect::<Vec<_>>()
        };
        assert!(codes("range_bars", &[src.clone()], &opts).is_empty());
        assert!(
            codes("range_bars", &[src.clone()], &WidgetOptions::default())
                .contains(&"aggregate_missing".to_string())
        );
        assert!(codes("range_bars", &[], &opts).contains(&"range_bars_source".to_string()));
        assert!(codes("stat", &[src], &opts).contains(&"aggregate_unexpected".to_string()));
    }

    #[test]
    fn widget_options_rules() {
        assert_eq!(AggregateWidgetOptions::default().check(), None);
        let mut o = AggregateWidgetOptions {
            window: "30d".into(),
            ..Default::default()
        };
        assert_eq!(o.check().map(|c| c.0), Some("aggregate_bad_window"));
        o.window = "7d".into();
        o.buckets = WidgetBuckets::Ranges {
            stream_id: " ".into(),
        };
        assert_eq!(o.check().map(|c| c.0), Some("aggregate_stream_missing"));
        o.buckets = WidgetBuckets::Slices {
            bounds: b(&["00:00"]),
            timezone: Some("Europe/Paris\"".into()),
        };
        assert_eq!(o.check().map(|c| c.0), Some("aggregate_bad_timezone"));
    }
}
