//! Aggregation by bucket (ontology.md D182, media-ingest.md D171) behind
//! `POST /api/v1/telemetry/aggregate`: one value of a series per time range
//! of a scope, or per time-of-day slice repeated over the days of a window.
//! Generic read primitive: a show, a shift, a production batch.
//!
//! - every input is validated before any PromQL is built (selector through
//!   [`visualization::series_selector`], closed op set, window and counts
//!   bounded, timezone resolved by `chrono-tz`);
//! - ranges are read in the caller's org only (R1); `actual_*` wins over
//!   `planned_*` and the row says which one was used;
//! - O2 is read like the dashboards: cached, bounded concurrency, one global
//!   budget; a failure degrades to `available: false`, never a 500.
//!
//! Queries: buckets of the same duration whose ends share a UTC time of day
//! are read in ONE `query_range` with a one-day step. O2 aligns `start` on
//! the step, so the evaluation times are the next UTC midnights and an
//! `offset` brings each window back onto its bucket. Daily slices then cost
//! one query per slice (a DST change or a clipped edge splits the group).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use futures_util::StreamExt;
use pnex_core::aggregate::{
    check_slice_bounds, parse_bound, AggregateBuckets, AggregateOp, AggregateRequest,
    AggregateResponse, AggregateRow, BucketBasis, RANGES_MAX, WINDOW_DAYS_MAX,
};
use pnex_core::err_codes;
use sea_orm::{DatabaseConnection, DbErr, QuerySelect};

use crate::models::_entities::time_ranges;
use crate::services::openobserve::{self, client::Client};
use crate::services::time_ranges::{self as ranges, ListFilter, RangeError};
use crate::services::visualization;

const DAY: i64 = 86_400;

#[derive(Debug, thiserror::Error)]
pub enum AggregateError {
    #[error("invalid {field}: {token}")]
    Invalid { field: String, token: String },
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn invalid(field: &str, token: impl Into<String>) -> AggregateError {
    AggregateError::Invalid {
        field: field.to_string(),
        token: token.into(),
    }
}

/// One bucket to read: `[start, end)` in epoch seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    start: i64,
    end: i64,
}

/// PromQL of one bucket: `op` over a `dur_secs` window ending `offset_secs`
/// before the evaluation time, the matching series combined by the outer
/// aggregation (`avg` = mean of the per-series means).
fn bucket_query(op: AggregateOp, selector: &str, dur_secs: i64, offset_secs: i64) -> String {
    let (outer, inner) = match op {
        AggregateOp::Sum => ("sum", "sum_over_time"),
        AggregateOp::Avg => ("avg", "avg_over_time"),
        AggregateOp::Max => ("max", "max_over_time"),
        AggregateOp::Min => ("min", "min_over_time"),
        AggregateOp::Increase => ("sum", "increase"),
    };
    let offset = if offset_secs > 0 {
        format!(" offset {offset_secs}s")
    } else {
        String::new()
    };
    format!("{outer}({inner}({selector}[{dur_secs}s]{offset}))")
}

/// One `query_range` serving several buckets: `(eval time, bucket index)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    dur: i64,
    offset: i64,
    evals: Vec<(i64, usize)>,
}

/// Groups the buckets by (duration, UTC time of day of their end): each
/// group is evaluated at the next UTC midnights (multiples of the step, so
/// O2's alignment is a no-op) with a constant offset.
fn plan_groups(spans: &[Span]) -> Vec<Group> {
    let mut groups: BTreeMap<(i64, i64), Group> = BTreeMap::new();
    for (i, s) in spans.iter().enumerate() {
        let dur = (s.end - s.start).max(1);
        let offset = (DAY - s.end.rem_euclid(DAY)) % DAY;
        groups
            .entry((dur, offset))
            .or_insert_with(|| Group {
                dur,
                offset,
                evals: Vec::new(),
            })
            .evals
            .push((s.end + offset, i));
    }
    groups.into_values().collect()
}

/// Ranges → buckets: realigned interval when complete, else announced.
fn range_rows(rows: &[time_ranges::Model]) -> Vec<(AggregateRow, Span)> {
    rows.iter()
        .filter_map(|r| {
            let (basis, start, end) = match (r.actual_start, r.actual_end) {
                (Some(s), Some(e)) => (BucketBasis::Actual, s, e),
                _ => (BucketBasis::Planned, r.planned_start?, r.planned_end?),
            };
            let span = Span {
                start: start.timestamp(),
                end: end.timestamp(),
            };
            Some((
                AggregateRow {
                    label: r.label.clone(),
                    start: start.to_rfc3339(),
                    end: end.to_rfc3339(),
                    basis,
                    value: None,
                    range_id: Some(r.id.to_string()),
                    external_id: r.external_id.clone(),
                    days: None,
                },
                span,
            ))
        })
        .collect()
}

/// Local wall time → UTC: the earlier instant when ambiguous (DST end), one
/// hour later when it does not exist (DST start gap).
fn local_instant(tz: chrono_tz::Tz, day: NaiveDate, minutes: u32) -> Option<DateTime<Utc>> {
    let naive = day.and_hms_opt(minutes / 60, minutes % 60, 0)?;
    tz.from_local_datetime(&naive)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(naive + chrono::Duration::hours(1)))
                .earliest()
        })
        .map(|t| t.with_timezone(&Utc))
}

/// Occurrences `(slice index, span)` of the slices over `[from, to)`,
/// clipped to the window. Slice `i` = `[bounds[i], bounds[i+1])`, the last
/// one wraps to the first bound of the next day.
fn slice_spans(from: i64, to: i64, bounds: &[u32], tz: chrono_tz::Tz) -> Vec<(usize, Span)> {
    let local_day =
        |ts: i64| DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&tz).date_naive());
    let (Some(first), Some(last)) = (local_day(from), local_day(to)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // The day before: its last slice may wrap into the window.
    let mut day = first.pred_opt().unwrap_or(first);
    while day <= last {
        for (i, b) in bounds.iter().enumerate() {
            let next = bounds.get(i + 1);
            let end_day = if next.is_some() {
                Some(day)
            } else {
                day.succ_opt()
            };
            let (Some(s), Some(e)) = (
                local_instant(tz, day, *b),
                end_day.and_then(|d| local_instant(tz, d, *next.unwrap_or(&bounds[0]))),
            ) else {
                continue;
            };
            let span = Span {
                start: s.timestamp().max(from),
                end: e.timestamp().min(to),
            };
            if span.end > span.start {
                out.push((i, span));
            }
        }
        let Some(d) = day.succ_opt() else { break };
        day = d;
    }
    out
}

/// Combines the per-day values of one slice: `(value, seconds)` pairs.
fn combine(op: AggregateOp, values: &[(f64, i64)]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let it = values.iter().map(|(v, _)| *v);
    Some(match op {
        AggregateOp::Sum | AggregateOp::Increase => it.sum(),
        AggregateOp::Max => it.fold(f64::NEG_INFINITY, f64::max),
        AggregateOp::Min => it.fold(f64::INFINITY, f64::min),
        AggregateOp::Avg => {
            let secs: i64 = values.iter().map(|(_, s)| *s).sum();
            values.iter().map(|(v, s)| v * *s as f64).sum::<f64>() / secs.max(1) as f64
        }
    })
}

fn parse_instant(field: &str, raw: &str) -> Result<i64, AggregateError> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|t| t.timestamp())
        .map_err(|_| invalid(field, err_codes::FIELD_INVALID))
}

/// `[from, to)` checked: `to > from`, at most [`WINDOW_DAYS_MAX`] days.
fn parse_window(from: &str, to: &str) -> Result<(i64, i64), AggregateError> {
    let (from, to) = (parse_instant("from", from)?, parse_instant("to", to)?);
    if to <= from || to - from > WINDOW_DAYS_MAX * DAY {
        return Err(invalid("to", err_codes::FIELD_INVALID));
    }
    Ok((from, to))
}

/// Reads one value per span. `false` = O2 not configured, unreachable or
/// past the budget (values not read stay `None`).
async fn read_spans(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    op: AggregateOp,
    selector: &str,
    spans: &[Span],
) -> (Vec<Option<f64>>, bool) {
    let mut values = vec![None; spans.len()];
    if spans.is_empty() {
        return (values, true);
    }
    let Some(client) = client else {
        return (values, false);
    };
    let Some(creds) = openobserve::provisioned_credentials(db, org_id)
        .await
        .ok()
        .flatten()
    else {
        return (values, false);
    };
    let groups = plan_groups(spans);
    let mut all_ok = true;
    let (values_ref, ok_ref) = (&mut values, &mut all_ok);
    let fetch = tokio::time::timeout(visualization::BATCH_TIMEOUT, async move {
        let jobs = groups.into_iter().map(|g| {
            let query = bucket_query(op, selector, g.dur, g.offset);
            let first = g.evals.iter().map(|(t, _)| *t).min().unwrap_or_default();
            let last = g.evals.iter().map(|(t, _)| *t).max().unwrap_or_default();
            let (o2_org, passcode) = (creds.o2_org.clone(), creds.email_passcode.clone());
            async move {
                let res = visualization::cached_prom_range_at(
                    client,
                    &o2_org,
                    &query,
                    (first, last, DAY),
                    &passcode,
                )
                .await;
                (g, res)
            }
        });
        let mut done =
            futures_util::stream::iter(jobs).buffer_unordered(visualization::O2_CONCURRENCY);
        while let Some((g, res)) = done.next().await {
            match res {
                Ok(samples) => {
                    let by_time: HashMap<i64, usize> = g.evals.iter().copied().collect();
                    for (ts, raw) in samples.iter().flat_map(|s| s.values.iter()) {
                        let Some(&i) = by_time.get(&(*ts as i64)) else {
                            continue;
                        };
                        if values_ref[i].is_none() {
                            values_ref[i] = raw.parse::<f64>().ok().filter(|v| v.is_finite());
                        }
                    }
                }
                Err(e) => {
                    *ok_ref = false;
                    tracing::warn!(org_id, error = %e, "aggregate: bucket query failed, degraded");
                }
            }
        }
    })
    .await;
    if fetch.is_err() {
        all_ok = false;
        tracing::warn!(
            org_id,
            "aggregate: budget exceeded, remaining buckets degraded"
        );
    }
    (values, all_ok)
}

/// A request checked before any query: PromQL selector, window, and for
/// slices the parsed bounds and timezone.
struct Checked {
    selector: String,
    from: i64,
    to: i64,
    slices: Option<(Vec<u32>, chrono_tz::Tz)>,
}

/// Every rule that keeps the PromQL safe and the work bounded.
fn check_request(req: &AggregateRequest) -> Result<Checked, AggregateError> {
    if !openobserve::valid_metric_name(&req.metric) {
        return Err(invalid("metric", err_codes::FIELD_INVALID));
    }
    if !req.device_id.is_empty() && !pnex_core::valid_device_label(&req.device_id) {
        return Err(invalid("device_id", err_codes::FIELD_INVALID));
    }
    let Some(selector) = visualization::series_selector(&req.metric, &req.device_id, &req.labels)
    else {
        return Err(invalid("labels", err_codes::FIELD_INVALID));
    };
    let (from, to, slices) = match &req.buckets {
        AggregateBuckets::Ranges(b) => {
            let (from, to) = parse_window(&b.from, &b.to)?;
            (from, to, None)
        }
        AggregateBuckets::Slices(b) => {
            let (from, to) = parse_window(&b.from, &b.to)?;
            if let Some(token) = check_slice_bounds(&b.bounds) {
                return Err(invalid("bounds", token));
            }
            let tz: chrono_tz::Tz = b
                .timezone
                .trim()
                .parse()
                .map_err(|_| invalid("timezone", err_codes::FIELD_INVALID))?;
            let bounds = b.bounds.iter().filter_map(|s| parse_bound(s)).collect();
            (from, to, Some((bounds, tz)))
        }
    };
    Ok(Checked {
        selector,
        from,
        to,
        slices,
    })
}

/// `POST /api/v1/telemetry/aggregate` — see the module doc.
pub async fn aggregate(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    req: &AggregateRequest,
) -> Result<AggregateResponse, AggregateError> {
    let Checked {
        selector,
        from,
        to,
        slices,
    } = check_request(req)?;
    match (&req.buckets, slices) {
        (AggregateBuckets::Slices(b), Some((bounds, tz))) => {
            let occurrences = slice_spans(from, to, &bounds, tz);
            let spans: Vec<Span> = occurrences.iter().map(|(_, s)| *s).collect();
            let (values, available) =
                read_spans(db, client, org_id, req.op, &selector, &spans).await;
            let rows = b
                .bounds
                .iter()
                .enumerate()
                .map(|(i, start)| {
                    let end = b.bounds.get(i + 1).unwrap_or(&b.bounds[0]);
                    let per_day: Vec<(f64, i64)> = occurrences
                        .iter()
                        .zip(&values)
                        .filter(|((slice, _), _)| *slice == i)
                        .filter_map(|((_, s), v)| v.map(|v| (v, s.end - s.start)))
                        .collect();
                    AggregateRow {
                        label: format!("{start}–{end}"),
                        start: start.clone(),
                        end: end.clone(),
                        basis: BucketBasis::Slice,
                        value: combine(req.op, &per_day),
                        range_id: None,
                        external_id: None,
                        days: Some(per_day.len() as u32),
                    }
                })
                .collect();
            Ok(AggregateResponse { available, rows })
        }
        (AggregateBuckets::Ranges(b), _) => {
            let scope = ranges::resolve_scope(db, org_id, &b.scope_kind, &b.scope_id)
                .await
                .map_err(|e| match e {
                    RangeError::Db(e) => AggregateError::Db(e),
                    RangeError::Invalid { field, token } => {
                        AggregateError::Invalid { field, token }
                    }
                    _ => invalid("scope_id", err_codes::FIELD_INVALID),
                })?;
            let filter = ListFilter {
                scope: Some(scope),
                from: DateTime::from_timestamp(from, 0).map(|t| t.fixed_offset()),
                to: DateTime::from_timestamp(to, 0).map(|t| t.fixed_offset()),
            };
            let found = ranges::select(org_id, &filter)
                .limit((RANGES_MAX + 1) as u64)
                .all(db)
                .await?;
            if found.len() > RANGES_MAX {
                return Err(invalid(
                    "buckets",
                    format!("{}:{RANGES_MAX}", err_codes::FIELD_MAX_LENGTH),
                ));
            }
            let (mut rows, spans): (Vec<AggregateRow>, Vec<Span>) =
                range_rows(&found).into_iter().unzip();
            let (values, available) =
                read_spans(db, client, org_id, req.op, &selector, &spans).await;
            for (row, v) in rows.iter_mut().zip(values) {
                row.value = v;
            }
            Ok(AggregateResponse { available, rows })
        }
        (AggregateBuckets::Slices(_), None) => Ok(AggregateResponse::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> i64 {
        DateTime::parse_from_rfc3339(s).unwrap().timestamp()
    }

    fn tz() -> chrono_tz::Tz {
        "Europe/Paris".parse().unwrap()
    }

    fn range(
        label: &str,
        planned: (&str, &str),
        actual: Option<(&str, &str)>,
    ) -> time_ranges::Model {
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap();
        time_ranges::Model {
            id: uuid::Uuid::new_v4(),
            org_id: 1,
            scope_kind: "org".into(),
            scope_id: "1".into(),
            label: label.into(),
            external_id: Some(format!("ext-{label}")),
            category: None,
            planned_start: Some(t(planned.0)),
            planned_end: Some(t(planned.1)),
            actual_start: actual.map(|a| t(a.0)),
            actual_end: actual.map(|a| t(a.1)),
            origin: "grid".into(),
            confidence: None,
            source_url: None,
            source_ref: None,
            attrs: serde_json::json!({}),
            created_at: t("2026-10-01T00:00:00Z"),
            updated_at: t("2026-10-01T00:00:00Z"),
        }
    }

    #[test]
    fn ranges_use_actual_when_complete_else_planned() {
        let rows = range_rows(&[
            range(
                "7/9",
                ("2026-10-10T05:00:00Z", "2026-10-10T07:00:00Z"),
                Some(("2026-10-10T05:02:00Z", "2026-10-10T07:01:00Z")),
            ),
            range(
                "journal",
                ("2026-10-10T11:00:00Z", "2026-10-10T11:30:00Z"),
                None,
            ),
        ]);
        assert_eq!(rows[0].0.basis, BucketBasis::Actual);
        assert_eq!(rows[0].1.start, utc("2026-10-10T05:02:00Z"));
        assert_eq!(rows[0].1.end, utc("2026-10-10T07:01:00Z"));
        assert_eq!(rows[0].0.external_id.as_deref(), Some("ext-7/9"));
        assert_eq!(rows[1].0.basis, BucketBasis::Planned);
        assert_eq!(rows[1].1.end - rows[1].1.start, 1800);
    }

    #[test]
    fn slices_repeat_over_days_and_wrap() {
        let bounds = [0, 6 * 60, 9 * 60];
        // Two local days in summer time (UTC+2).
        let spans = slice_spans(
            utc("2026-07-01T00:00:00+02:00"),
            utc("2026-07-03T00:00:00+02:00"),
            &bounds,
            tz(),
        );
        let durations: Vec<(usize, i64)> =
            spans.iter().map(|(i, s)| (*i, s.end - s.start)).collect();
        assert_eq!(
            durations,
            vec![
                (0, 6 * 3600),
                (1, 3 * 3600),
                (2, 15 * 3600),
                (0, 6 * 3600),
                (1, 3 * 3600),
                (2, 15 * 3600),
            ]
        );
        assert_eq!(spans[0].1.start, utc("2026-06-30T22:00:00Z"));
    }

    #[test]
    fn slices_across_dst_end_paris() {
        // Last Sunday of October 2026: 03:00 CEST → 02:00 CET (25 h day).
        let spans = slice_spans(
            utc("2026-10-25T00:00:00+02:00"),
            utc("2026-10-26T00:00:00+01:00"),
            &[0, 6 * 60],
            tz(),
        );
        let durations: Vec<(usize, i64)> =
            spans.iter().map(|(i, s)| (*i, s.end - s.start)).collect();
        assert_eq!(durations, vec![(0, 7 * 3600), (1, 18 * 3600)]);
        // Same wall-clock bound, different UTC time of day: two query groups.
        let next = slice_spans(
            utc("2026-10-24T00:00:00+02:00"),
            utc("2026-10-27T00:00:00+01:00"),
            &[6 * 60],
            tz(),
        );
        let ends: Vec<i64> = next.iter().map(|(_, s)| s.end.rem_euclid(DAY)).collect();
        assert!(ends.contains(&(4 * 3600)) && ends.contains(&(5 * 3600)));
    }

    #[test]
    fn slices_skip_the_dst_start_gap() {
        // 29 March 2026: 02:00 → 03:00; a 02:30 bound lands on 03:30 CEST.
        let t = local_instant(tz(), NaiveDate::from_ymd_opt(2026, 3, 29).unwrap(), 150).unwrap();
        assert_eq!(t.timestamp(), utc("2026-03-29T03:30:00+02:00"));
    }

    #[test]
    fn groups_share_one_query_per_daily_bucket() {
        let day = |d: i64| Span {
            start: utc("2026-07-01T04:00:00Z") + d * DAY,
            end: utc("2026-07-01T07:00:00Z") + d * DAY,
        };
        let odd = Span {
            start: utc("2026-07-01T10:00:00Z"),
            end: utc("2026-07-01T10:30:00Z"),
        };
        let groups = plan_groups(&[day(0), day(1), odd, day(2)]);
        assert_eq!(groups.len(), 2);
        let daily = groups.iter().find(|g| g.dur == 3 * 3600).unwrap();
        assert_eq!(daily.offset, 17 * 3600);
        assert_eq!(daily.evals.len(), 3);
        // Evaluation times are UTC midnights (O2 aligns start on the step).
        assert!(daily.evals.iter().all(|(t, _)| t % DAY == 0));
        let (t, i) = daily.evals[0];
        assert_eq!((t - daily.offset, i), (day(0).end, 0));
        // An end at midnight needs no offset.
        let midnight = plan_groups(&[Span {
            start: utc("2026-07-01T00:00:00Z"),
            end: utc("2026-07-02T00:00:00Z"),
        }]);
        assert_eq!(midnight[0].offset, 0);
    }

    #[test]
    fn query_builder_per_op() {
        let sel = r#"etl_m{stream="inter"}"#;
        assert_eq!(
            bucket_query(AggregateOp::Increase, sel, 3600, 7200),
            r#"sum(increase(etl_m{stream="inter"}[3600s] offset 7200s))"#
        );
        assert_eq!(
            bucket_query(AggregateOp::Avg, sel, 60, 0),
            r#"avg(avg_over_time(etl_m{stream="inter"}[60s]))"#
        );
        assert_eq!(
            bucket_query(AggregateOp::Max, sel, 60, 1),
            r#"max(max_over_time(etl_m{stream="inter"}[60s] offset 1s))"#
        );
    }

    #[test]
    fn combine_per_op() {
        let v = [(2.0, 3600), (4.0, 7200)];
        assert_eq!(combine(AggregateOp::Sum, &v), Some(6.0));
        assert_eq!(combine(AggregateOp::Increase, &v), Some(6.0));
        assert_eq!(combine(AggregateOp::Max, &v), Some(4.0));
        assert_eq!(combine(AggregateOp::Min, &v), Some(2.0));
        assert!((combine(AggregateOp::Avg, &v).unwrap() - 10.0 / 3.0).abs() < 1e-9);
        assert_eq!(combine(AggregateOp::Sum, &[]), None);
    }

    fn req(metric: &str, device: &str, labels: &[(&str, &str)]) -> AggregateRequest {
        AggregateRequest {
            metric: metric.into(),
            device_id: device.into(),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            op: AggregateOp::Sum,
            buckets: AggregateBuckets::Slices(pnex_core::aggregate::SliceBuckets {
                from: "2026-07-01T00:00:00Z".into(),
                to: "2026-07-02T00:00:00Z".into(),
                bounds: vec!["00:00".into()],
                timezone: "UTC".into(),
            }),
        }
    }

    fn field_of(r: Result<Checked, AggregateError>) -> String {
        match r {
            Err(AggregateError::Invalid { field, .. }) => field,
            Err(e) => panic!("expected a field error, got {e:?}"),
            Ok(_) => panic!("expected a field error, got a valid request"),
        }
    }

    #[test]
    fn injection_attempts_are_refused_before_any_query() {
        for (metric, device, labels, field) in [
            ("m}or{", "", vec![("a", "b")], "metric"),
            ("m", "d\"}", vec![], "device_id"),
            ("m", "", vec![("stream", r#"x"}"#)], "labels"),
            ("m", "", vec![("stream", "a\",device_id=\"x")], "labels"),
            ("m", "", vec![("x\"}", "a")], "labels"),
            ("m", "", vec![("__name__", "x")], "labels"),
            ("m", "", vec![], "labels"),
        ] {
            let r = check_request(&req(metric, device, &labels));
            assert_eq!(field_of(r), field, "{metric} {device} {labels:?}");
        }
        let with = |f: &dyn Fn(&mut pnex_core::aggregate::SliceBuckets)| {
            let mut r = req("m", "dev-1", &[]);
            if let AggregateBuckets::Slices(s) = &mut r.buckets {
                f(s);
            }
            check_request(&r)
        };
        assert_eq!(
            field_of(with(&|s| s.timezone = "Europe/Paris\") or vector(1)".into())),
            "timezone"
        );
        assert_eq!(
            field_of(with(&|s| s.to = "2026-08-15T00:00:00Z".into())),
            "to"
        );
        assert_eq!(field_of(with(&|s| s.to = s.from.clone())), "to");
        assert_eq!(field_of(with(&|s| s.from = "yesterday".into())), "from");
        assert_eq!(field_of(with(&|s| s.bounds = vec!["9h".into()])), "bounds");
        let ok = with(&|_| {}).expect("valid");
        assert_eq!(ok.selector, r#"m{device_id="dev-1"}"#);
    }
}
