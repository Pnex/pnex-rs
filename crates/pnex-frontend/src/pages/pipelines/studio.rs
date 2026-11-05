//! Studio of a pipeline (data-pipelines.md §9.5), in the shared editor shell (D222):
//! the graph of its tables fills the stage, with the first rows of the selected one
//! under it; a closable column holds the transform, its code (YAML), its runs and its
//! schedule. Versions in the shell (D221).

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::datasets::Dataset;
use pnex_core::pipelines::{
    Build, BuildState, BuildStep, BuildTargetKind, Checks, PipelineDocument, PipelineRunState,
    PipelineViolation, Transform,
};

use super::graph::{document_graph, GraphNode, TableGraph};
use super::table::SamplePanel;
use super::{
    build_state_label, code_label, schedule_label, schedule_of, tone, BuildSteps, BADGE, SCHEDULES,
};
use crate::api;
use crate::api::error_i18n::{localize, localize_field};
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::editor_shell::{Chip, EditorShell, EditorStatus, StatusTone};
use crate::components::modal::Modal;
use crate::components::ontology::short_time;
use crate::components::ui::{
    BTN_DANGER, BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT, TAB, TAB_ACTIVE,
    TAB_BAR,
};
use crate::components::versions_drawer::{VersionRow, VersionsDrawer};
use crate::state::access::can;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Transform,
    Code,
    Runs,
    Schedule,
}

async fn sleep_ms(ms: u64) {
    crate::util::sleep(std::time::Duration::from_millis(ms)).await;
}

fn violations_of(e: &api::error::ApiError) -> Vec<PipelineViolation> {
    e.violations()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// `transforms.2.sql` → `Transform 3 · sql`.
fn violation_place(path: &str) -> String {
    let mut parts = path.split('.');
    match (
        parts.next(),
        parts.next().and_then(|n| n.parse::<usize>().ok()),
    ) {
        (Some("transforms"), Some(n)) => {
            let rest: Vec<&str> = parts.collect();
            format!("{} · {}", t!("pipe-transform-n", n : n + 1), rest.join("."))
        }
        _ => path.to_string(),
    }
}

/// A name no table of the document has.
fn free_name(doc: &PipelineDocument) -> String {
    let (names, _) = document_graph(doc);
    (1..)
        .map(|n| format!("table_{n}"))
        .find(|n| !names.contains(n))
        .unwrap_or_default()
}

/// Renames a table everywhere the document names it, so that a chain stays a chain.
fn rename_output(doc: &mut PipelineDocument, index: usize, to: &str) {
    let Some(from) = doc.transforms.get(index).map(|t| t.output.clone()) else {
        return;
    };
    for t in &mut doc.transforms {
        for table in t.inputs.values_mut() {
            if *table == from {
                *table = to.to_string();
            }
        }
    }
    doc.transforms[index].output = to.to_string();
}

/// Removes a transform; the tables that read its output keep the name, which then reads
/// whatever table has it.
fn remove_transform(doc: &mut PipelineDocument, index: usize) {
    if index < doc.transforms.len() {
        doc.transforms.remove(index);
    }
}

#[component]
pub fn PipelineStudio(id: String) -> Element {
    let id = use_signal(|| id);
    let mut reload = use_signal(|| 0u32);
    let mut generation = use_signal(|| 0u32);
    let mut doc = use_signal(|| PipelineDocument {
        transforms: Vec::new(),
    });
    let mut yaml = use_signal(String::new);
    let mut version = use_signal(|| None::<i32>);
    let mut dirty = use_signal(|| false);
    let mut opened = use_signal(|| None::<i32>);
    let mut violations = use_signal(Vec::<PipelineViolation>::new);
    let mut selected = use_signal(|| None::<String>);
    let mut tab = use_signal(|| Tab::Transform);
    let mut column = use_signal(|| true);
    let mut message = use_signal(|| None::<String>);
    let mut versions_open = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let mut plan = use_signal(|| None::<Vec<BuildStep>>);
    let mut build = use_signal(|| None::<Build>);
    let mut gen = use_signal(|| 0u64);
    let mut restore_error = use_signal(|| None::<String>);
    let navigator = use_navigator();
    let can_write = can("pipelines:write");
    let can_run = can("pipelines:run");

    let saved = use_resource(move || async move {
        let _ = reload();
        api::pipelines::get(&id()).await
    });
    let tables = use_resource(move || async move {
        let _ = generation();
        api::pipelines::tables().await
    });
    let runs = use_resource(move || async move {
        let _ = generation();
        api::pipelines::runs(&id(), 20).await
    });
    let versions = use_resource(move || async move {
        let _ = reload();
        api::pipelines::versions(&id()).await
    });

    // A version read from the server opens once: a reload never drops unsaved edits.
    use_effect(move || {
        let Some(Ok(p)) = &*saved.read() else {
            return;
        };
        if *version.peek() == Some(p.version) || *dirty.peek() {
            return;
        }
        let p = p.clone();
        spawn(async move {
            match api::pipelines::validate(&p.document).await {
                Ok(parsed) => {
                    doc.set(parsed);
                    yaml.set(p.document.clone());
                    version.set(Some(p.version));
                    violations.set(Vec::new());
                    opened.set(None);
                }
                Err(e) => {
                    violations.set(violations_of(&e));
                    message.set(Some(localize(&e)));
                }
            }
        });
    });

    let mut changed = move || {
        dirty.set(true);
        violations.set(doc.peek().violations());
    };

    // Code edits: parsed by the server shortly after the last keystroke.
    let mut code_edit = move |text: String| {
        yaml.set(text.clone());
        dirty.set(true);
        gen.with_mut(|g| *g += 1);
        let mine = *gen.peek();
        spawn(async move {
            sleep_ms(400).await;
            if *gen.peek() != mine {
                return;
            }
            match api::pipelines::validate(&text).await {
                Ok(parsed) => {
                    violations.set(Vec::new());
                    doc.set(parsed);
                }
                Err(e) => violations.set(violations_of(&e)),
            }
        });
    };

    // Opening the code writes it from the structure; leaving it needs a code the server
    // accepts.
    let mut open_tab = move |to: Tab| {
        let from = tab();
        if to == from {
            return;
        }
        if from == Tab::Code && !violations().is_empty() {
            message.set(Some(t!("pipe-yaml-fix-first").to_string()));
            return;
        }
        if to == Tab::Code {
            yaml.set(doc.read().to_yaml());
        }
        message.set(None);
        tab.set(to);
    };

    let save = move |_| {
        message.set(None);
        let text = if tab() == Tab::Code {
            yaml()
        } else {
            doc.read().to_yaml()
        };
        let Some(expected) = version() else {
            return;
        };
        spawn(async move {
            match api::pipelines::update(&id(), &text, expected).await {
                Ok(p) => {
                    version.set(Some(p.version));
                    yaml.set(p.document);
                    dirty.set(false);
                    opened.set(None);
                    reload.with_mut(|n| *n += 1);
                }
                Err(e) => {
                    violations.set(violations_of(&e));
                    message.set(Some(localize(&e)));
                }
            }
        });
    };

    // Follows a chained run until it ends, then shows what it wrote.
    let mut follow = move |run: Build| {
        let build_id = run.id.clone();
        build.set(Some(run));
        spawn(async move {
            loop {
                sleep_ms(2000).await;
                match api::pipelines::build(&build_id).await {
                    Ok(b) => {
                        let over = b.state != BuildState::Running;
                        build.set(Some(b));
                        if over {
                            generation.with_mut(|n| *n += 1);
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    };

    let mut start_build = move || {
        plan.set(None);
        message.set(None);
        spawn(async move {
            match api::pipelines::start_build(BuildTargetKind::Pipeline, &id()).await {
                Ok(b) => follow(b),
                Err(e) => message.set(Some(localize(&e))),
            }
        });
    };

    // A run that brings dependencies along is shown before it starts: an import of an
    // API spends its quota.
    let run_clicked = move |_| {
        message.set(None);
        spawn(async move {
            match api::pipelines::plan(BuildTargetKind::Pipeline, &id()).await {
                Ok(p) if p.steps.len() > 1 => plan.set(Some(p.steps)),
                Ok(_) => start_build(),
                Err(e) => message.set(Some(localize(&e))),
            }
        });
    };

    let add_transform = move |_| {
        let current = doc.read().clone();
        let (names, _) = document_graph(&current);
        let Some(source) = selected().or_else(|| names.last().cloned()) else {
            return;
        };
        let output = free_name(&current);
        doc.write().transforms.push(Transform {
            output: output.clone(),
            inputs: BTreeMap::from([("source".to_string(), source)]),
            sql: "SELECT * FROM source".to_string(),
            checks: Checks::default(),
        });
        selected.set(Some(output));
        column.set(true);
        tab.set(Tab::Transform);
        changed();
    };

    // ── derived view ──
    let d = doc.read().clone();
    let known: Vec<Dataset> = match &*tables.read() {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };
    let dataset_of = |name: &str| known.iter().find(|t| t.name == name).cloned();
    let (names, edges) = document_graph(&d);
    let nodes: Vec<GraphNode> = names
        .iter()
        .map(|name| {
            let found = dataset_of(name);
            GraphNode {
                id: name.clone(),
                detail: match &found {
                    Some(t) if t.committed_at.is_some() => t!("pipe-rows", n : t.rows).to_string(),
                    _ => t!("pipe-not-computed").to_string(),
                },
                warn: found.is_some_and(|t| t.stale),
            }
        })
        .collect();
    let picked = selected();
    let picked_dataset = picked.as_deref().and_then(dataset_of);
    let picked_index = picked
        .as_ref()
        .and_then(|name| d.transforms.iter().position(|t| &t.output == name));
    let has_errors = !violations().is_empty();
    let running = build().is_some_and(|b| b.state == BuildState::Running);
    let name = match &*saved.read() {
        Some(Ok(p)) => p.name.clone(),
        _ => String::new(),
    };
    let current_schedule = match &*saved.read() {
        Some(Ok(p)) => schedule_of(&p.recurrence),
        _ => String::new(),
    };
    let next_run = match &*saved.read() {
        Some(Ok(p)) => p.next_run_at.clone(),
        _ => None,
    };
    let status = match build() {
        Some(b) => EditorStatus::new(
            match b.state {
                BuildState::Done => StatusTone::Green,
                BuildState::Failed => StatusTone::Red,
                BuildState::Running => StatusTone::Amber,
            },
            build_state_label(b.state),
        ),
        None => EditorStatus::new(StatusTone::Slate, t!("pipe-status-idle")),
    };
    let column_button = if column() {
        format!("{BTN_SECONDARY_SM} border-signal text-signal")
    } else {
        BTN_SECONDARY_SM.to_string()
    };
    let run_title = if dirty() {
        t!("pipe-save-before-run").to_string()
    } else {
        String::new()
    };
    let rows: Option<Result<Vec<VersionRow>, api::error::ApiError>> =
        versions.read().clone().map(|r| {
            r.map(|list| {
                list.into_iter()
                    .map(|v| VersionRow {
                        version: v.version,
                        yaml: v.document,
                        created_at: v.created_at,
                        author: v.author,
                    })
                    .collect()
            })
        });
    let table_names: Vec<String> = {
        let mut all: Vec<String> = known.iter().map(|t| t.name.clone()).collect();
        for n in &names {
            if !all.contains(n) {
                all.push(n.clone());
            }
        }
        all
    };

    rsx! {
        EditorShell {
            on_back: move |_| {
                navigator.push(Route::Pipelines {});
            },
            title: name.clone(),
            status,
            version: version().map(i64::from),
            extra_chips: rsx! {
                if dirty() {
                    Chip { tone: StatusTone::Amber, label: t!("pipe-unsaved").to_string() }
                }
                if let Some(v) = opened() {
                    Chip {
                        tone: StatusTone::Purple,
                        label: t!("web-sites-version-opened", v : v).to_string(),
                    }
                }
            },
            actions: rsx! {
                button {
                    class: "{column_button}",
                    "aria-pressed": column(),
                    onclick: move |_| column.toggle(),
                    {t!("pipe-column")}
                }
                button { class: BTN_SECONDARY_SM, onclick: move |_| versions_open.set(true),
                    {t!("web-sites-versions")}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    disabled: !can_run || has_errors || dirty() || running,
                    title: "{run_title}",
                    "data-testid": "pipe-run",
                    onclick: run_clicked,
                    {t!("pipe-run")}
                }
                button {
                    class: BTN_PRIMARY_SM,
                    disabled: !can_write || has_errors || !dirty(),
                    "data-testid": "pipe-save",
                    onclick: save,
                    {t!("pipe-save")}
                }
            },
            canvas: rsx! {
                div { class: "flex h-full w-full flex-col bg-canvas md:flex-row",
                    div { class: "flex min-h-0 min-w-0 flex-1 flex-col gap-3 p-3",
                        div { class: "min-h-0 flex-1 overflow-auto rounded-lg border border-line bg-surface",
                            TableGraph {
                                nodes,
                                edges,
                                selected: picked.clone(),
                                on_select: move |table: String| {
                                    selected.set(Some(table));
                                    column.set(true);
                                },
                            }
                        }
                        if let Some(b) = build() {
                            div { class: "rounded-lg border border-line bg-surface p-3",
                                if let Some(code) = b.code.clone() {
                                    p { class: "mb-2 text-sm text-alarm", "{code_label(&code, b.args.as_ref())}" }
                                }
                                BuildSteps { steps: b.steps.clone() }
                            }
                        }
                        if let Some(table) = picked_dataset.clone() {
                            div { class: "max-h-72 overflow-auto rounded-lg border border-line bg-surface",
                                SamplePanel {
                                    key: "{table.id}",
                                    dataset_id: table.id.clone(),
                                    columns: table.columns.clone(),
                                    generation: generation(),
                                }
                            }
                        } else if picked.is_some() {
                            p { class: "text-sm text-slate", {t!("pipe-not-computed-help")} }
                        }
                    }
                    if column() {
                        aside { class: "flex max-h-[55%] w-full shrink-0 flex-col border-t border-line bg-surface md:max-h-none md:w-[26rem] md:border-l md:border-t-0",
                            div { class: "{TAB_BAR} px-3 pt-2",
                                button {
                                    class: if tab() == Tab::Transform { TAB_ACTIVE } else { TAB },
                                    onclick: move |_| open_tab(Tab::Transform),
                                    {t!("pipe-tab-transform")}
                                }
                                button {
                                    class: if tab() == Tab::Code { TAB_ACTIVE } else { TAB },
                                    "data-testid": "pipe-tab-code",
                                    onclick: move |_| open_tab(Tab::Code),
                                    {t!("pipe-tab-code")}
                                }
                                button {
                                    class: if tab() == Tab::Runs { TAB_ACTIVE } else { TAB },
                                    onclick: move |_| open_tab(Tab::Runs),
                                    {t!("pipe-tab-runs")}
                                }
                                button {
                                    class: if tab() == Tab::Schedule { TAB_ACTIVE } else { TAB },
                                    onclick: move |_| open_tab(Tab::Schedule),
                                    {t!("pipe-tab-schedule")}
                                }
                            }
                            div { class: "min-h-0 flex-1 space-y-3 overflow-y-auto p-3",
                                if !violations().is_empty() {
                                    ul { class: "space-y-1", "data-testid": "pipe-violations",
                                        for (i, v) in violations().iter().enumerate() {
                                            li { key: "{i}", class: "text-sm text-alarm",
                                                "{violation_place(&v.path)} : {localize_field(&v.path, &v.token)}"
                                            }
                                        }
                                    }
                                }
                                if let Some(m) = message() {
                                    p {
                                        class: "text-sm text-alarm",
                                        "data-testid": "pipe-message",
                                        "{m}"
                                    }
                                }
                                if tab() == Tab::Transform {
                                    if can_write && !names.is_empty() {
                                        button {
                                            class: BTN_SECONDARY_SM,
                                            "data-testid": "pipe-add-transform",
                                            onclick: add_transform,
                                            {t!("pipe-transform-add")}
                                        }
                                    }
                                    if let Some(i) = picked_index {
                                        TransformEditor {
                                            key: "{i}",
                                            transform: d.transforms[i].clone(),
                                            tables: table_names.clone(),
                                            can_write,
                                            can_delete: d.transforms.len() > 1,
                                            on_rename: move |to: String| {
                                                rename_output(&mut doc.write(), i, &to);
                                                selected.set(Some(to));
                                                changed();
                                            },
                                            on_change: move |t: Transform| {
                                                if let Some(slot) = doc.write().transforms.get_mut(i) {
                                                    *slot = t;
                                                }
                                                changed();
                                            },
                                            on_delete: move |_| {
                                                remove_transform(&mut doc.write(), i);
                                                selected.set(None);
                                                changed();
                                            },
                                        }
                                    } else if let Some(table) = picked_dataset.clone() {
                                        p { class: FIELD_HELP, {t!("pipe-source-help")} }
                                        button {
                                            class: BTN_SECONDARY_SM,
                                            onclick: move |_| {
                                                navigator
                                                    .push(Route::PipelineTable {
                                                        id: table.id.clone(),
                                                    });
                                            },
                                            {t!("pipe-open-table")}
                                        }
                                    } else {
                                        p { class: FIELD_HELP, {t!("pipe-pick-node")} }
                                    }
                                }
                                if tab() == Tab::Code {
                                    textarea {
                                        class: "{INPUT} min-h-96 font-mono text-xs",
                                        spellcheck: "false",
                                        disabled: !can_write,
                                        "data-testid": "pipe-yaml",
                                        value: "{yaml}",
                                        oninput: move |e| code_edit(e.value()),
                                    }
                                }
                                if tab() == Tab::Runs {
                                    RunList {
                                        runs: match &*runs.read() {
                                            Some(Ok(p)) => p.results.clone(),
                                            _ => Vec::new(),
                                        },
                                    }
                                }
                                if tab() == Tab::Schedule {
                                    ScheduleForm {
                                        pipeline_id: id(),
                                        current: current_schedule.clone(),
                                        next_run: next_run.clone(),
                                        can_run,
                                        on_saved: move |_| reload.with_mut(|n| *n += 1),
                                    }
                                    if can("pipelines:delete") {
                                        button {
                                            class: BTN_DANGER,
                                            onclick: move |_| confirm_delete.set(true),
                                            {t!("pipe-delete-pipeline")}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
        if versions_open() {
            VersionsDrawer {
                rows,
                active_version: version().unwrap_or_default(),
                can_write,
                error: restore_error(),
                on_close: move |_| versions_open.set(false),
                on_open: move |v: VersionRow| {
                    versions_open.set(false);
                    spawn(async move {
                        match api::pipelines::validate(&v.yaml).await {
                            Ok(parsed) => {
                                doc.set(parsed);
                                yaml.set(v.yaml.clone());
                                violations.set(Vec::new());
                                opened.set(Some(v.version));
                                dirty.set(true);
                            }
                            Err(e) => message.set(Some(localize(&e))),
                        }
                    });
                },
                on_restore: move |old: i32| {
                    let document = match &*versions.read() {
                        Some(Ok(list)) => {
                            list.iter().find(|v| v.version == old).map(|v| v.document.clone())
                        }
                        _ => None,
                    };
                    let (Some(document), Some(expected)) = (document, version()) else {
                        return;
                    };
                    spawn(async move {
                        match api::pipelines::update(&id(), &document, expected).await {
                            Ok(_) => {
                                dirty.set(false);
                                versions_open.set(false);
                                reload.with_mut(|n| *n += 1);
                            }
                            Err(e) => restore_error.set(Some(localize(&e))),
                        }
                    });
                },
            }
        }
        if let Some(steps) = plan() {
            Modal {
                title: t!("pipe-plan-title").to_string(),
                max_width: "max-w-md".to_string(),
                on_close: move |_| plan.set(None),
                div { class: "space-y-3",
                    p { class: FIELD_HELP, {t!("pipe-plan-help")} }
                    BuildSteps { steps }
                    button {
                        class: BTN_PRIMARY_SM,
                        "data-testid": "pipe-plan-confirm",
                        onclick: move |_| start_build(),
                        {t!("pipe-run")}
                    }
                }
            }
        }
        if confirm_delete() {
            ConfirmDialog {
                title: t!("pipe-delete-pipeline").to_string(),
                message: t!("pipe-delete-message", name : name.clone()).to_string(),
                confirm_label: t!("pipe-delete").to_string(),
                on_cancel: move |_| confirm_delete.set(false),
                on_confirm: move |_| {
                    confirm_delete.set(false);
                    spawn(async move {
                        match api::pipelines::delete(&id()).await {
                            Ok(()) => {
                                navigator.push(Route::Pipelines {});
                            }
                            Err(e) => message.set(Some(localize(&e))),
                        }
                    });
                },
            }
        }
    }
}

#[component]
fn RunList(runs: Vec<pnex_core::pipelines::PipelineRun>) -> Element {
    if runs.is_empty() {
        return rsx! {
            p { class: FIELD_HELP, {t!("pipe-runs-empty")} }
        };
    }
    rsx! {
        ul { class: "divide-y divide-line", "data-testid": "pipe-runs",
            for r in runs {
                li { key: "{r.id}", class: "space-y-1 py-2 text-sm",
                    div { class: "flex flex-wrap items-center gap-2",
                        span { class: "{BADGE} {tone(r.state == PipelineRunState::Done, r.state == PipelineRunState::Failed)}",
                            {
                                match r.state {
                                    PipelineRunState::Queued => t!("pipe-run-queued"),
                                    PipelineRunState::Running => t!("pipe-run-running"),
                                    PipelineRunState::Done => t!("pipe-run-done"),
                                    PipelineRunState::Failed => t!("pipe-run-failed"),
                                }
                            }
                        }
                        span { class: "text-ink", "v{r.version}" }
                        span { class: "ml-auto text-xs text-slate", "{short_time(&r.created_at)}" }
                    }
                    if let Some(code) = r.code.clone() {
                        p { class: "text-xs text-alarm", "{code_label(&code, r.args.as_ref())}" }
                    }
                }
            }
        }
    }
}

#[component]
fn ScheduleForm(
    pipeline_id: String,
    current: String,
    next_run: Option<String>,
    can_run: bool,
    on_saved: EventHandler<()>,
) -> Element {
    let mut error = use_signal(|| None::<String>);
    rsx! {
        div { class: "space-y-2",
            label { class: FIELD_LABEL, r#for: "pipe-schedule", {t!("pipe-tab-schedule")} }
            select {
                id: "pipe-schedule",
                class: INPUT,
                disabled: !can_run,
                value: "{current}",
                onchange: move |e| {
                    let unit = e.value();
                    let id = pipeline_id.clone();
                    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                    let recurrence = (!unit.is_empty())
                        .then(|| serde_json::json!({ "start" : today, "every" : 1, "unit" : unit }));
                    spawn(async move {
                        match api::pipelines::schedule(&id, recurrence).await {
                            Ok(_) => {
                                error.set(None);
                                on_saved.call(());
                            }
                            Err(e) => error.set(Some(localize(&e))),
                        }
                    });
                },
                for s in SCHEDULES {
                    option { key: "{s}", value: "{s}", selected: current == s, "{schedule_label(s)}" }
                }
            }
            p { class: FIELD_HELP, {t!("pipe-schedule-help")} }
            if let Some(at) = next_run {
                p { class: FIELD_HELP, {t!("pipe-next-run", at : short_time(& at))} }
            }
            if let Some(e) = error() {
                p { class: "text-sm text-alarm", "{e}" }
            }
        }
    }
}

/// `name=5` lines, one per column.
fn parse_empty_pct(text: &str) -> BTreeMap<String, u8> {
    text.lines()
        .filter_map(|l| {
            let (column, pct) = l.split_once('=')?;
            Some((column.trim().to_string(), pct.trim().parse().ok()?))
        })
        .filter(|(c, _)| !c.is_empty())
        .collect()
}

fn format_empty_pct(map: &BTreeMap<String, u8>) -> String {
    map.iter()
        .map(|(c, p)| format!("{c}={p}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[component]
fn TransformEditor(
    transform: Transform,
    tables: Vec<String>,
    can_write: bool,
    can_delete: bool,
    on_rename: EventHandler<String>,
    on_change: EventHandler<Transform>,
    on_delete: EventHandler<()>,
) -> Element {
    let t = transform;
    let inputs: Vec<(String, String)> = t.inputs.clone().into_iter().collect();
    let edit = move |f: Box<dyn FnOnce(&mut Transform)>, base: &Transform| {
        let mut next = base.clone();
        f(&mut next);
        on_change.call(next);
    };
    let free_alias = (1..)
        .map(|n| format!("source_{n}"))
        .find(|a| !t.inputs.contains_key(a))
        .unwrap_or_default();
    rsx! {
        div { class: "space-y-3", "data-testid": "pipe-transform",
            datalist { id: "pipe-tables",
                for name in tables {
                    option { key: "{name}", value: "{name}" }
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "pipe-transform-output", {t!("pipe-output")} }
                input {
                    id: "pipe-transform-output",
                    class: INPUT,
                    disabled: !can_write,
                    value: "{t.output}",
                    onchange: move |e| on_rename.call(e.value().trim().to_string()),
                }
            }
            div { class: "space-y-2",
                span { class: FIELD_LABEL, {t!("pipe-inputs")} }
                for (alias, table) in inputs {
                    div { key: "{alias}", class: "flex items-center gap-2",
                        input {
                            class: "{INPUT} font-mono",
                            disabled: !can_write,
                            aria_label: t!("pipe-input-alias"),
                            value: "{alias}",
                            onchange: {
                                let (t, alias, table) = (t.clone(), alias.clone(), table.clone());
                                move |e: FormEvent| {
                                    let to = e.value().trim().to_string();
                                    let (old, table) = (alias.clone(), table.clone());
                                    edit(
                                        Box::new(move |n| {
                                            n.inputs.remove(&old);
                                            n.inputs.insert(to, table);
                                        }),
                                        &t,
                                    );
                                }
                            },
                        }
                        input {
                            class: INPUT,
                            disabled: !can_write,
                            list: "pipe-tables",
                            aria_label: t!("pipe-input-table"),
                            value: "{table}",
                            onchange: {
                                let (t, alias) = (t.clone(), alias.clone());
                                move |e: FormEvent| {
                                    let (to, alias) = (e.value().trim().to_string(), alias.clone());
                                    edit(
                                        Box::new(move |n| {
                                            n.inputs.insert(alias, to);
                                        }),
                                        &t,
                                    );
                                }
                            },
                        }
                        button {
                            class: BTN_SECONDARY_SM,
                            disabled: !can_write,
                            aria_label: t!("pipe-input-remove"),
                            onclick: {
                                let (t, alias) = (t.clone(), alias.clone());
                                move |_| {
                                    let alias = alias.clone();
                                    edit(
                                        Box::new(move |n| {
                                            n.inputs.remove(&alias);
                                        }),
                                        &t,
                                    );
                                }
                            },
                            "✕"
                        }
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    disabled: !can_write,
                    onclick: {
                        let t = t.clone();
                        move |_| {
                            let alias = free_alias.clone();
                            edit(
                                Box::new(move |n| {
                                    n.inputs.insert(alias, String::new());
                                }),
                                &t,
                            );
                        }
                    },
                    {t!("pipe-input-add")}
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "pipe-transform-sql", {t!("pipe-sql")} }
                textarea {
                    id: "pipe-transform-sql",
                    class: "{INPUT} min-h-48 font-mono text-xs",
                    spellcheck: "false",
                    disabled: !can_write,
                    value: "{t.sql}",
                    oninput: {
                        let t = t.clone();
                        move |e: FormEvent| {
                            let sql = e.value();
                            edit(Box::new(move |n| n.sql = sql), &t);
                        }
                    },
                }
                p { class: FIELD_HELP, {t!("pipe-sql-help")} }
            }
            details { class: "rounded-md border border-line",
                summary { class: "cursor-pointer px-3 py-2 text-sm font-medium text-ink",
                    {t!("pipe-checks")}
                }
                div { class: "space-y-3 p-3",
                    p { class: FIELD_HELP, {t!("pipe-checks-help")} }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-min-rows",
                            {t!("pipe-check-min-rows")}
                        }
                        input {
                            id: "pipe-min-rows",
                            class: INPUT,
                            inputmode: "numeric",
                            disabled: !can_write,
                            value: t.checks.min_rows.map(|v| v.to_string()).unwrap_or_default(),
                            onchange: {
                                let t = t.clone();
                                move |e: FormEvent| {
                                    let v = e.value().trim().parse().ok();
                                    edit(Box::new(move |n| n.checks.min_rows = v), &t);
                                }
                            },
                        }
                    }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-max-rows",
                            {t!("pipe-check-max-rows")}
                        }
                        input {
                            id: "pipe-max-rows",
                            class: INPUT,
                            inputmode: "numeric",
                            disabled: !can_write,
                            value: t.checks.max_rows.map(|v| v.to_string()).unwrap_or_default(),
                            onchange: {
                                let t = t.clone();
                                move |e: FormEvent| {
                                    let v = e.value().trim().parse().ok();
                                    edit(Box::new(move |n| n.checks.max_rows = v), &t);
                                }
                            },
                        }
                    }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-unique", {t!("pipe-check-unique")} }
                        input {
                            id: "pipe-unique",
                            class: INPUT,
                            disabled: !can_write,
                            value: t.checks.unique.join(", "),
                            onchange: {
                                let t = t.clone();
                                move |e: FormEvent| {
                                    let v = parse_list(&e.value());
                                    edit(Box::new(move |n| n.checks.unique = v), &t);
                                }
                            },
                        }
                    }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-max-empty",
                            {t!("pipe-check-max-empty")}
                        }
                        textarea {
                            id: "pipe-max-empty",
                            class: "{INPUT} font-mono text-xs",
                            disabled: !can_write,
                            value: format_empty_pct(&t.checks.max_empty_pct),
                            onchange: {
                                let t = t.clone();
                                move |e: FormEvent| {
                                    let v = parse_empty_pct(&e.value());
                                    edit(Box::new(move |n| n.checks.max_empty_pct = v), &t);
                                }
                            },
                        }
                    }
                    div {
                        label { class: FIELD_LABEL, r#for: "pipe-max-drop",
                            {t!("pipe-check-max-drop")}
                        }
                        input {
                            id: "pipe-max-drop",
                            class: INPUT,
                            inputmode: "numeric",
                            disabled: !can_write,
                            value: t.checks.max_drop_pct.map(|v| v.to_string()).unwrap_or_default(),
                            onchange: {
                                let t = t.clone();
                                move |e: FormEvent| {
                                    let v = e.value().trim().parse().ok();
                                    edit(Box::new(move |n| n.checks.max_drop_pct = v), &t);
                                }
                            },
                        }
                    }
                    label { class: "flex items-center gap-2 text-sm text-ink",
                        input {
                            r#type: "checkbox",
                            disabled: !can_write,
                            checked: t.checks.same_columns,
                            onchange: {
                                let t = t.clone();
                                move |e: FormEvent| {
                                    let v = e.checked();
                                    edit(Box::new(move |n| n.checks.same_columns = v), &t);
                                }
                            },
                        }
                        {t!("pipe-check-same-columns")}
                    }
                }
            }
            if can_write && can_delete {
                button { class: BTN_DANGER, onclick: move |_| on_delete.call(()),
                    {t!("pipe-transform-delete")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> PipelineDocument {
        let mut d = PipelineDocument::starter("raw", "clean");
        d.transforms.push(Transform {
            output: "final".into(),
            inputs: BTreeMap::from([("c".to_string(), "clean".to_string())]),
            sql: "SELECT * FROM c".into(),
            checks: Checks::default(),
        });
        d
    }

    #[test]
    fn renaming_an_output_keeps_the_chain() {
        let mut d = chain();
        rename_output(&mut d, 0, "tidy");
        assert_eq!(d.transforms[0].output, "tidy");
        assert_eq!(d.transforms[1].inputs["c"], "tidy");
        assert!(d.violations().is_empty(), "{:?}", d.violations());
    }

    #[test]
    fn a_new_table_name_is_free() {
        let mut d = chain();
        assert_eq!(free_name(&d), "table_1");
        d.transforms[0].output = "table_1".into();
        assert_eq!(free_name(&d), "table_2");
    }

    #[test]
    fn check_fields_read_what_they_write() {
        let map = parse_empty_pct("name = 5\nbad\n=3\ncity=x\nsiren=0");
        assert_eq!(map.len(), 2);
        assert_eq!(map["name"], 5);
        assert_eq!(parse_empty_pct(&format_empty_pct(&map)), map);
        assert_eq!(parse_list(" a, b ,,c"), ["a", "b", "c"]);
    }
}
