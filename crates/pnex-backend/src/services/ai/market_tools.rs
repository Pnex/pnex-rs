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
    let source: study::DateSource =
        serde_json::from_value(args.get("dates").cloned().unwrap_or(json!("filings")))
            .map_err(|_| ToolError::from("dates must be filings, geo_events or market_moves"))?;
    if source != study::DateSource::Filings {
        let t = |k: &str| text(args, k).unwrap_or_default().chars().take(64).collect();
        let rule = study::DatedRule {
            source,
            series: text(args, "series")
                .ok_or_else(|| ToolError::from("series is required with these dates"))?
                .chars()
                .take(64)
                .collect(),
            kind: t("kind"),
            country: t("country"),
            include_unconfirmed: args.get("include_unconfirmed").and_then(Value::as_bool)
                == Some(true),
            asset_class: t("asset_class"),
            direction: t("direction"),
            from: None,
            to: None,
        };
        let result = study::rule_on_dates(deps.db, deps.org_id, &rule)
            .await
            .map_err(|e| ToolError::from(format!("rule test not run: {e}")))?
            .ok_or_else(|| ToolError::from("no instrument or macro series has this code"))?;
        return outcome(json!({ "result": result, "note": CAUSE_NOTE }));
    }
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

/// Concomitance is not cause (D742).
const CAUSE_NOTE: &str = "An aid to analysis, not investment advice. Events in the same window are a concomitance, never a cause: say so, and never attribute a move yourself.";

pub async fn market_session(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let date = text(args, "date")
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .ok_or_else(|| ToolError::from("date must be YYYY-MM-DD"))?;
    let session = markets::moves::session(deps.db, deps.org_id, date)
        .await
        .map_err(internal("reading a market session"))?;
    outcome(json!({ "session": session, "note": CAUSE_NOTE }))
}

// ─────────────────────────── Registry entries ───────────────────────────

use super::provider::ToolSpec;
use super::tools::{count, trace, ToolDef};

/// The market tools: spec and trace summary of each, in one place.
pub(super) fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            spec: ToolSpec {
                name: "market_figures",
                description: "Dated figures of a company, an instrument or a macroeconomic series, read from primary sources (SEC filings, FRED, ECB, closing prices): metric, period, value, unit, known_at, source_ref. as_of (RFC 3339) gives the values as known at that instant. Cite source_ref. An aid to analysis, never a recommendation. Read-only.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"object_id":{"type":"string"},"metric":{"type":"string"},"as_of":{"type":"string"}},"required":["object_id"]}"#).expect("static schema"),
            },
            summary: |v| {
            let n = count(&v["figures"]);
            trace(
                "ai-trace-market-figures",
                json!({"count": n}),
                format!("{n} market figure(s)"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "market_rule_test",
                description: "How prices moved after dated events, against dates drawn at random: abnormal return per window, with the number of events. dates=filings (default): the followed instruments after a kind of filing (form 8-K, 10-Q…; item such as 2.02 for results). dates=geo_events or market_moves: one series (series = ticker or macro code such as DCOILWTICO) after world events (kind, country) or abnormal sessions (asset_class, direction), dated by when they became known; close events count once (merged), unconfirmed world events stay out unless include_unconfirmed (then give unconfirmed_share). Always give the number of events, say that instruments no longer followed are missing, and never present a result as a cause. An aid to analysis, never a recommendation. Read-only.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"dates":{"type":"string","enum":["filings","geo_events","market_moves"]},"form":{"type":"string"},"item":{"type":"string"},"series":{"type":"string"},"kind":{"type":"string","enum":["armed_conflict","attack","sanctions","blockade","ceasefire","natural_disaster","other"]},"country":{"type":"string"},"include_unconfirmed":{"type":"boolean"},"asset_class":{"type":"string","enum":["equity_index","commodity","rates","volatility","fx","crypto"]},"direction":{"type":"string","enum":["up","down"]}}}"#).expect("static schema"),
            },
            summary: |v| {
            let n = v["result"]["filings"].as_u64().unwrap_or(0);
            trace(
                "ai-trace-market-rule",
                json!({"count": n}),
                format!("reaction to {n} event(s)"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "market_session",
                description: "The abnormal sessions of the watched series (index, oil, rates, VIX…) around a date (YYYY-MM-DD), and the world events of the same window. A world event in the same window is a concomitance, never a cause: say \"same window, not a cause\" and never attribute a move yourself; only the user attributes one, from a case. Read-only.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"date":{"type":"string"}},"required":["date"]}"#).expect("static schema"),
            },
            summary: |v| {
            let n = count(&v["session"]["moves"]);
            trace(
                "ai-trace-market-session",
                json!({"count": n}),
                format!("{n} abnormal session move(s)"),
            )
        },
        },
    ]
}
