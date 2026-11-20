//! `pipeline_ask` flow node (pipelines.md D999): a flow asks a pack
//! pipeline for rows of its `asked` axis (D997), as a route does. serde-only,
//! wasm32-safe.
//!
//! The runtime is untrusted: the pipeline and the daily ceiling come from
//! the deployed config, the message gives the rows only. The server runs
//! them in the browser agent of the person who deployed the flow.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ontology::flow::FlowNodeRef;

/// Ports: asked, refused.
pub const NODE_PORT_ASKED: usize = 0;
pub const NODE_PORT_REFUSED: usize = 1;
pub const NODE_PORT_COUNT: usize = 2;
/// Compiled ceiling of rows a node asks per day, whatever its config.
pub const PER_DAY_MAX: u32 = 200;
/// Default rows per day.
pub const PER_DAY_DEFAULT: u32 = 20;
/// Most rows of one message.
pub const ROWS_MAX: usize = 50;

/// Configuration: the pipeline (`<pack>/<key>`) and the rows per day.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineAskConfig {
    #[serde(default)]
    pub pipeline: String,
    /// Rows asked per day by this node; 0 = the default.
    #[serde(default)]
    pub per_day: u32,
}

impl PipelineAskConfig {
    /// The day's ceiling, never above the compiled one.
    pub fn per_day(&self) -> u32 {
        match self.per_day {
            0 => PER_DAY_DEFAULT,
            n => n.min(PER_DAY_MAX),
        }
    }

    /// Save validation: an unset node is a valid draft.
    pub fn check(&self) -> Option<(&'static str, String)> {
        let ok = self.pipeline.is_empty()
            || self.pipeline.split_once('/').is_some_and(|(pack, key)| {
                let k = |s: &str| {
                    !s.is_empty()
                        && s.len() <= 64
                        && s.chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                };
                k(pack) && k(key)
            });
        if !ok || self.per_day > PER_DAY_MAX {
            return Some((
                "pipeline_ask_invalid",
                format!("pipeline is <pack>/<key>; at most {PER_DAY_MAX} rows a day"),
            ));
        }
        None
    }

    /// Deploy gate and runtime build: the pipeline is named.
    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.pipeline.is_empty() {
            return Some(("pipeline_ask_unset", "pick a pipeline".into()));
        }
        self.check()
    }
}

/// `POST /internal/flow/pipeline-ask`: the rows the runtime read from
/// `msg.payload` (one object, or an array of objects).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineAskRequest {
    pub node: FlowNodeRef,
    #[serde(default)]
    pub rows: Vec<Map<String, Value>>,
}

/// Reply: port 0 with the count asked, port 1 with a code and its args.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PipelineAskReply {
    pub port: usize,
    #[serde(default)]
    pub asked: usize,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub args: Value,
}

/// The rows of a payload: an object is one row, an array its objects.
pub fn rows_of(payload: &Value) -> Vec<Map<String, Value>> {
    match payload {
        Value::Object(m) => vec![m.clone()],
        Value::Array(a) => a.iter().filter_map(|v| v.as_object().cloned()).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_and_ceiling() {
        assert!(PipelineAskConfig::default().check().is_none());
        assert_eq!(
            PipelineAskConfig::default().check_deployable().unwrap().0,
            "pipeline_ask_unset"
        );
        let ok = PipelineAskConfig {
            pipeline: "prospecting_b2b/person_match_linkedin".into(),
            per_day: 0,
        };
        assert!(ok.check_deployable().is_none());
        assert_eq!(ok.per_day(), PER_DAY_DEFAULT);
        for bad in ["nopack", "a/B", "/x", "a/b/c"] {
            let c = PipelineAskConfig {
                pipeline: bad.into(),
                per_day: 1,
            };
            assert!(c.check().is_some(), "{bad}");
        }
        let over = PipelineAskConfig {
            pipeline: "a/b".into(),
            per_day: PER_DAY_MAX + 1,
        };
        assert!(over.check().is_some());
    }

    #[test]
    fn rows_from_object_or_array() {
        assert_eq!(rows_of(&serde_json::json!({"a": 1})).len(), 1);
        assert_eq!(
            rows_of(&serde_json::json!([{"a": 1}, 2, {"b": 2}])).len(),
            2
        );
        assert!(rows_of(&serde_json::json!("x")).is_empty());
    }
}
