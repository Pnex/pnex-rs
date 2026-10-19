//! Calendar tools of the assistant (ontology.md annex B): read the org
//! occurrences, propose one change to an occurrence. The assistant never
//! writes a range: a proposal is reviewed and applied by the user from the
//! button under the answer (same dialog as the Calendar page).

use chrono::{Days, NaiveDate, Utc};
use pnex_core::err_codes;
use pnex_core::time_range::occurrence_state;
use sea_orm::QuerySelect;
use serde_json::{json, Value};
use uuid::Uuid;

use super::tools::{internal, ToolDeps, ToolError, ToolOutcome};
use crate::services::time_ranges::{self, ListFilter, RangeError};

/// Rows returned by one listing.
const LIST_MAX: u64 = 200;
const STATES: [&str; 5] = ["planned", "due", "late", "done", "cancelled"];
const CHANGES: [&str; 3] = ["done", "move", "cancel"];

fn outcome(value: Value) -> Result<ToolOutcome, ToolError> {
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

fn date_arg(args: &Value, key: &str) -> Result<Option<NaiveDate>, ToolError> {
    match args.get(key).and_then(Value::as_str).map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => s
            .parse()
            .map(Some)
            .map_err(|_| format!("{key} must be a date YYYY-MM-DD").into()),
    }
}

fn utc(d: NaiveDate) -> chrono::DateTime<chrono::FixedOffset> {
    d.and_time(chrono::NaiveTime::MIN).and_utc().fixed_offset()
}

/// Ranges of the org in a window (default: 30 days back, 90 ahead), with
/// their computed state; optional `state` filter.
pub async fn list_calendar(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let today = Utc::now().date_naive();
    let from = date_arg(args, "from")?.unwrap_or(today - Days::new(30));
    let to = date_arg(args, "to")?.unwrap_or(today + Days::new(90));
    if to <= from || (to - from).num_days() > 366 {
        return Err("the window must be at most 366 days, with to after from".into());
    }
    let state = args.get("state").and_then(Value::as_str);
    if state.is_some_and(|s| !STATES.contains(&s)) {
        return Err(format!("state must be one of {}", STATES.join(", ")).into());
    }
    let filter = ListFilter {
        scope: None,
        from: Some(utc(from)),
        to: Some(utc(to)),
    };
    let rows = time_ranges::select(deps.org_id, &filter)
        .limit(LIST_MAX)
        .all(deps.db)
        .await
        .map_err(internal("calendar listing"))?;
    let now = Utc::now();
    let items: Vec<Value> = rows
        .iter()
        .map(time_ranges::view)
        .filter_map(|r| {
            let st = occurrence_state(&r, now);
            if state.is_some_and(|s| s != st) {
                return None;
            }
            let object = (r.scope_kind.wire() == "object").then(|| r.scope_id.clone());
            Some(json!({
                "id": r.id, "label": r.label, "object_id": object, "category": r.category,
                "planned_start": r.planned_start, "planned_end": r.planned_end,
                "actual_end": r.actual_end, "state": st,
            }))
        })
        .collect();
    outcome(json!({
        "from": from.to_string(),
        "to": to.to_string(),
        "truncated": rows.len() as u64 == LIST_MAX,
        "occurrences": items,
    }))
}

/// Checks one change to an occurrence and returns it as a proposal;
/// nothing is written (the user applies it).
pub async fn propose_occurrence_change(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    let coded = |code: &'static str, message: &str| ToolError {
        message: message.into(),
        code: Some(code),
        args: None,
    };
    let id = args
        .get("range_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ToolError::from("range_id missing or not a UUID (from list_calendar)"))?;
    let change = args
        .get("change")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !CHANGES.contains(&change) {
        return Err(format!("change must be one of {}", CHANGES.join(", ")).into());
    }
    let date = date_arg(args, "date")?;
    if change == "move" && date.is_none() {
        return Err("a move needs the new date (date = YYYY-MM-DD)".into());
    }
    let row = match time_ranges::find(deps.db, deps.org_id, id).await {
        Ok(r) => r,
        Err(RangeError::NotFound) => {
            return Err(coded(
                err_codes::TIME_RANGE_NOT_FOUND,
                "no such occurrence in this organization",
            ))
        }
        Err(e) => return Err(internal("occurrence lookup")(e)),
    };
    let role = deps
        .actor
        .as_ref()
        .map(|a| a.role.clone())
        .ok_or_else(|| ToolError::from("this tool needs a signed-in member"))?;
    let writable =
        time_ranges::scope_writable(deps.db, deps.org_id, &role, &row.scope_kind, &row.scope_id)
            .await
            .map_err(internal("occurrence role check"))?;
    if !writable {
        return Err(coded(
            err_codes::ONTOLOGY_WRITE_FORBIDDEN,
            "the user's role cannot write occurrences of this object type",
        ));
    }
    let view = time_ranges::view(&row);
    let state = occurrence_state(&view, Utc::now());
    if matches!(state, "done" | "cancelled") {
        return Err(format!(
            "this occurrence is already {state}: it is history and stays as it is"
        )
        .into());
    }
    outcome(json!({
        "range_id": view.id,
        "label": view.label,
        "state": state,
        "change": change,
        "date": date.map(|d| d.to_string()),
        "executed": false,
        "next": "the user reviews the change and applies it from the button under your answer",
    }))
}
