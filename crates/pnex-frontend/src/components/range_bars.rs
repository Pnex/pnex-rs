//! Body of the `range_bars` dashboard widget (D182): one row per time range
//! of a stream or per time-of-day slice — label, start–end, a bar
//! proportional to the value, the value. Reads
//! `POST /api/v1/telemetry/aggregate` on the dashboard cadence (15 s), on
//! its own timer: the aggregation is not part of the `series-batch`.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::aggregate::{
    AggregateBuckets, AggregateRequest, AggregateResponse, AggregateRow, BucketBasis, RangeBuckets,
    SliceBuckets, WidgetBuckets, DEFAULT_TIMEZONE,
};
use pnex_core::Widget;

use crate::api;
use crate::components::charts::format_value;
use crate::util::sleep;

/// Same cadence as the live dashboards.
const POLL_SECS: u64 = 15;

/// Request of a widget over `[now - window, now)`, `None` when the widget
/// is not configured yet.
fn request_of(w: &Widget, now: chrono::DateTime<chrono::Utc>) -> Option<AggregateRequest> {
    let opts = w.options.aggregate.as_ref()?;
    let src = w.source.first().filter(|s| !s.metric.is_empty())?;
    let from = (now - chrono::Duration::seconds(opts.window_secs()?)).to_rfc3339();
    let to = now.to_rfc3339();
    let buckets = match &opts.buckets {
        WidgetBuckets::Ranges { stream_id } => AggregateBuckets::Ranges(RangeBuckets {
            scope_kind: "stream".into(),
            scope_id: stream_id.clone(),
            from,
            to,
        }),
        WidgetBuckets::Slices { bounds, timezone } => AggregateBuckets::Slices(SliceBuckets {
            from,
            to,
            bounds: bounds.clone(),
            timezone: timezone.clone().unwrap_or_else(|| DEFAULT_TIMEZONE.into()),
        }),
    };
    Some(AggregateRequest {
        metric: src.metric.clone(),
        device_id: src.device_id.clone(),
        labels: src.labels.clone(),
        op: opts.op,
        buckets,
    })
}

/// `HH:MM–HH:MM` of a range row in local time (day shown over 7 days);
/// slice rows already carry their bounds.
fn span_text(row: &AggregateRow, with_day: bool) -> String {
    let local = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&chrono::Local))
    };
    match (local(&row.start), local(&row.end)) {
        (Some(s), Some(e)) if with_day => {
            format!(
                "{} {}–{}",
                s.format("%d/%m"),
                s.format("%H:%M"),
                e.format("%H:%M")
            )
        }
        (Some(s), Some(e)) => format!("{}–{}", s.format("%H:%M"), e.format("%H:%M")),
        _ => format!("{}–{}", row.start, row.end),
    }
}

/// Bar width in % of the largest value (0 for no data or a non-positive max).
fn bar_pct(value: Option<f64>, max: f64) -> f64 {
    match value {
        Some(v) if max > 0.0 && v > 0.0 => (v / max * 100.0).clamp(0.0, 100.0),
        _ => 0.0,
    }
}

#[component]
pub fn RangeBarsBody(widget: Widget) -> Element {
    let mut tick = use_signal(|| 0u32);
    let mut polling = use_signal(|| false);
    // Props are not tracked by the resource: mirror them in a signal so an
    // edit in the inspector refetches at once.
    let mut current = use_signal(|| widget.clone());
    if *current.peek() != widget {
        current.set(widget.clone());
    }
    let data = use_resource(move || {
        let req = request_of(&current(), chrono::Utc::now());
        let _ = tick();
        async move {
            let req = req?;
            api::dashboards::aggregate(&req).await.ok()
        }
    });
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            tick.with_mut(|t| *t += 1);
        });
    }

    let response: Option<AggregateResponse> = data.read().as_ref().cloned().flatten();
    let decimals = widget.options.decimals.unwrap_or(1);
    let unit = widget.options.unit.clone().unwrap_or_default();
    let with_day = widget
        .options
        .aggregate
        .as_ref()
        .is_some_and(|a| a.window != "24h");
    let configured = widget.options.aggregate.is_some()
        && widget.source.first().is_some_and(|s| !s.metric.is_empty());
    let (rows, available) = response
        .map(|r| (r.rows, r.available))
        .unwrap_or((Vec::new(), true));
    let max = rows.iter().filter_map(|r| r.value).fold(0.0_f64, f64::max);

    rsx! {
        div { class: "flex flex-1 flex-col gap-1 overflow-y-auto px-3 pb-2 text-xs",
            if !configured {
                p { class: "text-gray-400", {t!("rb-not-configured")} }
            } else if !available {
                p { class: "text-gray-400", {t!("rb-unavailable")} }
            } else if rows.is_empty() {
                p { class: "text-gray-400", {t!("rb-empty")} }
            }
            for (i, row) in rows.iter().enumerate() {
                RangeBarRow {
                    key: "{i}",
                    row: row.clone(),
                    span: span_text(row, with_day),
                    pct: bar_pct(row.value, max),
                    value: row.value.map(|v| format!("{} {unit}", format_value(v, decimals))),
                }
            }
        }
    }
}

#[component]
fn RangeBarRow(row: AggregateRow, span: String, pct: f64, value: Option<String>) -> Element {
    let title = match row.basis {
        BucketBasis::Actual => t!("rb-basis-actual").to_string(),
        BucketBasis::Planned => t!("rb-basis-planned").to_string(),
        BucketBasis::Slice => t!("rb-basis-slice", days: row.days.unwrap_or(0)).to_string(),
    };
    let value = value.unwrap_or_else(|| "—".to_string());
    rsx! {
        div {
            class: "grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto] items-center gap-2",
            title: "{title}",
            div { class: "min-w-0",
                p { class: "truncate font-medium text-gray-700", "{row.label}" }
                p { class: "truncate text-[10px] text-gray-400", "{span}" }
            }
            div { class: "h-3 rounded bg-gray-100",
                div {
                    class: if row.basis == BucketBasis::Planned { "h-3 rounded bg-teal-300" } else { "h-3 rounded bg-teal-600" },
                    style: "width: {pct}%;",
                }
            }
            span { class: "tabular-nums text-gray-700", "{value}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::aggregate::AggregateWidgetOptions;

    fn widget(opts: Option<AggregateWidgetOptions>) -> Widget {
        Widget {
            id: "w".into(),
            widget_type: "range_bars".into(),
            title: String::new(),
            x: 0,
            y: 0,
            w: 100,
            h: 100,
            source: vec![pnex_core::SourceRef {
                role: "primary".into(),
                metric: "etl_mentions".into(),
                device_id: String::new(),
                window: "24h".into(),
                memory: None,
                labels: [("stream".to_string(), "inter".to_string())].into(),
            }],
            options: pnex_core::WidgetOptions {
                aggregate: opts,
                ..Default::default()
            },
        }
    }

    #[test]
    fn request_follows_the_widget() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-10T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(request_of(&widget(None), now).is_none());
        let req = request_of(&widget(Some(AggregateWidgetOptions::default())), now).unwrap();
        let AggregateBuckets::Slices(s) = req.buckets else {
            panic!("slices")
        };
        assert_eq!(s.timezone, DEFAULT_TIMEZONE);
        assert!(s.from.starts_with("2026-10-09T12:00:00"));
        assert_eq!(req.labels.get("stream").map(String::as_str), Some("inter"));
        let ranges = AggregateWidgetOptions {
            window: "7d".into(),
            buckets: WidgetBuckets::Ranges {
                stream_id: "s-1".into(),
            },
            ..Default::default()
        };
        let req = request_of(&widget(Some(ranges)), now).unwrap();
        let AggregateBuckets::Ranges(r) = req.buckets else {
            panic!("ranges")
        };
        assert_eq!(
            (r.scope_kind.as_str(), r.scope_id.as_str()),
            ("stream", "s-1")
        );
        assert!(r.from.starts_with("2026-10-03T12:00:00"));
    }

    #[test]
    fn bars_are_relative_to_the_largest_value() {
        assert_eq!(bar_pct(Some(5.0), 10.0), 50.0);
        assert_eq!(bar_pct(None, 10.0), 0.0);
        assert_eq!(bar_pct(Some(-1.0), 10.0), 0.0);
        assert_eq!(bar_pct(Some(3.0), 0.0), 0.0);
    }
}
