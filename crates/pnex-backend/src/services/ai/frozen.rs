//! A conversation frozen at an instant (markets-research.md D902): the
//! harness, not the model, sets the instant of every tool call. A tool with
//! an instant gets it, brought back to the frozen one when the model asked
//! for later (and told so in the answer); a tool without one that only
//! describes the schema stays; every other tool is withdrawn, writes too.

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

/// How a tool is run in a frozen conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The instant is the argument at this path.
    AsOf(&'static [&'static str]),
    /// No dated state: run as is.
    Timeless,
    /// Documents: those created after the instant are hidden (Q-PT3).
    Documents,
}

/// The tools of a frozen conversation; any other is withdrawn.
pub const TOOLS: &[(&str, Policy)] = &[
    ("case_proposals", Policy::Timeless),
    ("case_signals", Policy::AsOf(&["to"])),
    ("describe_node_types", Policy::Timeless),
    ("describe_ontology", Policy::Timeless),
    ("get_object", Policy::AsOf(&["as_of"])),
    ("market_figures", Policy::AsOf(&["as_of"])),
    ("open_page", Policy::Documents),
    ("query_ontology", Policy::AsOf(&["query", "as_of"])),
    ("read_chunk", Policy::Documents),
    ("read_knowledge", Policy::Timeless),
    ("search_docs", Policy::Documents),
    ("search_knowledge", Policy::Timeless),
];

pub fn policy(tool: &str) -> Option<Policy> {
    TOOLS.iter().find(|(n, _)| *n == tool).map(|(_, p)| *p)
}

/// The arguments with the frozen instant set; `true` when the model asked
/// for a later instant (brought back). Earlier instants stay.
pub fn set_instant(args: &Value, path: &[&str], at: DateTime<FixedOffset>) -> (Value, bool) {
    let mut args = if args.is_object() {
        args.clone()
    } else {
        Value::Object(Default::default())
    };
    let Some((last, parents)) = path.split_last() else {
        return (args, false);
    };
    let mut node = &mut args;
    for p in parents {
        if !node.get(*p).is_some_and(Value::is_object) {
            node[*p] = Value::Object(Default::default());
        }
        node = &mut node[*p];
    }
    let asked = node
        .get(*last)
        .and_then(Value::as_str)
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok());
    let later = asked.is_some_and(|t| t > at);
    if asked.is_none() || later {
        node[*last] = Value::String(at.to_rfc3339());
    }
    (args, later)
}

/// Tells the model, in the tool's answer, the instant it was run at.
pub fn annotate(value: &mut Value, at: DateTime<FixedOffset>, brought_back: bool) {
    if let Value::Object(m) = value {
        m.insert("as_of".into(), Value::String(at.to_rfc3339()));
        if brought_back {
            m.insert(
                "as_of_note".into(),
                Value::String(format!(
                    "This conversation is frozen at {}: a later instant was brought back to it.",
                    at.to_rfc3339()
                )),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-01-06T10:15:00Z").unwrap()
    }

    #[test]
    fn later_instants_are_brought_back_earlier_ones_stay() {
        let (a, back) = set_instant(&serde_json::json!({}), &["as_of"], at());
        assert_eq!(
            (a["as_of"].as_str(), back),
            (Some(at().to_rfc3339().as_str()), false)
        );
        let (a, back) = set_instant(
            &serde_json::json!({ "as_of": "2026-11-09T00:00:00Z" }),
            &["as_of"],
            at(),
        );
        assert!(back);
        assert_eq!(a["as_of"], at().to_rfc3339());
        let (a, back) = set_instant(
            &serde_json::json!({ "as_of": "2025-01-01T00:00:00Z" }),
            &["as_of"],
            at(),
        );
        assert!(!back);
        assert_eq!(a["as_of"], "2025-01-01T00:00:00Z");
        let (a, _) = set_instant(
            &serde_json::json!({ "query": { "type_key": "x" } }),
            &["query", "as_of"],
            at(),
        );
        assert_eq!(a["query"]["type_key"], "x");
        assert_eq!(a["query"]["as_of"], at().to_rfc3339());
    }

    #[test]
    fn writes_and_present_reads_are_withdrawn() {
        for t in [
            "create_object",
            "update_flow",
            "list_devices",
            "query_telemetry",
            "case_workspace",
        ] {
            assert_eq!(policy(t), None, "{t}");
        }
        let names: Vec<&str> = TOOLS.iter().map(|(n, _)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
