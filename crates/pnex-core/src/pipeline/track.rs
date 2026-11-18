//! The state of each enumerated row between runs (pipeline-engine.md D937):
//! which rows a run takes, and in what order.

use serde::{Deserialize, Serialize};

/// Days before an empty answer is asked again, when the step says nothing.
pub const RETRY_DAYS_DEFAULT: u32 = 7;
/// Failures in a row before a row waits a day between attempts.
pub const FAILURES_BEFORE_BACKOFF: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Done,
    /// Answered empty; `settled` when nothing will ever be there.
    Empty,
    Failed,
    /// A navigation runs for it (D995); taken again only once its wait
    /// is over, the navigation then counted lost.
    Waiting,
}

/// What is kept of one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub key: String,
    pub state: State,
    /// Not taken again before this instant (µs); `None` = never again.
    pub retry_after_us: Option<i64>,
    pub failures: u32,
}

/// The keys a run takes, at most `max`: first the rows never asked, in the
/// order of the enumeration, then those due again, the oldest due first.
pub fn due<'a>(
    enumerated: &'a [String],
    known: &std::collections::HashMap<String, Item>,
    now_us: i64,
    max: usize,
) -> Vec<&'a str> {
    let mut fresh: Vec<&str> = Vec::new();
    let mut again: Vec<(i64, &str)> = Vec::new();
    for k in enumerated {
        match known.get(k) {
            None => fresh.push(k),
            Some(i) => {
                if let Some(at) = i.retry_after_us.filter(|at| *at <= now_us) {
                    again.push((at, k));
                }
            }
        }
    }
    again.sort_by_key(|(at, _)| *at);
    fresh
        .into_iter()
        .chain(again.into_iter().map(|(_, k)| k))
        .take(max)
        .collect()
}

const DAY_US: i64 = 86_400_000_000;

/// How long a row waits for its navigation before it is taken again.
pub const WAIT_MAX_US: i64 = DAY_US;

/// The state a row keeps after an answer.
pub fn after(
    key: &str,
    previous: Option<&Item>,
    outcome: State,
    settled: bool,
    retry_days: Option<u32>,
    again_days: Option<u32>,
    now_us: i64,
) -> Item {
    let failures = match outcome {
        State::Failed => previous.map_or(0, |p| p.failures) + 1,
        State::Waiting => previous.map_or(0, |p| p.failures),
        _ => 0,
    };
    let retry_after_us = match outcome {
        // A row read once stays done, unless the step reads it again every N days.
        State::Done => again_days.map(|d| now_us + i64::from(d) * DAY_US),
        State::Empty if settled => None,
        State::Empty => Some(now_us + i64::from(retry_days.unwrap_or(RETRY_DAYS_DEFAULT)) * DAY_US),
        // A failure is retried at the next run, then daily once it keeps failing.
        State::Failed if failures < FAILURES_BEFORE_BACKOFF => Some(now_us),
        State::Failed => Some(now_us + DAY_US),
        State::Waiting => Some(now_us + WAIT_MAX_US),
    };
    Item {
        key: key.to_string(),
        state: outcome,
        retry_after_us,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn never_asked_rows_come_before_retries() {
        let keys: Vec<String> = ["a/2025", "a/2024", "b/2024", "c/2024"]
            .map(String::from)
            .into();
        let mut known = HashMap::new();
        known.insert(
            "a/2025".into(),
            after("a/2025", None, State::Empty, false, Some(1), None, 0),
        );
        known.insert(
            "a/2024".into(),
            after("a/2024", None, State::Done, false, None, None, 0),
        );
        let now = 2 * DAY_US;
        assert_eq!(due(&keys, &known, now, 10), ["b/2024", "c/2024", "a/2025"]);
        assert_eq!(due(&keys, &known, now, 1), ["b/2024"]);
    }

    #[test]
    fn settled_and_done_rows_are_never_taken_again() {
        assert_eq!(
            after("k", None, State::Empty, true, None, None, 5).retry_after_us,
            None
        );
        let mut failed = after("k", None, State::Failed, false, None, None, 5);
        assert_eq!(failed.retry_after_us, Some(5));
        for _ in 0..2 {
            failed = after("k", Some(&failed), State::Failed, false, None, None, 5);
        }
        assert_eq!(
            (failed.failures, failed.retry_after_us),
            (3, Some(5 + DAY_US))
        );
    }

    #[test]
    fn done_rows_are_read_again_after_again_days() {
        let keys: Vec<String> = ["a", "b"].map(String::from).into();
        let mut known = HashMap::new();
        known.insert(
            "a".into(),
            after("a", None, State::Done, false, None, Some(2), 0),
        );
        known.insert(
            "b".into(),
            after("b", None, State::Done, false, None, None, 0),
        );
        assert!(due(&keys, &known, DAY_US, 10).is_empty());
        assert_eq!(due(&keys, &known, 2 * DAY_US, 10), ["a"]);
    }
}
