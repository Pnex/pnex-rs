//! Imports tab (partner-watch.md D301, D303): declarative imports of
//! records into the ontology. A preview shows what would be created,
//! changed and closed, and the errors by row and column; nothing is
//! written before « Apply ». URL and API imports may run on a schedule.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::import::{
    ImportApply, ImportMode, ImportPreview, ImportRunView, ImportSource, ObjectImportInput,
    ObjectImportView, RowIssue,
};

use crate::api;
use crate::api::error_i18n::{localize_code, localize_field};
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN, PRIMARY_BTN};
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::modal::Modal;
use crate::components::ontology::short_time;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::toasts;

/// Schedules offered in the form (a `recurrence` from today).
const SCHEDULES: [&str; 4] = ["", "day", "week", "month"];

fn schedule_of(v: &Option<serde_json::Value>) -> String {
    v.as_ref()
        .and_then(|r| r.get("unit"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string()
}

fn schedule_label(unit: &str) -> String {
    match unit {
        "day" => t!("onto-imp-schedule-day").to_string(),
        "week" => t!("onto-imp-schedule-week").to_string(),
        "month" => t!("onto-imp-schedule-month").to_string(),
        _ => t!("onto-imp-schedule-none").to_string(),
    }
}

fn source_label(s: ImportSource) -> String {
    match s {
        ImportSource::File => t!("onto-imp-source-file").to_string(),
        ImportSource::Url => t!("onto-imp-source-url").to_string(),
        ImportSource::Api => t!("onto-imp-source-api").to_string(),
    }
}

fn state_label(r: &ImportRunView) -> String {
    let state = match r.state.as_str() {
        "running" => t!("onto-imp-state-running"),
        "preview" => t!("onto-imp-state-preview"),
        "applied" => t!("onto-imp-state-applied"),
        "refused" => t!("onto-imp-state-refused"),
        "stale" => t!("onto-imp-state-stale"),
        _ => t!("onto-imp-state-failed"),
    };
    match &r.state_code {
        Some(code) => format!("{state} — {}", run_message(code, r.state_args.as_ref())),
        None => state.to_string(),
    }
}

/// The message of a run's code, with what a source too large held.
fn run_message(code: &str, args: Option<&serde_json::Value>) -> String {
    let num = |k: &str| {
        args.and_then(|a| a.get(k))
            .and_then(serde_json::Value::as_u64)
    };
    let too_large = code == pnex_core::err_codes::IMPORT_SOURCE_TOO_LARGE;
    match (num("rows"), num("bytes"), num("max")) {
        (Some(found), _, Some(max)) if too_large => {
            t!("onto-imp-too-large-rows", found : found, max : max).to_string()
        }
        (_, Some(found), Some(max)) if too_large => {
            let mb = |b: u64| b.div_ceil(1_000_000);
            t!("onto-imp-too-large-bytes", found : mb(found), max : mb(max)).to_string()
        }
        _ => localize_code(code, args, code),
    }
}

#[component]
pub fn ImportsTab(tabs: Element) -> Element {
    let mut reload = use_signal(|| 0u32);
    let can_write = crate::state::access::can("ontology:write");
    let list = use_resource(move || async move {
        let _ = reload();
        api::ontology::imports().await.unwrap_or_default()
    });
    let rows: Vec<ObjectImportView> = list.read().clone().unwrap_or_default();
    let mut editing = use_signal(|| None::<Option<ObjectImportView>>);
    let mut previewing = use_signal(|| None::<ObjectImportView>);
    let mut history = use_signal(|| None::<ObjectImportView>);
    let mut deleting = use_signal(|| None::<ObjectImportView>);
    let columns = vec![
        Column::new(t!("onto-imp-name").to_string(), |i: &ObjectImportView| {
            rsx! {
                div { "data-import": "{i.name}",
                    span { class: "font-medium text-gray-900", "{i.name}" }
                    p { class: "text-xs text-gray-500",
                        {format!("{} · {}", source_label(i.source), schedule_label(&schedule_of(&i.recurrence)))}
                    }
                }
            }
        }),
        Column::new(
            t!("onto-imp-last-run").to_string(),
            |i: &ObjectImportView| match &i.last_run {
                None => rsx! {
                    span { class: "text-xs text-gray-400", "—" }
                },
                Some(r) => {
                    let label = state_label(r);
                    let at = short_time(r.finished_at.as_deref().unwrap_or(&r.started_at));
                    rsx! {
                        span { class: "text-xs text-gray-700", "{label}" }
                        p { class: "text-xs text-gray-500", "{at}" }
                    }
                }
            },
        ),
        Column::new(
            t!("common-actions").to_string(),
            move |i: &ObjectImportView| {
                let (a, b, c, d) = (i.clone(), i.clone(), i.clone(), i.clone());
                // Something to fetch: a URL or API source, or its own steps.
                let runnable = i.source != ImportSource::File
                    || i.mapping.lines().any(|l| l.starts_with("steps:"));
                let id = i.id.clone();
                rsx! {
                    div { class: "flex flex-wrap gap-2",
                        if can_write && runnable {
                            button {
                                class: BTN_SECONDARY_SM,
                                "data-testid": "onto-imp-run-now",
                                onclick: move |_| {
                                    let id = id.clone();
                                    let mut reload = reload;
                                    spawn(async move {
                                        match api::ontology::start_import(&id).await {
                                            Ok(_) => {
                                                toasts::success(t!("onto-imp-run-started"));
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(e) => toasts::error(e),
                                        }
                                    });
                                },
                                {t!("onto-imp-run-now")}
                            }
                        }
                        if can_write {
                            button {
                                class: BTN_SECONDARY_SM,
                                "data-testid": "onto-imp-preview",
                                onclick: move |_| previewing.set(Some(a.clone())),
                                {t!("onto-imp-preview")}
                            }
                            button {
                                class: BTN_SECONDARY_SM,
                                onclick: move |_| editing.set(Some(Some(b.clone()))),
                                {t!("onto-edit")}
                            }
                        }
                        button {
                            class: BTN_SECONDARY_SM,
                            onclick: move |_| history.set(Some(c.clone())),
                            {t!("onto-imp-runs")}
                        }
                        if can_write {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| deleting.set(Some(d.clone())),
                                {t!("common-delete")}
                            }
                        }
                    }
                }
            },
        )
        .actions(),
    ];
    rsx! {
        ListLayout {
            title: t!("onto-tab-imports").to_string(),
            tabs,
            can_write,
            subtitle: Some(t!("onto-imp-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            actions: rsx! {
                if can_write {
                    button {
                        class: PRIMARY_BTN,
                        "data-testid": "onto-imp-add",
                        onclick: move |_| editing.set(Some(None)),
                        {t!("onto-imp-add")}
                    }
                }
            },
            if rows.is_empty() {
                p { class: "text-sm text-gray-500 py-6", {t!("onto-imp-empty")} }
            }
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|i: &ObjectImportView| i.id.clone()),
            }
            if let Some(initial) = editing() {
                ImportForm {
                    initial,
                    on_close: move |_| editing.set(None),
                    on_saved: move |_| {
                        editing.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some(i) = previewing() {
                PreviewDialog {
                    import: i,
                    on_close: move |_| {
                        previewing.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some(i) = history() {
                RunsDialog { import: i, on_close: move |_| history.set(None) }
            }
            if let Some(i) = deleting() {
                ConfirmDialog {
                    title: t!("common-delete").to_string(),
                    message: t!("onto-imp-delete-confirm", name : i.name.clone()).to_string(),
                    confirm_label: t!("common-delete").to_string(),
                    on_confirm: move |_| {
                        let id = i.id.clone();
                        spawn(async move {
                            match api::ontology::delete_import(&id).await {
                                Ok(()) => reload.with_mut(|r| *r += 1),
                                Err(e) => toasts::error(e),
                            }
                            deleting.set(None);
                        });
                    },
                    on_cancel: move |_| deleting.set(None),
                }
            }
        }
    }
}

const MAPPING_TEMPLATE: &str = "key: my_import\nname: My import\nobjects:\n  - as: item\n    type: <type>\n    key: <unique property>\n    title: <column>\n    properties:\n      <unique property>: <column>\n";

#[component]
fn ImportForm(
    initial: Option<ObjectImportView>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let init = initial.clone();
    let mut name = use_signal(|| init.as_ref().map(|i| i.name.clone()).unwrap_or_default());
    let mut mapping = use_signal(|| {
        init.as_ref()
            .map(|i| i.mapping.clone())
            .unwrap_or_else(|| MAPPING_TEMPLATE.to_string())
    });
    let mut source = use_signal(|| init.as_ref().map(|i| i.source).unwrap_or_default());
    let mut url = use_signal(|| {
        init.as_ref()
            .and_then(|i| i.source_url.clone())
            .unwrap_or_default()
    });
    let mut mode = use_signal(|| init.as_ref().map(|i| i.mode).unwrap_or_default());
    let mut schedule = use_signal(|| {
        init.as_ref()
            .map(|i| schedule_of(&i.recurrence))
            .unwrap_or_default()
    });
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let title = if initial.is_some() {
        t!("onto-imp-edit")
    } else {
        t!("onto-imp-add")
    };
    let submit = move |_| {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let recurrence = (source() != ImportSource::File && !schedule().is_empty())
            .then(|| serde_json::json!({ "start": today, "every": 1, "unit": schedule() }));
        let input = ObjectImportInput {
            name: name(),
            mapping: mapping(),
            source: source(),
            source_url: Some(Some(url().trim().to_string()).filter(|u| !u.is_empty())),
            mode: mode(),
            recurrence: Some(recurrence),
            max_pages: Some(initial.as_ref().and_then(|i| i.max_pages)),
            expected_version: initial.as_ref().map(|i| i.version),
        };
        let id = initial.as_ref().map(|i| i.id.clone());
        busy.set(true);
        spawn(async move {
            let r = match &id {
                Some(id) => api::ontology::update_import(id, &input).await,
                None => api::ontology::create_import(&input).await,
            };
            match r {
                Ok(_) => on_saved.call(()),
                Err(e) => error.set(error_text(&e)),
            }
            busy.set(false);
        });
    };
    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("common-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: busy(),
            valid: !name().trim().is_empty(),
            max_width: "max-w-2xl".to_string(),
            label { class: FIELD_LABEL, {t!("onto-imp-name")} }
            input {
                class: INPUT,
                value: "{name}",
                oninput: move |e| name.set(e.value()),
            }
            div { class: "grid grid-cols-1 gap-3 sm:grid-cols-3",
                div {
                    label { class: FIELD_LABEL, {t!("onto-imp-source")} }
                    select {
                        class: INPUT,
                        onchange: move |e| {
                            source
                                .set(
                                    match e.value().as_str() {
                                        "url" => ImportSource::Url,
                                        "api" => ImportSource::Api,
                                        _ => ImportSource::File,
                                    },
                                );
                        },
                        for s in [ImportSource::File, ImportSource::Url, ImportSource::Api] {
                            option { value: s.wire(), selected: source() == s, {source_label(s)} }
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, {t!("onto-imp-mode")} }
                    select {
                        class: INPUT,
                        onchange: move |e| {
                            mode
                                .set(
                                if e.value() == "snapshot" {
                                    ImportMode::Snapshot
                                } else {
                                    ImportMode::Incremental
                                },
                            );
                        },
                        option {
                            value: "incremental",
                            selected: mode() == ImportMode::Incremental,
                            {t!("onto-imp-mode-incremental")}
                        }
                        option {
                            value: "snapshot",
                            selected: mode() == ImportMode::Snapshot,
                            {t!("onto-imp-mode-snapshot")}
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, {t!("onto-imp-schedule")} }
                    select {
                        class: INPUT,
                        disabled: source() == ImportSource::File,
                        onchange: move |e| schedule.set(e.value()),
                        for unit in SCHEDULES {
                            option { value: unit, selected: schedule() == unit, {schedule_label(unit)} }
                        }
                    }
                }
            }
            if source() != ImportSource::File {
                label { class: FIELD_LABEL, {t!("onto-imp-url")} }
                input {
                    class: INPUT,
                    r#type: "url",
                    value: "{url}",
                    oninput: move |e| url.set(e.value()),
                }
            }
            p { class: "text-xs text-gray-500", {t!("onto-imp-mode-help")} }
            label { class: FIELD_LABEL, {t!("onto-imp-mapping")} }
            textarea {
                class: "w-full h-64 px-3 py-2 border border-gray-300 rounded-lg text-xs font-mono",
                "data-testid": "onto-imp-mapping",
                value: "{mapping}",
                oninput: move |e| mapping.set(e.value()),
            }
            p { class: "text-xs text-gray-500", {t!("onto-imp-mapping-help")} }
            if !error().is_empty() {
                p { class: "text-sm text-red-600", "{error}" }
            }
        }
    }
}

/// A field error as `field: message` (the mapping path tells where), any
/// other error localized.
fn error_text(e: &crate::api::error::ApiError) -> String {
    match e.fields().and_then(|f| f.iter().next()) {
        Some((field, token)) => format!(
            "{field}: {}",
            localize_field(field, token.as_str().unwrap_or_default())
        ),
        None => crate::api::error_i18n::localize(e),
    }
}

/// Polls a run until it leaves `running`.
async fn settle(id: &str, run: &str) -> Result<ImportPreview, crate::api::error::ApiError> {
    loop {
        let p = api::ontology::import_run(id, run).await?;
        if p.run.state != "running" {
            return Ok(p);
        }
        crate::util::sleep(Duration::from_millis(800)).await;
    }
}

#[component]
fn PreviewDialog(import: ObjectImportView, on_close: EventHandler<()>) -> Element {
    let mut file = use_signal(|| None::<(String, Vec<u8>)>);
    let mut preview = use_signal(|| None::<ImportPreview>);
    let mut busy = use_signal(|| false);
    let needs_file = import.source == ImportSource::File;
    let iid = import.id.clone();
    let start = move |_| {
        let id = iid.clone();
        let (name, bytes) = file().unwrap_or_default();
        busy.set(true);
        preview.set(None);
        spawn(async move {
            let r = match api::ontology::preview_import(&id, &name, bytes).await {
                Ok(run) => settle(&id, &run.id).await,
                Err(e) => Err(e),
            };
            match r {
                Ok(p) => preview.set(Some(p)),
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    let aid = import.id.clone();
    let version = import.version;
    let apply = move |_| {
        let Some(p) = preview() else {
            return;
        };
        let id = aid.clone();
        let done = t!("onto-imp-applied").to_string();
        busy.set(true);
        spawn(async move {
            let input = ImportApply {
                run_id: p.run.id.clone(),
                expected_version: version,
            };
            let r = match api::ontology::apply_import(&id, &input).await {
                Ok(run) => settle(&id, &run.id).await,
                Err(e) => Err(e),
            };
            match r {
                Ok(p) if p.run.state == "applied" => {
                    toasts::success(done);
                    on_close.call(());
                }
                Ok(p) => preview.set(Some(p)),
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    let p = preview();
    let can_apply = p
        .as_ref()
        .is_some_and(|p| p.run.state == "preview" && p.run.counts.errors == 0);
    rsx! {
        Modal {
            title: t!("onto-imp-preview-title", name : import.name.clone()).to_string(),
            max_width: "max-w-4xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                if needs_file {
                    input {
                        class: INPUT,
                        r#type: "file",
                        accept: ".csv,.xlsx,.ods,.json,.xml,.zip",
                        aria_label: t!("onto-imp-file"),
                        onchange: move |evt| async move {
                            if let Some(f) = evt.files().first().cloned() {
                                if let Ok(bytes) = f.read_bytes().await {
                                    file.set(Some((f.name(), bytes.to_vec())));
                                }
                            }
                        },
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    button {
                        class: BTN_SECONDARY_SM,
                        "data-testid": "onto-imp-run-preview",
                        disabled: busy() || (needs_file && file().is_none()),
                        onclick: start,
                        {t!("onto-imp-preview")}
                    }
                    button {
                        class: PRIMARY_BTN,
                        "data-testid": "onto-imp-apply",
                        disabled: busy() || !can_apply,
                        onclick: apply,
                        {t!("onto-imp-apply")}
                    }
                }
                if busy() {
                    p { class: "text-sm text-gray-500", {t!("common-loading")} }
                }
                if let Some(p) = p {
                    DiffView { preview: p }
                }
            }
        }
    }
}

#[component]
fn DiffView(preview: ImportPreview) -> Element {
    let c = preview.run.counts.clone();
    let d = preview.diff;
    let state = state_label(&preview.run);
    rsx! {
        div { class: "space-y-3 text-sm", "data-testid": "onto-imp-diff",
            p { class: "text-gray-700",
                {
                    t!(
                        "onto-imp-counts", rows : preview.run.rows, created : c.created, updated : c
                        .updated, closed : c.closed, links : c.links_opened + c.links_updated, errors
                        : c.errors
                    )
                }
            }
            p { class: "text-xs text-gray-500", "{state}" }
            if !d.errors.is_empty() {
                IssueList {
                    title: t!("onto-imp-errors").to_string(),
                    issues: d.errors.clone(),
                    tone: "text-red-700",
                }
            }
            if !d.warnings.is_empty() {
                IssueList {
                    title: t!("onto-imp-warnings").to_string(),
                    issues: d.warnings.clone(),
                    tone: "text-amber-700",
                }
            }
            if !d.created.is_empty() {
                h4 { class: "font-medium text-gray-900", {t!("onto-imp-created")} }
                ul { class: "text-xs text-gray-700 space-y-0.5",
                    for (i, o) in d.created.iter().enumerate() {
                        li { key: "{i}", "{o.type_key} · {o.title} ({o.key})" }
                    }
                }
            }
            if !d.updated.is_empty() {
                h4 { class: "font-medium text-gray-900", {t!("onto-imp-updated")} }
                ul { class: "text-xs text-gray-700 space-y-0.5",
                    for (i, u) in d.updated.iter().enumerate() {
                        li { key: "{i}",
                            "{u.title} : "
                            for (k, ch) in u.changes.iter() {
                                span { key: "{k}", class: "mr-2", "{k} {ch.from} → {ch.to}" }
                            }
                            if let Some(t) = &u.new_title {
                                span { {t!("onto-imp-new-title", title : t.clone())} }
                            }
                        }
                    }
                }
            }
            if !d.closed.is_empty() {
                h4 { class: "font-medium text-gray-900", {t!("onto-imp-closed")} }
                ul { class: "text-xs text-gray-700 space-y-0.5",
                    for (i, x) in d.closed.iter().enumerate() {
                        li { key: "{i}", "{x.type_key} · {x.label}" }
                    }
                }
            }
            if !d.links_opened.is_empty() {
                h4 { class: "font-medium text-gray-900", {t!("onto-imp-links")} }
                ul { class: "text-xs text-gray-700 space-y-0.5",
                    for (i, l) in d.links_opened.iter().enumerate() {
                        li { key: "{i}", "{l.from.label} — {l.link_type} → {l.to.label}" }
                    }
                }
            }
        }
    }
}

#[component]
fn IssueList(title: String, issues: Vec<RowIssue>, tone: String) -> Element {
    rsx! {
        h4 { class: "font-medium {tone}", "{title}" }
        ul { class: "text-xs {tone} space-y-0.5",
            for (i, e) in issues.iter().enumerate() {
                li { key: "{i}",
                    {
                        t!(
                            "onto-imp-issue", row : e.row, column : e.column.clone(), message :
                            localize_field(& e.column, & e.token)
                        )
                    }
                }
            }
        }
    }
}

#[component]
fn RunsDialog(import: ObjectImportView, on_close: EventHandler<()>) -> Element {
    let open = use_signal(|| None::<String>);
    let id = import.id.clone();
    let runs = use_resource(move || {
        let id = id.clone();
        async move { api::ontology::import_runs(&id).await.unwrap_or_default() }
    });
    let rows = runs.read().clone().unwrap_or_default();
    rsx! {
        Modal {
            title: t!("onto-imp-runs-title", name : import.name.clone()).to_string(),
            max_width: "max-w-2xl".to_string(),
            on_close: move |_| on_close.call(()),
            if rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("onto-imp-runs-empty")} }
            }
            ul { class: "divide-y divide-gray-100 text-sm",
                for r in rows {
                    li { key: "{r.id}", class: "py-2",
                        div { class: "flex flex-wrap items-center gap-2",
                            span { class: "font-medium", "#{r.number}" }
                            span { {state_label(&r)} }
                            span { class: "text-xs text-gray-500", {short_time(&r.started_at)} }
                            StepsToggle { run: r.id.clone(), open }
                        }
                        p { class: "text-xs text-gray-600",
                            {
                                t!(
                                    "onto-imp-counts", rows : r.rows, created : r.counts.created, updated : r
                                    .counts.updated, closed : r.counts.closed, links : r.counts.links_opened + r
                                    .counts.links_updated, errors : r.counts.errors
                                )
                            }
                        }
                        if open().as_deref() == Some(r.id.as_str()) {
                            RunSteps { import: import.id.clone(), run: r.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StepsToggle(run: String, open: Signal<Option<String>>) -> Element {
    let shown = open().as_deref() == Some(run.as_str());
    rsx! {
        button {
            class: "text-xs text-signal-600 hover:underline",
            "data-testid": "import-run-steps",
            onclick: move |_| open.set((!shown).then(|| run.clone())),
            {t!("onto-imp-steps")}
        }
    }
}

/// The steps of a run (an import with `steps:`): outcome, duration, rows.
#[component]
fn RunSteps(import: String, run: ImportRunView) -> Element {
    let run_id = run.id.clone();
    let detail = use_resource(move || {
        let (import, run_id) = (import.clone(), run_id.clone());
        async move { api::ontology::import_run(&import, &run_id).await.ok() }
    });
    let steps = detail
        .read()
        .clone()
        .flatten()
        .map(|p| p.steps)
        .unwrap_or_default();
    let at = |t: &str| chrono::DateTime::parse_from_rfc3339(t).ok();
    // A step's duration runs from the end of the one before (the run's start).
    let mut before = at(&run.started_at);
    let lines: Vec<(pnex_core::ingest::IngestStepView, String)> = steps
        .into_iter()
        .map(|s| {
            let end = at(&s.created_at);
            let took = match (before, end) {
                (Some(a), Some(b)) => {
                    format!("{:.1} s", (b - a).num_milliseconds() as f64 / 1000.0)
                }
                _ => String::new(),
            };
            before = end;
            (s, took)
        })
        .collect();
    rsx! {
        div {
            class: "mt-2 ml-4 space-y-1",
            "data-testid": "import-run-step-list",
            if lines.is_empty() {
                p { class: "text-xs text-gray-500", {t!("onto-imp-steps-empty")} }
            }
            for (s, took) in lines {
                StepLine { key: "{s.id}", step: s, took }
            }
        }
    }
}

fn step_name(step: &str) -> String {
    match step {
        "source_resolve" => t!("flows-palette-ingest-resolve"),
        "file_download" => t!("flows-palette-ingest-download"),
        "archive_extract" => t!("flows-palette-ingest-extract"),
        "table_inspect" => t!("flows-palette-ingest-inspect"),
        "rows_filter" => t!("flows-palette-ingest-filter"),
        "ontology_import" => t!("flows-palette-ingest-import"),
        _ => t!("flows-palette-ingest-cleanup"),
    }
    .to_string()
}

#[component]
fn StepLine(step: pnex_core::ingest::IngestStepView, took: String) -> Element {
    let (badge, label) = match step.port {
        0 => ("bg-green-50 text-green-700", t!("flows-ingest-port-ok")),
        2 => (
            "bg-gray-100 text-gray-600",
            t!("flows-ingest-port-unchanged"),
        ),
        _ => ("bg-red-50 text-red-700", t!("flows-ingest-port-error")),
    };
    let m = &step.manifest;
    let num = |k: &str| m.get(k).and_then(serde_json::Value::as_u64);
    let rows = match (num("rows_in"), num("rows_out"), num("rows")) {
        (Some(i), Some(o), _) => {
            t!("onto-imp-step-rows-in-out", rows_in : i, rows_out : o).to_string()
        }
        (_, _, Some(n)) => t!("onto-imp-step-rows", rows : n).to_string(),
        _ => String::new(),
    };
    let error = step.code.as_deref().map(|c| run_message(c, m.get("args")));
    rsx! {
        div { class: "text-xs", "data-step": "{step.step}",
            div { class: "flex flex-wrap items-center gap-2",
                span { class: "font-medium text-gray-800", {step_name(&step.step)} }
                span { class: "px-1.5 py-0.5 rounded {badge}", "{label}" }
                span { class: "text-gray-500", "{took}" }
                span { class: "text-gray-600", "{rows}" }
            }
            if let Some(e) = error {
                p { class: "text-red-600", "{e}" }
            }
        }
    }
}
