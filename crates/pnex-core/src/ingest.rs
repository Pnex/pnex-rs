//! Open-data ingestion nodes (ingestion.md D317–D322): one flow node kind,
//! `ingest`, whose `step` picks what it does — resolve a source, download
//! it to the S3 staging area, extract an archive, inspect a table, filter
//! rows, run a declarative import (D301), clean the staging area. The
//! runtime holds no bytes and no credentials: each step calls
//! `POST /internal/flow/ingest` naming its deployed node; the backend reads
//! the config from the deployed graph, does the work and returns the
//! message fields plus a manifest (what came in, what went out).
//!
//! Flat config (no internally tagged enum: serde_json
//! `arbitrary_precision` breaks them), serde-only, wasm-safe.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ontology::flow::FlowNodeRef;
use crate::ontology::import::Convert;

/// Serialized size cap of a manifest (node panel, run log).
pub const MANIFEST_MAX_BYTES: usize = 64 * 1024;
/// Default `max_bytes` of a download.
pub const DOWNLOAD_MAX_BYTES: u64 = 500 * 1024 * 1024;
/// Staging bytes an org may hold at once (`staging_quota`).
pub const STAGING_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Age after which a run's staging objects are purged.
pub const STAGING_TTL_DAYS: i64 = 7;
/// Default retention of a downloaded raw file (proof of what was imported).
pub const KEEP_RAW_DAYS: u32 = 30;
/// Default share of what an import wrote that one run may close.
pub const MAX_CLOSURES_PCT: u32 = 20;
/// Values of an `equals`/`in` filter.
pub const FILTER_VALUES_MAX: usize = 1_000;
/// Globs of an `archive_extract`.
pub const ENTRY_GLOBS_MAX: usize = 32;

/// Output ports: 0 = done, 1 = error (`msg.error = {code, args}`), and for
/// `file_download` 2 = unchanged since the last successful run.
pub const PORT_OK: usize = 0;
pub const PORT_ERROR: usize = 1;
pub const PORT_UNCHANGED: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStep {
    #[default]
    SourceResolve,
    FileDownload,
    ArchiveExtract,
    TableInspect,
    RowsFilter,
    OntologyImport,
    StagingCleanup,
}

impl IngestStep {
    pub const ALL: [IngestStep; 7] = [
        Self::SourceResolve,
        Self::FileDownload,
        Self::ArchiveExtract,
        Self::TableInspect,
        Self::RowsFilter,
        Self::OntologyImport,
        Self::StagingCleanup,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::SourceResolve => "source_resolve",
            Self::FileDownload => "file_download",
            Self::ArchiveExtract => "archive_extract",
            Self::TableInspect => "table_inspect",
            Self::RowsFilter => "rows_filter",
            Self::OntologyImport => "ontology_import",
            Self::StagingCleanup => "staging_cleanup",
        }
    }

    pub fn port_count(self) -> usize {
        match self {
            Self::FileDownload => 3,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveMode {
    /// A fixed URL.
    #[default]
    Url,
    /// The most recent resource of a data.gouv.fr dataset matching a glob.
    Datagouv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    #[default]
    Equals,
    In,
    /// The values of a property on the org's live objects of a type.
    InObjects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportAction {
    /// Computes the difference and stops (visible in the Imports tab).
    #[default]
    Preview,
    Apply,
}

/// Config of an `ingest` node. Only the fields of its `step` are read.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IngestConfig {
    #[serde(default)]
    pub step: IngestStep,
    // ── source_resolve ──
    #[serde(default)]
    pub mode: ResolveMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// data.gouv.fr dataset id or slug.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dataset: String,
    /// Glob on the resource title (`export-fiches-csv-*.zip`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_match: String,
    // ── file_download ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Same sha256 as the last successful download → port 2 (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_if_unchanged: Option<bool>,
    /// Days the raw file stays after the run (default 30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_raw_days: Option<u32>,
    // ── archive_extract ──
    /// Globs of the entries to extract; empty = all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<String>,
    // ── table_inspect / rows_filter / ontology_import ──
    /// Glob picking one of `msg.entries`; empty = `msg.staging_key`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entry: String,
    /// table_inspect: a missing column → `import-schema-changed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_columns: Vec<String>,
    // ── rows_filter ──
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub column: String,
    #[serde(default)]
    pub op: FilterOp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// `in_objects`: type and property whose values are kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub object_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub property: String,
    /// `in_objects`: only the objects whose property holds one of these
    /// values (the active certifications, not every one known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<crate::ontology::import::PerObjectOnly>,
    /// D301 conversions applied to the column before comparing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub convert: Vec<Convert>,
    // ── ontology_import ──
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub import_id: String,
    #[serde(default)]
    pub action: ImportAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_closures_pct: Option<u32>,
}

fn http_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") || s.starts_with("http://")) && s.len() <= 2048
}

impl IngestConfig {
    pub fn port_count(&self) -> usize {
        self.step.port_count()
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.unwrap_or(DOWNLOAD_MAX_BYTES)
    }

    pub fn keep_raw_days(&self) -> u32 {
        self.keep_raw_days.unwrap_or(KEEP_RAW_DAYS)
    }

    pub fn max_closures_pct(&self) -> u32 {
        self.max_closures_pct.unwrap_or(MAX_CLOSURES_PCT)
    }

    /// Save check (editor and backend): what is filled is well formed. An
    /// unset field saves as a draft; [`Self::check_deployable`] refuses it.
    pub fn check(&self) -> Option<(&'static str, &'static str)> {
        let bad = |m| Some(("ingest_config_invalid", m));
        let url_bad = !self.url.trim().is_empty() && !http_url(&self.url);
        match self.step {
            IngestStep::SourceResolve | IngestStep::FileDownload if url_bad => {
                bad("the URL must be http(s), 2,048 characters at most")
            }
            IngestStep::SourceResolve if self.resource_match.len() > 200 => {
                bad("the resource pattern is 200 characters at most")
            }
            IngestStep::FileDownload => match self.max_bytes {
                Some(0) => bad("max_bytes must be positive"),
                Some(n) if n > 4 * DOWNLOAD_MAX_BYTES => bad("max_bytes is capped at 2 GB"),
                _ if self.keep_raw_days.is_some_and(|d| d > 365) => {
                    bad("keep_raw_days is capped at 365")
                }
                _ => None,
            },
            IngestStep::ArchiveExtract
                if self.entries.len() > ENTRY_GLOBS_MAX
                    || self
                        .entries
                        .iter()
                        .any(|g| g.trim().is_empty() || g.len() > 200) =>
            {
                bad("entry patterns: 1–200 characters, 32 at most")
            }
            IngestStep::TableInspect
                if self.expected_columns.iter().any(|c| c.trim().is_empty()) =>
            {
                bad("expected columns cannot be empty")
            }
            IngestStep::RowsFilter if self.values.len() > FILTER_VALUES_MAX => {
                bad("1,000 values at most")
            }
            IngestStep::RowsFilter if self.op == FilterOp::Equals && self.values.len() > 1 => {
                bad("equals takes one value")
            }
            IngestStep::RowsFilter
                if self
                    .only
                    .as_ref()
                    .is_some_and(|o| o.values.len() > FILTER_VALUES_MAX) =>
            {
                bad("1,000 values at most")
            }
            IngestStep::OntologyImport
                if !self.import_id.trim().is_empty()
                    && uuid::Uuid::parse_str(self.import_id.trim()).is_err() =>
            {
                bad("invalid import id")
            }
            IngestStep::OntologyImport if self.max_closures_pct.is_some_and(|p| p > 100) => {
                bad("max_closures_pct is a percentage")
            }
            _ => None,
        }
    }

    /// Deploy check: [`Self::check`] plus the fields the step needs.
    pub fn check_deployable(&self) -> Option<(&'static str, &'static str)> {
        let unset = |m| Some(("ingest_config_unset", m));
        self.check().or(match self.step {
            IngestStep::SourceResolve => match self.mode {
                ResolveMode::Url if self.url.trim().is_empty() => {
                    unset("an http(s) URL is required")
                }
                ResolveMode::Datagouv
                    if self.dataset.trim().is_empty() || self.resource_match.trim().is_empty() =>
                {
                    unset("a data.gouv.fr dataset and a resource pattern are required")
                }
                _ => None,
            },
            IngestStep::RowsFilter if self.column.trim().is_empty() => {
                unset("a column is required")
            }
            IngestStep::RowsFilter => match self.op {
                FilterOp::Equals | FilterOp::In if self.values.is_empty() => {
                    unset("the values to keep are required")
                }
                FilterOp::InObjects
                    if self.object_type.trim().is_empty() || self.property.trim().is_empty() =>
                {
                    unset("in_objects needs a type and a property")
                }
                FilterOp::InObjects
                    if self.only.as_ref().is_some_and(|o| {
                        o.property.trim().is_empty()
                            || !o.values.iter().any(|v| !v.trim().is_empty())
                    }) =>
                {
                    unset("in_objects only needs a property and its values")
                }
                _ => None,
            },
            IngestStep::OntologyImport if self.import_id.trim().is_empty() => {
                unset("pick an import")
            }
            _ => None,
        })
    }
}

/// A file of the staging area, as carried by the messages.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StagedEntry {
    pub name: String,
    pub staging_key: String,
    #[serde(default)]
    pub bytes: u64,
}

/// The ingestion fields of a flow message (`msg.ingest`), carried from node
/// to node. Keys only, never bytes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IngestMsg {
    /// One id per pass of the chain; created by the first step.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    /// Resolved source URL (source_resolve → file_download).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Downloaded raw file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_key: String,
    /// The current file (raw, extracted entry or filtered table).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub staging_key: String,
    /// Extracted entries (archive_extract).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<StagedEntry>,
}

/// `POST /internal/flow/ingest`: the node and the message fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestRequest {
    pub node: FlowNodeRef,
    #[serde(default)]
    pub msg: IngestMsg,
}

/// Answer of a step: the next message fields, the output port and the
/// manifest. A step that fails answers 200 too, on port 1 with its code:
/// a broken source is a flow outcome, not a transport error.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IngestReply {
    pub port: usize,
    #[serde(default)]
    pub msg: IngestMsg,
    #[serde(default)]
    pub manifest: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub args: Value,
    /// The step still runs on the server: the node sends the same request
    /// again (with this `msg`, which carries the pass id) until it is done.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// One journaled step (`GET /flows/{id}/ingest-steps`): node panel and
/// run log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct IngestStepView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// One id per pass of the chain.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub run_id: String,
    pub node_id: String,
    pub flow_version: i64,
    /// `source_resolve`, `file_download`, `archive_extract`, `table_inspect`,
    /// `rows_filter`, `ontology_import` or `staging_cleanup`.
    pub step: String,
    /// 0 done, 1 error, 2 unchanged.
    pub port: u32,
    /// Machine code of an error (`err-<code>` in the UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// What the step received and produced (64 KB at most).
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub manifest: Value,
    /// Staging bytes the step wrote.
    pub bytes: u64,
    /// Its staging files are deleted.
    pub purged: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// Glob over a file name: `*` (any run), `?` (one character), the rest
/// literal, case-insensitive. A pattern without `/` matches the base name.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let name = if pattern.contains('/') {
        name
    } else {
        name.rsplit('/').next().unwrap_or(name)
    };
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    // Iterative wildcard match with one backtrack point.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

/// A manifest cut to [`MANIFEST_MAX_BYTES`]: arrays of an oversized object
/// are truncated (with `truncated: true`) until it fits.
pub fn bounded_manifest(mut v: Value) -> Value {
    let size = |v: &Value| serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());
    while size(&v) > MANIFEST_MAX_BYTES {
        let Some(obj) = v.as_object_mut() else {
            return serde_json::json!({ "truncated": true });
        };
        let longest = obj
            .iter()
            .filter_map(|(k, x)| x.as_array().map(|a| (k.clone(), a.len())))
            .max_by_key(|(_, n)| *n);
        match longest {
            Some((k, n)) if n > 1 => {
                if let Some(a) = obj.get_mut(&k).and_then(Value::as_array_mut) {
                    a.truncate(n / 2);
                }
                obj.insert("truncated".into(), Value::Bool(true));
            }
            _ => return serde_json::json!({ "truncated": true }),
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_base_names() {
        assert!(glob_match(
            "export-fiches-csv-*.zip",
            "export-fiches-csv-2026-10-20.zip"
        ));
        assert!(glob_match(
            "*Partenaires*.csv",
            "export/export_fiches_CSV_Partenaires_2026.csv"
        ));
        assert!(glob_match("*.CSV", "a.csv"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("*Standard*.csv", "Partenaires.csv"));
        assert!(!glob_match("a", "ab"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn check_per_step() {
        let mut c = IngestConfig::default();
        assert!(c.check().is_none(), "an unset url saves as a draft");
        assert!(c.check_deployable().is_some(), "but does not deploy");
        c.url = "ftp://x".into();
        assert!(c.check().is_some());
        c.url = "https://www.data.gouv.fr/x.zip".into();
        assert!(c.check_deployable().is_none());
        c.mode = ResolveMode::Datagouv;
        c.dataset = "5eebbc067a14b6fecc9c9976".into();
        assert!(c.check_deployable().is_some(), "no resource pattern");
        c.resource_match = "export-fiches-csv-*.zip".into();
        assert!(c.check_deployable().is_none());

        let f = IngestConfig {
            step: IngestStep::RowsFilter,
            column: "Numero_Fiche".into(),
            op: FilterOp::InObjects,
            ..Default::default()
        };
        assert!(f.check_deployable().is_some(), "in_objects without type");
        let f = IngestConfig {
            object_type: "certification".into(),
            property: "code".into(),
            ..f
        };
        assert!(f.check_deployable().is_none());
        let only = |property: &str, values: &[&str]| IngestConfig {
            only: Some(crate::ontology::import::PerObjectOnly {
                property: property.into(),
                values: values.iter().map(|v| v.to_string()).collect(),
            }),
            ..f.clone()
        };
        assert!(only("statut", &["actif"]).check_deployable().is_none());
        assert!(
            only("", &["actif"]).check_deployable().is_some(),
            "no property"
        );
        assert!(
            only("statut", &[" "]).check_deployable().is_some(),
            "no value"
        );
        let parsed: IngestConfig = serde_json::from_value(serde_json::json!({
            "step": "rows_filter", "column": "Numero_Fiche", "op": "in_objects",
            "object_type": "certification", "property": "code",
            "only": {"property": "statut", "values": ["actif"]},
        }))
        .unwrap();
        assert_eq!(parsed, only("statut", &["actif"]));
        let empty_in = IngestConfig {
            op: FilterOp::In,
            ..f
        };
        assert!(empty_in.check().is_none() && empty_in.check_deployable().is_some());

        let i = IngestConfig {
            step: IngestStep::OntologyImport,
            import_id: "nope".into(),
            ..Default::default()
        };
        assert!(i.check().is_some());
        assert_eq!(IngestStep::FileDownload.port_count(), 3);
    }

    #[test]
    fn config_round_trips_with_conversions() {
        let v = serde_json::json!({
            "step": "rows_filter", "column": "Siret_Certificateur", "op": "in",
            "values": ["80259488700019"], "convert": ["digits"]
        });
        let c: IngestConfig = serde_json::from_value(v).unwrap();
        assert_eq!(c.convert, vec![Convert::Digits]);
        let back: IngestConfig = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn manifest_is_bounded() {
        let big: Vec<Value> = (0..20_000)
            .map(|i| Value::String(format!("entry-{i}")))
            .collect();
        let m = bounded_manifest(serde_json::json!({ "rows": 3, "entries": big }));
        assert!(serde_json::to_vec(&m).unwrap().len() <= MANIFEST_MAX_BYTES);
        assert_eq!(m["truncated"], true);
        assert_eq!(m["rows"], 3);
    }
}
