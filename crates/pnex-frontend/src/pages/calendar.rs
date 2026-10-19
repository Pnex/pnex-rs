//! Calendar of the org time ranges (ontology.md annex B): a month grid of
//! every range, recurrence occurrences included (maintenance plans). An
//! occurrence is completed, cancelled or moved here; the plan is never
//! touched (the materializer keeps what a user acted on).
//!
//! shortcut: month grid only; the resource timeline (team load) and the
//! shutdown Gantt come with teams and dependencies.

use chrono::{DateTime, Datelike, Days, FixedOffset, Months, NaiveDate, Utc};
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::time_range::{occurrence_state, TimeRange, TimeRangeInput};

use crate::api;
use crate::app::Route;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, GHOST_BTN, PRIMARY_BTN};
use crate::state::toasts;

fn parse(s: &Option<String>) -> Option<DateTime<FixedOffset>> {
    s.as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
}

/// Start shown for a range: actual when known, else planned.
fn start_of(r: &TimeRange) -> Option<NaiveDate> {
    parse(&r.actual_start)
        .or_else(|| parse(&r.planned_start))
        .map(|t| t.date_naive())
}

fn chip_class(state: &str) -> &'static str {
    match state {
        "done" => "bg-emerald-100 text-emerald-800",
        "cancelled" => "bg-gray-100 text-gray-500 line-through",
        "late" => "bg-red-100 text-red-800",
        "due" => "bg-amber-100 text-amber-800",
        _ => "bg-signal-100 text-signal-800",
    }
}

/// First day of the 6-week grid holding `month` (weeks start on Monday).
fn grid_start(month: NaiveDate) -> NaiveDate {
    let back = u64::from(month.weekday().num_days_from_monday());
    month.checked_sub_days(Days::new(back)).unwrap_or(month)
}

fn midnight(d: NaiveDate) -> String {
    d.and_hms_opt(0, 0, 0)
        .map(|t| t.and_utc().to_rfc3339())
        .unwrap_or_default()
}

#[component]
pub fn Calendar() -> Element {
    let today = Utc::now().date_naive();
    let mut month = use_signal(|| today.with_day(1).unwrap_or(today));
    let mut reload = use_signal(|| 0u32);
    let mut open = use_signal(|| None::<TimeRange>);
    let can_write = crate::state::access::can("ontology:write");
    let ranges = use_resource(move || async move {
        let _ = reload();
        let first = grid_start(month());
        let last = first + Days::new(42);
        api::time_ranges::list_all("", "", &midnight(first), &midnight(last)).await
    });
    let rows: Vec<TimeRange> = match &*ranges.read() {
        Some(Ok(r)) => r.clone(),
        _ => Vec::new(),
    };
    let now = Utc::now();
    let first = grid_start(month());
    let title = format!("{:04}-{:02}", month().year(), month().month());
    rsx! {
        ListLayout {
            title: t!("calendar-title"),
            subtitle: t!("calendar-subtitle"),
            can_write,
            on_refresh: move |_| reload += 1,
            div { class: "flex items-center gap-2 mb-3",
                button {
                    class: GHOST_BTN,
                    "data-testid": "calendar-prev",
                    onclick: move |_| month.set(month() - Months::new(1)),
                    "‹"
                }
                span {
                    class: "font-mono text-sm w-20 text-center",
                    "data-testid": "calendar-month",
                    "{title}"
                }
                button {
                    class: GHOST_BTN,
                    "data-testid": "calendar-next",
                    onclick: move |_| month.set(month() + Months::new(1)),
                    "›"
                }
                button {
                    class: GHOST_BTN,
                    onclick: move |_| month.set(today.with_day(1).unwrap_or(today)),
                    {t!("calendar-today")}
                }
            }
            if let Some(Err(e)) = &*ranges.read() {
                p { class: "text-sm text-red-600", "{e}" }
            }
            div { class: "grid grid-cols-7 gap-px rounded-lg overflow-hidden border border-line bg-line text-xs",
                for w in 0..7u64 {
                    div {
                        key: "h{w}",
                        class: "bg-gray-50 px-2 py-1 font-medium text-gray-500",
                        {t!("calendar-weekday", day : w)}
                    }
                }
                for i in 0..42u64 {
                    DayCell {
                        key: "{i}",
                        day: first + Days::new(i),
                        in_month: (first + Days::new(i)).month() == month().month(),
                        today: first + Days::new(i) == today,
                        ranges: rows.iter()
                            .filter(|r| start_of(r) == Some(first + Days::new(i)))
                            .cloned()
                            .collect::<Vec<_>>(),
                        now,
                        on_open: move |r| open.set(Some(r)),
                    }
                }
            }
        }
        if let Some(r) = open() {
            OccurrenceDialog {
                range: r,
                can_write,
                on_close: move |_| open.set(None),
                on_done: move |_| {
                    open.set(None);
                    reload += 1;
                },
            }
        }
    }
}

#[component]
fn DayCell(
    day: NaiveDate,
    in_month: bool,
    today: bool,
    ranges: Vec<TimeRange>,
    now: DateTime<Utc>,
    on_open: EventHandler<TimeRange>,
) -> Element {
    // Tokens and mirrored scales follow dark.css; `dark:` variants would
    // mirror twice back to light (O53).
    let shade = if in_month {
        "bg-surface text-ink"
    } else {
        "bg-gray-50 text-gray-400"
    };
    let num = if today {
        "inline-flex h-5 w-5 items-center justify-center rounded-full bg-signal-600 text-white"
    } else {
        ""
    };
    rsx! {
        div { class: "{shade} min-h-24 p-1 space-y-1", "data-day": "{day}",
            span { class: "{num}", "{day.day()}" }
            for r in ranges {
                button {
                    key: "{r.id}",
                    class: "block w-full truncate rounded px-1 text-left {chip_class(occurrence_state(&r, now))}",
                    "data-range": "{r.id}",
                    "data-state": "{occurrence_state(&r, now)}",
                    title: "{r.label}",
                    onclick: move |_| on_open.call(r.clone()),
                    "{r.label}"
                }
            }
        }
    }
}

/// Changes offered on an open occurrence.
const CHANGES: [&str; 3] = ["done", "move", "cancel"];

/// The update of one change; `None` while the form is incomplete.
fn change_input(change: &str, date: &str, hours: &str, days: u64) -> Option<TimeRangeInput> {
    match change {
        "done" => {
            let d = date.parse::<NaiveDate>().ok()?;
            let h = hours
                .trim()
                .replace(',', ".")
                .parse::<f64>()
                .ok()
                .filter(|h| *h > 0.0 && *h <= 24.0 * 31.0)?;
            // Done today ends now; an earlier day is recorded at noon UTC.
            let now = Utc::now();
            let end = if d == now.date_naive() {
                now
            } else {
                d.and_hms_opt(12, 0, 0)?.and_utc()
            };
            let start = end - chrono::Duration::minutes((h * 60.0).round() as i64);
            Some(TimeRangeInput {
                actual_start: Some(start.to_rfc3339()),
                actual_end: Some(end.to_rfc3339()),
                attrs: Some(serde_json::json!({ "status" : "done" })),
                ..Default::default()
            })
        }
        "move" => {
            let d = date.parse::<NaiveDate>().ok()?;
            Some(TimeRangeInput {
                planned_start: Some(midnight(d)),
                planned_end: Some(midnight(d + Days::new(days))),
                ..Default::default()
            })
        }
        "cancel" => Some(TimeRangeInput {
            attrs: Some(serde_json::json!({ "status" : "cancelled" })),
            ..Default::default()
        }),
        _ => None,
    }
}

/// Complete, move or cancel one occurrence (also opened pre-filled from
/// an assistant proposal).
#[component]
pub fn OccurrenceDialog(
    range: TimeRange,
    can_write: bool,
    #[props(default)] initial_change: Option<String>,
    #[props(default)] initial_date: Option<String>,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let state = occurrence_state(&range, Utc::now());
    let open = state != "done" && state != "cancelled";
    let planned = parse(&range.planned_start).map(|t| t.date_naive());
    let today = Utc::now().date_naive().to_string();
    let first = initial_change.filter(|c| CHANGES.contains(&c.as_str()));
    let mut change = use_signal(move || first.unwrap_or_else(|| "done".into()));
    let date0 = initial_date.unwrap_or(today);
    let mut date = use_signal(move || date0);
    let mut hours = use_signal(|| "1".to_string());
    let mut busy = use_signal(|| false);
    let days = match (parse(&range.planned_start), parse(&range.planned_end)) {
        (Some(a), Some(b)) => (b - a).num_days().max(1) as u64,
        _ => 1,
    };
    let input = change_input(&change(), &date(), &hours(), days);
    let valid = can_write && open && input.is_some();
    let id = range.id.clone();
    let object = (range.scope_kind.wire() == "object").then(|| range.scope_id.clone());
    rsx! {
        FormDialog {
            title: range.label.clone(),
            submit_label: t!("calendar-apply", change : change()),
            busy: busy(),
            valid,
            on_close: move |_| on_close.call(()),
            on_submit: move |_| {
                if let Some(input) = input.clone() {
                    let id = id.clone();
                    busy.set(true);
                    spawn(async move {
                        match api::time_ranges::update(&id, &input).await {
                            Ok(_) => {
                                toasts::success(t!("calendar-saved"));
                                on_done.call(());
                            }
                            Err(e) => toasts::error(e),
                        }
                        busy.set(false);
                    });
                }
            },
            p { class: "text-sm",
                span { class: "rounded px-1 {chip_class(state)}",
                    {t!("calendar-state", state : state)}
                }
            }
            if let Some(d) = planned {
                p { class: "text-sm text-gray-500", {t!("calendar-planned", date : d.to_string())} }
            }
            if let Some(id) = object {
                Link {
                    class: "text-sm text-signal-600 hover:underline",
                    to: Route::OntologyObject { id },
                    {t!("calendar-open-object")}
                }
            }
            if can_write && open {
                div { class: "flex gap-2", role: "radiogroup",
                    for c in CHANGES {
                        button {
                            key: "{c}",
                            r#type: "button",
                            role: "radio",
                            aria_checked: change() == c,
                            "data-testid": "calendar-change-{c}",
                            class: if change() == c { PRIMARY_BTN } else { GHOST_BTN },
                            onclick: move |_| change.set(c.to_string()),
                            {t!("calendar-change", change : c)}
                        }
                    }
                }
                if change() != "cancel" {
                    label { class: "block text-xs font-medium text-gray-500",
                        {t!("calendar-date", change : change())}
                    }
                    input {
                        r#type: "date",
                        class: crate::components::ui::INPUT,
                        "data-testid": "calendar-date",
                        value: "{date}",
                        oninput: move |e| date.set(e.value()),
                    }
                }
                if change() == "done" {
                    label { class: "block text-xs font-medium text-gray-500", {t!("calendar-hours")} }
                    input {
                        r#type: "number",
                        min: "0.25",
                        step: "0.25",
                        class: crate::components::ui::INPUT,
                        value: "{hours}",
                        oninput: move |e| hours.set(e.value()),
                    }
                }
            }
        }
    }
}

/// Occurrences of one object (30 days back, 90 ahead), with the same
/// dialog as the calendar: the Time tab of a maintenance plan.
#[component]
pub fn OccurrencesCard(object_id: String, can_write: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut open = use_signal(|| None::<TimeRange>);
    let oid = object_id.clone();
    let ranges = use_resource(move || {
        let oid = oid.clone();
        async move {
            let _ = reload();
            let today = Utc::now().date_naive();
            let (from, to) = (today - Days::new(30), today + Days::new(90));
            api::time_ranges::list_all("object", &oid, &midnight(from), &midnight(to))
                .await
                .unwrap_or_default()
        }
    });
    let rows = ranges.read().clone().unwrap_or_default();
    let now = Utc::now();
    rsx! {
        div { class: "rounded-lg border border-line p-4 space-y-2",
            h3 { class: "text-sm font-medium", {t!("calendar-occurrences")} }
            if rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("calendar-no-occurrence")} }
            }
            ul { class: "space-y-1",
                for r in rows {
                    li { key: "{r.id}",
                        button {
                            class: "flex w-full items-center gap-2 text-left text-sm hover:underline",
                            "data-range": "{r.id}",
                            onclick: {
                                let r = r.clone();
                                move |_| open.set(Some(r.clone()))
                            },
                            span { class: "rounded px-1 text-xs {chip_class(occurrence_state(&r, now))}",
                                {t!("calendar-state", state : occurrence_state(& r, now))}
                            }
                            span { class: "font-mono",
                                {start_of(&r).map(|d| d.to_string()).unwrap_or_default()}
                            }
                        }
                    }
                }
            }
        }
        if let Some(r) = open() {
            OccurrenceDialog {
                range: r,
                can_write,
                on_close: move |_| open.set(None),
                on_done: move |_| {
                    open.set(None);
                    reload += 1;
                },
            }
        }
    }
}

/// Occurrences of the recurring objects linked to this one (the plans that
/// maintain a pump, a machine or a device), one card per object.
#[component]
pub fn LinkedOccurrences(object_id: String, can_write: bool) -> Element {
    let oid = object_id.clone();
    let linked = use_resource(move || {
        let oid = oid.clone();
        async move {
            let links = api::ontology::object_links(&oid, None, false)
                .await
                .unwrap_or_default();
            let types = api::ontology::types().await.unwrap_or_default();
            let recurring = |key: &str| {
                types.iter().any(|t| {
                    t.def.key == key
                        && t.def.properties.iter().any(|p| {
                            matches!(p.kind, pnex_core::ontology::PropertyKind::Recurrence)
                        })
                })
            };
            let mut out: Vec<(String, String)> = Vec::new();
            for l in links {
                let other = if l.target.id == oid {
                    l.source
                } else {
                    l.target
                };
                if recurring(&other.type_key) && !out.iter().any(|(id, _)| *id == other.id) {
                    out.push((other.id, other.title));
                }
            }
            out
        }
    });
    let rows = linked.read().clone().unwrap_or_default();
    rsx! {
        for (id, title) in rows {
            div { key: "{id}", class: "space-y-1",
                Link {
                    class: "text-sm font-medium text-signal-600 hover:underline",
                    to: Route::OntologyObject {
                        id: id.clone(),
                    },
                    "{title}"
                }
                OccurrencesCard { object_id: id.clone(), can_write }
            }
        }
    }
}
