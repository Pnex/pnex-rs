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
    (!key.is_empty()).then(|| format!("{:x}", Sha256::digest(key.as_bytes())))
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
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

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
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub committed_at: Option<String>,
}

/// First rows of a view, values as text (`null` for a missing one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetSample {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

#[cfg(test)]
mod tests {
    use super::fingerprint;

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
