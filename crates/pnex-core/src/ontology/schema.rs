//! Typed properties (D178) and the validation shared by the backend and
//! the UI (same rules on both sides, wasm32-safe).
//!
//! Errors are machine field tokens (`required`, `max_length:255`, `invalid`,
//! `unknown`), keyed by the property key, never pre-rendered text.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};

/// Unknown property key in a value document.
pub const FIELD_UNKNOWN: &str = "unknown";
/// Key charset and length: `[a-z][a-z0-9_]{0,63}`.
pub const KEY_MAX: usize = 64;
pub const NAME_MAX: usize = 255;
pub const TEXT_MAX_DEFAULT: usize = 2000;
pub const PROPERTIES_MAX: usize = 100;
pub const ENUM_VALUES_MAX: usize = 200;

pub fn valid_key(key: &str) -> bool {
    key.len() <= KEY_MAX
        && key.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// What a property holds. Temporal kinds (`series`, `events`) designate
/// where the data lives; they never hold a value in the object (D178).
// Serialize only: deserialization goes through `PropertyDef` (manual, via
// `Value`) because an internally tagged enum carrying numbers breaks under
// serde_json `arbitrary_precision`, unified on workspace builds.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PropertyKind {
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_len: Option<usize>,
    },
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    Bool,
    /// `YYYY-MM-DD`.
    Date,
    /// RFC 3339.
    DateTime,
    Enum {
        values: Vec<String>,
    },
    /// `{"lat": f64, "lon": f64}`.
    GeoPoint,
    /// A circle or a one-ring polygon (`crate::geo_area`, K29).
    GeoArea,
    /// `http(s)` only (R11).
    Url,
    /// Id (UUID) of an object of `to_type`.
    Ref {
        to_type: String,
    },
    /// A telemetry series (O2 metric). Bound per object by `measures`
    /// links (D181); `metric` is the default metric name of a binding.
    Series {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metric: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// An O2 log stream, filtered on the object (`object=<id>` field).
    Events {
        stream: String,
    },
    /// An instant known only up to a precision (cases.md K5):
    /// `{"from": RFC 3339 UTC, "to": RFC 3339 UTC, "precision": …}`.
    FuzzyTime,
    /// A day-granular period (`super::recurrence::Recurrence`): its
    /// occurrences are materialized as time ranges of the object.
    Recurrence,
    /// An amount of money (cases-money.md K54):
    /// `{"amount": f64, "currency": ISO 4217, "estimated"?: bool}`.
    Money,
}

/// Precisions of a `fuzzy_time` value (cases.md K5).
pub const FUZZY_PRECISIONS: [&str; 8] = [
    "instant", "minute", "hour", "day", "month", "year", "range", "approx",
];

impl PropertyKind {
    pub fn is_temporal(&self) -> bool {
        matches!(self, Self::Series { .. } | Self::Events { .. })
    }

    pub fn wire(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Number { .. } => "number",
            Self::Bool => "bool",
            Self::Date => "date",
            Self::DateTime => "date_time",
            Self::Enum { .. } => "enum",
            Self::GeoPoint => "geo_point",
            Self::GeoArea => "geo_area",
            Self::Url => "url",
            Self::Ref { .. } => "ref",
            Self::Series { .. } => "series",
            Self::Events { .. } => "events",
            Self::FuzzyTime => "fuzzy_time",
            Self::Recurrence => "recurrence",
            Self::Money => "money",
        }
    }
}

/// A property of an object type (or an attribute of a link type).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PropertyDef {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub names: super::Names,
    #[serde(flatten)]
    pub kind: PropertyKind,
    #[serde(default)]
    pub required: bool,
    /// Backed by the GIN index of `objects.properties` in queries.
    #[serde(default)]
    pub indexed: bool,
    /// Natural key (D302): one live object per org × type × value. Text or
    /// number only; implies `indexed`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unique: bool,
    /// Pack that added this property to a type it does not own (D306).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    /// Enum only: value → colour of the map, hierarchy and graph
    /// (account-mapping.md D339), one of [`COLORS`].
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub colors: std::collections::BTreeMap<String, String>,
    /// Enum only: value → displayed label by language (ontology.md D370).
    /// The value itself is a stable key, never renamed to translate it.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, super::Names>,
    /// Data classification (iam.md D298, crm.md D442): `personal` and
    /// `sensitive` values are hidden from a role without the matching read
    /// permission and removed by an erasure.
    #[serde(default, skip_serializing_if = "Classification::is_internal")]
    pub classification: Classification,
}

/// Closed classification list of a property, a link attribute or a type's
/// title (iam.md D298).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Classification {
    Public,
    #[default]
    Internal,
    Personal,
    Sensitive,
}

impl Classification {
    pub fn is_internal(&self) -> bool {
        *self == Self::Internal
    }

    /// Personal data: hidden by role, removed by an erasure (D443, D446).
    pub fn is_personal(&self) -> bool {
        matches!(self, Self::Personal | Self::Sensitive)
    }
}

/// Languages a value label may carry.
const LABEL_LANGS_MAX: usize = 8;

/// Closed palette of property colours: design tokens, never raw CSS.
pub const COLORS: &[&str] = &["green", "amber", "red", "blue", "violet", "gray"];

impl PropertyDef {
    /// Colour of `value` for this property, when it has one.
    pub fn color_of(&self, value: &Value) -> Option<&str> {
        value
            .as_str()
            .and_then(|v| self.colors.get(v))
            .map(String::as_str)
    }

    /// Label of enum `value` in `lang` (`fr-FR` or `fr`), when one is set.
    pub fn label_of(&self, value: &str, lang: &str) -> Option<&str> {
        name_in(self.labels.get(value)?, lang)
    }
}

/// The text of `names` for `lang` (`fr-FR`, then `fr`, then English, then
/// any); none when empty or blank.
pub fn name_in<'a>(names: &'a super::Names, lang: &str) -> Option<&'a str> {
    let short = lang.split(['-', '_']).next().unwrap_or(lang);
    names
        .get(lang)
        .or_else(|| names.get(short))
        .or_else(|| names.get("en"))
        .or_else(|| names.values().next())
        .map(String::as_str)
        .filter(|l| !l.trim().is_empty())
}

impl<'de> Deserialize<'de> for PropertyDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        #[derive(Deserialize)]
        struct Head {
            key: String,
            #[serde(default)]
            name: String,
            #[serde(default)]
            names: super::Names,
            kind: String,
            #[serde(default)]
            required: bool,
            #[serde(default)]
            indexed: bool,
            #[serde(default)]
            unique: bool,
            #[serde(default)]
            pack: Option<String>,
            #[serde(default)]
            colors: std::collections::BTreeMap<String, String>,
            #[serde(default)]
            labels: std::collections::BTreeMap<String, super::Names>,
            #[serde(default)]
            classification: Classification,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Cfg {
            max_len: Option<usize>,
            unit: Option<String>,
            min: Option<f64>,
            max: Option<f64>,
            values: Option<Vec<String>>,
            to_type: Option<String>,
            metric: Option<String>,
            stream: Option<String>,
        }
        let v = Value::deserialize(d)?;
        let h: Head = serde_json::from_value(v.clone()).map_err(D::Error::custom)?;
        let c: Cfg = serde_json::from_value(v).map_err(D::Error::custom)?;
        let missing = |f: &'static str| D::Error::missing_field(f);
        let kind = match h.kind.as_str() {
            "text" => PropertyKind::Text { max_len: c.max_len },
            "number" => PropertyKind::Number {
                unit: c.unit,
                min: c.min,
                max: c.max,
            },
            "bool" => PropertyKind::Bool,
            "date" => PropertyKind::Date,
            "date_time" => PropertyKind::DateTime,
            "enum" => PropertyKind::Enum {
                values: c.values.ok_or_else(|| missing("values"))?,
            },
            "geo_point" => PropertyKind::GeoPoint,
            "geo_area" => PropertyKind::GeoArea,
            "url" => PropertyKind::Url,
            "ref" => PropertyKind::Ref {
                to_type: c.to_type.ok_or_else(|| missing("to_type"))?,
            },
            "series" => PropertyKind::Series {
                metric: c.metric,
                unit: c.unit,
            },
            "events" => PropertyKind::Events {
                stream: c.stream.ok_or_else(|| missing("stream"))?,
            },
            "fuzzy_time" => PropertyKind::FuzzyTime,
            "recurrence" => PropertyKind::Recurrence,
            "money" => PropertyKind::Money,
            other => {
                return Err(D::Error::unknown_variant(
                    other,
                    &[
                        "text",
                        "number",
                        "bool",
                        "date",
                        "date_time",
                        "enum",
                        "geo_point",
                        "url",
                        "ref",
                        "series",
                        "events",
                        "fuzzy_time",
                        "recurrence",
                        "money",
                    ],
                ))
            }
        };
        Ok(Self {
            key: h.key,
            name: h.name,
            names: h.names,
            kind,
            required: h.required,
            indexed: h.indexed || h.unique,
            unique: h.unique,
            pack: h.pack,
            colors: h.colors,
            labels: h.labels,
            classification: h.classification,
        })
    }
}

/// A `{field: token}` error.
pub type FieldError = (String, String);

fn err(field: impl Into<String>, token: impl Into<String>) -> FieldError {
    (field.into(), token.into())
}

/// Checks property definitions: keys, names, kinds; `known_type` answers
/// whether a `ref` target type exists. Field paths are `properties.<i>.…`.
pub fn check_defs(
    defs: &[PropertyDef],
    prefix: &str,
    known_type: &dyn Fn(&str) -> bool,
) -> Result<(), FieldError> {
    if defs.len() > PROPERTIES_MAX {
        return Err(err(prefix, format!("{FIELD_MAX_LENGTH}:{PROPERTIES_MAX}")));
    }
    for (i, d) in defs.iter().enumerate() {
        let at = |f: &str| format!("{prefix}.{i}.{f}");
        if !valid_key(&d.key) {
            return Err(err(at("key"), FIELD_INVALID));
        }
        if defs[..i].iter().any(|o| o.key == d.key) {
            return Err(err(at("key"), "duplicate"));
        }
        if d.name.chars().count() > NAME_MAX {
            return Err(err(at("name"), format!("{FIELD_MAX_LENGTH}:{NAME_MAX}")));
        }
        match &d.kind {
            PropertyKind::Enum { values } => {
                if values.is_empty()
                    || values.len() > ENUM_VALUES_MAX
                    || values
                        .iter()
                        .any(|v| v.trim().is_empty() || v.len() > NAME_MAX)
                {
                    return Err(err(at("values"), FIELD_INVALID));
                }
                if d.colors
                    .iter()
                    .any(|(v, c)| !values.contains(v) || !COLORS.contains(&c.as_str()))
                {
                    return Err(err(at("colors"), FIELD_INVALID));
                }
                if d.labels.iter().any(|(v, names)| {
                    !values.contains(v)
                        || names.len() > LABEL_LANGS_MAX
                        || names
                            .iter()
                            .any(|(l, t)| l.len() > 8 || t.chars().count() > NAME_MAX)
                }) {
                    return Err(err(at("labels"), FIELD_INVALID));
                }
            }
            _ if !d.colors.is_empty() => return Err(err(at("colors"), FIELD_INVALID)),
            _ if !d.labels.is_empty() => return Err(err(at("labels"), FIELD_INVALID)),
            PropertyKind::Ref { to_type } if !known_type(to_type) => {
                return Err(err(at("to_type"), FIELD_INVALID));
            }
            PropertyKind::Number {
                min: Some(min),
                max: Some(max),
                ..
            } if min > max => return Err(err(at("min"), FIELD_INVALID)),
            PropertyKind::Text { max_len: Some(0) } => {
                return Err(err(at("max_len"), FIELD_INVALID));
            }
            PropertyKind::Events { stream } if stream.trim().is_empty() => {
                return Err(err(at("stream"), FIELD_REQUIRED));
            }
            _ => {}
        }
        if d.kind.is_temporal() && d.required {
            return Err(err(at("required"), FIELD_INVALID));
        }
        if d.unique
            && !matches!(
                d.kind,
                PropertyKind::Text { .. } | PropertyKind::Number { .. }
            )
        {
            return Err(err(at("unique"), FIELD_INVALID));
        }
    }
    Ok(())
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|p| p.parse::<u32>().ok());
    matches!((num(0..4), num(5..7), num(8..10)), (Some(_), Some(m), Some(d)) if (1..=12).contains(&m) && (1..=31).contains(&d))
}

/// RFC 3339 shape check (`YYYY-MM-DDTHH:MM:SS[.f](Z|±HH:MM)`); the backend
/// parses it again with chrono.
fn valid_date_time(s: &str) -> bool {
    let Some((date, time)) = s.split_once(['T', 't']) else {
        return false;
    };
    let tb = time.as_bytes();
    valid_date(date)
        && tb.len() >= 9
        && tb[2] == b':'
        && tb[5] == b':'
        && (time.ends_with(['Z', 'z']) || time[8..].contains(['+', '-']))
}

pub fn valid_http_url(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.has_host())
}

fn check_value(d: &PropertyDef, v: &Value) -> Result<Value, String> {
    let invalid = || FIELD_INVALID.to_string();
    match (&d.kind, v) {
        (PropertyKind::Text { max_len }, Value::String(s)) => {
            let max = max_len.unwrap_or(TEXT_MAX_DEFAULT);
            if s.chars().count() > max {
                return Err(format!("{FIELD_MAX_LENGTH}:{max}"));
            }
            Ok(Value::String(s.trim().to_string()))
        }
        (PropertyKind::Number { min, max, .. }, Value::Number(n)) => {
            let x = n.as_f64().ok_or_else(invalid)?;
            if min.is_some_and(|m| x < m) || max.is_some_and(|m| x > m) {
                return Err(invalid());
            }
            Ok(v.clone())
        }
        (PropertyKind::Bool, Value::Bool(_)) => Ok(v.clone()),
        (PropertyKind::Date, Value::String(s)) if valid_date(s) => Ok(v.clone()),
        (PropertyKind::DateTime, Value::String(s)) if valid_date_time(s) => Ok(v.clone()),
        (PropertyKind::Enum { values }, Value::String(s)) if values.contains(s) => Ok(v.clone()),
        (PropertyKind::GeoPoint, Value::Object(o)) => {
            let lat = o.get("lat").and_then(Value::as_f64).ok_or_else(invalid)?;
            let lon = o.get("lon").and_then(Value::as_f64).ok_or_else(invalid)?;
            if o.len() != 2 || !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                return Err(invalid());
            }
            Ok(v.clone())
        }
        (PropertyKind::GeoArea, Value::Object(_))
            if crate::geo_area::GeoArea::parse(v).is_some() =>
        {
            Ok(v.clone())
        }
        (PropertyKind::Url, Value::String(s)) if s.len() <= 2048 && valid_http_url(s) => {
            Ok(v.clone())
        }
        (PropertyKind::Ref { .. }, Value::String(s)) if uuid::Uuid::parse_str(s).is_ok() => {
            Ok(v.clone())
        }
        (PropertyKind::FuzzyTime, Value::Object(o)) => check_fuzzy(o).ok_or_else(invalid),
        (PropertyKind::Money, Value::Object(_)) => {
            Money::parse(v).map(|m| m.to_value()).ok_or_else(invalid)
        }
        (PropertyKind::Recurrence, Value::Object(_)) => super::recurrence::Recurrence::parse(v)
            .map(|r| r.to_value())
            .ok_or_else(invalid),
        _ => Err(invalid()),
    }
}

/// A `money` value. No implicit currency conversion anywhere: amounts add
/// up per currency only.
#[derive(Debug, Clone, PartialEq)]
pub struct Money {
    pub amount: f64,
    /// ISO 4217 code, upper case.
    pub currency: String,
    /// Known only as an estimate (a range, an order of magnitude).
    pub estimated: bool,
}

impl Money {
    pub fn parse(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let amount = o.get("amount")?.as_f64().filter(|a| a.is_finite())?;
        let currency = o.get("currency")?.as_str()?;
        let estimated = match o.get("estimated") {
            None => false,
            Some(e) => e.as_bool()?,
        };
        let known = 2 + usize::from(o.contains_key("estimated"));
        (o.len() == known
            && currency.len() == 3
            && currency.bytes().all(|b| b.is_ascii_uppercase()))
        .then(|| Self {
            amount,
            currency: currency.to_string(),
            estimated,
        })
    }

    pub fn to_value(&self) -> Value {
        let mut v = serde_json::json!({"amount": self.amount, "currency": self.currency});
        if self.estimated {
            v["estimated"] = Value::Bool(true);
        }
        v
    }
}

/// `fuzzy_time`: both bounds RFC 3339 in UTC (`Z`, so that they compare to
/// the second as text), `from <= to`, `instant` ⇒ `from == to`.
fn check_fuzzy(o: &Map<String, Value>) -> Option<Value> {
    let bound = |k: &str| {
        o.get(k)
            .and_then(Value::as_str)
            .filter(|s| valid_date_time(s) && s.ends_with(['Z', 'z']))
    };
    let (from, to) = (bound("from")?, bound("to")?);
    let precision = o.get("precision").and_then(Value::as_str)?;
    let second = |s: &str| s.get(..19).map(str::to_string);
    if o.len() != 3 || !FUZZY_PRECISIONS.contains(&precision) || second(from)? > second(to)? {
        return None;
    }
    if precision == "instant" && second(from)? != second(to)? {
        return None;
    }
    Some(Value::Object(o.clone()))
}

/// Bounds of a `fuzzy_time` entered as one start at a precision: `start` is
/// `YYYY-MM-DDTHH:MM` (UTC); the bounds cover the whole minute, hour, day,
/// month or year. `instant` is the start itself. `range` and `approx` take
/// both bounds from the user: `None` here.
pub fn fuzzy_bounds(precision: &str, start: &str) -> Option<(String, String)> {
    let num = |r: std::ops::Range<usize>| start.get(r)?.parse::<u32>().ok();
    let (y, mo, d, h, mi) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
    );
    if !valid_date(start.get(..10)?) || h > 23 || mi > 59 {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = match mo {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let at = |y: u32, mo: u32, d: u32, h: u32, mi: u32, s: u32| {
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    };
    Some(match precision {
        "instant" => (at(y, mo, d, h, mi, 0), at(y, mo, d, h, mi, 0)),
        "minute" => (at(y, mo, d, h, mi, 0), at(y, mo, d, h, mi, 59)),
        "hour" => (at(y, mo, d, h, 0, 0), at(y, mo, d, h, 59, 59)),
        "day" => (at(y, mo, d, 0, 0, 0), at(y, mo, d, 23, 59, 59)),
        "month" => (at(y, mo, 1, 0, 0, 0), at(y, mo, month_days, 23, 59, 59)),
        "year" => (at(y, 1, 1, 0, 0, 0), at(y, 12, 31, 23, 59, 59)),
        _ => return None,
    })
}

/// Validates a property document against its definitions and returns the
/// normalized document (`null` and empty strings drop the key). A `ref`
/// value is only checked for shape here: the backend checks the target.
pub fn check_values(
    defs: &[PropertyDef],
    values: &Map<String, Value>,
) -> Result<Map<String, Value>, FieldError> {
    let mut out = Map::new();
    for (k, v) in values {
        let Some(d) = defs.iter().find(|d| &d.key == k) else {
            return Err(err(k.clone(), FIELD_UNKNOWN));
        };
        if v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()) {
            continue;
        }
        if d.kind.is_temporal() {
            return Err(err(k.clone(), FIELD_INVALID));
        }
        out.insert(k.clone(), check_value(d, v).map_err(|t| err(k.clone(), t))?);
    }
    if let Some(d) = defs
        .iter()
        .find(|d| d.required && !out.contains_key(&d.key))
    {
        return Err(err(d.key.clone(), FIELD_REQUIRED));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(key: &str, kind: PropertyKind) -> PropertyDef {
        PropertyDef {
            names: Default::default(),
            key: key.into(),
            name: String::new(),
            kind,
            required: false,
            indexed: false,
            unique: false,
            pack: None,
            colors: Default::default(),
            labels: Default::default(),
            classification: Default::default(),
        }
    }

    fn pump() -> Vec<PropertyDef> {
        vec![
            PropertyDef {
                required: true,
                ..def("serial", PropertyKind::Text { max_len: Some(8) })
            },
            def(
                "flow",
                PropertyKind::Number {
                    unit: Some("m3/h".into()),
                    min: Some(0.0),
                    max: None,
                },
            ),
            def(
                "state",
                PropertyKind::Enum {
                    values: vec!["run".into(), "stop".into()],
                },
            ),
            def("since", PropertyKind::Date),
            def("seen", PropertyKind::DateTime),
            def("where", PropertyKind::GeoPoint),
            def("doc", PropertyKind::Url),
            def(
                "temperature",
                PropertyKind::Series {
                    metric: None,
                    unit: None,
                },
            ),
        ]
    }

    fn check(v: Value) -> Result<Map<String, Value>, FieldError> {
        check_values(&pump(), v.as_object().unwrap())
    }

    #[test]
    fn valid_document_is_normalized() {
        let out = check(json!({
            "serial": " P-12 ", "flow": 3.5, "state": "run", "since": "2026-03-01",
            "seen": "2026-03-01T10:00:00+02:00", "where": {"lat": 45.1, "lon": 5.7},
            "doc": "https://example.com/p12", "since_note": null
        }));
        assert_eq!(out, Err(("since_note".into(), FIELD_UNKNOWN.into())));
        let out = check(json!({"serial": " P-12 ", "flow": 3.5, "doc": ""})).unwrap();
        assert_eq!(out["serial"], json!("P-12"));
        assert!(!out.contains_key("doc"));
    }

    #[test]
    fn invalid_values_answer_tokens() {
        let e = |v: Value| check(v).unwrap_err();
        assert_eq!(e(json!({})), ("serial".into(), FIELD_REQUIRED.into()));
        assert_eq!(e(json!({"serial": "123456789"})).1, "max_length:8");
        assert_eq!(e(json!({"serial": "a", "flow": -1})).1, FIELD_INVALID);
        assert_eq!(e(json!({"serial": "a", "state": "x"})).1, FIELD_INVALID);
        assert_eq!(
            e(json!({"serial": "a", "since": "2026-13-01"})).1,
            FIELD_INVALID
        );
        assert_eq!(
            e(json!({"serial": "a", "seen": "2026-03-01"})).1,
            FIELD_INVALID
        );
        assert_eq!(
            e(json!({"serial": "a", "where": {"lat": 91, "lon": 0}})).1,
            FIELD_INVALID
        );
        assert_eq!(
            e(json!({"serial": "a", "doc": "javascript:alert(1)"})).1,
            FIELD_INVALID
        );
        assert_eq!(e(json!({"serial": "a", "temperature": 3})).1, FIELD_INVALID);
    }

    #[test]
    fn fuzzy_time_values() {
        let d = vec![def("occurred", PropertyKind::FuzzyTime)];
        let c = |v: Value| check_values(&d, json!({ "occurred": v }).as_object().unwrap());
        let ok = json!({"from": "2019-03-01T00:00:00Z", "to": "2019-03-31T23:59:59Z", "precision": "month"});
        assert_eq!(c(ok.clone()).unwrap()["occurred"], ok);
        let bad = [
            json!({"from": "2019-03-31T00:00:00Z", "to": "2019-03-01T00:00:00Z", "precision": "range"}),
            json!({"from": "2019-03-01T00:00:00Z", "to": "2019-03-01T00:00:01Z", "precision": "instant"}),
            json!({"from": "2019-03-01T00:00:00+02:00", "to": "2019-03-02T00:00:00Z", "precision": "day"}),
            json!({"from": "2019-03-01T00:00:00Z", "to": "2019-03-02T00:00:00Z", "precision": "week"}),
            json!({"from": "2019-03-01T00:00:00Z", "to": "2019-03-02T00:00:00Z", "precision": "day", "x": 1}),
            json!("2019-03-01"),
        ];
        for b in bad {
            assert_eq!(c(b.clone()).unwrap_err().1, FIELD_INVALID, "{b}");
        }
        let v = serde_json::to_value(&d[0]).unwrap();
        assert_eq!(serde_json::from_value::<PropertyDef>(v).unwrap(), d[0]);
    }

    #[test]
    fn money_values() {
        let d = vec![def("amount", PropertyKind::Money)];
        let c = |v: Value| check_values(&d, json!({ "amount": v }).as_object().unwrap());
        let ok = json!({"amount": 1250.5, "currency": "EUR"});
        assert_eq!(c(ok.clone()).unwrap()["amount"], ok);
        let est = json!({"amount": 3e6, "currency": "USD", "estimated": true});
        assert_eq!(c(est.clone()).unwrap()["amount"], est);
        // `estimated: false` is the default and is dropped.
        let f = c(json!({"amount": 1, "currency": "EUR", "estimated": false})).unwrap();
        assert_eq!(f["amount"], json!({"amount": 1.0, "currency": "EUR"}));
        let bad = [
            json!({"amount": 1, "currency": "eur"}),
            json!({"amount": 1, "currency": "EURO"}),
            json!({"amount": "1", "currency": "EUR"}),
            json!({"currency": "EUR"}),
            json!({"amount": 1, "currency": "EUR", "x": 1}),
            json!({"amount": 1, "currency": "EUR", "estimated": "yes"}),
            json!(12),
        ];
        for b in bad {
            assert_eq!(c(b.clone()).unwrap_err().1, FIELD_INVALID, "{b}");
        }
        let v = serde_json::to_value(&d[0]).unwrap();
        assert_eq!(serde_json::from_value::<PropertyDef>(v).unwrap(), d[0]);
    }

    #[test]
    fn fuzzy_bounds_cover_the_precision() {
        let b = |p: &str| fuzzy_bounds(p, "2024-02-10T21:35").unwrap();
        assert_eq!(
            b("instant"),
            ("2024-02-10T21:35:00Z".into(), "2024-02-10T21:35:00Z".into())
        );
        assert_eq!(b("hour").1, "2024-02-10T21:59:59Z");
        assert_eq!(
            b("day"),
            ("2024-02-10T00:00:00Z".into(), "2024-02-10T23:59:59Z".into())
        );
        assert_eq!(b("month").1, "2024-02-29T23:59:59Z");
        assert_eq!(
            fuzzy_bounds("month", "2023-02-10T00:00").unwrap().1,
            "2023-02-28T23:59:59Z"
        );
        assert_eq!(
            b("year"),
            ("2024-01-01T00:00:00Z".into(), "2024-12-31T23:59:59Z".into())
        );
        assert_eq!(fuzzy_bounds("range", "2024-02-10T21:35"), None);
        assert_eq!(fuzzy_bounds("day", "2024-13-10T21:35"), None);
        // Derived bounds always pass the validation.
        let d = vec![def("t", PropertyKind::FuzzyTime)];
        for p in ["instant", "minute", "hour", "day", "month", "year"] {
            let (from, to) = b(p);
            let v = json!({"t": {"from": from, "to": to, "precision": p}});
            assert!(check_values(&d, v.as_object().unwrap()).is_ok(), "{p}");
        }
    }

    #[test]
    fn colours_name_enum_values_from_the_palette() {
        let known = |_: &str| true;
        let status = |colors: &[(&str, &str)]| PropertyDef {
            colors: colors
                .iter()
                .map(|(v, c)| (v.to_string(), c.to_string()))
                .collect(),
            ..def(
                "status",
                PropertyKind::Enum {
                    values: vec!["client".into(), "target".into()],
                },
            )
        };
        assert!(check_defs(&[status(&[("client", "green")])], "p", &known).is_ok());
        for bad in [
            status(&[("client", "#00ff00")]),
            status(&[("other", "green")]),
        ] {
            assert_eq!(check_defs(&[bad], "p", &known).unwrap_err().0, "p.0.colors");
        }
        let text = PropertyDef {
            colors: [("a".to_string(), "green".to_string())].into(),
            ..def("t", PropertyKind::Text { max_len: None })
        };
        assert!(check_defs(&[text], "p", &known).is_err());
        assert_eq!(
            status(&[("client", "green")]).color_of(&json!("client")),
            Some("green")
        );
        assert_eq!(
            status(&[("client", "green")]).color_of(&json!("target")),
            None
        );
    }

    #[test]
    fn labels_translate_enum_values_without_renaming_them() {
        let known = |_: &str| true;
        let mut d = def(
            "status",
            PropertyKind::Enum {
                values: vec!["client".into(), "target".into()],
            },
        );
        d.labels.insert(
            "client".into(),
            [
                ("en".to_string(), "Customer".to_string()),
                ("fr".to_string(), "Client".to_string()),
            ]
            .into(),
        );
        assert!(check_defs(&[d.clone()], "p", &known).is_ok());
        assert_eq!(d.label_of("client", "fr-FR"), Some("Client"));
        assert_eq!(d.label_of("client", "de"), Some("Customer"));
        assert_eq!(d.label_of("target", "fr"), None);
        let mut bad = d.clone();
        bad.labels.insert("other".into(), Default::default());
        assert_eq!(check_defs(&[bad], "p", &known).unwrap_err().0, "p.0.labels");
        let back: PropertyDef = serde_json::from_value(serde_json::to_value(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn definitions_are_checked() {
        let known = |t: &str| t == "site";
        assert!(check_defs(&pump(), "properties", &known).is_ok());
        let bad = vec![def("Bad", PropertyKind::Bool)];
        assert_eq!(
            check_defs(&bad, "properties", &known).unwrap_err().0,
            "properties.0.key"
        );
        let dup = vec![def("a", PropertyKind::Bool), def("a", PropertyKind::Date)];
        assert_eq!(check_defs(&dup, "p", &known).unwrap_err().1, "duplicate");
        let r = vec![def(
            "site",
            PropertyKind::Ref {
                to_type: "nope".into(),
            },
        )];
        assert_eq!(check_defs(&r, "p", &known).unwrap_err().0, "p.0.to_type");
    }

    #[test]
    fn serde_shape_is_flat() {
        let d = def(
            "flow",
            PropertyKind::Number {
                unit: Some("m3/h".into()),
                min: None,
                max: None,
            },
        );
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(
            v,
            json!({"key": "flow", "name": "", "kind": "number", "unit": "m3/h", "required": false, "indexed": false})
        );
        assert_eq!(serde_json::from_value::<PropertyDef>(v).unwrap(), d);
    }
}
