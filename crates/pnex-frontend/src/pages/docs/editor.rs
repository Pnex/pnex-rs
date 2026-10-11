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

/// Entries of the "/" menu (doc.js `BLOCK_ITEMS`): (command, label key).
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

/// Other texts drawn by doc.js: (label name, key).
const CHROME: &[(&str, &str)] = &[
    ("placeholder", "docs-placeholder"),
    ("basicBlocks", "docs-slash-basic"),
    ("noResults", "docs-slash-empty"),
    ("addBlock", "docs-add-block"),
    ("bold", "docs-cmd-bold"),
    ("italic", "docs-cmd-italic"),
    ("code", "docs-cmd-code"),
    ("link", "docs-cmd-link"),
    ("comment", "docs-cmd-comment"),
];

const HEADER_BTN: &str =
    "inline-flex items-center gap-1 rounded px-2 py-1 text-sm text-gray-600 hover:bg-gray-100";

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
    let notice = use_signal(|| Option::<String>::None);
    // Resolved here (render scope), never inside the mount task.
    let labels = use_hook(|| {
        let map = BLOCKS
            .iter()
            .chain(CHROME)
            .map(|(name, key)| (name.to_string(), t!(key).to_string().into()))
            .collect();
        serde_json::Value::Object(map)
    });

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
        let labels = labels.clone();
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
            let ok = doc_editor::mount(&host, &url, can_write, &name, color, &labels).await;
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
                // Link / comment asked from the selection bubble.
                let asked = s.request.clone();
                if asked.as_ref().map(|r| r.seq) != state.peek().request.as_ref().map(|r| r.seq) {
                    match asked.as_ref().map(|r| r.kind.as_str()) {
                        Some("link") => link_href.set(Some(String::new())),
                        Some("comment") => {
                            if let Some(a) = doc_editor::selection_anchor(&host).await {
                                draft.set(Some(a));
                                show_comments.set(true);
                            }
                        }
                        _ => {}
                    }
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
        move |name: &'static str| {
            let host = host.clone();
            spawn(async move {
                doc_editor::command(&host, name).await;
            });
        }
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
        div { class: "flex min-h-full flex-col bg-white",
            div { class: "flex flex-wrap items-center gap-1 px-3 py-2 text-sm text-gray-600",
                Link {
                    to: Route::Docs { id: String::new() },
                    class: "{HEADER_BTN}",
                    aria_label: t!("docs-back"),
                    icons::ArrowLeft { class: "h-4 w-4" }
                }
                span { class: "min-w-0 truncate px-1 text-gray-800", {title()} }
                div { class: "ml-auto flex flex-wrap items-center gap-1",
                    span { class: "flex items-center gap-1.5 px-2",
                        span { class: "inline-block h-2 w-2 rounded-full {status_dot}" }
                        {status_label}
                    }
                    if s.peers > 0 {
                        span { class: "px-2", {t!("docs-peers", count : s.peers)} }
                    }
                    if !can_write {
                        span { class: "rounded bg-gray-100 px-2 py-0.5 text-xs",
                            {t!("docs-read-only")}
                        }
                    }
                    if can_write {
                        button {
                            class: HEADER_BTN,
                            title: t!("docs-cmd-undo"),
                            aria_label: t!("docs-cmd-undo"),
                            onclick: {
                                let run = run.clone();
                                move |_| run("undo")
                            },
                            icons::Undo { class: "h-4 w-4" }
                        }
                        button {
                            class: HEADER_BTN,
                            title: t!("docs-cmd-redo"),
                            aria_label: t!("docs-cmd-redo"),
                            onclick: {
                                let run = run.clone();
                                move |_| run("redo")
                            },
                            icons::Redo { class: "h-4 w-4" }
                        }
                    }
                    MarkdownActions {
                        host: host.clone(),
                        title: title(),
                        can_write,
                        notice,
                    }
                    button {
                        class: HEADER_BTN,
                        onclick: move |_| show_comments.toggle(),
                        icons::MessageCircle { class: "h-4 w-4" }
                        {t!("docs-comments")}
                    }
                    button {
                        class: HEADER_BTN,
                        onclick: move |_| show_versions.toggle(),
                        icons::History { class: "h-4 w-4" }
                        {t!("docs-versions")}
                    }
                }
            }
            div { class: "flex flex-1 gap-4",
                div { class: "min-w-0 flex-1 px-6 pb-24 pt-8 md:px-24 md:pt-16",
                    div { class: "mx-auto w-full max-w-[900px]",
                        if let Some(msg) = notice() {
                            div { class: "mb-3 rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800",
                                {msg}
                            }
                        }
                        if let Some(href) = link_href() {
                            LinkBar { host: host.clone(), href, link_href }
                        }
                        TitleField { id, title, can_write }
                        if mounted() == Some(false) {
                            div { class: "rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700",
                                {t!("docs-editor-unavailable")}
                            }
                        }
                        div { id: "{host}", class: "relative mt-4" }
                    }
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
            h1 { class: "break-words text-4xl font-bold text-gray-900", {title()} }
        };
    }
    rsx! {
        input {
            class: "w-full bg-transparent text-4xl font-bold text-gray-900 outline-none placeholder:text-gray-300",
            class: if error() { "rounded ring-1 ring-red-400" },
            aria_label: t!("docs-field-title"),
            placeholder: t!("docs-field-title"),
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
        button { class: HEADER_BTN, onclick: export,
            icons::Download { class: "h-4 w-4" }
            {t!("docs-export-md")}
        }
        if can_write {
            label { class: "{HEADER_BTN} cursor-pointer",
                icons::Upload { class: "h-4 w-4" }
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
