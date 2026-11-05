//! Reads the O2 `object_transitions` stream: how far objects go through the
//! values of a property and how long they stay in each (sales-dashboards.md
//! D722). Written best effort: trends, never accounting.

use std::collections::{BTreeMap, HashMap};

use loco_rs::prelude::*;
use pnex_core::ontology::aggregate::{
    AggregateGroup, AggregateResult, AggregateValue, TransitionsSource,
};
use pnex_core::ontology::schema::PropertyHistory;
use serde_json::Value;

use super::changes::OBJECT_TRANSITIONS_STREAM;
use super::{invalid, OntologyError, OrgSchema};
use crate::services::openobserve::{provisioned_credentials, Client, OpenobserveSettings};

/// Rows read at most; past that the answer says it is truncated.
const ROWS_MAX: i64 = 10_000;
const WINDOW_US: i64 = 3 * 365 * 24 * 3600 * 1_000_000;
const DAY_US: f64 = 24.0 * 3600.0 * 1_000_000.0;

/// The history the source names: a type and a property that has one with a
/// funnel. The caller's keys end up in SQL only once they matched the schema.
pub fn validate<'a>(
    s: &'a OrgSchema,
    t: &TransitionsSource,
) -> Result<&'a PropertyHistory, OntologyError> {
    let def = s
        .object_type(&t.type_key)
        .ok_or_else(|| invalid("type_key", "invalid"))?;
    def.properties
        .iter()
        .find(|p| p.key == t.property)
        .and_then(|p| p.history.as_ref())
        .filter(|h| !h.funnel.is_empty())
        .ok_or_else(|| invalid("property", "invalid"))
}

/// One group per funnel value: `reached` objects (a skipped value counts as
/// passed), `rate` the percentage of them that reached the next value, `days`
/// the average stay among stays that ended. `rows` = (object, value, instant),
/// oldest first.
pub fn summarize(funnel: &[String], rows: &[(String, String, i64)]) -> Vec<AggregateGroup> {
    let mut furthest: HashMap<&str, usize> = HashMap::new();
    let mut last: HashMap<&str, (&str, i64)> = HashMap::new();
    let mut stays: HashMap<&str, (f64, u32)> = HashMap::new();
    for (object, value, at) in rows {
        if let Some(i) = funnel.iter().position(|v| v == value) {
            let f = furthest.entry(object).or_insert(i);
            *f = (*f).max(i);
        }
        if let Some((previous, since)) = last.insert(object, (value, *at)) {
            let s = stays.entry(previous).or_default();
            s.0 += (*at - since) as f64 / DAY_US;
            s.1 += 1;
        }
    }
    let reached: Vec<u32> = (0..funnel.len())
        .map(|i| furthest.values().filter(|f| **f >= i).count() as u32)
        .collect();
    let number = |v: Option<f64>| AggregateValue {
        value: v,
        ..Default::default()
    };
    funnel
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let rate = reached
                .get(i + 1)
                .filter(|_| reached[i] > 0)
                .map(|next| f64::from(*next) * 100.0 / f64::from(reached[i]));
            let days = stays
                .get(value.as_str())
                .map(|(sum, n)| sum / f64::from(*n));
            AggregateGroup {
                key: vec![Some(value.clone())],
                labels: vec![None],
                values: BTreeMap::from([
                    ("reached".to_string(), number(Some(f64::from(reached[i])))),
                    ("rate".to_string(), number(rate)),
                    ("days".to_string(), number(days)),
                ]),
            }
        })
        .collect()
}

/// `Ok(None)` when O2 is absent or cannot be read.
pub async fn run(
    ctx: &AppContext,
    org_id: i64,
    t: &TransitionsSource,
) -> Result<Option<AggregateResult>, OntologyError> {
    let s = super::schema(&ctx.db, org_id).await?;
    let funnel = validate(&s, t)?.funnel.clone();
    let Some(client) = OpenobserveSettings::from_config(&ctx.config).map(|c| Client::new(&c))
    else {
        return Ok(None);
    };
    let unreadable = |e: String| {
        tracing::warn!("object transitions read failed: {e}");
    };
    // An org not provisioned yet has recorded nothing: zeros, not an outage.
    let creds = match provisioned_credentials(&ctx.db, org_id).await {
        Ok(Some(c)) => Some(c),
        Ok(None) => None,
        Err(e) => {
            unreadable(e);
            return Ok(None);
        }
    };
    let sql = format!(
        "SELECT object_id, to_value, _timestamp FROM \"{OBJECT_TRANSITIONS_STREAM}\" \
         WHERE type_key = '{}' AND property = '{}' ORDER BY _timestamp ASC",
        t.type_key, t.property
    );
    let now = chrono::Utc::now().timestamp_micros();
    let found = match &creds {
        Some(c) => {
            client
                .search_logs(
                    &c.o2_org,
                    &sql,
                    now - WINDOW_US,
                    now,
                    0,
                    ROWS_MAX,
                    &c.email_passcode,
                )
                .await
        }
        None => Err("not found".to_string()),
    };
    let hits = match found {
        Ok(page) => page.hits,
        // Nothing moved yet: the stream does not exist.
        Err(e) if e.contains("not found") => Vec::new(),
        Err(e) => {
            unreadable(e);
            return Ok(None);
        }
    };
    let text = |h: &Value, k: &str| h.get(k).and_then(Value::as_str).map(str::to_string);
    let rows: Vec<(String, String, i64)> = hits
        .iter()
        .filter_map(|h| {
            Some((
                text(h, "object_id")?,
                text(h, "to_value")?,
                h.get("_timestamp")?.as_i64()?,
            ))
        })
        .collect();
    Ok(Some(AggregateResult {
        groups: summarize(&funnel, &rows),
        others: None,
        truncated: hits.len() as i64 >= ROWS_MAX,
        computed_at: chrono::Utc::now().to_rfc3339(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600 * 1_000_000;

    fn value(g: &AggregateGroup, k: &str) -> Option<f64> {
        g.values[k].value
    }

    #[test]
    fn a_skip_counts_as_passed_and_a_loss_stops_the_way() {
        let funnel: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let row = |o: &str, v: &str, d: i64| (o.to_string(), v.to_string(), d * DAY);
        let rows = [
            // Through a and b, then lost after 4 days in b.
            row("1", "a", 0),
            row("1", "b", 2),
            row("1", "lost", 6),
            // Straight to c: it passed a and b, and stayed nowhere yet.
            row("2", "c", 0),
            // Back from b to a, then on to b: not counted twice.
            row("3", "a", 0),
            row("3", "b", 2),
            row("3", "a", 4),
            row("3", "b", 8),
        ];
        let g = summarize(&funnel, &rows);
        let reached: Vec<_> = g.iter().map(|g| value(g, "reached")).collect();
        assert_eq!(reached, [Some(3.0), Some(3.0), Some(1.0)]);
        assert_eq!(value(&g[0], "rate"), Some(100.0));
        assert!((value(&g[1], "rate").unwrap() - 100.0 / 3.0).abs() < 0.01);
        assert_eq!(value(&g[2], "rate"), None, "no value after the last");
        // Stays in a: 2, 2 and 4 days; in b: 4 and 2.
        assert!((value(&g[0], "days").unwrap() - 8.0 / 3.0).abs() < 0.01);
        assert_eq!(value(&g[1], "days"), Some(3.0));
        assert_eq!(
            value(&g[2], "days"),
            None,
            "a stay not ended counts nowhere"
        );
    }

    #[test]
    fn nothing_moved_gives_zeros_and_no_rate() {
        let g = summarize(&["a".to_string(), "b".to_string()], &[]);
        assert_eq!(value(&g[0], "reached"), Some(0.0));
        assert_eq!(value(&g[0], "rate"), None);
    }
}
