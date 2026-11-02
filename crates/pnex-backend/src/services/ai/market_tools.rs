//! Assistant reads of the primary market figures (markets-research.md D536):
//! it reads, summarises and cites; it never decides, and no order exists.

use serde_json::{json, Value};
use uuid::Uuid;

use super::tools::{internal, ToolDeps, ToolError, ToolOutcome};
use crate::services::markets::{self, study};

/// Values returned to the model per call.
const FIGURES_MAX: usize = 200;
const NOTE: &str = "An aid to analysis, not investment advice: state what the figures show, cite source_ref for each, and give no recommendation.";

fn outcome(value: Value) -> Result<ToolOutcome, ToolError> {
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

pub async fn market_figures(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = text(args, "object_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ToolError::from("object_id must be the UUID of an object"))?;
    let as_of = match text(args, "as_of") {
        None => None,
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| ToolError::from("as_of must be an RFC 3339 instant"))?,
        ),
    };
    crate::services::ontology::objects::find(deps.db, deps.org_id, id)
        .await
        .map_err(|_| ToolError::from("no such object in this organization"))?;
    let rows = markets::read(deps.db, deps.org_id, id, text(args, "metric"), as_of, None)
        .await
        .map_err(internal("reading market figures"))?;
    let total = rows.len();
    let shown: Vec<_> = rows.into_iter().take(FIGURES_MAX).collect();
    outcome(json!({ "total": total, "figures": shown, "note": NOTE }))
}

pub async fn market_rule_test(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let form = text(args, "form").ok_or_else(|| ToolError::from("form is required"))?;
    let rule = study::Rule {
        form: form.chars().take(16).collect(),
        item: text(args, "item")
            .unwrap_or_default()
            .chars()
            .take(16)
            .collect(),
        from: None,
        to: None,
    };
    let result = study::rule(deps.db, deps.org_id, &rule)
        .await
        .map_err(|e| ToolError::from(format!("rule test not run: {e}")))?;
    outcome(json!({ "result": result, "note": NOTE }))
}
