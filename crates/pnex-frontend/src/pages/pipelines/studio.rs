//! Studio of an org's pipeline (pipelines.md §6.3, D934), in the shared editor
//! shell (D222): the graph of the tables it reads and writes fills the stage, with
//! the first rows of the selected one under it; a closable column holds its YAML
//! document, checked and tested by the server as it is typed, its runs and its
//! schedule. Versions in the shell (D221).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::datasets::Dataset;
use pnex_core::pipelines::{
    Build, BuildState, BuildStep, BuildTargetKind, PipelineCheck, PipelineProbeResult,
    PipelineRunState, PipelineViolation,
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

/// `steps.2.sql` → `Step 3 · sql`, `tests.0.expect.rows` → `Test 1 · expect.rows`.
fn violation_place(path: &str) -> String {
    let mut parts = path.split('.');
    let (head, n) = (
        parts.next(),
        parts.next().and_then(|n| n.parse::<usize>().ok()),
    );
    let rest: Vec<&str> = parts.collect();
    let place = match (head, n) {
        (Some("steps"), Some(n)) => t!("pipe-step-n", n : n + 1).to_string(),
        (Some("tests"), Some(n)) => t!("pipe-test-n", n : n + 1).to_string(),
        _ => return path.to_string(),
    };
    match rest.is_empty() {
        true => place,
        false => format!("{place} · {}", rest.join(".")),
    }
}

#[component]
pub fn PipelineStudio(id: String) -> Element {
    let id = use_signal(|| id);
    let mut reload = use_signal(|| 0u32);
    let mut generation = use_signal(|| 0u32);
    let mut check = use_signal(|| None::<PipelineCheck>);
    let mut yaml = use_signal(String::new);
    let mut version = use_signal(|| None::<i32>);
    let mut dirty = use_signal(|| false);
    let mut opened = use_signal(|| None::<i32>);
    let mut violations = use_signal(Vec::<PipelineViolation>::new);
    let mut selected = use_signal(|| None::<String>);
    let mut tab = use_signal(|| Tab::Code);
    let mut column = use_signal(|| true);
    let mut message = use_signal(|| None::<String>);
    let mut versions_open = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let mut plan = use_signal(|| None::<Vec<BuildStep>>);
    let mut build = use_signal(|| None::<Build>);
    let mut gen = use_signal(|| 0u64);
    let mut restore_error = use_signal(|| None::<String>);
    let mut probe_step = use_signal(String::new);
    let mut probe = use_signal(|| None::<(String, PipelineProbeResult)>);
    let mut probing = use_signal(|| false);
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
                    check.set(Some(parsed));
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
                    check.set(Some(parsed));
                }
                Err(e) => violations.set(violations_of(&e)),
            }
        });
    };

    let mut open_tab = move |to: Tab| {
        message.set(None);
        tab.set(to);
    };

    let save = move |_| {
        message.set(None);
        let text = yaml();
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

    // Reads the draft as typed, saved or not: a probe writes nothing.
    let probe_clicked = move |step: String| {
        message.set(None);
        probing.set(true);
        let text = yaml();
        spawn(async move {
            match api::pipelines::probe(&text, &step).await {
                Ok(r) => probe.set(Some((step, r))),
                Err(e) => {
                    probe.set(None);
                    message.set(Some(localize(&e)));
                }
            }
            probing.set(false);
        });
    };

    // ── derived view ──
    let known: Vec<Dataset> = match &*tables.read() {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };
    let dataset_of = |name: &str| known.iter().find(|t| t.name == name).cloned();
    let (names, edges) = check().as_ref().map(document_graph).unwrap_or_default();
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
                        if let Some((step, r)) = probe() {
                            div {
                                class: "space-y-3 rounded-lg border border-line bg-surface p-3",
                                "data-testid": "pipe-probe-result",
                                div { class: "flex items-center gap-2",
                                    p { class: "min-w-0 break-all text-sm font-medium text-ink",
                                        {t!("pipe-probe-title", step : step)}
                                    }
                                    button {
                                        class: "{BTN_SECONDARY_SM} ml-auto",
                                        onclick: move |_| probe.set(None),
                                        {t!("pipe-probe-close")}
                                    }
                                }
                                ProbeSteps { steps: r.steps.clone() }
                                ProbeSample { rows: r.sample.clone() }
                            }
                        }
                    }
                    if column() {
                        aside { class: "flex max-h-[55%] w-full shrink-0 flex-col border-t border-line bg-surface md:max-h-none md:w-[26rem] md:border-l md:border-t-0",
                            div { class: "{TAB_BAR} px-3 pt-2",
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
                                if tab() == Tab::Code {
                                    if let Some(c) = check() {
                                        p { class: FIELD_HELP, "data-testid": "pipe-check",
                                            {t!("pipe-check-summary", steps : c.steps.len(), tests : c.tests)}
                                        }
                                        if can_run && !c.probes.is_empty() {
                                            ProbeForm {
                                                probes: c.probes.clone(),
                                                step: probe_step(),
                                                busy: probing() || has_errors,
                                                on_step: move |s: String| probe_step.set(s),
                                                on_probe: probe_clicked,
                                            }
                                        }
                                    }
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
                                check.set(Some(parsed));
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

#[component]
fn ProbeForm(
    probes: Vec<String>,
    step: String,
    busy: bool,
    on_step: EventHandler<String>,
    on_probe: EventHandler<String>,
) -> Element {
    // The last output is the default: probing the whole read is the common case.
    let current = match probes.contains(&step) {
        true => step,
        false => probes.last().cloned().unwrap_or_default(),
    };
    let chosen = current.clone();
    rsx! {
        div { class: "space-y-1",
            label { class: FIELD_LABEL, r#for: "pipe-probe-step", {t!("pipe-probe-step")} }
            div { class: "flex gap-2",
                select {
                    id: "pipe-probe-step",
                    class: "{INPUT} min-w-0 flex-1",
                    value: "{current}",
                    onchange: move |e| on_step.call(e.value()),
                    for p in probes {
                        option {
                            key: "{p}",
                            value: "{p}",
                            selected: p == current,
                            "{p}"
                        }
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    disabled: busy,
                    "data-testid": "pipe-probe",
                    onclick: move |_| on_probe.call(chosen.clone()),
                    {t!("pipe-probe")}
                }
            }
            p { class: FIELD_HELP, {t!("pipe-probe-help")} }
        }
    }
}

#[component]
fn ProbeSteps(steps: Vec<pnex_core::pipelines::PipelineProbeStep>) -> Element {
    rsx! {
        ol { class: "space-y-1", "data-testid": "pipe-probe-steps",
            for (i, s) in steps.iter().enumerate() {
                li {
                    key: "{i}",
                    class: "flex min-w-0 flex-wrap items-center gap-2 text-sm",
                    span { class: "text-slate", "{i + 1}." }
                    span { class: "font-medium text-ink", "{s.step}" }
                    span { class: "text-slate", {t!("pipe-rows", n : s.rows)} }
                    if s.bytes > 0 {
                        span { class: "text-slate", {t!("pipe-probe-bytes", n : s.bytes)} }
                    }
                    if let Some(url) = s.url.clone() {
                        span {
                            class: "min-w-0 max-w-full truncate font-mono text-xs text-slate",
                            title: "{url}",
                            "{url}"
                        }
                    }
                    if let Some(code) = s.code.clone() {
                        span { class: "text-xs text-alarm", "{code_label(&code, s.args.as_ref())}" }
                    }
                }
            }
        }
    }
}

/// The sample of a probe, as text: rows may not share their columns.
#[component]
fn ProbeSample(rows: Vec<std::collections::BTreeMap<String, String>>) -> Element {
    if rows.is_empty() {
        return rsx! {
            p { class: "text-sm text-slate", {t!("pipe-probe-no-rows")} }
        };
    }
    let columns: std::collections::BTreeSet<String> =
        rows.iter().flat_map(|r| r.keys().cloned()).collect();
    rsx! {
        div {
            class: "max-h-72 overflow-auto",
            "data-testid": "pipe-probe-sample",
            table { class: "min-w-full text-xs",
                thead {
                    tr {
                        for c in columns.iter() {
                            th {
                                key: "{c}",
                                class: "border-b border-line px-2 py-1.5 text-left font-medium text-ink",
                                "{c}"
                            }
                        }
                    }
                }
                tbody {
                    for (i, row) in rows.iter().enumerate() {
                        tr { key: "{i}",
                            for c in columns.iter() {
                                td {
                                    key: "{c}",
                                    class: "max-w-xs truncate border-b border-line px-2 py-1 text-ink",
                                    title: row.get(c).cloned().unwrap_or_default(),
                                    {row.get(c).cloned().unwrap_or_default()}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
