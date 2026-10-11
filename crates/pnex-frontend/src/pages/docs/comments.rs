//! Comments panel of a page (P12): threads anchored on text, replies,
//! resolve / reopen. Open threads are highlighted in the editor; clicking a
//! quote selects its text. Reloads when the server says they changed.

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api;
use crate::api::error::ApiError;
use crate::components::ui::BTN_PRIMARY_SM;
use crate::doc_editor::{self, SelectionAnchor};
use pnex_core::pages::{CreateDocComment, DocComment, PatchDocComment};

const SMALL_BTN: &str = "text-xs text-blue-700 hover:underline";

#[component]
pub fn CommentsPanel(
    id: Uuid,
    host: String,
    can_write: bool,
    /// Bumped by the editor when the server announces a change.
    comments_seq: u64,
    /// Thread whose highlighted text was clicked.
    active: Option<i64>,
    /// New thread being written (anchor of the selection).
    draft: Signal<Option<SelectionAnchor>>,
) -> Element {
    let reload = use_signal(|| 0u32);
    let mut show_resolved = use_signal(|| false);
    let error = use_signal(|| Option::<ApiError>::None);

    let list = use_resource(use_reactive!(|comments_seq| async move {
        let _ = (reload(), comments_seq);
        api::docs::comments(id).await
    }));
    let all: Vec<DocComment> = match &*list.value().read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };

    // Highlight the open threads in the editor.
    let host_marks = host.clone();
    use_effect(move || {
        let roots: Vec<(i64, Option<String>, bool)> = match &*list.value().read() {
            Some(Ok(rows)) => rows
                .iter()
                .filter(|c| c.parent_id.is_none())
                .map(|c| (c.id, c.anchor.clone(), c.resolved))
                .collect(),
            _ => return,
        };
        let host = host_marks.clone();
        spawn(async move {
            doc_editor::set_comments(&host, &roots).await;
        });
    });

    let roots: Vec<DocComment> = all
        .iter()
        .filter(|c| c.parent_id.is_none() && (show_resolved() || !c.resolved))
        .cloned()
        .collect();
    let resolved_count = all
        .iter()
        .filter(|c| c.parent_id.is_none() && c.resolved)
        .count();

    rsx! {
        aside { class: "w-80 shrink-0 rounded-lg border border-gray-200 bg-white p-3 text-sm",
            h2 { class: "mb-2 font-semibold text-gray-900", {t!("docs-comments")} }
            if let Some(err) = error() {
                p { class: "mb-2 text-xs text-red-600", {crate::api::error_i18n::localize(&err)} }
            }
            if let Some(d) = draft() {
                NewThread {
                    id,
                    anchor: d,
                    draft,
                    error,
                    reload,
                }
            }
            if roots.is_empty() && draft().is_none() {
                p { class: "text-gray-500", {t!("docs-comments-empty")} }
            }
            ul { class: "space-y-3",
                for root in roots {
                    Thread {
                        key: "{root.id}",
                        id,
                        host: host.clone(),
                        replies: all.iter().filter(|c| c.parent_id == Some(root.id)).cloned().collect(),
                        active: active == Some(root.id),
                        root,
                        can_write,
                        error,
                        reload,
                    }
                }
            }
            if resolved_count > 0 {
                button {
                    class: "mt-3 {SMALL_BTN}",
                    onclick: move |_| show_resolved.toggle(),
                    if show_resolved() {
                        {t!("docs-comments-hide-resolved")}
                    } else {
                        {t!("docs-comments-show-resolved", count : resolved_count)}
                    }
                }
            }
        }
    }
}

#[component]
fn NewThread(
    id: Uuid,
    anchor: SelectionAnchor,
    draft: Signal<Option<SelectionAnchor>>,
    error: Signal<Option<ApiError>>,
    reload: Signal<u32>,
) -> Element {
    let mut body = use_signal(String::new);
    let quote = anchor.quote.clone();
    let post = move |_| {
        let input = CreateDocComment {
            body: body(),
            parent_id: None,
            anchor: Some(anchor.anchor.clone()),
            quote: Some(anchor.quote.clone()),
        };
        spawn(async move {
            match api::docs::create_comment(id, input).await {
                Ok(()) => {
                    error.set(None);
                    draft.set(None);
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };
    rsx! {
        div { class: "mb-3 rounded border border-blue-200 bg-blue-50 p-2",
            p { class: "mb-1 border-l-2 border-yellow-400 pl-2 text-xs italic text-gray-600",
                {quote}
            }
            textarea {
                class: "w-full rounded border border-gray-300 p-1 text-sm",
                rows: "3",
                aria_label: t!("docs-comment-new"),
                placeholder: t!("docs-comment-new"),
                value: "{body}",
                oninput: move |e| body.set(e.value()),
            }
            div { class: "mt-1 flex justify-end gap-2",
                button { class: SMALL_BTN, onclick: move |_| draft.set(None), {t!("docs-cancel")} }
                button {
                    class: BTN_PRIMARY_SM,
                    disabled: body().trim().is_empty(),
                    onclick: post,
                    {t!("docs-comment-post")}
                }
            }
        }
    }
}

#[component]
fn Thread(
    id: Uuid,
    host: String,
    root: DocComment,
    replies: Vec<DocComment>,
    active: bool,
    can_write: bool,
    error: Signal<Option<ApiError>>,
    reload: Signal<u32>,
) -> Element {
    let mut reply = use_signal(String::new);
    let mut gone = use_signal(|| false);
    let cid = root.id;
    let resolved = root.resolved;

    let anchor = root.anchor.clone();
    let focus = move |_| {
        let host = host.clone();
        let anchor = anchor.clone();
        spawn(async move {
            let found = match anchor {
                Some(a) => doc_editor::focus_anchor(&host, &a).await,
                None => false,
            };
            gone.set(!found);
        });
    };
    let send_reply = move |_| {
        let input = CreateDocComment {
            body: reply(),
            parent_id: Some(cid),
            anchor: None,
            quote: None,
        };
        spawn(async move {
            match api::docs::create_comment(id, input).await {
                Ok(()) => {
                    reply.set(String::new());
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };
    let toggle = move |_| {
        let input = PatchDocComment {
            body: None,
            resolved: Some(!resolved),
        };
        spawn(async move {
            match api::docs::update_comment(id, cid, input).await {
                Ok(()) => reload.with_mut(|r| *r += 1),
                Err(e) => error.set(Some(e)),
            }
        });
    };
    let ring = if active { "ring-2 ring-yellow-400" } else { "" };
    rsx! {
        li { class: "rounded border border-gray-100 p-2 {ring}",
            if let Some(q) = root.quote.clone() {
                button {
                    class: "mb-1 block w-full border-l-2 border-yellow-400 pl-2 text-left text-xs italic text-gray-600 hover:bg-yellow-50",
                    onclick: focus,
                    {q}
                }
            }
            if gone() {
                p { class: "mb-1 text-xs text-amber-700", {t!("docs-comment-anchor-gone")} }
            }
            CommentLine {
                id,
                comment: root.clone(),
                can_write,
                error,
                reload,
            }
            for r in replies {
                div {
                    key: "{r.id}",
                    class: "ml-3 mt-1 border-l border-gray-200 pl-2",
                    CommentLine {
                        id,
                        comment: r.clone(),
                        can_write,
                        error,
                        reload,
                    }
                }
            }
            if can_write && !resolved {
                div { class: "mt-1 flex gap-1",
                    input {
                        class: "min-w-0 flex-1 rounded border border-gray-300 px-1 py-0.5 text-xs",
                        placeholder: t!("docs-comment-reply"),
                        aria_label: t!("docs-comment-reply"),
                        value: "{reply}",
                        oninput: move |e| reply.set(e.value()),
                    }
                    button {
                        class: SMALL_BTN,
                        disabled: reply().trim().is_empty(),
                        onclick: send_reply,
                        {t!("docs-comment-post")}
                    }
                }
            }
            if can_write {
                button { class: "mt-1 {SMALL_BTN}", onclick: toggle,
                    if resolved {
                        {t!("docs-comment-reopen")}
                    } else {
                        {t!("docs-comment-resolve")}
                    }
                }
            }
        }
    }
}

#[component]
fn CommentLine(
    id: Uuid,
    comment: DocComment,
    can_write: bool,
    error: Signal<Option<ApiError>>,
    reload: Signal<u32>,
) -> Element {
    let cid = comment.id;
    let delete = move |_| {
        spawn(async move {
            match api::docs::delete_comment(id, cid).await {
                Ok(()) => reload.with_mut(|r| *r += 1),
                Err(e) => error.set(Some(e)),
            }
        });
    };
    let author = comment.author.clone().unwrap_or_default();
    let date = super::short_date(&comment.created_at);
    rsx! {
        div {
            div { class: "text-xs text-gray-500",
                span { class: "font-medium text-gray-800", {author} }
                " · {date}"
                if let Some(v) = comment.version_number {
                    " · v{v}"
                }
            }
            p { class: "whitespace-pre-wrap text-sm text-gray-900", {comment.body.clone()} }
            if can_write && comment.mine {
                button {
                    class: "text-xs text-red-600 hover:underline",
                    onclick: delete,
                    {t!("docs-delete")}
                }
            }
        }
    }
}
