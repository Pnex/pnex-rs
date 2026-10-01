use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::firmware::{
    FirmwareCheckStatus, FirmwareProjectDetail, SaveFirmwareRevision, SourceViolation,
};

use super::chip_label;
use super::panels::{ApiMenu, LibChips, LibMenu, RevisionsDrawer};
use crate::api;
use crate::components::code_editing::CodeLang;
use crate::components::code_highlight::{AreaHandle, FunctionCodeEditor, LineMark, MarkSeverity};
use crate::components::icons;
use crate::state::toasts;
use crate::util;

/// Poll period of a running check.
const CHECK_POLL_MS: u64 = 2000;

/// Editor of one project: name, `main.cpp`, catalog libraries, revisions,
/// compile-only check. Remounted by `key`. The server truth replaces the
/// local fields from the detail the API returns (initial GET, PATCH
/// response, re-GET after a 409) — never from a refetching resource, whose
/// stale value would be applied as the new state.
#[component]
pub(super) fn FirmwareEditor(project_id: i64, can_write: bool, on_back: Callback<()>) -> Element {
    let detail = use_resource(move || async move { api::firmware::detail(project_id).await });

    let mut name = use_signal(String::new);
    let mut code = use_signal(String::new);
    let mut libs = use_signal(Vec::<String>::new);
    let mut loaded = use_signal(|| None::<FirmwareProjectDetail>);
    let mut apply = move |d: FirmwareProjectDetail| {
        name.set(d.name.clone());
        code.set(d.main_cpp.clone());
        libs.set(d.lib_deps.clone());
        loaded.set(Some(d));
    };
    use_effect(move || {
        let Some(Ok(d)) = &*detail.read() else { return };
        if loaded.peek().is_none() {
            apply(d.clone());
        }
    });

    let mut violations = use_signal(Vec::<SourceViolation>::new);
    let mut check = use_signal(|| None::<FirmwareCheckStatus>);
    let mut checking = use_signal(|| false);
    let mut saving = use_signal(|| false);
    let mut history_open = use_signal(|| false);

    // Insert-at-caret bridge (API reference snippets, #include lines).
    let mut insert_req: Signal<Option<(u64, String)>> = use_signal(|| None);
    let editor_area: Signal<Option<AreaHandle>> = use_signal(|| None);
    let mut insert_gen = use_signal(|| 0u64);
    let mut insert_at_caret = move |text: String| {
        insert_gen.with_mut(|g| *g += 1);
        insert_req.set(Some((insert_gen(), text)));
    };

    let dirty = loaded()
        .is_some_and(|d| code() != d.main_cpp || libs() != d.lib_deps || name().trim() != d.name);

    // Compile-only check of the saved revision, polled until it settles.
    let mut run_check = move || {
        checking.set(true);
        check.set(None);
        spawn(async move {
            let check_id = match api::firmware::start_check(project_id).await {
                Ok(id) => id,
                Err(err) => {
                    checking.set(false);
                    toasts::error(err);
                    return;
                }
            };
            loop {
                util::sleep(std::time::Duration::from_millis(CHECK_POLL_MS)).await;
                match api::firmware::check_status(project_id, &check_id).await {
                    Ok(status) => {
                        let done = !matches!(status.status.as_str(), "queued" | "running");
                        check.set(Some(status));
                        if done {
                            break;
                        }
                    }
                    Err(err) => {
                        toasts::error(err);
                        break;
                    }
                }
            }
            checking.set(false);
        });
    };

    // Save (new revision when content changed), optionally chained with a
    // check — « Verify » always compiles the saved revision.
    let mut save = move |then_check: bool| {
        let Some(base) = loaded() else { return };
        if !dirty {
            if then_check {
                run_check();
            }
            return;
        }
        let params = SaveFirmwareRevision {
            expected_revision_number: base.current_revision_number,
            main_cpp: (code() != base.main_cpp).then(|| code()),
            lib_deps: (libs() != base.lib_deps).then(|| libs()),
            name: (name().trim() != base.name).then(|| name().trim().to_string()),
            description: None,
            note: None,
        };
        saving.set(true);
        spawn(async move {
            match api::firmware::save(project_id, params).await {
                Ok(fresh) => {
                    violations.set(Vec::new());
                    toasts::success("toast-firmware-saved");
                    apply(fresh);
                    if then_check {
                        run_check();
                    }
                }
                Err(err) if err.status == Some(409) => {
                    // Someone saved first: reload the server truth.
                    toasts::error(err);
                    if let Ok(fresh) = api::firmware::detail(project_id).await {
                        apply(fresh);
                    }
                }
                Err(err) => {
                    violations.set(api::firmware::source_violations(&err));
                    toasts::error(err);
                }
            }
            saving.set(false);
        });
    };

    let format_code = move |_| {
        let current = code();
        let formatted = crate::components::code_editing::reindent(&current, CodeLang::Cpp);
        if formatted != current {
            code.set(formatted);
        }
    };

    // Gutter marks: guard violations, then compiler diagnostics of the
    // sketch (errors win over warnings on the same line).
    let mut marks: Vec<LineMark> = violations()
        .iter()
        .map(|v| LineMark {
            line: v.line,
            severity: MarkSeverity::Error,
        })
        .collect();
    if let Some(status) = check() {
        for d in status
            .diagnostics
            .iter()
            .filter(|d| d.in_sketch && d.severity != "note")
        {
            if marks.iter().any(|m| m.line == d.line) {
                continue;
            }
            let severity = if d.severity == "error" {
                MarkSeverity::Error
            } else {
                MarkSeverity::Warning
            };
            marks.push(LineMark {
                line: d.line,
                severity,
            });
        }
    }

    let chip = loaded().map(|d| d.chip_family).unwrap_or_default();
    let revision = loaded().map(|d| d.current_revision_number).unwrap_or(0);
    let code_value = code();

    rsx! {
        div { class: "space-y-6",
            // ── Header: back, name, chip, revision, actions ──
            div { class: "flex items-start justify-between flex-wrap gap-3",
                div { class: "flex items-start gap-3 flex-grow min-w-0",
                    button {
                        class: "px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors shrink-0",
                        onclick: move |_| on_back.call(()),
                        icons::ArrowLeft { class: "h-4 w-4" }
                    }
                    div { class: "flex-grow min-w-0",
                        input {
                            class: "w-full text-xl font-bold text-gray-900 bg-transparent rounded-lg px-2 py-0.5 -ml-2 border border-transparent hover:border-gray-300 focus:border-blue-500 focus:bg-white outline-none",
                            value: "{name}",
                            disabled: !can_write,
                            oninput: move |event| name.set(event.value()),
                        }
                        div { class: "flex items-center gap-2 mt-1 flex-wrap",
                            span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-slate-100 text-slate-800 w-fit",
                                {chip_label(&chip)}
                            }
                            span { class: "text-sm text-gray-500", {format!("r{revision}")} }
                            if dirty {
                                span { class: "text-xs text-amber-600", {t!("firmware-dirty")} }
                            }
                        }
                    }
                }
                div { class: "flex items-center gap-2 shrink-0",
                    button {
                        class: "px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                        onclick: move |_| history_open.set(true),
                        icons::History { class: "h-4 w-4 inline mr-1" }
                        {t!("firmware-history")}
                    }
                    button {
                        class: "px-3 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors disabled:opacity-40",
                        disabled: checking() || saving() || (dirty && !can_write),
                        onclick: move |_| save(true),
                        icons::CheckCircle { class: "h-4 w-4 inline mr-1" }
                        if dirty { {t!("firmware-save-and-verify")} } else { {t!("firmware-verify")} }
                    }
                    if can_write {
                        button {
                            class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: !dirty || saving(),
                            onclick: move |_| save(false),
                            icons::Save { class: "h-4 w-4 inline mr-1" }
                            {t!("firmware-save")}
                        }
                    }
                }
            }

            div {
                div { class: "flex items-center gap-2 flex-wrap px-3.5 py-2 rounded-t-xl border border-b-0 border-gray-200 bg-white",
                    span { class: "text-[13px] font-semibold font-mono", "src/main.cpp" }
                    LibChips { selected: libs, readonly: !can_write }
                    span { class: "flex-grow" }
                    if can_write {
                        LibMenu {
                            chip: chip.clone(),
                            selected: libs,
                            readonly: !can_write,
                            on_include: move |header: String| insert_at_caret(format!("#include <{header}>\n")),
                        }
                        ApiMenu {
                            readonly: !can_write,
                            on_insert: move |snippet: String| insert_at_caret(snippet),
                        }
                        button {
                            class: "h-8 px-2.5 rounded-md border border-gray-300 bg-white text-[13px] text-gray-700 hover:bg-gray-50",
                            onclick: format_code,
                            {t!("firmware-format")}
                        }
                    }
                }
                FunctionCodeEditor {
                    value: code_value,
                    language: CodeLang::Cpp,
                    readonly: !can_write,
                    oninput: move |v: String| code.set(v),
                    marks,
                    insert_request: insert_req,
                    area: editor_area,
                }
                CheckReport { violations: violations(), check: check(), checking: checking() }
            }

            if history_open() {
                RevisionsDrawer {
                    project_id,
                    current_revision: revision,
                    on_close: move |_| history_open.set(false),
                    on_load: move |(main_cpp, lib_deps): (String, Vec<String>)| {
                        code.set(main_cpp);
                        libs.set(lib_deps);
                        history_open.set(false);
                    },
                }
            }
        }
    }
}

/// Bottom report: guard violations, then the last check (running spinner,
/// success, diagnostics list, or infra error log).
#[component]
fn CheckReport(
    violations: Vec<SourceViolation>,
    check: Option<FirmwareCheckStatus>,
    checking: bool,
) -> Element {
    let box_class = "px-3.5 py-2 rounded-b-xl border border-t-0 text-[13px]";
    if !violations.is_empty() {
        return rsx! {
            div { class: "{box_class} border-red-200 bg-red-50 text-red-800 space-y-1",
                for v in violations {
                    div { class: "flex gap-2",
                        b { class: "font-mono", {v.line.to_string()} }
                        span { {crate::api::error_i18n::resolve(&pnex_core::err_codes::fluent_key(&v.code), None)} }
                    }
                }
            }
        };
    }
    // What the user acts on: the sketch diagnostics first, then errors from
    // other files; library warnings/notes are noise (not the user's code).
    let relevant = |status: &FirmwareCheckStatus| -> Vec<pnex_core::firmware::CompileDiagnostic> {
        let mut out: Vec<_> = status
            .diagnostics
            .iter()
            .filter(|d| d.in_sketch && d.severity != "note")
            .cloned()
            .collect();
        out.extend(
            status
                .diagnostics
                .iter()
                .filter(|d| !d.in_sketch && d.severity == "error")
                .cloned(),
        );
        out
    };
    match (checking, check) {
        (true, None) => rsx! {
            div { class: "{box_class} border-gray-200 bg-gray-50 text-gray-700 flex items-center gap-2",
                span { class: "animate-spin rounded-full h-4 w-4 border-b-2 border-blue-600" }
                {t!("firmware-check-running")}
            }
        },
        (_, Some(status)) if matches!(status.status.as_str(), "queued" | "running") => rsx! {
            div { class: "{box_class} border-gray-200 bg-gray-50 text-gray-700 flex items-center gap-2",
                span { class: "animate-spin rounded-full h-4 w-4 border-b-2 border-blue-600" }
                {t!("firmware-check-running")}
            }
        },
        (_, Some(status)) if status.status == "succeeded" => rsx! {
            div { class: "{box_class} border-green-200 bg-green-50 text-green-800 flex items-center gap-2",
                icons::CheckCircle { class: "h-4 w-4" }
                {t!("firmware-check-ok", revision: status.revision_number)}
                if !relevant(&status).is_empty() {
                    span { class: "text-amber-700", {t!("firmware-check-warnings", count: relevant(&status).len())} }
                }
            }
        },
        (_, Some(status)) if status.status == "failed" => rsx! {
            div { class: "{box_class} border-red-200 bg-red-50 text-red-800 space-y-1",
                div { class: "font-semibold", {t!("firmware-check-failed", revision: status.revision_number)} }
                if relevant(&status).is_empty() {
                    pre { class: "font-mono text-xs whitespace-pre-wrap", {status.log_tail.clone()} }
                }
                for d in relevant(&status) {
                    div { class: "font-mono text-xs",
                        span { class: if d.severity == "error" { "text-red-700 font-semibold mr-2" } else { "text-amber-700 font-semibold mr-2" },
                            {format!("{}:{}", d.file, d.line)}
                        }
                        span { class: "break-words", {d.message.clone()} }
                    }
                }
            }
        },
        (_, Some(status)) => rsx! {
            div { class: "{box_class} border-red-200 bg-red-50 text-red-800 space-y-1",
                div { class: "font-semibold", {t!("firmware-check-error")} }
                pre { class: "font-mono text-xs whitespace-pre-wrap", {status.log_tail.clone()} }
            }
        },
        (false, None) => rsx! {
            div { class: "{box_class} border-gray-200 bg-white text-gray-500", {t!("firmware-check-hint")} }
        },
    }
}
