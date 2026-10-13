//! Named versions of a page (P5): freeze the current content, view a
//! version read-only, diff it with the previous one, restore it (a normal
//! edit of the live page, then a new version that says so).

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api;
use crate::api::error::ApiError;
use crate::components::modal::Modal;
use crate::components::ui::BTN_PRIMARY_SM;
use crate::doc_editor::{self, DiffBlock};
use pnex_core::pages::DocVersionSummary;

/// What the modal shows.
#[derive(Clone, PartialEq)]
enum Shown {
    View(i64),
    Diff(i64, Vec<DiffBlock>),
}

const STATIC_HOST: &str = "pnex-doc-version";

#[component]
pub fn VersionsPanel(id: Uuid, host: String, can_write: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut label = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| Option::<ApiError>::None);
    let mut shown = use_signal(|| Option::<Shown>::None);
    let notice = use_signal(|| Option::<String>::None);

    let list = use_resource(move || async move {
        let _ = reload();
        api::docs::versions(id).await
    });
    let rows: Vec<DocVersionSummary> = match &*list.value().read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };

    let save_host = host.clone();
    let mut save_notice = notice;
    let save = move |_| {
        let name = label().trim().to_string();
        let host = save_host.clone();
        busy.set(true);
        spawn(async move {
            // The version is frozen from the server: last keystrokes first.
            if !doc_editor::flush(&host).await {
                save_notice.set(Some(t!("docs-version-not-flushed").to_string()));
                busy.set(false);
                return;
            }
            save_notice.set(None);
            match api::docs::create_version(id, name).await {
                Ok(_) => {
                    label.set(String::new());
                    error.set(None);
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    rsx! {
        aside { class: "w-72 shrink-0 rounded-lg border border-gray-200 bg-white p-3 text-sm",
            h2 { class: "mb-2 font-semibold text-gray-900", {t!("docs-versions")} }
            if can_write {
                div { class: "mb-3 flex gap-1",
                    input {
                        class: "min-w-0 flex-1 rounded border border-gray-300 px-2 py-1",
                        placeholder: t!("docs-version-label"),
                        aria_label: t!("docs-version-label"),
                        value: "{label}",
                        oninput: move |e| label.set(e.value()),
                    }
                    button {
                        class: BTN_PRIMARY_SM,
                        disabled: busy() || label().trim().is_empty(),
                        onclick: save,
                        {t!("docs-version-save")}
                    }
                }
            }
            if let Some(err) = error() {
                p { class: "mb-2 text-xs text-red-600", {crate::api::error_i18n::localize(&err)} }
            }
            if let Some(msg) = notice() {
                p { class: "mb-2 text-xs text-amber-700", {msg} }
            }
            if rows.is_empty() {
                p { class: "text-gray-500", {t!("docs-versions-empty")} }
            }
            ul { class: "space-y-2",
                for (i, v) in rows.iter().enumerate() {
                    VersionRow {
                        key: "{v.version_number}",
                        id,
                        host: host.clone(),
                        version: v.clone(),
                        previous: rows.get(i + 1).map(|p| p.version_number),
                        can_write,
                        shown,
                        notice,
                        reload,
                    }
                }
            }
        }
        if let Some(s) = shown() {
            VersionModal { id, shown: s, on_close: move |_| shown.set(None) }
        }
    }
}

#[component]
fn VersionRow(
    id: Uuid,
    host: String,
    version: DocVersionSummary,
    previous: Option<i64>,
    can_write: bool,
    shown: Signal<Option<Shown>>,
    notice: Signal<Option<String>>,
    reload: Signal<u32>,
) -> Element {
    let n = version.version_number;
    let diff = move |_| {
        let Some(prev) = previous else {
            return;
        };
        spawn(async move {
            let (Ok(a), Ok(b)) = (
                api::docs::version_state(id, prev).await,
                api::docs::version_state(id, n).await,
            ) else {
                notice.set(Some(t!("docs-version-load-failed").to_string()));
                return;
            };
            match doc_editor::diff(&a.state_b64, &b.state_b64).await {
                Some(blocks) => shown.set(Some(Shown::Diff(n, blocks))),
                None => notice.set(Some(t!("docs-version-load-failed").to_string())),
            }
        });
    };
    let restore = move |_| {
        let host = host.clone();
        spawn(async move {
            let Ok(state) = api::docs::version_state(id, n).await else {
                notice.set(Some(t!("docs-version-load-failed").to_string()));
                return;
            };
            if !doc_editor::restore(&host, &state.state_b64).await {
                notice.set(Some(t!("docs-restore-failed").to_string()));
                return;
            }
            // Freeze it only once the server has it (a refused or too large
            // edit must not produce a mislabelled version).
            if !doc_editor::flush(&host).await {
                notice.set(Some(t!("docs-restore-failed").to_string()));
                return;
            }
            let label = t!("docs-restored-label", n : n).to_string();
            if api::docs::create_version(id, label).await.is_ok() {
                reload.with_mut(|r| *r += 1);
            }
        });
    };
    let pdf_name = format!("v{n} {}.pdf", version.label);
    let pdf = move |_| {
        let name = pdf_name.clone();
        spawn(async move {
            match api::docs::version_pdf(id, n).await {
                Ok(bytes) => crate::util::save_blob(&name, &bytes),
                Err(e) => notice.set(Some(crate::api::error_i18n::localize(&e))),
            }
        });
    };
    let approve = move |_| {
        spawn(async move {
            match api::docs::approve_version(id, n).await {
                Ok(_) => reload.with_mut(|r| *r += 1),
                Err(e) => notice.set(Some(crate::api::error_i18n::localize(&e))),
            }
        });
    };
    let approved = version.approved_at.as_ref().map(|at| {
        let by = version.approved_by.clone().unwrap_or_default();
        t!("docs-version-approved", by : by, date : super::short_date(at)).to_string()
    });
    let needs_review = version.ai_generated && approved.is_none();
    rsx! {
        li { class: "rounded border border-gray-100 p-2",
            div { class: "font-medium text-gray-900", "v{n} · {version.label}" }
            div { class: "text-xs text-gray-500",
                {super::short_date(&version.created_at)}
                if let Some(by) = version.created_by.clone() {
                    " · {by}"
                }
                if version.flow_id.is_some() {
                    " · "
                    {t!("docs-version-by-flow")}
                }
            }
            if needs_review {
                div { class: "mt-1 inline-block rounded bg-amber-50 px-1.5 py-0.5 text-xs text-amber-800",
                    {t!("docs-version-ai-review")}
                }
            } else if let Some(text) = approved {
                div { class: "mt-1 inline-block rounded bg-emerald-50 px-1.5 py-0.5 text-xs text-emerald-800",
                    "{text}"
                }
            }
            div { class: "mt-1 flex flex-wrap gap-2 text-xs",
                button {
                    class: "text-signal-700 hover:underline",
                    onclick: move |_| shown.set(Some(Shown::View(n))),
                    {t!("docs-version-view")}
                }
                if previous.is_some() {
                    button {
                        class: "text-signal-700 hover:underline",
                        onclick: diff,
                        {t!("docs-version-diff")}
                    }
                }
                button {
                    class: "text-signal-700 hover:underline",
                    title: t!("docs-version-pdf-help").to_string(),
                    onclick: pdf,
                    {t!("docs-version-pdf")}
                }
                if can_write {
                    button {
                        class: "text-signal-700 hover:underline",
                        onclick: restore,
                        {t!("docs-version-restore")}
                    }
                }
                if can_write && needs_review {
                    button {
                        class: "font-medium text-emerald-700 hover:underline",
                        onclick: approve,
                        {t!("docs-version-approve")}
                    }
                }
            }
        }
    }
}

#[component]
fn VersionModal(id: Uuid, shown: Shown, on_close: EventHandler<()>) -> Element {
    let view_n = match &shown {
        Shown::View(n) => Some(*n),
        Shown::Diff(..) => None,
    };
    let mut failed = use_signal(|| false);
    use_effect(use_reactive!(|view_n| {
        if let Some(n) = view_n {
            spawn(async move {
                let ok = match api::docs::version_state(id, n).await {
                    Ok(s) => doc_editor::mount_static(STATIC_HOST, &s.state_b64).await,
                    Err(_) => false,
                };
                failed.set(!ok);
            });
        }
    }));
    use_drop(|| doc_editor::unmount(STATIC_HOST));
    let title = match &shown {
        Shown::View(n) => t!("docs-version-view-title", n : *n).to_string(),
        Shown::Diff(n, _) => t!("docs-version-diff-title", n : *n).to_string(),
    };
    let blocks = match &shown {
        Shown::Diff(_, blocks) => blocks.clone(),
        Shown::View(_) => Vec::new(),
    };
    rsx! {
        Modal {
            title,
            on_close: move |_| on_close.call(()),
            max_width: "max-w-3xl".to_string(),
            if failed() {
                p { class: "text-sm text-red-600", {t!("docs-version-load-failed")} }
            }
            if view_n.is_some() {
                div {
                    id: STATIC_HOST,
                    class: "prose max-w-none max-h-[70vh] overflow-auto",
                }
            } else {
                div { class: "max-h-[70vh] space-y-1 overflow-auto text-sm",
                    for (i, b) in blocks.into_iter().enumerate() {
                        DiffLine { key: "{i}", block: b }
                    }
                }
            }
        }
    }
}

#[component]
fn DiffLine(block: DiffBlock) -> Element {
    let (class, sign) = match block.op.as_str() {
        "add" => ("bg-green-50 text-green-900", "+"),
        "del" => ("bg-red-50 text-red-900 line-through", "−"),
        _ => ("text-gray-600", " "),
    };
    rsx! {
        div { class: "flex gap-2 rounded px-2 py-1 {class}",
            span { class: "w-3 shrink-0 font-mono", {sign} }
            span { class: "whitespace-pre-wrap", {block.text} }
        }
    }
}
