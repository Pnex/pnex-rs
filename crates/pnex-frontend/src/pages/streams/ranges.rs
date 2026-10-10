//! "Ranges" tab of /streams (media-ingest.md D169): time ranges of a stream
//! (or of the org) for one local day — planned vs actual, origin, source
//! link — with create / edit / delete and CSV / ICS import for writers.
//! A planned-vs-actual timeline of the day sits above the table (lot 4).

use chrono::TimeZone;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::MediaStream;
use pnex_core::time_range::{
    is_http_url, RangeImportResult, RangeOrigin, ScopeKind, TimeRange, TimeRangeInput,
};

use crate::api;
use crate::api::error_i18n::localize_field;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::pager::ListPager;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::pages::events::local_input_to_rfc3339;
use crate::state::toasts;

const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";
const LABEL: &str = "block text-xs text-gray-600 mb-1";
const BTN: &str =
    "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40";
const NAV_BTN: &str = "px-2 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50";
const RANGES_PAGE: i64 = 50;
/// Scope value of the org in the picker (stream ids are uuids).
const ORG_SCOPE: &str = "org";

/// Origin of a range → label.
pub(crate) fn origin_label(origin: RangeOrigin) -> String {
    match origin {
        RangeOrigin::Grid => t!("ranges-origin-grid").to_string(),
        RangeOrigin::Detected => t!("ranges-origin-detected").to_string(),
        RangeOrigin::Epg => t!("ranges-origin-epg").to_string(),
        RangeOrigin::Manual => t!("ranges-origin-manual").to_string(),
        RangeOrigin::Import => t!("ranges-origin-import").to_string(),
    }
}

/// Picker value → (scope_kind, scope_id) of the API.
fn scope_of(value: &str) -> (&'static str, String) {
    if value == ORG_SCOPE || value.is_empty() {
        (ScopeKind::Org.wire(), String::new())
    } else {
        (ScopeKind::Stream.wire(), value.to_string())
    }
}

/// Local day `[00:00, next 00:00)` as RFC 3339 bounds.
fn day_window(date: chrono::NaiveDate) -> Option<(String, String)> {
    let at = |d: chrono::NaiveDate| {
        chrono::Local
            .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
            .earliest()
            .map(|t| t.to_rfc3339())
    };
    Some((at(date)?, at(date.succ_opt()?)?))
}

fn local(ts: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&chrono::Local))
}

/// `HH:MM–HH:MM` (the day is shown when it is not `day`), `—` when unset.
fn span_label(start: Option<&str>, end: Option<&str>, day: chrono::NaiveDate) -> String {
    let fmt = |t: chrono::DateTime<chrono::Local>| {
        if t.date_naive() == day {
            t.format("%H:%M").to_string()
        } else {
            t.format("%d/%m %H:%M").to_string()
        }
    };
    match (start.and_then(local), end.and_then(local)) {
        (Some(s), Some(e)) => format!("{}–{}", fmt(s), fmt(e)),
        _ => "—".to_string(),
    }
}

/// RFC 3339 → `datetime-local` input value.
fn input_value(ts: Option<&str>) -> String {
    ts.and_then(local)
        .map(|t| t.format("%Y-%m-%dT%H:%M").to_string())
        .unwrap_or_default()
}

/// Which dialog is open.
#[derive(Clone, PartialEq)]
enum Dialog {
    Edit(TimeRange),
    Delete(TimeRange),
}

#[component]
pub fn RangesTab(
    streams: Vec<MediaStream>,
    can_write: bool,
    reload: Signal<u32>,
    creating: Signal<bool>,
) -> Element {
    let mut reload = reload;
    let mut creating = creating;
    let first = streams.first().map(|s| s.id.clone());
    let mut scope = use_signal(move || first.unwrap_or_else(|| ORG_SCOPE.to_string()));
    let mut day = use_signal(|| chrono::Local::now().date_naive());
    let mut page = use_signal(|| 0i64);
    let mut dialog = use_signal(|| None::<Dialog>);
    let mut importing = use_signal(|| false);

    let list = use_resource(move || {
        let (kind, id) = scope_of(&scope());
        let window = day_window(day());
        let offset = page() * RANGES_PAGE;
        async move {
            let _ = reload();
            let (from, to) = window.unwrap_or_default();
            api::time_ranges::list(kind, &id, &from, &to, RANGES_PAGE, offset).await
        }
    });
    let (rows, count, loaded) = match &*list.value().read() {
        Some(Ok(p)) => (p.results.clone(), p.count, true),
        Some(Err(err)) => {
            let message = crate::api::error_i18n::localize(err);
            return rsx! {
                p { class: "text-sm text-red-700", "{message}" }
            };
        }
        None => (Vec::new(), 0, false),
    };
    let today = chrono::Local::now().date_naive();
    let current_day = day();
    let day_value = current_day.format("%Y-%m-%d").to_string();
    let current_scope = scope();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("ranges-help")} }
            div { class: "flex flex-wrap items-center gap-2",
                select {
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    aria_label: t!("ranges-scope"),
                    onchange: move |e| {
                        page.set(0);
                        scope.set(e.value());
                    },
                    for s in streams.iter() {
                        option {
                            key: "{s.id}",
                            value: "{s.id}",
                            selected: current_scope == s.id,
                            "{s.name}"
                        }
                    }
                    option {
                        value: ORG_SCOPE,
                        selected: current_scope == ORG_SCOPE,
                        {t!("ranges-scope-org")}
                    }
                }
                button {
                    class: NAV_BTN,
                    r#type: "button",
                    title: t!("cameras-day-prev"),
                    onclick: move |_| {
                        if let Some(d) = day().pred_opt() {
                            page.set(0);
                            day.set(d);
                        }
                    },
                    icons::ChevronLeft { class: "h-4 w-4" }
                }
                input {
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm",
                    r#type: "date",
                    aria_label: t!("ranges-day"),
                    value: "{day_value}",
                    onchange: move |event| {
                        if let Ok(d) = chrono::NaiveDate::parse_from_str(&event.value(), "%Y-%m-%d") {
                            page.set(0);
                            day.set(d);
                        }
                    },
                }
                button {
                    class: NAV_BTN,
                    r#type: "button",
                    title: t!("cameras-day-next"),
                    onclick: move |_| {
                        if let Some(d) = day().succ_opt() {
                            page.set(0);
                            day.set(d);
                        }
                    },
                    icons::ChevronRight { class: "h-4 w-4" }
                }
                if current_day != today {
                    button {
                        class: BTN,
                        r#type: "button",
                        onclick: move |_| {
                            page.set(0);
                            day.set(chrono::Local::now().date_naive());
                        },
                        {t!("ranges-today")}
                    }
                }
                if can_write {
                    button {
                        class: "{BTN} ml-auto",
                        r#type: "button",
                        onclick: move |_| importing.set(true),
                        {t!("ranges-import")}
                    }
                }
            }
            if loaded && rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("ranges-empty")} }
            }
            if !rows.is_empty() {
                RangesTimeline { rows: rows.clone(), day: current_day }
                div { class: "overflow-x-auto bg-white rounded-lg shadow border border-gray-200",
                    table { class: "min-w-full divide-y divide-gray-200 text-sm",
                        thead { class: "bg-gray-50",
                            tr {
                                th { class: "th", {t!("ranges-col-label")} }
                                th { class: "th", {t!("ranges-col-planned")} }
                                th { class: "th", {t!("ranges-col-actual")} }
                                th { class: "th hidden md:table-cell", {t!("ranges-col-origin")} }
                                th { class: "th" }
                            }
                        }
                        tbody { class: "divide-y divide-gray-100",
                            for r in rows {
                                RangeRow {
                                    key: "{r.id}",
                                    range: r.clone(),
                                    day: current_day,
                                    can_write,
                                    on_action: move |d: Dialog| dialog.set(Some(d)),
                                }
                            }
                        }
                    }
                }
                ListPager { count, page, page_size: RANGES_PAGE }
            }
        }
        if creating() {
            RangeForm {
                existing: None,
                scope: current_scope.clone(),
                day: current_day,
                on_close: move |_| creating.set(false),
                on_saved: move |_| {
                    creating.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if importing() {
            ImportDialog {
                scope: current_scope.clone(),
                on_close: move |_| importing.set(false),
                on_imported: move |_| reload.with_mut(|r| *r += 1),
            }
        }
        match dialog() {
            Some(Dialog::Edit(r)) => rsx! {
                RangeForm {
                    key: "edit-{r.id}",
                    existing: Some(r.clone()),
                    scope: current_scope.clone(),
                    day: current_day,
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(Dialog::Delete(r)) => rsx! {
                ConfirmDialog {
                    title: t!("ranges-delete-title"),
                    message: t!("ranges-delete-message", name : r.label.clone()),
                    confirm_label: t!("streams-delete-confirm"),
                    on_confirm: {
                        let id = r.id.clone();
                        move |_| {
                            let id = id.clone();
                            dialog.set(None);
                            spawn(async move {
                                match api::time_ranges::delete(&id).await {
                                    Ok(_) => reload.with_mut(|r| *r += 1),
                                    Err(err) => toasts::error(err),
                                }
                            });
                        }
                    },
                    on_cancel: move |_| dialog.set(None),
                }
            },
            None => rsx! {},
        }
    }
}

/// Position of `[start, end)` on the day axis `[t0, t0 + len)` (seconds),
/// as `(left %, width %)`, clipped to the day; `None` when unset or outside.
fn timeline_bar(t0: i64, len: i64, start: Option<&str>, end: Option<&str>) -> Option<(f64, f64)> {
    let at = |ts: Option<&str>| {
        let t = chrono::DateTime::parse_from_rfc3339(ts?).ok()?.timestamp();
        Some(((t - t0) as f64 / len.max(1) as f64 * 100.0).clamp(0.0, 100.0))
    };
    let (left, right) = (at(start)?, at(end)?);
    (right > left).then_some((left, right - left))
}

/// Signed minutes from `planned` to `actual` (`+2` = two minutes late).
fn gap_minutes(planned: Option<&str>, actual: Option<&str>) -> Option<String> {
    let t = |s: Option<&str>| chrono::DateTime::parse_from_rfc3339(s?).ok();
    let m = (t(actual)? - t(planned)?).num_minutes();
    Some(format!("{m:+}"))
}

/// Planned vs actual timeline of the day: two bars per range (planned on
/// top, actual below) on a local-hours axis, as positioned divs.
#[component]
fn RangesTimeline(rows: Vec<TimeRange>, day: chrono::NaiveDate) -> Element {
    let Some((from, to)) = day_window(day) else {
        return rsx! {};
    };
    let (Some(start), Some(end)) = (local(&from), local(&to)) else {
        return rsx! {};
    };
    let (t0, len) = (start.timestamp(), (end - start).num_seconds());
    // A tick every 3 local hours (positions follow the real day length).
    let ticks: Vec<(String, f64)> = (0..24)
        .step_by(3)
        .filter_map(|h| {
            let t = chrono::Local
                .from_local_datetime(&day.and_hms_opt(h, 0, 0)?)
                .earliest()?;
            Some((
                format!("{h:02}h"),
                (t.timestamp() - t0) as f64 / len as f64 * 100.0,
            ))
        })
        .collect();
    rsx! {
        div { class: "bg-white rounded-lg shadow border border-gray-200 p-3 space-y-1",
            div { class: "flex items-center gap-4 text-[10px] text-gray-500",
                span { class: "inline-flex items-center gap-1",
                    span { class: "inline-block h-2 w-4 rounded bg-sky-300" }
                    {t!("ranges-col-planned")}
                }
                span { class: "inline-flex items-center gap-1",
                    span { class: "inline-block h-2 w-4 rounded bg-teal-600" }
                    {t!("ranges-col-actual")}
                }
            }
            div { class: "flex",
                div { class: "w-28 shrink-0" }
                div { class: "relative h-4 flex-1 text-[10px] text-gray-400",
                    for (label, left) in ticks.iter() {
                        span {
                            key: "{label}",
                            class: "absolute -translate-x-1/2",
                            style: "left: {left}%;",
                            "{label}"
                        }
                    }
                }
            }
            for r in rows.iter() {
                TimelineRow {
                    key: "{r.id}",
                    range: r.clone(),
                    t0,
                    len,
                }
            }
        }
    }
}

#[component]
fn TimelineRow(range: TimeRange, t0: i64, len: i64) -> Element {
    let planned = timeline_bar(
        t0,
        len,
        range.planned_start.as_deref(),
        range.planned_end.as_deref(),
    );
    let actual = timeline_bar(
        t0,
        len,
        range.actual_start.as_deref(),
        range.actual_end.as_deref(),
    );
    let start_gap = gap_minutes(
        range.planned_start.as_deref(),
        range.actual_start.as_deref(),
    );
    let end_gap = gap_minutes(range.planned_end.as_deref(), range.actual_end.as_deref());
    let title = match (start_gap, end_gap) {
        (Some(start), Some(end)) => t!("ranges-timeline-gap", start : start, end : end).to_string(),
        _ => range.label.clone(),
    };
    rsx! {
        div { class: "flex items-center", title: "{title}",
            p { class: "w-28 shrink-0 truncate pr-2 text-xs text-gray-700", "{range.label}" }
            div { class: "relative h-6 flex-1 rounded bg-gray-50",
                if let Some((left, width)) = planned {
                    div {
                        class: "absolute top-0.5 h-2 rounded bg-sky-300",
                        style: "left: {left}%; width: {width}%;",
                    }
                }
                if let Some((left, width)) = actual {
                    div {
                        class: "absolute bottom-0.5 h-2 rounded bg-teal-600",
                        style: "left: {left}%; width: {width}%;",
                    }
                }
            }
        }
    }
}

#[component]
fn RangeRow(
    range: TimeRange,
    day: chrono::NaiveDate,
    can_write: bool,
    on_action: Callback<Dialog>,
) -> Element {
    let planned = span_label(
        range.planned_start.as_deref(),
        range.planned_end.as_deref(),
        day,
    );
    let actual = span_label(
        range.actual_start.as_deref(),
        range.actual_end.as_deref(),
        day,
    );
    // Shown as a link only when http(s) (R11), never as HTML.
    let link = range.source_url.clone().filter(|u| is_http_url(u));
    let origin = origin_label(range.origin);
    let (r_edit, r_delete) = (range.clone(), range.clone());
    rsx! {
        tr {
            td { class: "td",
                p { class: "font-medium text-gray-900", "{range.label}" }
                p { class: "text-xs text-gray-500",
                    if let Some(c) = range.category.as_deref() {
                        span { class: "mr-2", "{c}" }
                    }
                    if let Some(e) = range.external_id.as_deref() {
                        span { class: "font-mono", "{e}" }
                    }
                }
                if let Some(url) = link {
                    a {
                        class: "text-xs text-blue-700 hover:underline",
                        href: "{url}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        {t!("ranges-source")}
                    }
                }
            }
            td { class: "td text-gray-700 whitespace-nowrap", "{planned}" }
            td { class: "td text-gray-700 whitespace-nowrap", "{actual}" }
            td { class: "td hidden text-gray-600 md:table-cell", "{origin}" }
            td { class: "td text-right",
                if can_write {
                    div { class: "flex justify-end gap-2",
                        button {
                            class: BTN,
                            r#type: "button",
                            onclick: move |_| on_action.call(Dialog::Edit(r_edit.clone())),
                            {t!("streams-edit")}
                        }
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| on_action.call(Dialog::Delete(r_delete.clone())),
                            {t!("streams-delete")}
                        }
                    }
                }
            }
        }
    }
}

/// Create (in the tab's scope) or edit form. Every field is sent: an
/// empty one clears the stored value.
#[component]
fn RangeForm(
    existing: Option<TimeRange>,
    scope: String,
    day: chrono::NaiveDate,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let text =
        |f: fn(&TimeRange) -> Option<String>| existing.as_ref().and_then(f).unwrap_or_default();
    let when =
        |f: fn(&TimeRange) -> Option<String>| input_value(existing.as_ref().and_then(f).as_deref());
    let is_new = existing.is_none();
    let (ps0, pe0) = if is_new {
        let d = day.format("%Y-%m-%d");
        (format!("{d}T08:00"), format!("{d}T09:00"))
    } else {
        (
            when(|r| r.planned_start.clone()),
            when(|r| r.planned_end.clone()),
        )
    };
    let label0 = existing
        .as_ref()
        .map(|r| r.label.clone())
        .unwrap_or_default();
    let (ext0, cat0) = (
        text(|r| r.external_id.clone()),
        text(|r| r.category.clone()),
    );
    let (as0, ae0) = (
        when(|r| r.actual_start.clone()),
        when(|r| r.actual_end.clone()),
    );
    let url0 = text(|r| r.source_url.clone());
    let mut label = use_signal(move || label0);
    let mut external_id = use_signal(move || ext0);
    let mut category = use_signal(move || cat0);
    let mut planned_start = use_signal(move || ps0);
    let mut planned_end = use_signal(move || pe0);
    let mut actual_start = use_signal(move || as0);
    let mut actual_end = use_signal(move || ae0);
    let mut source_url = use_signal(move || url0);
    let mut busy = use_signal(|| false);
    let edit_id = existing.as_ref().map(|r| r.id.clone());

    let save = move |_| {
        let time = |raw: String| local_input_to_rfc3339(&raw).unwrap_or(raw);
        let (kind, id) = scope_of(&scope);
        let mut input = TimeRangeInput {
            label: Some(label()),
            external_id: Some(external_id()),
            category: Some(category()),
            planned_start: Some(time(planned_start())),
            planned_end: Some(time(planned_end())),
            actual_start: Some(time(actual_start())),
            actual_end: Some(time(actual_end())),
            source_url: Some(source_url()),
            ..Default::default()
        };
        if edit_id.is_none() {
            input.scope_kind = Some(kind.to_string());
            input.scope_id = Some(id);
        }
        let edit_id = edit_id.clone();
        busy.set(true);
        spawn(async move {
            let res = match edit_id {
                Some(id) => api::time_ranges::update(&id, &input).await.map(|_| ()),
                None => api::time_ranges::create(&input).await.map(|_| ()),
            };
            busy.set(false);
            match res {
                Ok(()) => on_saved.call(()),
                Err(err) => toasts::error(err),
            }
        });
    };
    let title = if is_new {
        t!("ranges-create-title").to_string()
    } else {
        t!("ranges-edit-title").to_string()
    };
    let valid = !label().trim().is_empty();
    rsx! {
        FormDialog {
            title,
            submit_label: t!("streams-save").to_string(),
            on_close,
            on_submit: save,
            busy: busy(),
            valid,
            max_width: "max-w-lg".to_string(),
            div {
                label { r#for: "range-label", class: LABEL, {t!("ranges-col-label")} }
                input {
                    id: "range-label",
                    class: INPUT,
                    r#type: "text",
                    maxlength: "200",
                    value: "{label}",
                    oninput: move |e| label.set(e.value()),
                }
            }
            div { class: "grid grid-cols-1 sm:grid-cols-2 gap-3",
                div {
                    label { r#for: "range-category", class: LABEL, {t!("ranges-category")} }
                    input {
                        id: "range-category",
                        class: INPUT,
                        r#type: "text",
                        maxlength: "64",
                        value: "{category}",
                        oninput: move |e| category.set(e.value()),
                    }
                }
                div {
                    label { r#for: "range-external-id", class: LABEL, {t!("ranges-external-id")} }
                    input {
                        id: "range-external-id",
                        class: INPUT,
                        r#type: "text",
                        maxlength: "200",
                        value: "{external_id}",
                        oninput: move |e| external_id.set(e.value()),
                    }
                }
                div {
                    label { r#for: "range-planned-start", class: LABEL, {t!("ranges-planned-start")} }
                    input {
                        id: "range-planned-start",
                        class: INPUT,
                        r#type: "datetime-local",
                        value: "{planned_start}",
                        oninput: move |e| planned_start.set(e.value()),
                    }
                }
                div {
                    label { r#for: "range-planned-end", class: LABEL, {t!("ranges-planned-end")} }
                    input {
                        id: "range-planned-end",
                        class: INPUT,
                        r#type: "datetime-local",
                        value: "{planned_end}",
                        oninput: move |e| planned_end.set(e.value()),
                    }
                }
                div {
                    label { r#for: "range-actual-start", class: LABEL, {t!("ranges-actual-start")} }
                    input {
                        id: "range-actual-start",
                        class: INPUT,
                        r#type: "datetime-local",
                        value: "{actual_start}",
                        oninput: move |e| actual_start.set(e.value()),
                    }
                }
                div {
                    label { r#for: "range-actual-end", class: LABEL, {t!("ranges-actual-end")} }
                    input {
                        id: "range-actual-end",
                        class: INPUT,
                        r#type: "datetime-local",
                        value: "{actual_end}",
                        oninput: move |e| actual_end.set(e.value()),
                    }
                }
            }
            div {
                label { r#for: "range-source-url", class: LABEL, {t!("ranges-source-url")} }
                input {
                    id: "range-source-url",
                    class: INPUT,
                    r#type: "url",
                    maxlength: "2048",
                    placeholder: "https://",
                    value: "{source_url}",
                    oninput: move |e| source_url.set(e.value()),
                }
            }
            p { class: "text-xs text-gray-500", {t!("ranges-form-help")} }
        }
    }
}

/// CSV / ICS import into the tab's scope; shows the counts and the first
/// rejected rows.
#[component]
fn ImportDialog(scope: String, on_close: Callback<()>, on_imported: Callback<()>) -> Element {
    let mut file_name = use_signal(String::new);
    let mut file_bytes = use_signal(|| None::<Vec<u8>>);
    let mut busy = use_signal(|| false);
    let mut result = use_signal(|| None::<RangeImportResult>);
    let run = move |_| {
        let Some(bytes) = file_bytes() else {
            return;
        };
        let format = if file_name().to_lowercase().ends_with(".ics") {
            "ics"
        } else {
            "csv"
        };
        let (kind, id) = scope_of(&scope);
        busy.set(true);
        spawn(async move {
            let res = api::time_ranges::import(format, kind, &id, bytes).await;
            busy.set(false);
            match res {
                Ok(r) => {
                    result.set(Some(r));
                    on_imported.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };
    let outcome = result();
    let ready = file_bytes.read().is_some() && !busy();
    rsx! {
        Modal {
            title: t!("ranges-import-title").to_string(),
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-3",
                p { class: "text-xs text-gray-500", {t!("ranges-import-help")} }
                input {
                    class: INPUT,
                    r#type: "file",
                    accept: ".csv,.ics,text/csv,text/calendar",
                    aria_label: t!("ranges-import-file"),
                    onchange: move |evt| async move {
                        if let Some(file) = evt.files().first().cloned() {
                            file_name.set(file.name());
                            result.set(None);
                            if let Ok(bytes) = file.read_bytes().await {
                                file_bytes.set(Some(bytes.to_vec()));
                            }
                        }
                    },
                }
                if let Some(r) = outcome {
                    ImportOutcome { result: r }
                }
                div { class: crate::components::modal::MODAL_FOOTER,
                    button {
                        class: "px-4 py-2 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("ranges-close")}
                    }
                    button {
                        class: "px-4 py-2 text-sm text-white bg-blue-600 rounded-lg hover:bg-blue-700 disabled:opacity-40",
                        r#type: "button",
                        disabled: !ready,
                        onclick: run,
                        {t!("ranges-import-run")}
                    }
                }
            }
        }
    }
}

#[component]
fn ImportOutcome(result: RangeImportResult) -> Element {
    let summary = t!(
        "ranges-import-result",
        created : result.created.to_string(),
        updated : result.updated.to_string(),
        rejected : result.rejected.to_string()
    )
    .to_string();
    let lines: Vec<String> = result
        .errors
        .iter()
        .map(|e| {
            let reason = localize_field(&e.field, &e.token);
            t!("ranges-import-row", line : e.line.to_string(), field : e.field.clone(), reason : reason)
                .to_string()
        })
        .collect();
    rsx! {
        div { class: "rounded-lg border border-gray-200 bg-gray-50 p-3 space-y-1",
            p { class: "text-sm text-gray-800", "{summary}" }
            ul { class: "text-xs text-red-700 space-y-0.5 max-h-48 overflow-y-auto",
                for (i, line) in lines.into_iter().enumerate() {
                    li { key: "{i}", "{line}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_and_day_helpers() {
        assert_eq!(scope_of(ORG_SCOPE), ("org", String::new()));
        assert_eq!(scope_of("abc"), ("stream", "abc".to_string()));
        let d = chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let (from, to) = day_window(d).unwrap();
        let span = chrono::DateTime::parse_from_rfc3339(&to).unwrap()
            - chrono::DateTime::parse_from_rfc3339(&from).unwrap();
        assert!((23..=25).contains(&span.num_hours()));
        assert_eq!(span_label(None, Some("2026-10-12T09:00:00Z"), d), "—");
        let start = chrono::Local
            .from_local_datetime(&d.and_hms_opt(7, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .to_rfc3339();
        let end = chrono::Local
            .from_local_datetime(&d.and_hms_opt(9, 30, 0).unwrap())
            .earliest()
            .unwrap()
            .to_rfc3339();
        assert_eq!(span_label(Some(&start), Some(&end), d), "07:00–09:30");
        assert_eq!(input_value(Some(&start)), "2026-10-12T07:00");
        assert_eq!(input_value(None), "");
    }

    #[test]
    fn timeline_bars_and_gaps() {
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z")
            .unwrap()
            .timestamp();
        let bar = timeline_bar(
            t0,
            86_400,
            Some("2026-10-12T06:00:00Z"),
            Some("2026-10-12T12:00:00Z"),
        );
        assert_eq!(bar, Some((25.0, 25.0)));
        // Clipped to the day, unset or empty spans are not drawn.
        let late = timeline_bar(
            t0,
            86_400,
            Some("2026-10-12T18:00:00Z"),
            Some("2026-10-13T06:00:00Z"),
        );
        assert_eq!(late, Some((75.0, 25.0)));
        assert_eq!(
            timeline_bar(t0, 86_400, None, Some("2026-10-12T06:00:00Z")),
            None
        );
        assert_eq!(
            gap_minutes(Some("2026-10-12T07:00:00Z"), Some("2026-10-12T07:02:00Z")).as_deref(),
            Some("+2")
        );
        assert_eq!(
            gap_minutes(Some("2026-10-12T09:00:00Z"), Some("2026-10-12T08:59:00Z")).as_deref(),
            Some("-1")
        );
        assert_eq!(gap_minutes(Some("2026-10-12T09:00:00Z"), None), None);
    }
}
