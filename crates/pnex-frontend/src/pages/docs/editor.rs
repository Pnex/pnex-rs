//! Live editor of one page: ProseMirror + Yjs mounted by `doc_editor`,
//! toolbar, "/" palette, presence, Markdown import/export and the versions
//! panel. Viewers get the same page read-only (the server enforces it).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api;
use crate::app::Route;
use crate::components::icons;
use crate::doc_editor::{self, DocState};

/// Toolbar entries: (command, label key).
const BLOCKS: &[(&str, &str)] = &[
    ("paragraph", "docs-cmd-paragraph"),
    ("h1", "docs-cmd-h1"),
    ("h2", "docs-cmd-h2"),
    ("h3", "docs-cmd-h3"),
    ("bullet", "docs-cmd-bullet"),
    ("ordered", "docs-cmd-ordered"),
    ("quote", "docs-cmd-quote"),
    ("codeblock", "docs-cmd-codeblock"),
    ("rule", "docs-cmd-rule"),
];

const TOOL_BTN: &str = "px-2 py-1 text-sm rounded hover:bg-gray-100 text-gray-700";

/// Stable cursor color of a user (presence).
fn user_color(id: i64) -> &'static str {
    const PALETTE: [&str; 8] = [
        "#2563eb", "#dc2626", "#16a34a", "#9333ea", "#ea580c", "#0891b2", "#db2777", "#65a30d",
    ];
    PALETTE[id.rem_euclid(PALETTE.len() as i64) as usize]
}

#[component]
pub fn DocEditor(id: Uuid) -> Element {
    let host = format!("pnex-doc-{id}");
    let can_write = crate::state::org::current_can_write();
    let mut state = use_signal(DocState::default);
    let mut mounted = use_signal(|| Option::<bool>::None);
    let mut title = use_signal(String::new);
    let mut show_versions = use_signal(|| false);
    let mut show_comments = use_signal(|| false);
    let mut draft = use_signal(|| Option::<doc_editor::SelectionAnchor>::None);
    let mut link_href = use_signal(|| Option::<String>::None);
    let mut notice = use_signal(|| Option::<String>::None);

    let detail = use_resource(move || async move { api::docs::detail(id).await });
    use_effect(move || {
        if let Some(Ok(d)) = &*detail.read() {
            title.set(d.title.clone());
        }
    });

    // Mount, then poll the JS state; a disconnect asks for a new ticket.
    let host_poll = host.clone();
    use_future(move || {
        let host = host_poll.clone();
        async move {
            let (name, color) = match crate::state::session::user() {
                Some(u) => (
                    u.full_name.clone().unwrap_or(u.username.clone()),
                    user_color(u.id),
                ),
                None => (String::new(), user_color(0)),
            };
            let Some(url) = api::docs::live_ws_url(id).await else {
                mounted.set(Some(false));
                return;
            };
            let ok = doc_editor::mount(&host, &url, can_write, &name, color).await;
            mounted.set(Some(ok));
            if !ok {
                return;
            }
            // Reconnection: a ticket per attempt, exponential backoff
            // (0.5 s → 30 s), reset once synced; a failed ticket fetch is
            // retried, never given up.
            let mut served = 0u64;
            let mut failures = 0u32;
            let mut clock_ms = 0u64;
            let mut next_try_ms = 0u64;
            loop {
                crate::util::sleep(Duration::from_millis(300)).await;
                clock_ms += 300;
                let Some(s) = doc_editor::state(&host).await else {
                    continue;
                };
                if s.status == "synced" {
                    failures = 0;
                }
                if s.need_ticket > served && clock_ms >= next_try_ms {
                    if let Some(url) = api::docs::live_ws_url(id).await {
                        doc_editor::reconnect(&host, &url).await;
                        served = s.need_ticket;
                    }
                    failures += 1;
                    next_try_ms = clock_ms + (500u64 << failures.min(6)).min(30_000);
                }
                let clicked = s.active_comment.as_ref().map(|a| a.seq);
                if clicked.is_some()
                    && clicked != state.peek().active_comment.as_ref().map(|a| a.seq)
                {
                    show_comments.set(true);
                }
                if *state.peek() != s {
                    state.set(s);
                }
            }
        }
    });
    let host_drop = host.clone();
    use_drop(move || doc_editor::unmount(&host_drop));

    let run = {
        let host = host.clone();
        move |name: &'static str, drop_slash: bool| {
            let host = host.clone();
            spawn(async move {
                doc_editor::command(&host, name, drop_slash).await;
            });
        }
    };

    let host_comment = host.clone();
    let comment = move |_| {
        let host = host_comment.clone();
        spawn(async move {
            match doc_editor::selection_anchor(&host).await {
                Some(a) => {
                    draft.set(Some(a));
                    show_comments.set(true);
                }
                None => notice.set(Some(t!("docs-comment-select-first").to_string())),
            }
        });
    };

    let s = state();
    let status_label = match s.status.as_str() {
        "synced" => t!("docs-status-synced"),
        "offline" => t!("docs-status-offline"),
        "refused" => t!("docs-status-refused"),
        "closed" => t!("docs-status-closed"),
        "toolarge" => t!("docs-status-toolarge"),
        _ => t!("docs-status-connecting"),
    };
    let status_dot = match s.status.as_str() {
        "synced" => "bg-green-500",
        "offline" | "connecting" => "bg-amber-400",
        _ => "bg-red-500",
    };

    rsx! {
        div { class: "p-4 pb-24 md:p-6 max-w-5xl mx-auto",
            div { class: "mb-4 flex flex-wrap items-center gap-3",
                Link {
                    to: Route::Docs { id: String::new() },
                    class: "text-gray-500 hover:text-gray-900",
                    aria_label: t!("docs-back"),
                    icons::ArrowLeft { class: "h-5 w-5" }
                }
                TitleField { id, title, can_write }
                div { class: "ml-auto flex items-center gap-3 text-sm text-gray-600",
                    span { class: "flex items-center gap-1.5",
                        span { class: "inline-block h-2 w-2 rounded-full {status_dot}" }
                        {status_label}
                    }
                    if s.peers > 0 {
                        span { {t!("docs-peers", count : s.peers)} }
                    }
                    if !can_write {
                        span { class: "rounded bg-gray-100 px-2 py-0.5 text-xs",
                            {t!("docs-read-only")}
                        }
                    }
                    MarkdownActions {
                        host: host.clone(),
                        title: title(),
                        can_write,
                        notice,
                    }
                    button {
                        class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| show_comments.toggle(),
                        icons::MessageCircle { class: "h-4 w-4 inline mr-1" }
                        {t!("docs-comments")}
                    }
                    button {
                        class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| show_versions.toggle(),
                        icons::History { class: "h-4 w-4 inline mr-1" }
                        {t!("docs-versions")}
                    }
                }
            }
            if let Some(msg) = notice() {
                div { class: "mb-3 rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800",
                    {msg}
                }
            }
            if can_write {
                div { class: "sticky top-0 z-10 mb-2 flex flex-wrap items-center gap-1 rounded-lg border border-gray-200 bg-white p-1",
                    button {
                        class: "{TOOL_BTN} font-bold",
                        title: t!("docs-cmd-bold"),
                        onclick: {
                            let run = run.clone();
                            move |_| run("bold", false)
                        },
                        "B"
                    }
                    button {
                        class: "{TOOL_BTN} italic",
                        title: t!("docs-cmd-italic"),
                        onclick: {
                            let run = run.clone();
                            move |_| run("italic", false)
                        },
                        "I"
                    }
                    button {
                        class: "{TOOL_BTN} font-mono",
                        title: t!("docs-cmd-code"),
                        onclick: {
                            let run = run.clone();
                            move |_| run("code", false)
                        },
                        "</>"
                    }
                    button {
                        class: TOOL_BTN,
                        onclick: move |_| link_href.set(Some(String::new())),
                        {t!("docs-cmd-link")}
                    }
                    button { class: TOOL_BTN, onclick: comment, {t!("docs-cmd-comment")} }
                    span { class: "mx-1 h-5 w-px bg-gray-200" }
                    for (cmd, key) in BLOCKS.iter().copied() {
                        button {
                            key: "{cmd}",
                            class: TOOL_BTN,
                            onclick: {
                                let run = run.clone();
                                move |_| run(cmd, false)
                            },
                            {t!(key)}
                        }
                    }
                    span { class: "mx-1 h-5 w-px bg-gray-200" }
                    button {
                        class: TOOL_BTN,
                        title: t!("docs-cmd-undo"),
                        onclick: {
                            let run = run.clone();
                            move |_| run("undo", false)
                        },
                        icons::Undo { class: "h-4 w-4" }
                    }
                    button {
                        class: TOOL_BTN,
                        title: t!("docs-cmd-redo"),
                        onclick: {
                            let run = run.clone();
                            move |_| run("redo", false)
                        },
                        icons::Redo { class: "h-4 w-4" }
                    }
                }
            }
            if let Some(href) = link_href() {
                LinkBar { host: host.clone(), href, link_href }
            }
            if mounted() == Some(false) {
                div { class: "rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700",
                    {t!("docs-editor-unavailable")}
                }
            }
            div { class: "flex gap-4",
                div {
                    id: "{host}",
                    class: "prose max-w-none flex-1 min-w-0 rounded-lg border border-gray-200 bg-white p-4 md:p-8",
                }
                if show_comments() {
                    super::comments::CommentsPanel {
                        id,
                        host: host.clone(),
                        can_write,
                        comments_seq: s.comments_seq,
                        active: s.active_comment.as_ref().map(|a| a.id),
                        draft,
                    }
                }
                if show_versions() {
                    super::versions::VersionsPanel { id, host: host.clone(), can_write }
                }
            }
            if let Some(slash) = s.slash.clone() {
                if can_write {
                    div {
                        class: "fixed z-20 w-56 rounded-lg border border-gray-200 bg-white py-1 shadow-lg",
                        style: "left: {slash.x}px; top: {slash.y + 4.0}px;",
                        for (cmd, key) in BLOCKS.iter().copied() {
                            button {
                                key: "{cmd}",
                                class: "block w-full px-3 py-1.5 text-left text-sm hover:bg-gray-100",
                                onclick: {
                                    let run = run.clone();
                                    move |_| run(cmd, true)
                                },
                                {t!(key)}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Inline title, renamed on blur / Enter (writers only).
#[component]
fn TitleField(id: Uuid, title: Signal<String>, can_write: bool) -> Element {
    let mut error = use_signal(|| false);
    let mut save = move || {
        let name = title().trim().to_string();
        if name.is_empty() {
            error.set(true);
            return;
        }
        spawn(async move {
            error.set(api::docs::rename(id, name).await.is_err());
        });
    };
    if !can_write {
        return rsx! {
            h1 { class: "text-xl font-semibold text-gray-900 truncate", {title()} }
        };
    }
    rsx! {
        input {
            class: "min-w-0 flex-1 basis-60 rounded px-2 py-1 text-xl font-semibold text-gray-900 hover:bg-gray-50 focus:bg-white",
            class: if error() { "ring-1 ring-red-400" },
            aria_label: t!("docs-field-title"),
            value: "{title}",
            oninput: move |e| title.set(e.value()),
            onblur: move |_| save(),
            onkeydown: move |e| {
                if e.key() == Key::Enter {
                    save();
                }
            },
        }
    }
}

/// Link input for the selection; refused schemes stay in the bar.
#[component]
fn LinkBar(host: String, href: String, link_href: Signal<Option<String>>) -> Element {
    let mut refused = use_signal(|| false);
    let apply = move |_| {
        let host = host.clone();
        let href = link_href().unwrap_or_default();
        spawn(async move {
            if doc_editor::link(&host, &href).await {
                link_href.set(None);
            } else {
                refused.set(true);
            }
        });
    };
    rsx! {
        div { class: "mb-2 flex flex-wrap items-center gap-2 rounded-lg border border-gray-200 bg-gray-50 p-2",
            input {
                class: "min-w-0 flex-1 rounded border border-gray-300 px-2 py-1 text-sm",
                placeholder: "https://",
                aria_label: t!("docs-cmd-link"),
                value: "{href}",
                oninput: move |e| link_href.set(Some(e.value())),
            }
            button {
                class: "px-3 py-1 text-sm bg-blue-600 text-white rounded-lg",
                onclick: apply,
                {t!("docs-link-apply")}
            }
            button {
                class: "px-3 py-1 text-sm text-gray-600",
                onclick: move |_| link_href.set(None),
                {t!("docs-cancel")}
            }
            if refused() {
                span { class: "w-full text-xs text-red-600", {t!("docs-link-refused")} }
            }
        }
    }
}

/// Markdown export (download) and import (appended to the page).
#[component]
fn MarkdownActions(
    host: String,
    title: String,
    can_write: bool,
    notice: Signal<Option<String>>,
) -> Element {
    let host_export = host.clone();
    let export = move |_| {
        let host = host_export.clone();
        let file = format!("{}.md", title.trim().replace(['/', '\\'], "-"));
        spawn(async move {
            match doc_editor::export_markdown(&host).await {
                Some(md)
                    if crate::util::trigger_download(&file, "text/markdown", md.as_bytes()) => {}
                _ => notice.set(Some(t!("docs-export-failed").to_string())),
            }
        });
    };
    rsx! {
        button {
            class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
            onclick: export,
            icons::Download { class: "h-4 w-4 inline mr-1" }
            {t!("docs-export-md")}
        }
        if can_write {
            label { class: "cursor-pointer px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                icons::Upload { class: "h-4 w-4 inline mr-1" }
                {t!("docs-import-md")}
                input {
                    class: "hidden",
                    r#type: "file",
                    accept: ".md,.markdown,text/markdown,text/plain",
                    onchange: move |evt| {
                        let host = host.clone();
                        async move {
                            let Some(file) = evt.files().first().cloned() else {
                                return;
                            };
                            let text = file.read_string().await.unwrap_or_default();
                            if !doc_editor::import_markdown(&host, &text).await {
                                notice.set(Some(t!("docs-import-failed").to_string()));
                            }
                        }
                    },
                }
            }
        }
    }
}
