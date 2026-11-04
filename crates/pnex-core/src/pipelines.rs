//! Pipelines (data-pipelines.md D571): the document, its validation and the
//! DTOs of the API. Pure, wasm-safe. The YAML is parsed by the callers.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

pub const NAME_MAX: usize = 255;
pub const TRANSFORMS_MAX: usize = 50;
pub const INPUTS_MAX: usize = 32;
pub const ALIAS_MAX: usize = 63;
pub const SQL_MAX_BYTES: usize = 64 * 1024;
pub const DOCUMENT_MAX_BYTES: usize = 1024 * 1024;

/// What a pipeline computes, transform by transform, in order. A transform may
/// read the output of an earlier one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineDocument {
    pub transforms: Vec<Transform>,
}

/// One SQL query from datasets to a dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Dataset it writes, created on the first run.
    pub output: String,
    /// SQL table name → dataset read. Nothing else is visible to the query.
    pub inputs: BTreeMap<String, String>,
    pub sql: String,
}

/// A problem of a document: where, and a machine token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineViolation {
    pub path: String,
    pub token: String,
}

fn valid_alias(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= ALIAS_MAX
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_name(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= NAME_MAX
}

impl PipelineDocument {
    /// Empty when the document can run.
    pub fn violations(&self) -> Vec<PipelineViolation> {
        let mut out = Vec::new();
        let mut bad = |path: String, token: &str| {
            out.push(PipelineViolation {
                path,
                token: token.to_string(),
            })
        };
        if self.transforms.is_empty() {
            bad("transforms".into(), "required");
        }
        if self.transforms.len() > TRANSFORMS_MAX {
            bad("transforms".into(), &format!("max_items:{TRANSFORMS_MAX}"));
        }
        let outputs: Vec<&str> = self.transforms.iter().map(|t| t.output.as_str()).collect();
        let mut seen: HashSet<&str> = HashSet::new();
        for (i, t) in self.transforms.iter().enumerate() {
            if !valid_name(&t.output) {
                bad(format!("transforms.{i}.output"), "invalid");
            }
            if !seen.insert(&t.output) {
                bad(format!("transforms.{i}.output"), "duplicate");
            }
            if t.sql.trim().is_empty() {
                bad(format!("transforms.{i}.sql"), "required");
            }
            if t.sql.len() > SQL_MAX_BYTES {
                bad(
                    format!("transforms.{i}.sql"),
                    &format!("max_bytes:{SQL_MAX_BYTES}"),
                );
            }
            if t.inputs.is_empty() {
                bad(format!("transforms.{i}.inputs"), "required");
            }
            if t.inputs.len() > INPUTS_MAX {
                bad(
                    format!("transforms.{i}.inputs"),
                    &format!("max_items:{INPUTS_MAX}"),
                );
            }
            for (alias, dataset) in &t.inputs {
                let path = format!("transforms.{i}.inputs.{alias}");
                if !valid_alias(alias) || !valid_name(dataset) {
                    bad(path.clone(), "invalid");
                }
                // Reading an output written later, or its own, is a cycle (D575).
                if outputs[i..].contains(&dataset.as_str()) {
                    bad(path, "cycle");
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Pipeline {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub version: i32,
    /// The document, in YAML.
    pub document: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineInput {
    pub name: String,
    /// The document, in YAML.
    pub document: String,
}

/// A new version, written over the version that was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineUpdate {
    pub document: String,
    pub expected_version: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineVersion {
    pub version: i32,
    pub document: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum PipelineRunState {
    Queued,
    Running,
    Done,
    Failed,
}

/// One run of a pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineRun {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// Version of the pipeline it runs.
    pub version: i32,
    pub state: PipelineRunState,
    pub attempts: i32,
    /// Machine code of a failure.
    pub code: Option<String>,
    /// Arguments of `code`.
    pub args: Option<serde_json::Value>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub started_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
}

/// A dataset linked to another by a pipeline run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LineageLink {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub dataset_id: String,
    pub dataset_name: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub pipeline_id: String,
    pub pipeline_name: String,
}

/// What a dataset reads and what reads it, as written by the runs (D574).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetLineage {
    pub upstream: Vec<LineageLink>,
    pub downstream: Vec<LineageLink>,
    /// An input has a transaction newer than the one the current view read.
    pub stale: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(transforms: Vec<(&str, Vec<(&str, &str)>, &str)>) -> PipelineDocument {
        PipelineDocument {
            transforms: transforms
                .into_iter()
                .map(|(output, inputs, sql)| Transform {
                    output: output.into(),
                    inputs: inputs
                        .into_iter()
                        .map(|(a, d)| (a.to_string(), d.to_string()))
                        .collect(),
                    sql: sql.into(),
                })
                .collect(),
        }
    }

    fn tokens(d: &PipelineDocument) -> Vec<String> {
        d.violations()
            .into_iter()
            .map(|v| format!("{}={}", v.path, v.token))
            .collect()
    }

    #[test]
    fn a_chain_is_valid() {
        let d = doc(vec![
            ("clean", vec![("r", "raw")], "SELECT * FROM r"),
            ("final", vec![("c", "clean")], "SELECT * FROM c"),
        ]);
        assert!(d.violations().is_empty());
    }

    #[test]
    fn an_output_cannot_be_read_by_itself_or_before_it_is_written() {
        let own = doc(vec![("a", vec![("x", "a")], "SELECT 1")]);
        assert_eq!(tokens(&own), ["transforms.0.inputs.x=cycle"]);
        let later = doc(vec![
            ("a", vec![("x", "b")], "SELECT 1"),
            ("b", vec![("y", "raw")], "SELECT 1"),
        ]);
        assert_eq!(tokens(&later), ["transforms.0.inputs.x=cycle"]);
    }

    #[test]
    fn bad_documents_name_their_path() {
        assert_eq!(tokens(&doc(vec![])), ["transforms=required"]);
        let d = doc(vec![("a", vec![], " ")]);
        assert_eq!(
            tokens(&d),
            ["transforms.0.sql=required", "transforms.0.inputs=required"]
        );
        let d = doc(vec![("a", vec![("not valid", "raw")], "SELECT 1")]);
        assert_eq!(tokens(&d), ["transforms.0.inputs.not valid=invalid"]);
        let d = doc(vec![
            ("a", vec![("x", "raw")], "SELECT 1"),
            ("a", vec![("x", "raw")], "SELECT 1"),
        ]);
        assert_eq!(tokens(&d), ["transforms.1.output=duplicate"]);
    }
}
