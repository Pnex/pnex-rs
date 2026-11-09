//! Datasets (data-pipelines.md D569): DTOs of the catalogue. Pure, wasm-safe.

use serde::{Deserialize, Serialize};

/// Max length of a dataset name.
pub const NAME_MAX: usize = 255;
/// Rows a sample answers at most.
pub const SAMPLE_ROWS_MAX: usize = 200;
/// Characters a sampled value keeps.
pub const SAMPLE_VALUE_MAX: usize = 1000;
/// Identity columns a dataset declares at most.
pub const IDENTITY_COLUMNS_MAX: usize = 32;
/// `DatasetIdentityColumn::property` naming the title of the object.
pub const IDENTITY_TITLE: &str = "$title";

/// What is kept of an erased value, compared without case or accents: « Élodie » and
/// « ELODIE » are one person. An empty value has none, it would match every empty cell.
pub fn fingerprint(value: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let key = crate::fold::fold_words(value).join(" ");
    (!key.is_empty()).then(|| hex::encode(Sha256::digest(key.as_bytes())))
}

/// What a transaction does to the view: replace it, or add to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum DatasetTransactionKind {
    Snapshot,
    Append,
}

impl DatasetTransactionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Append => "append",
        }
    }
}

/// Only `committed` transactions are visible to readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum DatasetTransactionState {
    Open,
    Committed,
    Aborted,
}

/// A column of a dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetColumn {
    pub name: String,
    /// Stored type. A loaded file is all `text`.
    pub data_type: String,
}

/// A column whose value identifies an object: erasing the object removes the
/// rows where the column equals its property, ignoring case and accents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetIdentityColumn {
    pub column: String,
    /// Object type it identifies.
    pub type_key: String,
    /// Property of that type, or `$title` for the title of the object.
    pub property: String,
}

/// The identity columns of a dataset, replacing the current list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetIdentityInput {
    pub columns: Vec<DatasetIdentityColumn>,
}

/// A dataset and its current view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Dataset {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub columns: Vec<DatasetColumn>,
    /// Columns through which erasing an object removes its rows.
    pub identity: Vec<DatasetIdentityColumn>,
    /// Rows of the current view.
    pub rows: i64,
    /// Parquet bytes of the current view.
    pub bytes: i64,
    /// Latest commit, absent before the first one.
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub committed_at: Option<String>,
    /// Longest wait for a new version before the org is warned (D576).
    pub max_age_hours: Option<i32>,
    /// `max_age_hours` has passed without a new version.
    pub stale: bool,
    /// The import that fills it: neither a load nor a pipeline may write it.
    pub import_name: Option<String>,
    /// Keys of its markings (governance.md D800): reading its rows needs a clearance
    /// for each.
    #[serde(default)]
    pub markings: Vec<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

/// How often a dataset must get a new version; `null` stops the check.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetFreshnessInput {
    pub max_age_hours: Option<i32>,
}

/// Longest wait a freshness check accepts: a year.
pub const FRESHNESS_HOURS_MAX: i32 = 24 * 366;

/// Creation of a dataset.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetInput {
    pub name: String,
}

/// One transaction of a dataset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetTransaction {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub kind: DatasetTransactionKind,
    pub state: DatasetTransactionState,
    pub rows: i64,
    pub bytes: i64,
    /// Machine code of the failure of an aborted transaction.
    pub code: Option<String>,
    /// Outcome of the checks of the transform that wrote it (D576).
    pub checks: Vec<crate::pipelines::CheckResult>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub committed_at: Option<String>,
}

/// Largest part of an upload, under the request limit of proxies (Cloudflare: 100 MB).
pub const UPLOAD_PART_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const UPLOAD_PARTS_MAX: i32 = 1000;
/// Largest source file a load accepts.
pub const SOURCE_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum DatasetUploadState {
    /// Receiving parts.
    Open,
    /// Complete, waiting for a server.
    Ready,
    Converting,
    Done,
    Failed,
}

impl DatasetUploadState {
    pub fn ended(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// A Parquet file of a view, read whole by `GET /datasets/{id}/files/{index}`
/// (markets-research.md D823): training code reads the rows without the JSON API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetFile {
    /// Position in the view, from 0, in commit order.
    pub index: i64,
    /// The version that wrote it.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub transaction_id: String,
    pub rows: i64,
    pub bytes: i64,
}

/// A file sent in several requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetUpload {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub dataset_id: String,
    pub kind: DatasetTransactionKind,
    pub delimiter: Option<String>,
    pub state: DatasetUploadState,
    /// Parts received.
    pub parts: i32,
    /// Bytes received.
    pub bytes: i64,
    /// The version it became, once done.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub transaction_id: Option<String>,
    /// Machine code of the failure.
    pub code: Option<String>,
    /// Arguments of the failure.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub args: Option<serde_json::Value>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

/// Opens an upload. Default kind: snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetUploadInput {
    pub kind: Option<DatasetTransactionKind>,
    /// One ASCII character. Default: detected from the first line.
    pub delimiter: Option<String>,
}

/// Closes an upload: parts 1 to `parts` make the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetUploadComplete {
    pub parts: i32,
}

/// First rows of a view, values as text (`null` for a missing one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetSample {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

/// How full each column of the current view is, read from the file footers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetProfile {
    pub rows: i64,
    pub columns: Vec<DatasetColumnProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetColumnProfile {
    pub name: String,
    pub data_type: String,
    /// Empty values; absent when a file does not record them.
    pub nulls: Option<i64>,
}

impl DatasetProfile {
    /// Share of filled values of a column, in percent, rounded down so that a column
    /// with a single gap never reads 100.
    pub fn filled_pct(&self, column: &str) -> Option<i64> {
        let nulls = self.columns.iter().find(|c| c.name == column)?.nulls?;
        (self.rows > 0).then(|| (self.rows - nulls).max(0) * 100 / self.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::fingerprint;
    use super::{DatasetColumnProfile, DatasetProfile};

    #[test]
    fn a_fill_rate_never_rounds_a_gap_away() {
        let profile = |rows, nulls| DatasetProfile {
            rows,
            columns: vec![DatasetColumnProfile {
                name: "a".into(),
                data_type: "text".into(),
                nulls,
            }],
        };
        assert_eq!(profile(1000, Some(1)).filled_pct("a"), Some(99));
        assert_eq!(profile(4, Some(0)).filled_pct("a"), Some(100));
        assert_eq!(profile(4, Some(4)).filled_pct("a"), Some(0));
        assert_eq!(profile(0, Some(0)).filled_pct("a"), None);
        assert_eq!(profile(4, None).filled_pct("a"), None);
        assert_eq!(profile(4, Some(0)).filled_pct("b"), None);
    }

    #[test]
    fn one_person_has_one_fingerprint() {
        assert_eq!(
            fingerprint("  Élodie  MARTIN "),
            fingerprint("elodie martin")
        );
        assert_ne!(fingerprint("elodie"), fingerprint("martin"));
        assert_eq!(fingerprint(" -- "), None);
    }
}
