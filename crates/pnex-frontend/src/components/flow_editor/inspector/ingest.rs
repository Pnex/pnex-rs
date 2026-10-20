use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::ingest::{FilterOp, ImportAction, IngestConfig, IngestStep, ResolveMode};

// ─────────────── ingestion nodes (ingestion.md D317–D322) ───────────────

const INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";
const AREA: &str = "w-full h-20 px-2 py-1 border border-gray-300 rounded-lg text-xs font-mono";

/// Edits the selected node's config in place.
fn edit(mut cx: EditorCx, f: impl FnOnce(&mut IngestConfig) + 'static) {
    patch_selected(&mut cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::Ingest { config } = &mut node.kind {
            f(config);
        }
    });
}

fn lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[component]
fn LineField(
    label: String,
    value: String,
    testid: String,
    disabled: bool,
    on_commit: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: FIELD_LABEL, "{label}" }
        input {
            class: INPUT,
            "data-testid": "{testid}",
            disabled,
            value: "{value}",
            onchange: move |e| on_commit.call(e.value().trim().to_string()),
        }
    }
}

/// One value per line.
#[component]
fn ListField(
    label: String,
    value: Vec<String>,
    testid: String,
    disabled: bool,
    on_commit: EventHandler<Vec<String>>,
) -> Element {
    let text = value.join("\n");
    rsx! {
        label { class: FIELD_LABEL, "{label}" }
        textarea {
            class: AREA,
            "data-testid": "{testid}",
            disabled,
            value: "{text}",
            onchange: move |e| on_commit.call(lines(&e.value())),
        }
    }
}

#[component]
pub(super) fn IngestForm(
    cx: EditorCx,
    initial: IngestConfig,
    can_write: bool,
    flow_id: i64,
    node_id: String,
) -> Element {
    let off = !can_write;
    let fields = match initial.step {
        IngestStep::SourceResolve => rsx! {
            ResolveFields { cx, initial: initial.clone(), off }
        },
        IngestStep::FileDownload => rsx! {
            DownloadFields { cx, initial: initial.clone(), off }
        },
        IngestStep::ArchiveExtract => rsx! {
            ListField {
                label: t!("flows-ingest-entries").to_string(),
                value: initial.entries.clone(),
                testid: "ingest-entries",
                disabled: off,
                on_commit: move |v| edit(cx, move |c| c.entries = v),
            }
            p { class: "text-xs text-gray-500", {t!("flows-ingest-entries-help")} }
        },
        IngestStep::TableInspect => rsx! {
            EntryField { cx, value: initial.entry.clone(), off }
            ListField {
                label: t!("flows-ingest-expected-columns").to_string(),
                value: initial.expected_columns.clone(),
                testid: "ingest-expected-columns",
                disabled: off,
                on_commit: move |v| edit(cx, move |c| c.expected_columns = v),
            }
        },
        IngestStep::RowsFilter => rsx! {
            FilterFields { cx, initial: initial.clone(), off }
        },
        IngestStep::OntologyImport => rsx! {
            ImportFields { cx, initial: initial.clone(), off }
        },
        IngestStep::StagingCleanup => rsx! {},
    };
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {step_help(initial.step)} }
            {fields}
            IngestRuns { key: "{node_id}", flow_id, node_id: node_id.clone() }
        }
    }
}

fn step_help(step: IngestStep) -> String {
    match step {
        IngestStep::SourceResolve => t!("flows-palette-ingest-resolve-help"),
        IngestStep::FileDownload => t!("flows-palette-ingest-download-help"),
        IngestStep::ArchiveExtract => t!("flows-palette-ingest-extract-help"),
        IngestStep::TableInspect => t!("flows-palette-ingest-inspect-help"),
        IngestStep::RowsFilter => t!("flows-palette-ingest-filter-help"),
        IngestStep::OntologyImport => t!("flows-palette-ingest-import-help"),
        IngestStep::StagingCleanup => t!("flows-palette-ingest-cleanup-help"),
    }
    .to_string()
}

#[component]
fn EntryField(cx: EditorCx, value: String, off: bool) -> Element {
    rsx! {
        LineField {
            label: t!("flows-ingest-entry").to_string(),
            value,
            testid: "ingest-entry",
            disabled: off,
            on_commit: move |v| edit(cx, move |c| c.entry = v),
        }
        p { class: "text-xs text-gray-500", {t!("flows-ingest-entry-help")} }
    }
}

#[component]
fn ResolveFields(cx: EditorCx, initial: IngestConfig, off: bool) -> Element {
    let datagouv = initial.mode == ResolveMode::Datagouv;
    rsx! {
        label { class: FIELD_LABEL, {t!("flows-ingest-mode")} }
        select {
            class: INPUT,
            "data-testid": "ingest-mode",
            disabled: off,
            onchange: move |e| {
                let mode = if e.value() == "datagouv" {
                    ResolveMode::Datagouv
                } else {
                    ResolveMode::Url
                };
                edit(cx, move |c| c.mode = mode);
            },
            option { value: "url", selected: !datagouv, {t!("flows-ingest-mode-url")} }
            option { value: "datagouv", selected: datagouv, {t!("flows-ingest-mode-datagouv")} }
        }
        if datagouv {
            LineField {
                label: t!("flows-ingest-dataset").to_string(),
                value: initial.dataset.clone(),
                testid: "ingest-dataset",
                disabled: off,
                on_commit: move |v| edit(cx, move |c| c.dataset = v),
            }
            LineField {
                label: t!("flows-ingest-resource-match").to_string(),
                value: initial.resource_match.clone(),
                testid: "ingest-resource-match",
                disabled: off,
                on_commit: move |v| edit(cx, move |c| c.resource_match = v),
            }
        } else {
            LineField {
                label: t!("flows-ingest-url").to_string(),
                value: initial.url.clone(),
                testid: "ingest-url",
                disabled: off,
                on_commit: move |v| edit(cx, move |c| c.url = v),
            }
        }
    }
}

#[component]
fn DownloadFields(cx: EditorCx, initial: IngestConfig, off: bool) -> Element {
    let mb = initial.max_bytes() / (1024 * 1024);
    let days = initial.keep_raw_days();
    let skip = initial.skip_if_unchanged.unwrap_or(true);
    rsx! {
        div { class: "grid grid-cols-2 gap-2",
            div {
                LineField {
                    label: t!("flows-ingest-max-mb").to_string(),
                    value: mb.to_string(),
                    testid: "ingest-max-mb",
                    disabled: off,
                    on_commit: move |v: String| {
                        let n = parse_u32(&v).map(|n| n as u64 * 1024 * 1024);
                        edit(cx, move |c| c.max_bytes = n);
                    },
                }
            }
            div {
                LineField {
                    label: t!("flows-ingest-keep-raw-days").to_string(),
                    value: days.to_string(),
                    testid: "ingest-keep-raw-days",
                    disabled: off,
                    on_commit: move |v: String| edit(cx, move |c| c.keep_raw_days = parse_u32(&v)),
                }
            }
        }
        label { class: "flex items-center gap-2 text-sm",
            input {
                r#type: "checkbox",
                disabled: off,
                checked: skip,
                onchange: move |e| {
                    let on = e.checked();
                    edit(cx, move |c| c.skip_if_unchanged = Some(on));
                },
            }
            {t!("flows-ingest-skip-unchanged")}
        }
    }
}

#[component]
fn FilterFields(cx: EditorCx, initial: IngestConfig, off: bool) -> Element {
    let op = initial.op;
    let convert = serde_json::to_value(&initial.convert)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join(", ");
    rsx! {
        EntryField { cx, value: initial.entry.clone(), off }
        LineField {
            label: t!("flows-ingest-column").to_string(),
            value: initial.column.clone(),
            testid: "ingest-column",
            disabled: off,
            on_commit: move |v| edit(cx, move |c| c.column = v),
        }
        label { class: FIELD_LABEL, {t!("flows-ingest-op")} }
        select {
            class: INPUT,
            "data-testid": "ingest-op",
            disabled: off,
            onchange: move |e| {
                let op = match e.value().as_str() {
                    "in" => FilterOp::In,
                    "in_objects" => FilterOp::InObjects,
                    _ => FilterOp::Equals,
                };
                edit(cx, move |c| c.op = op);
            },
            option { value: "equals", selected: op == FilterOp::Equals, {t!("flows-ingest-op-equals")} }
            option { value: "in", selected: op == FilterOp::In, {t!("flows-ingest-op-in")} }
            option { value: "in_objects", selected: op == FilterOp::InObjects,
                {t!("flows-ingest-op-in-objects")}
            }
        }
        if op == FilterOp::InObjects {
            div { class: "grid grid-cols-2 gap-2",
                div {
                    LineField {
                        label: t!("flows-ingest-object-type").to_string(),
                        value: initial.object_type.clone(),
                        testid: "ingest-object-type",
                        disabled: off,
                        on_commit: move |v| edit(cx, move |c| c.object_type = v),
                    }
                }
                div {
                    LineField {
                        label: t!("flows-ingest-property").to_string(),
                        value: initial.property.clone(),
                        testid: "ingest-property",
                        disabled: off,
                        on_commit: move |v| edit(cx, move |c| c.property = v),
                    }
                }
            }
        } else {
            ListField {
                label: t!("flows-ingest-values").to_string(),
                value: initial.values.clone(),
                testid: "ingest-values",
                disabled: off,
                on_commit: move |v| edit(cx, move |c| c.values = v),
            }
        }
        ConvertField { cx, value: convert, off }
    }
}

/// Conversions written `digits, trim, left:9` (D301 names).
#[component]
fn ConvertField(cx: EditorCx, value: String, off: bool) -> Element {
    let mut bad = use_signal(|| false);
    rsx! {
        label { class: FIELD_LABEL, {t!("flows-ingest-convert")} }
        input {
            class: INPUT,
            "data-testid": "ingest-convert",
            disabled: off,
            value: "{value}",
            onchange: move |e| {
                let names: Vec<serde_json::Value> = e
                    .value()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect();
                match serde_json::from_value(serde_json::Value::Array(names)) {
                    Ok(list) => {
                        bad.set(false);
                        edit(cx, move |c| c.convert = list);
                    }
                    Err(_) => bad.set(true),
                }
            },
        }
        if bad() {
            p { class: "text-xs text-red-600", {t!("flows-ingest-convert-invalid")} }
        }
    }
}

#[component]
fn ImportFields(cx: EditorCx, initial: IngestConfig, off: bool) -> Element {
    let imports = use_resource(|| async { api::ontology::imports().await.unwrap_or_default() });
    let list = imports.read().clone().unwrap_or_default();
    let apply = initial.action == ImportAction::Apply;
    let pct = initial.max_closures_pct();
    rsx! {
        label { class: FIELD_LABEL, {t!("flows-ingest-import")} }
        select {
            class: INPUT,
            "data-testid": "ingest-import",
            disabled: off,
            onchange: move |e| {
                let v = e.value();
                edit(cx, move |c| c.import_id = v);
            },
            option { value: "", selected: initial.import_id.is_empty(), "—" }
            for imp in list {
                option {
                    key: "{imp.id}",
                    value: "{imp.id}",
                    selected: imp.id == initial.import_id,
                    "{imp.name}"
                }
            }
        }
        label { class: FIELD_LABEL, {t!("flows-ingest-action")} }
        select {
            class: INPUT,
            "data-testid": "ingest-action",
            disabled: off,
            onchange: move |e| {
                let a = if e.value() == "apply" {
                    ImportAction::Apply
                } else {
                    ImportAction::Preview
                };
                edit(cx, move |c| c.action = a);
            },
            option { value: "preview", selected: !apply, {t!("flows-ingest-action-preview")} }
            option { value: "apply", selected: apply, {t!("flows-ingest-action-apply")} }
        }
        LineField {
            label: t!("flows-ingest-max-closures").to_string(),
            value: pct.to_string(),
            testid: "ingest-max-closures",
            disabled: off,
            on_commit: move |v: String| edit(cx, move |c| c.max_closures_pct = parse_u32(&v)),
        }
    }
}

/// Last runs of the node: the newest manifest in full, the others as a
/// line (time, step outcome).
#[component]
fn IngestRuns(flow_id: i64, node_id: String) -> Element {
    let mut tick = use_signal(|| 0u32);
    let nid = node_id.clone();
    let runs = use_resource(move || {
        let nid = nid.clone();
        let _ = tick();
        async move { api::flows::ingest_steps(flow_id, &nid, 5).await.ok() }
    });
    let steps = runs
        .read()
        .clone()
        .flatten()
        .map(|p| p.results)
        .unwrap_or_default();
    rsx! {
        div {
            class: "border-t border-gray-200 pt-3 space-y-2",
            "data-testid": "ingest-runs",
            div { class: "flex items-center justify-between",
                span { class: FIELD_LABEL, {t!("flows-ingest-last-runs")} }
                button {
                    class: "text-xs text-signal-600 hover:underline",
                    onclick: move |_| tick += 1,
                    {t!("flows-ingest-refresh")}
                }
            }
            if steps.is_empty() {
                p { class: "text-xs text-gray-500", {t!("flows-ingest-no-run")} }
            }
            for (i, s) in steps.into_iter().enumerate() {
                RunLine { key: "{s.id}", step: s, open: i == 0 }
            }
        }
    }
}

#[component]
fn RunLine(step: pnex_core::ingest::IngestStepView, open: bool) -> Element {
    let (badge, label) = match step.port {
        0 => ("bg-green-50 text-green-700", t!("flows-ingest-port-ok")),
        2 => (
            "bg-gray-100 text-gray-600",
            t!("flows-ingest-port-unchanged"),
        ),
        _ => ("bg-red-50 text-red-700", t!("flows-ingest-port-error")),
    };
    let error = step
        .code
        .as_deref()
        .map(|c| crate::api::error_i18n::localize_code(c, step.manifest.get("args"), c));
    let when = step
        .created_at
        .get(..19)
        .unwrap_or(&step.created_at)
        .replace('T', " ");
    let pretty = serde_json::to_string_pretty(&step.manifest).unwrap_or_default();
    rsx! {
        div { class: "text-xs space-y-1",
            div { class: "flex items-center gap-2",
                span { class: "px-1.5 py-0.5 rounded {badge}", "{label}" }
                span { class: "text-gray-500", "{when}" }
            }
            if let Some(e) = error {
                p { class: "text-red-600", "{e}" }
            }
            if open {
                pre { class: "max-h-64 overflow-auto bg-gray-50 rounded p-2 font-mono text-xs",
                    "{pretty}"
                }
            }
        }
    }
}
