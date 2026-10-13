//! Time ranges (media-ingest.md D169, D182): named intervals of a scope —
//! a show, a shift, a production batch — announced (`planned_*`) and/or
//! realigned (`actual_*`). Pure, wasm-safe.
//!
//! - DTOs: [`TimeRange`], [`TimeRangeInput`], [`RangeImportResult`];
//! - [`ScopeKind`], [`RangeOrigin`]: closed sets stored as varchar;
//! - [`is_http_url`]: `source_url` is checked at write AND at render (R11);
//! - [`RangeUpsertConfig`]: the `range_upsert` flow node;
//! - [`RangeUpsertRequest`]: body of `POST /internal/flow/time-range`.

use serde::{Deserialize, Serialize};

pub const LABEL_MAX: usize = 200;
pub const EXTERNAL_ID_MAX: usize = 200;
pub const CATEGORY_MAX: usize = 64;
pub const SOURCE_URL_MAX: usize = 2048;
/// Serialized size cap of `attrs`.
pub const ATTRS_MAX_BYTES: usize = 8 * 1024;
/// Import body cap (CSV or ICS).
pub const IMPORT_MAX_BYTES: usize = 1024 * 1024;
/// Rows (CSV) or events (ICS) per import.
pub const IMPORT_MAX_ROWS: usize = 5000;
/// Row-level reasons returned by an import (the counts stay exact).
pub const IMPORT_ERRORS_MAX: usize = 100;

/// What a range belongs to: a media stream of the org, the org, or any
/// ontology object (D182: a line's shift, a machine's batch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Stream,
    Org,
    Object,
}

impl ScopeKind {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Org => "org",
            Self::Object => "object",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "stream" => Some(Self::Stream),
            "org" => Some(Self::Org),
            "object" => Some(Self::Object),
            _ => None,
        }
    }
}

/// Where a range comes from (D169).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeOrigin {
    Epg,
    Grid,
    Detected,
    Manual,
    Import,
}

impl RangeOrigin {
    pub const ALL: [RangeOrigin; 5] = [
        Self::Epg,
        Self::Grid,
        Self::Detected,
        Self::Manual,
        Self::Import,
    ];
    /// Origins a flow node may write (manual and import are the UI's).
    pub const FLOW: [RangeOrigin; 3] = [Self::Grid, Self::Detected, Self::Epg];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Epg => "epg",
            Self::Grid => "grid",
            Self::Detected => "detected",
            Self::Manual => "manual",
            Self::Import => "import",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.wire() == s)
    }
}

/// True for an absolute `http(s)` URL with a host — the only links shown.
pub fn is_http_url(raw: &str) -> bool {
    url::Url::parse(raw.trim())
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// Provenance of a range written by a flow (D184).
pub fn flow_ref(flow_id: i64, version: i64) -> String {
    format!("flow:{flow_id}@{version}")
}

/// Read DTO of a range. Timestamps are RFC 3339.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    pub id: String,
    pub scope_kind: ScopeKind,
    pub scope_id: String,
    pub label: String,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub planned_start: Option<String>,
    #[serde(default)]
    pub planned_end: Option<String>,
    #[serde(default)]
    pub actual_start: Option<String>,
    #[serde(default)]
    pub actual_end: Option<String>,
    pub origin: RangeOrigin,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub attrs: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

/// Lifecycle of an occurrence (ontology.md annex B), computed, never
/// stored: `cancelled` / `done` come from `attrs.status` or the actual end;
/// `late` = planned end passed; `due` = within `attrs.lead_days` of the
/// planned start.
pub fn occurrence_state(r: &TimeRange, now: chrono::DateTime<chrono::Utc>) -> &'static str {
    let at = |s: &Option<String>| {
        s.as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    };
    let status = r.attrs.get("status").and_then(|s| s.as_str());
    if status == Some("cancelled") {
        return "cancelled";
    }
    if r.actual_end.is_some() || status == Some("done") {
        return "done";
    }
    if at(&r.planned_end).is_some_and(|e| e < now) {
        return "late";
    }
    let lead = r
        .attrs
        .get("lead_days")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if at(&r.planned_start).is_some_and(|s| s - chrono::Duration::days(lead) <= now) {
        return "due";
    }
    "planned"
}

/// Create / update body (and the payload of `range_upsert`). On update,
/// an absent field keeps its value and an empty string clears it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimeRangeInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<serde_json::Value>,
}

/// One rejected import row: `line` in the file, field + machine token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeImportError {
    pub line: u32,
    pub field: String,
    pub token: String,
}

/// Outcome of `POST /api/v1/time-ranges/import`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeImportResult {
    pub created: u32,
    pub updated: u32,
    pub rejected: u32,
    /// The first [`IMPORT_ERRORS_MAX`] rejected rows.
    #[serde(default)]
    pub errors: Vec<RangeImportError>,
}

/// Configuration of the `range_upsert` flow node (D169): the scope and the
/// origin are fixed on the node, the range comes in `msg.payload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeUpsertConfig {
    #[serde(default = "default_scope_kind")]
    pub scope_kind: String,
    /// Stream id when `scope_kind = stream` (checked in the org at deploy).
    #[serde(default)]
    pub scope_id: String,
    #[serde(default = "default_origin")]
    pub origin: String,
}

fn default_scope_kind() -> String {
    ScopeKind::Stream.wire().into()
}

fn default_origin() -> String {
    RangeOrigin::Grid.wire().into()
}

impl Default for RangeUpsertConfig {
    fn default() -> Self {
        Self {
            scope_kind: default_scope_kind(),
            scope_id: String::new(),
            origin: default_origin(),
        }
    }
}

impl RangeUpsertConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        match ScopeKind::from_wire(&self.scope_kind) {
            Some(ScopeKind::Stream) if uuid::Uuid::parse_str(self.scope_id.trim()).is_err() => {
                return Some(("range_upsert_scope_invalid", "select a stream".into()));
            }
            Some(_) => {}
            None => {
                return Some((
                    "range_upsert_scope_invalid",
                    format!("unknown scope kind `{}`", self.scope_kind),
                ))
            }
        }
        let origin_ok =
            RangeOrigin::from_wire(&self.origin).is_some_and(|o| RangeOrigin::FLOW.contains(&o));
        if !origin_ok {
            return Some((
                "range_upsert_origin_invalid",
                "origin must be grid, detected or epg".into(),
            ));
        }
        None
    }
}

/// Body of `POST /internal/flow/time-range` (flow runtime → backend). The
/// backend re-checks everything: scope in the org, origin, fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeUpsertRequest {
    pub org_id: i64,
    pub flow_id: i64,
    pub version: i64,
    #[serde(default)]
    pub node_id: String,
    pub scope_kind: String,
    #[serde(default)]
    pub scope_id: String,
    pub origin: String,
    pub range: TimeRangeInput,
}

/// Answer of the internal endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeUpsertAck {
    pub id: String,
    pub created: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_urls_only() {
        assert!(is_http_url("https://radio.example/grille"));
        assert!(is_http_url(" http://a.example "));
        for bad in [
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://a.example",
            "/relative",
            "",
        ] {
            assert!(!is_http_url(bad), "{bad}");
        }
    }

    #[test]
    fn node_config_checks_scope_and_origin() {
        let stream = "6f1c8a52-3b8e-4c1a-9d0e-7a2b5c4d3e21";
        let ok = RangeUpsertConfig {
            scope_id: stream.into(),
            ..Default::default()
        };
        assert_eq!(ok.check(), None);
        assert_eq!(ok.origin, "grid");
        let org = RangeUpsertConfig {
            scope_kind: "org".into(),
            origin: "detected".into(),
            ..Default::default()
        };
        assert_eq!(org.check(), None);
        let code = |c: RangeUpsertConfig| c.check().map(|(code, _)| code);
        assert_eq!(
            code(RangeUpsertConfig::default()),
            Some("range_upsert_scope_invalid")
        );
        assert_eq!(
            code(RangeUpsertConfig {
                scope_kind: "device".into(),
                ..ok.clone()
            }),
            Some("range_upsert_scope_invalid")
        );
        for origin in ["manual", "import", "x"] {
            assert_eq!(
                code(RangeUpsertConfig {
                    origin: origin.into(),
                    ..ok.clone()
                }),
                Some("range_upsert_origin_invalid"),
                "{origin}"
            );
        }
    }

    #[test]
    fn wire_roundtrips() {
        for o in RangeOrigin::ALL {
            assert_eq!(RangeOrigin::from_wire(o.wire()), Some(o));
        }
        assert_eq!(ScopeKind::from_wire("org"), Some(ScopeKind::Org));
        assert_eq!(flow_ref(4, 2), "flow:4@2");
    }

    #[test]
    fn occurrence_states() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-03-10T12:00:00Z")
            .unwrap()
            .to_utc();
        let r = |start: &str, end: &str, attrs: serde_json::Value| TimeRange {
            id: String::new(),
            scope_kind: ScopeKind::Object,
            scope_id: String::new(),
            label: String::new(),
            external_id: None,
            category: None,
            planned_start: Some(start.into()),
            planned_end: Some(end.into()),
            actual_start: None,
            actual_end: None,
            origin: RangeOrigin::Grid,
            confidence: None,
            source_url: None,
            source_ref: None,
            attrs,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let s = |x: &TimeRange| occurrence_state(x, now);
        assert_eq!(
            s(&r(
                "2026-03-01T00:00:00Z",
                "2026-03-02T00:00:00Z",
                serde_json::json!({})
            )),
            "late"
        );
        assert_eq!(
            s(&r(
                "2026-03-20T00:00:00Z",
                "2026-03-21T00:00:00Z",
                serde_json::json!({})
            )),
            "planned"
        );
        let lead = serde_json::json!({"lead_days": 14});
        assert_eq!(
            s(&r("2026-03-20T00:00:00Z", "2026-03-21T00:00:00Z", lead)),
            "due"
        );
        let done = serde_json::json!({"status": "done"});
        assert_eq!(
            s(&r("2026-03-01T00:00:00Z", "2026-03-02T00:00:00Z", done)),
            "done"
        );
        let cancelled = serde_json::json!({"status": "cancelled"});
        assert_eq!(
            s(&r(
                "2026-03-01T00:00:00Z",
                "2026-03-02T00:00:00Z",
                cancelled
            )),
            "cancelled"
        );
    }
}
