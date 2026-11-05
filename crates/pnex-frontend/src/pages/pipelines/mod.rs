//! Data pipelines (data-pipelines.md §9.5, lot OP1.5): `/pipelines` lists pipelines,
//! tables, chained runs and connectors; `/pipelines/:id` is the studio of a pipeline,
//! `/pipelines/tables/:id` the page of a table.

mod connectors;
mod graph;
mod studio;
mod table;

pub use studio::PipelineStudio;
pub use table::PipelineTable;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::datasets::Dataset;
use pnex_core::pipelines::{
    Build, BuildState, BuildStep, BuildStepState, Pipeline, PipelineDocument,
};

use crate::api;
use crate::api::error_i18n::{localize, localize_code};
use crate::app::Route;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{page_slice, ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::modal::Modal;
use crate::components::ontology::short_time;
use crate::components::ui::{
    FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT, TAB, TAB_ACTIVE, TAB_BAR,
};
use crate::state::access::can;

pub(super) const BADGE: &str = "rounded border px-1.5 py-0.5 text-xs";
const GOOD: &str = "bg-green-50 text-green-800 border-green-200";
const BAD: &str = "bg-red-50 text-alarm border-red-200";
const WAITING: &str = "bg-gray-50 text-slate border-line";

pub(super) fn tone(good: bool, bad: bool) -> &'static str {
    match (good, bad) {
        (_, true) => BAD,
        (true, _) => GOOD,
        _ => WAITING,
    }
}

/// Schedules offered, as a `recurrence` from today.
pub(super) const SCHEDULES: [&str; 4] = ["", "day", "week", "month"];

pub(super) fn schedule_of(v: &Option<serde_json::Value>) -> String {
    v.as_ref()
        .and_then(|r| r.get("unit"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string()
}

pub(super) fn schedule_label(unit: &str) -> String {
    match unit {
        "day" => t!("onto-imp-schedule-day").to_string(),
        "week" => t!("onto-imp-schedule-week").to_string(),
        "month" => t!("onto-imp-schedule-month").to_string(),
        _ => t!("onto-imp-schedule-none").to_string(),
    }
}

pub(super) fn build_state_label(s: BuildState) -> String {
    match s {
        BuildState::Running => t!("pipe-build-running").to_string(),
        BuildState::Done => t!("pipe-build-done").to_string(),
        BuildState::Failed => t!("pipe-build-failed").to_string(),
    }
}

fn step_label(s: BuildStepState) -> String {
    match s {
        BuildStepState::Pending => t!("pipe-step-pending").to_string(),
        BuildStepState::Running => t!("pipe-step-running").to_string(),
        BuildStepState::Done => t!("pipe-step-done").to_string(),
        BuildStepState::Failed => t!("pipe-step-failed").to_string(),
        BuildStepState::Blocked => t!("pipe-step-blocked").to_string(),
        BuildStepState::Skipped => t!("pipe-step-skipped").to_string(),
    }
}

/// A machine code of a run or a step, in words.
pub(super) fn code_label(code: &str, args: Option<&serde_json::Value>) -> String {
    localize_code(code, args, code)
}

/// The steps of a chained run, or of one about to start.
#[component]
pub(super) fn BuildSteps(steps: Vec<BuildStep>) -> Element {
    rsx! {
        ol { class: "space-y-1", "data-testid": "build-steps",
            for (i, s) in steps.iter().enumerate() {
                li {
                    key: "{i}",
                    class: "flex flex-wrap items-center gap-2 text-sm",
                    span { class: "text-slate", "{i + 1}." }
                    span { class: "font-medium text-ink", "{s.name}" }
                    span { class: "{BADGE} {tone(s.state == BuildStepState::Done, matches!(s.state, BuildStepState::Failed | BuildStepState::Blocked))}",
                        "{step_label(s.state)}"
                    }
                    if let Some(code) = s.code.clone() {
                        span { class: "text-xs text-alarm", "{code_label(&code, None)}" }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Pipelines,
    Tables,
    Builds,
    Connectors,
}

#[component]
pub fn Pipelines() -> Element {
    let mut tab = use_signal(|| Tab::Pipelines);
    let mut reload = use_signal(|| 0u32);
    let mut adding = use_signal(|| false);
    let (can_write, add_label) = match tab() {
        Tab::Pipelines => (can("pipelines:write"), Some(t!("pipe-add").to_string())),
        Tab::Tables => (
            can("datasets:write"),
            Some(t!("pipe-table-add").to_string()),
        ),
        Tab::Builds => (false, None),
        Tab::Connectors => (
            can("connectors:write"),
            Some(t!("pipe-connector-add").to_string()),
        ),
    };
    let mut open = move |to: Tab| {
        adding.set(false);
        tab.set(to);
    };
    rsx! {
        ListLayout {
            title: t!("pipe-title").to_string(),
            subtitle: Some(t!("pipe-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            add_label,
            on_add: move |_| adding.set(true),
            tabs: rsx! {
                div { class: TAB_BAR,
                    button {
                        class: if tab() == Tab::Pipelines { TAB_ACTIVE } else { TAB },
                        onclick: move |_| open(Tab::Pipelines),
                        {t!("pipe-tab-pipelines")}
                    }
                    button {
                        class: if tab() == Tab::Tables { TAB_ACTIVE } else { TAB },
                        onclick: move |_| open(Tab::Tables),
                        {t!("pipe-tab-tables")}
                    }
                    button {
                        class: if tab() == Tab::Builds { TAB_ACTIVE } else { TAB },
                        onclick: move |_| open(Tab::Builds),
                        {t!("pipe-tab-builds")}
                    }
                    if can("connectors:read") {
                        button {
                            class: if tab() == Tab::Connectors { TAB_ACTIVE } else { TAB },
                            onclick: move |_| open(Tab::Connectors),
                            {t!("pipe-tab-connectors")}
                        }
                    }
                }
            },
            match tab() {
                Tab::Pipelines => rsx! {
                    PipelineList { reload, adding }
                },
                Tab::Tables => rsx! {
                    TableList { reload, adding }
                },
                Tab::Builds => rsx! {
                    BuildList { reload }
                },
                Tab::Connectors => rsx! {
                    connectors::ConnectorList { reload, adding }
                },
            }
        }
    }
}

/// The whole list, filtered by name and paged here.
fn page_of<T: Clone>(
    all: Vec<T>,
    name: impl Fn(&T) -> &str,
    query: &str,
    page: i64,
) -> (i64, Vec<T>) {
    let query = query.trim().to_lowercase();
    let kept: Vec<T> = all
        .into_iter()
        .filter(|x| query.is_empty() || name(x).to_lowercase().contains(&query))
        .collect();
    (kept.len() as i64, page_slice(&kept, page))
}

#[component]
fn PipelineList(reload: Signal<u32>, adding: Signal<bool>) -> Element {
    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut name = use_signal(String::new);
    let mut source = use_signal(String::new);
    let mut output = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let navigator = use_navigator();
    let list = use_resource(move || async move {
        let _ = reload();
        api::pipelines::list().await
    });
    let tables = use_resource(move || async move {
        let _ = reload();
        api::pipelines::tables().await
    });
    let (state, all) = match &*list.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let (count, rows) = page_of(all, |p: &Pipeline| &p.name, &search(), page());
    let columns = vec![
        Column::new(t!("pipe-col-name").to_string(), |p: &Pipeline| {
            rsx! { "{p.name}" }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("pipe-col-version").to_string(), |p: &Pipeline| {
            rsx! { "v{p.version}" }
        }),
        Column::new(t!("pipe-col-schedule").to_string(), |p: &Pipeline| {
            rsx! { "{schedule_label(&schedule_of(&p.recurrence))}" }
        }),
        Column::new(t!("pipe-col-updated").to_string(), |p: &Pipeline| {
            rsx! { "{short_time(&p.updated_at)}" }
        }),
    ];
    let table_names: Vec<String> = match &*tables.read() {
        Some(Ok(v)) => v.iter().map(|t: &Dataset| t.name.clone()).collect(),
        _ => Vec::new(),
    };
    let submit = move |_| {
        let document = PipelineDocument::starter(&source(), output().trim()).to_yaml();
        let title = name().trim().to_string();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::pipelines::create(&title, &document).await {
                Ok(p) => {
                    adding.set(false);
                    navigator.push(Route::PipelineStudio { id: p.id });
                }
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    let valid = !name().trim().is_empty() && !source().is_empty() && !output().trim().is_empty();
    rsx! {
        div { class: "space-y-0",
            FilterBar {
                SearchInput {
                    placeholder: t!("pipe-search").to_string(),
                    value: search,
                    on_submit: move |_| page.set(0),
                    page: Some(page),
                }
            }
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("pipe-empty").to_string(),
                div { class: "relative",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|p: &Pipeline| p.id.clone()),
                        on_row_click: move |id: String| {
                            navigator.push(Route::PipelineStudio { id });
                        },
                    }
                    ListPager { count, page }
                }
            }
        }
        if adding() {
            FormDialog {
                title: t!("pipe-add").to_string(),
                submit_label: t!("pipe-create").to_string(),
                on_close: move |_| adding.set(false),
                on_submit: submit,
                busy: busy(),
                valid,
                div { class: "space-y-3",
                    if table_names.is_empty() {
                        p { class: FIELD_HELP, {t!("pipe-needs-table")} }
                    }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-name", {t!("pipe-col-name")} }
                        input {
                            id: "pipe-name",
                            class: INPUT,
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-source", {t!("pipe-source")} }
                        select {
                            id: "pipe-source",
                            class: INPUT,
                            value: "{source}",
                            onchange: move |e| source.set(e.value()),
                            option { value: "", {t!("pipe-pick-table")} }
                            for t in table_names.clone() {
                                option {
                                    key: "{t}",
                                    value: "{t}",
                                    selected: source() == t,
                                    "{t}"
                                }
                            }
                        }
                    }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-output", {t!("pipe-output")} }
                        input {
                            id: "pipe-output",
                            class: INPUT,
                            value: "{output}",
                            oninput: move |e| output.set(e.value()),
                        }
                        p { class: FIELD_HELP, {t!("pipe-output-help")} }
                    }
                    if let Some(e) = error() {
                        p { class: FIELD_ERROR, "{e}" }
                    }
                }
            }
        }
    }
}

#[component]
fn TableList(reload: Signal<u32>, adding: Signal<bool>) -> Element {
    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut name = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let navigator = use_navigator();
    let list = use_resource(move || async move {
        let _ = reload();
        api::pipelines::tables().await
    });
    let (state, all) = match &*list.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let (count, rows) = page_of(all, |d: &Dataset| &d.name, &search(), page());
    let columns = vec![
        Column::new(t!("pipe-col-name").to_string(), |d: &Dataset| {
            rsx! { "{d.name}" }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("pipe-col-rows").to_string(), |d: &Dataset| {
            rsx! { "{d.rows}" }
        }),
        Column::new(t!("pipe-col-size").to_string(), |d: &Dataset| {
            rsx! { "{crate::pages::system::format_bytes(d.bytes as f64)}" }
        }),
        Column::new(t!("pipe-col-committed").to_string(), |d: &Dataset| {
            let at = d
                .committed_at
                .as_deref()
                .map(short_time)
                .unwrap_or_else(|| "—".into());
            rsx! {
                span { class: "inline-flex items-center gap-2",
                    "{at}"
                    if d.stale {
                        span { class: "{BADGE} {tone(false, true)}", {t!("pipe-table-stale")} }
                    }
                }
            }
        }),
    ];
    let submit = move |_| {
        let title = name().trim().to_string();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::pipelines::create_table(&title).await {
                Ok(d) => {
                    adding.set(false);
                    navigator.push(Route::PipelineTable { id: d.id });
                }
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    rsx! {
        div { class: "space-y-0",
            FilterBar {
                SearchInput {
                    placeholder: t!("pipe-table-search").to_string(),
                    value: search,
                    on_submit: move |_| page.set(0),
                    page: Some(page),
                }
            }
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("pipe-table-empty").to_string(),
                div { class: "relative",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|d: &Dataset| d.id.clone()),
                        on_row_click: move |id: String| {
                            navigator.push(Route::PipelineTable { id });
                        },
                    }
                    ListPager { count, page }
                }
            }
        }
        if adding() {
            FormDialog {
                title: t!("pipe-table-add").to_string(),
                submit_label: t!("pipe-create").to_string(),
                on_close: move |_| adding.set(false),
                on_submit: submit,
                busy: busy(),
                valid: !name().trim().is_empty(),
                div { class: "space-y-3",
                    label { class: FIELD_LABEL, r#for: "pipe-table-name", {t!("pipe-col-name")} }
                    input {
                        id: "pipe-table-name",
                        class: INPUT,
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                    if let Some(e) = error() {
                        p { class: FIELD_ERROR, "{e}" }
                    }
                }
            }
        }
    }
}

#[component]
fn BuildList(reload: Signal<u32>) -> Element {
    let page = use_signal(|| 0i64);
    let mut open = use_signal(|| None::<Build>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::pipelines::builds(PAGE_SIZE, page() * PAGE_SIZE).await
    });
    let (state, count, rows) = match &*list.read() {
        None => (None, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), 0, Vec::new()),
    };
    let found = rows.clone();
    let columns = vec![
        Column::new(t!("pipe-col-target").to_string(), |b: &Build| {
            rsx! { "{b.target_name}" }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("pipe-col-state").to_string(), |b: &Build| {
            rsx! {
                span {
                    class: "{BADGE} {tone(b.state == BuildState::Done, b.state == BuildState::Failed)}",
                    "{build_state_label(b.state)}"
                }
            }
        }),
        Column::new(t!("pipe-col-steps").to_string(), |b: &Build| {
            let done = b
                .steps
                .iter()
                .filter(|s| s.state == BuildStepState::Done)
                .count();
            rsx! { "{done} / {b.steps.len()}" }
        }),
        Column::new(t!("pipe-col-trigger").to_string(), |b: &Build| {
            let label = if b.trigger == "schedule" {
                t!("pipe-trigger-schedule")
            } else {
                t!("pipe-trigger-manual")
            };
            rsx! { "{label}" }
        }),
        Column::new(t!("pipe-col-started").to_string(), |b: &Build| {
            rsx! { "{short_time(&b.created_at)}" }
        }),
    ];
    rsx! {
        ListStates {
            state,
            is_empty: count == 0,
            empty_message: t!("pipe-build-empty").to_string(),
            div { class: "relative",
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|b: &Build| b.id.clone()),
                    on_row_click: move |id: String| {
                        open.set(found.iter().find(|b| b.id == id).cloned());
                    },
                }
                ListPager { count, page }
            }
        }
        if let Some(b) = open() {
            Modal {
                title: b.target_name.clone(),
                max_width: "max-w-md".to_string(),
                on_close: move |_| open.set(None),
                div { class: "space-y-3",
                    if let Some(code) = b.code.clone() {
                        p { class: "text-sm text-alarm", "{code_label(&code, b.args.as_ref())}" }
                    }
                    BuildSteps { steps: b.steps.clone() }
                }
            }
        }
    }
}
