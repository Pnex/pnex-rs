//! Page of a table: its first rows, its versions with their checks, its lineage, how
//! fresh it must stay, and a CSV to load.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::datasets::{
    Dataset, DatasetTransaction, DatasetTransactionState, FRESHNESS_HOURS_MAX,
};
use pnex_core::pipelines::DatasetLineage;

use super::graph::{GraphNode, TableGraph};
use super::{code_label, tone, BADGE};
use crate::api;
use crate::api::error_i18n::localize;
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::ontology::short_time;
use crate::components::ui::{
    BTN_DANGER, BTN_SECONDARY_SM, FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT, PANEL,
};
use crate::state::access::can;
use crate::state::toasts;

/// First rows of a table, with the type of each column.
#[component]
pub(super) fn SamplePanel(
    dataset_id: String,
    columns: Vec<pnex_core::datasets::DatasetColumn>,
    #[props(default)] as_of: Option<String>,
    /// Changes when the table may have a new version.
    #[props(default)]
    generation: u32,
) -> Element {
    let profile_id = dataset_id.clone();
    let reading_past = as_of.is_some();
    let sample = use_resource(use_reactive!(|dataset_id, as_of, generation| async move {
        let _ = generation;
        api::pipelines::sample(&dataset_id, as_of.as_deref()).await
    }));
    // The footers describe the current view only.
    let profile = use_resource(use_reactive!(
        |profile_id, reading_past, generation| async move {
            let _ = generation;
            if reading_past {
                return None;
            }
            api::pipelines::profile(&profile_id).await.ok()
        }
    ));
    let filled = move |name: &str| {
        let p = profile.read();
        let pct = p.as_ref()?.as_ref()?.filled_pct(name)?;
        Some(t!("pipe-column-filled", pct: pct).to_string())
    };
    let type_of = move |name: &str| {
        columns
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.data_type.clone())
            .unwrap_or_default()
    };
    let shown = match &*sample.read() {
        None => rsx! {},
        Some(Err(e)) => rsx! {
            p { class: "p-3 text-sm text-alarm", "{localize(e)}" }
        },
        Some(Ok(s)) if s.columns.is_empty() => rsx! {
            p { class: "p-3 text-sm text-slate", {t!("pipe-table-no-rows")} }
        },
        Some(Ok(s)) => rsx! {
            div { class: "overflow-x-auto", "data-testid": "table-sample",
                table { class: "min-w-full text-xs",
                    thead {
                        tr {
                            for c in s.columns.iter() {
                                th {
                                    key: "{c}",
                                    class: "border-b border-line px-2 py-1.5 text-left font-medium text-ink",
                                    div { "{c}" }
                                    div { class: "font-normal text-slate", "{type_of(c)}" }
                                    if let Some(text) = filled(c) {
                                        div { class: "font-normal text-slate", "{text}" }
                                    }
                                }
                            }
                        }
                    }
                    tbody {
                        for (i, row) in s.rows.iter().enumerate() {
                            tr { key: "{i}",
                                for (j, v) in row.iter().enumerate() {
                                    td {
                                        key: "{j}",
                                        class: "max-w-xs truncate border-b border-line px-2 py-1 text-ink",
                                        {v.clone().unwrap_or_default()}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    };
    shown
}

fn lineage_graph(table: &Dataset, l: &DatasetLineage) -> (Vec<GraphNode>, Vec<(String, String)>) {
    let node = |id: &str, detail: String| GraphNode {
        id: id.to_string(),
        detail,
        warn: false,
    };
    let mut nodes = vec![GraphNode {
        warn: l.stale,
        ..node(
            &table.name,
            if l.stale {
                t!("pipe-table-behind").to_string()
            } else {
                t!("pipe-rows", n : table.rows).to_string()
            },
        )
    }];
    let mut edges = Vec::new();
    for u in &l.upstream {
        if !nodes.iter().any(|n| n.id == u.dataset_name) {
            nodes.push(node(&u.dataset_name, u.pipeline_name.clone()));
        }
        edges.push((u.dataset_name.clone(), table.name.clone()));
    }
    for d in &l.downstream {
        if !nodes.iter().any(|n| n.id == d.dataset_name) {
            nodes.push(node(&d.dataset_name, d.pipeline_name.clone()));
        }
        edges.push((table.name.clone(), d.dataset_name.clone()));
    }
    (nodes, edges)
}

#[component]
pub fn PipelineTable(id: String) -> Element {
    let id = use_signal(|| id);
    let mut reload = use_signal(|| 0u32);
    let mut as_of = use_signal(|| None::<String>);
    let mut confirm_delete = use_signal(|| false);
    let mut message = use_signal(|| None::<String>);
    let mut delimiter = use_signal(|| ",".to_string());
    let mut uploading = use_signal(|| false);
    let mut progress = use_signal(|| None::<u8>);
    let mut hours = use_signal(String::new);
    let navigator = use_navigator();
    let table = use_resource(move || async move {
        let _ = reload();
        api::pipelines::table(&id()).await
    });
    let versions = use_resource(move || async move {
        let _ = reload();
        api::pipelines::versions_of_table(&id(), 20).await
    });
    let lineage = use_resource(move || async move {
        let _ = reload();
        api::pipelines::lineage(&id()).await
    });
    // The stored value fills the field once, then belongs to the person typing.
    use_effect(move || {
        if let Some(Ok(t)) = &*table.read() {
            if hours.peek().is_empty() {
                hours.set(t.max_age_hours.map(|h| h.to_string()).unwrap_or_default());
            }
        }
    });
    let current = match &*table.read() {
        Some(Ok(t)) => t.clone(),
        Some(Err(e)) => {
            let text = localize(e);
            return rsx! {
                p { class: "p-6 text-sm text-alarm", "{text}" }
            };
        }
        None => return rsx! {},
    };
    let can_write = can("datasets:write");
    let tables = use_resource(api::pipelines::tables);
    let open_by_name = move |name: String| {
        let found = match &*tables.read() {
            Some(Ok(list)) => list.iter().find(|t| t.name == name).map(|t| t.id.clone()),
            _ => None,
        };
        if let Some(id) = found {
            navigator.push(Route::PipelineTable { id });
        }
    };
    let save_freshness = move |_| {
        let text = hours().trim().to_string();
        let value = if text.is_empty() {
            None
        } else {
            match text.parse::<i32>() {
                Ok(h) if (1..=FRESHNESS_HOURS_MAX).contains(&h) => Some(h),
                _ => {
                    message.set(Some(t!("err-invalid").to_string()));
                    return;
                }
            }
        };
        message.set(None);
        spawn(async move {
            match api::pipelines::set_freshness(&id(), value).await {
                Ok(_) => {
                    toasts::success(t!("pipe-saved").to_string());
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => message.set(Some(localize(&e))),
            }
        });
    };
    let linked = match &*lineage.read() {
        Some(Ok(l)) if !l.upstream.is_empty() || !l.downstream.is_empty() => Some(l.clone()),
        _ => None,
    };
    let version_list = match &*versions.read() {
        Some(Ok(page)) => rsx! {
            ul { class: "divide-y divide-line",
                for v in page.results.clone() {
                    VersionLine {
                        key: "{v.id}",
                        version: v,
                        on_read: move |at: String| as_of.set(Some(at)),
                    }
                }
            }
        },
        Some(Err(e)) => rsx! {
            p { class: "p-3 text-sm text-alarm", "{localize(e)}" }
        },
        None => rsx! {},
    };
    let upload = move |evt: FormEvent| async move {
        if let Some(f) = evt.files().first().cloned() {
            uploading.set(true);
            message.set(None);
            let size = f.size().max(1);
            progress.set(Some(0));
            let on_sent = move |sent: u64| progress.set(Some((sent * 100 / size).min(100) as u8));
            let done = api::pipelines::upload_csv(&id(), &delimiter(), f.byte_stream(), on_sent)
                .await
                .map_err(|e| localize(&e))
                .and_then(|u| match u.code {
                    Some(code) => Err(code_label(&code, u.args.as_ref())),
                    None => Ok(()),
                });
            progress.set(None);
            uploading.set(false);
            match done {
                Ok(_) => {
                    as_of.set(None);
                    reload.with_mut(|r| *r += 1);
                }
                Err(text) => message.set(Some(text)),
            }
        }
    };
    let summary = t!(
        "pipe-table-summary",
        rows: current.rows,
        size: crate::pages::system::format_bytes(current.bytes as f64)
    )
    .to_string();
    let sample_title = match as_of() {
        Some(at) => t!("pipe-table-sample-at", at: short_time(&at)).to_string(),
        None => t!("pipe-table-sample").to_string(),
    };
    rsx! {
        ListLayout {
            title: current.name.clone(),
            subtitle: Some(summary),
            on_back: move |_| {
                navigator.push(Route::Pipelines {});
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: false,
            div { class: "space-y-4",
                if current.stale {
                    p { class: "text-sm text-alarm", {t!("pipe-table-stale-help")} }
                }
                if let Some(m) = message() {
                    p { class: FIELD_ERROR, "{m}" }
                }
                section { class: PANEL,
                    h2 { class: "border-b border-line px-3 py-2 text-sm font-semibold text-ink",
                        "{sample_title}"
                    }
                    SamplePanel {
                        dataset_id: id(),
                        columns: current.columns.clone(),
                        as_of: as_of(),
                        generation: reload(),
                    }
                }
                if let Some(l) = linked {
                    LineagePanel {
                        table: current.clone(),
                        lineage: l,
                        on_select: open_by_name,
                    }
                }
                section { class: PANEL,
                    h2 { class: "border-b border-line px-3 py-2 text-sm font-semibold text-ink",
                        {t!("pipe-table-versions")}
                    }
                    {version_list}
                    if as_of().is_some() {
                        div { class: "p-3",
                            button {
                                class: BTN_SECONDARY_SM,
                                onclick: move |_| as_of.set(None),
                                {t!("pipe-table-back-current")}
                            }
                        }
                    }
                }
                if let Some(import) = current.import_name.clone() {
                    p { class: FIELD_HELP, {t!("pipe-table-filled-by", name : import)} }
                } else if can_write {
                    section { class: "{PANEL} space-y-3 p-3",
                        h2 { class: "text-sm font-semibold text-ink", {t!("pipe-table-load")} }
                        p { class: FIELD_HELP, {t!("pipe-table-load-help")} }
                        div {
                            label { class: FIELD_LABEL, r#for: "table-delimiter",
                                {t!("pipe-table-delimiter")}
                            }
                            select {
                                id: "table-delimiter",
                                class: INPUT,
                                value: "{delimiter}",
                                onchange: move |e| delimiter.set(e.value()),
                                option { value: ",", "," }
                                option { value: ";", ";" }
                                option { value: "\t", {t!("pipe-table-delimiter-tab")} }
                                option { value: "|", "|" }
                            }
                        }
                        input {
                            class: INPUT,
                            r#type: "file",
                            accept: ".csv,.tsv,.txt",
                            disabled: uploading(),
                            aria_label: t!("pipe-table-load"),
                            "data-testid": "table-upload",
                            onchange: upload,
                        }
                        if let Some(percent) = progress() {
                            p {
                                class: FIELD_HELP,
                                role: "status",
                                "data-testid": "table-upload-progress",
                                if percent < 100 {
                                    {t!("pipe-table-sending", percent : percent)}
                                } else {
                                    {t!("pipe-table-converting")}
                                }
                            }
                        }
                    }
                }
                if can_write {
                    section { class: "{PANEL} space-y-3 p-3",
                        h2 { class: "text-sm font-semibold text-ink", {t!("pipe-table-freshness")} }
                        p { class: FIELD_HELP, {t!("pipe-table-freshness-help")} }
                        div { class: "flex items-center gap-2",
                            input {
                                class: INPUT,
                                inputmode: "numeric",
                                aria_label: t!("pipe-table-freshness"),
                                value: "{hours}",
                                oninput: move |e| hours.set(e.value()),
                            }
                            button {
                                class: BTN_SECONDARY_SM,
                                onclick: save_freshness,
                                {t!("pipe-save")}
                            }
                        }
                    }
                }
                if can("datasets:delete") {
                    div {
                        button {
                            class: BTN_DANGER,
                            onclick: move |_| confirm_delete.set(true),
                            {t!("pipe-table-delete")}
                        }
                    }
                }
            }
        }
        if confirm_delete() {
            ConfirmDialog {
                title: t!("pipe-table-delete").to_string(),
                message: t!("pipe-table-delete-message", name : current.name.clone()).to_string(),
                confirm_label: t!("pipe-delete").to_string(),
                on_cancel: move |_| confirm_delete.set(false),
                on_confirm: move |_| {
                    confirm_delete.set(false);
                    spawn(async move {
                        match api::pipelines::delete_table(&id()).await {
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

fn check_line(c: &pnex_core::pipelines::CheckResult) -> String {
    t!(
        "pipe-check-line",
        check: c.check.clone(),
        column: c.column.clone().unwrap_or_default(),
        value: c.value.clone(),
        limit: c.limit.clone()
    )
    .to_string()
}

#[component]
fn LineagePanel(
    table: Dataset,
    lineage: DatasetLineage,
    on_select: EventHandler<String>,
) -> Element {
    let (nodes, edges) = lineage_graph(&table, &lineage);
    rsx! {
        section { class: PANEL,
            h2 { class: "border-b border-line px-3 py-2 text-sm font-semibold text-ink",
                {t!("pipe-table-lineage")}
            }
            div { class: "overflow-x-auto",
                TableGraph {
                    nodes,
                    edges,
                    selected: Some(table.name.clone()),
                    on_select,
                }
            }
        }
    }
}

#[component]
fn VersionLine(version: DatasetTransaction, on_read: EventHandler<String>) -> Element {
    let v = version;
    let (label, good, bad) = match v.state {
        DatasetTransactionState::Committed => (t!("pipe-version-committed"), true, false),
        DatasetTransactionState::Open => (t!("pipe-version-open"), false, false),
        DatasetTransactionState::Aborted => (t!("pipe-version-aborted"), false, true),
    };
    let at = v.committed_at.clone();
    rsx! {
        li { class: "space-y-1 px-3 py-2 text-sm",
            div { class: "flex flex-wrap items-center gap-2",
                span { class: "{BADGE} {tone(good, bad)}", "{label}" }
                span { class: "text-ink", {t!("pipe-rows", n : v.rows)} }
                span { class: "text-xs text-slate",
                    "{short_time(at.as_deref().unwrap_or(&v.created_at))}"
                }
                if let Some(at) = at {
                    button {
                        class: "{BTN_SECONDARY_SM} ml-auto",
                        onclick: move |_| on_read.call(at.clone()),
                        {t!("pipe-version-read")}
                    }
                }
            }
            if let Some(code) = v.code.clone() {
                p { class: "text-xs text-alarm", "{code_label(&code, None)}" }
            }
            for (i, c) in v.checks.iter().enumerate() {
                p {
                    key: "{i}",
                    class: if c.passed { "text-xs text-slate" } else { "text-xs text-alarm" },
                    "{check_line(c)}"
                }
            }
        }
    }
}
