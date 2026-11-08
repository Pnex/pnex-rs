//! Read-only SQL on tables and objects (analysis.md D825–D828). DTOs and pure
//! checks, wasm-safe.

use serde::{Deserialize, Serialize};

pub const SQL_MAX_BYTES: usize = 64 * 1024;
/// Rows a query returns at most (D826).
pub const ROWS_MAX: usize = 1_000;
/// Object types one query may read.
pub const OBJECT_TYPES_MAX: usize = 5;
/// Schema under which object types are read: `objects.<type>`.
pub const OBJECTS_SCHEMA: &str = "objects";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnalysisQueryInput {
    pub sql: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AnalysisState {
    Queued,
    Running,
    Done,
    Failed,
}

/// A query and, once done, its rows as text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnalysisQuery {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub state: AnalysisState,
    pub sql: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows than [`ROWS_MAX`]: only the first ones are here.
    #[serde(default)]
    pub truncated: bool,
    /// Machine code of a failure.
    pub code: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    #[serde(default)]
    pub args: serde_json::Value,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
}

/// The object types a query names as `objects.<type>` (case kept, keys are
/// lowercase in the ontology), without duplicates, in order of appearance.
pub fn object_types(sql: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = sql.to_ascii_lowercase();
    let prefix = format!("{OBJECTS_SCHEMA}.");
    let bytes = lower.as_bytes();
    let mut at = 0;
    while let Some(i) = lower[at..].find(&prefix) {
        let start = at + i;
        let word_start =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let key: String = lower[start + prefix.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if word_start && !key.is_empty() && !out.contains(&key) {
            out.push(key);
        }
        at = start + prefix.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_types_are_found_in_the_text() {
        assert_eq!(
            object_types("SELECT e.title FROM objects.employee e JOIN Objects.Team t ON true"),
            vec!["employee", "team"]
        );
        assert_eq!(
            object_types("SELECT * FROM objects.employee, objects.employee"),
            vec!["employee"]
        );
        assert!(object_types("SELECT * FROM my_objects.x, staff").is_empty());
        assert!(object_types("SELECT 'objects.' FROM t").is_empty());
    }
}
