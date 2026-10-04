//! Panneau assistant IA — bouton flottant global (toutes pages, mode
//! authentifié) + drawer de chat. La trace d'outils est repliable ; une
//! carte « Ouvrir dans l'éditeur » apparaît quand un outil a touché un
//! flow (deep-link via `state::flows::OPEN_FLOW`).
//!
//! Conversations (D145): private to the user within the current org,
//! stored by the server. The drawer lists them (new, resume, rename,
//! delete, delete all, export); only the new message is sent, the server
//! rebuilds the history.
//!
//! Monté dans `pages/shell.rs` (branche Authenticated), à côté du
//! ToastContainer. Invisible tant que le statut n'est pas hydraté ou que
//! l'assistant est désactivé/non configuré (kill-switch env).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{AiConversation, AiMessage, AiPageContext, AiSendMessage, AiToolTrace};
use uuid::Uuid;

use crate::api;
use crate::state::flows::OPEN_FLOW;
use crate::state::{ai, toasts};

/// Conversation shown by the drawer, kept across open/close (reset when
/// the org changes: a conversation belongs to one org).
static CURRENT: GlobalSignal<Option<Uuid>> = Signal::global(|| None);

#[derive(Clone, Debug, PartialEq)]
enum ChatBubble {
    User(String),
    Assistant {
        text: String,
        trace: Vec<AiToolTrace>,
    },
}

fn bubbles_of(messages: Vec<AiMessage>) -> Vec<ChatBubble> {
    messages
        .into_iter()
        .map(|m| {
            if m.role == "user" {
                ChatBubble::User(m.content)
            } else {
                ChatBubble::Assistant {
                    text: m.content,
                    trace: m.tool_trace,
                }
            }
        })
        .collect()
}

#[component]
pub fn AssistantPanel() -> Element {
    let mut open = use_signal(|| false);
    // Hydratation du statut : au montage, puis à CHAQUE changement d'org
    // (la config IA est par org — un statut périmé masquerait/afficherait
    // le panneau à tort). L'org est lu dans le scope réactif de l'effet.
    let mut hydrated_org = use_signal(|| Option::<i64>::None);
    use_effect(move || {
        let org_id = crate::state::org::current();
        if hydrated_org() == org_id {
            return;
        }
        hydrated_org.set(org_id);
        *CURRENT.write() = None;
        spawn(async move {
            ai::refresh_status().await;
        });
    });

    if !ai::visible() {
        return rsx! {};
    }
    rsx! {
        div {
            button {
                class: "fixed bottom-6 right-6 z-30 rounded-full h-12 w-12 bg-blue-600 text-white shadow-lg hover:bg-blue-700 text-xl leading-none",
                onclick: move |_| open.set(true),
                aria_label: t!("ai-title"),
                {"💬"}
            }
            if open() {
                AssistantDrawer { on_close: move |_| open.set(false) }
            }
        }
    }
}

#[component]
fn AssistantDrawer(on_close: Callback) -> Element {
    let mut messages: Signal<Vec<ChatBubble>> = use_signal(Vec::new);
    let mut input = use_signal(String::new);
    let mut thinking = use_signal(|| false);
    let mut show_list = use_signal(|| false);

    // Resume the current conversation when the drawer opens.
    use_effect(move || {
        let Some(id) = CURRENT() else {
            return;
        };
        spawn(async move {
            match api::ai::conversation(id).await {
                Ok(detail) => messages.set(bubbles_of(detail.messages)),
                Err(_) => *CURRENT.write() = None,
            }
        });
    });

    let mut send = move |text: String| {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() || thinking() {
            return;
        }
        messages.with_mut(|m| m.push(ChatBubble::User(trimmed.clone())));
        input.set(String::new());
        thinking.set(true);
        // Current page path: the server injects its knowledge card (D142).
        let page = AiPageContext {
            page: Some(router().current::<crate::app::Route>().to_string()),
            ..Default::default()
        };
        spawn(async move {
            let id = match CURRENT() {
                Some(id) => Ok(id),
                None => api::ai::create_conversation().await.map(|c| c.id),
            };
            let result = match id {
                Ok(id) => {
                    *CURRENT.write() = Some(id);
                    let msg = AiSendMessage {
                        content: trimmed,
                        language: Some(crate::i18n::current_tag()),
                        page: Some(page),
                    };
                    api::ai::send_message(id, msg).await
                }
                Err(err) => Err(err),
            };
            match result {
                Ok(resp) => messages.with_mut(|m| {
                    m.push(ChatBubble::Assistant {
                        text: resp.answer,
                        trace: resp.tool_trace,
                    })
                }),
                Err(err) => toasts::error(err),
            }
            thinking.set(false);
        });
    };

    let new_conversation = move |_| {
        *CURRENT.write() = None;
        messages.set(Vec::new());
        show_list.set(false);
    };
    let close = move |_| on_close.call(());
    let bubbles = messages().clone();

    rsx! {
        div { class: "fixed inset-0 z-40",
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                header { class: "flex items-center justify-between gap-2 px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900 flex-1", {t!("ai-title")} }
                    button {
                        class: "text-xs text-gray-600 hover:text-gray-900",
                        onclick: move |_| show_list.set(!show_list()),
                        {t!("ai-conversations")}
                    }
                    button {
                        class: "text-xs text-blue-600 hover:text-blue-800",
                        onclick: new_conversation,
                        {t!("ai-new-conversation")}
                    }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: close,
                        {"✕"}
                    }
                }
                if show_list() {
                    ConversationList {
                        on_open: move |id: Uuid| {
                            *CURRENT.write() = Some(id);
                            show_list.set(false);
                        },
                        on_cleared: move |_| {
                            *CURRENT.write() = None;
                            messages.set(Vec::new());
                        },
                    }
                } else {
                    div { class: "flex-1 overflow-y-auto px-4 py-3 space-y-3",
                        if !ai::configured() {
                            p { class: "text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded-md px-3 py-2",
                                {t!("ai-not-configured")}
                            }
                        }
                        if bubbles.is_empty() && ai::configured() {
                            p { class: "text-sm text-gray-400", {t!("ai-empty")} }
                        }
                        for (idx, bubble) in bubbles.into_iter().enumerate() {
                            Bubble { key: "{idx}", bubble }
                        }
                        if thinking() {
                            p { class: "text-sm text-gray-400 italic", {t!("ai-thinking")} }
                            span { class: "animate-spin rounded-full h-4 w-4 border-b-2 border-blue-600" }
                        }
                    }
                    footer { class: "border-t border-gray-200 p-3",
                        textarea {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            rows: "2",
                            value: "{input}",
                            placeholder: t!("ai-placeholder"),
                            oninput: move |event| input.set(event.value()),
                            onkeydown: move |event: Event<KeyboardData>| {
                                if event.key() == Key::Enter {
                                    event.prevent_default();
                                    let text = input();
                                    send(text);
                                }
                            },
                        }
                        div { class: "flex items-center justify-between gap-2 mt-2",
                            p { class: "text-[11px] text-gray-400 leading-tight",
                                {t!("ai-provider-notice")}
                            }
                            button {
                                class: "px-3 py-1.5 rounded-md bg-blue-600 text-white text-sm hover:bg-blue-700 disabled:opacity-50",
                                disabled: thinking(),
                                onclick: move |_| {
                                    let text = input();
                                    send(text);
                                },
                                {t!("ai-send")}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Conversation list of the drawer: resume, rename, delete, delete all,
/// export.
#[component]
fn ConversationList(on_open: Callback<Uuid>, on_cleared: Callback<()>) -> Element {
    let mut version = use_signal(|| 0u32);
    let rows = use_resource(move || {
        let _ = version();
        async move { api::ai::conversations().await }
    });
    let mut renaming = use_signal(|| Option::<(Uuid, String)>::None);
    let mut to_delete = use_signal(|| Option::<Uuid>::None);
    let mut confirm_all = use_signal(|| false);

    let export = move |_| {
        spawn(async move {
            match api::ai::export_conversations().await {
                Ok(json) => {
                    let bytes = serde_json::to_vec_pretty(&json).unwrap_or_default();
                    crate::util::trigger_download(
                        "pnex-assistant-conversations.json",
                        "application/json",
                        &bytes,
                    );
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    let list: Vec<AiConversation> = match &*rows.read() {
        Some(Ok(page)) => page.results.clone(),
        _ => Vec::new(),
    };
    let loading = rows.read().is_none();

    rsx! {
        div { class: "flex-1 overflow-y-auto px-2 py-2 space-y-1",
            if loading {
                p { class: "text-sm text-gray-400 px-2", {t!("ai-thinking")} }
            } else if list.is_empty() {
                p { class: "text-sm text-gray-400 px-2", {t!("ai-no-conversations")} }
            }
            for conv in list {
                ConversationRow {
                    key: "{conv.id}",
                    conv: conv.clone(),
                    editing: renaming().filter(|(id, _)| *id == conv.id).map(|(_, t)| t),
                    on_open,
                    on_edit: move |(id, title): (Uuid, String)| renaming.set(Some((id, title))),
                    on_edit_input: move |text: String| {
                        renaming
                            .with_mut(|r| {
                                if let Some((_, t)) = r {
                                    *t = text;
                                }
                            })
                    },
                    on_edit_commit: move |_| {
                        let Some((id, title)) = renaming() else {
                            return;
                        };
                        renaming.set(None);
                        spawn(async move {
                            if let Err(err) = api::ai::rename_conversation(id, title).await {
                                toasts::error(err);
                            }
                            version += 1;
                        });
                    },
                    on_delete: move |id: Uuid| to_delete.set(Some(id)),
                }
            }
        }
        footer { class: "border-t border-gray-200 p-3 space-y-2",
            p { class: "text-[11px] text-gray-400 leading-tight", {t!("ai-history-notice")} }
            div { class: "flex justify-between",
                button {
                    class: "text-xs text-gray-600 hover:text-gray-900",
                    onclick: export,
                    {t!("ai-export")}
                }
                button {
                    class: "text-xs text-red-600 hover:text-red-800",
                    onclick: move |_| confirm_all.set(true),
                    {t!("ai-delete-all")}
                }
            }
        }
        if let Some(id) = to_delete() {
            crate::components::confirm::ConfirmDialog {
                title: t!("ai-delete-title"),
                message: t!("ai-delete-message"),
                confirm_label: t!("ai-delete"),
                on_confirm: move |_| {
                    to_delete.set(None);
                    spawn(async move {
                        match api::ai::delete_conversation(id).await {
                            Ok(()) => {
                                if CURRENT() == Some(id) {
                                    on_cleared.call(());
                                }
                            }
                            Err(err) => toasts::error(err),
                        }
                        version += 1;
                    });
                },
                on_cancel: move |_| to_delete.set(None),
            }
        }
        if confirm_all() {
            crate::components::confirm::ConfirmDialog {
                title: t!("ai-delete-all-title"),
                message: t!("ai-delete-all-message"),
                confirm_label: t!("ai-delete-all"),
                on_confirm: move |_| {
                    confirm_all.set(false);
                    spawn(async move {
                        match api::ai::delete_all_conversations().await {
                            Ok(()) => on_cleared.call(()),
                            Err(err) => toasts::error(err),
                        }
                        version += 1;
                    });
                },
                on_cancel: move |_| confirm_all.set(false),
            }
        }
    }
}

#[component]
fn ConversationRow(
    conv: AiConversation,
    editing: Option<String>,
    on_open: Callback<Uuid>,
    on_edit: Callback<(Uuid, String)>,
    on_edit_input: Callback<String>,
    on_edit_commit: Callback<()>,
    on_delete: Callback<Uuid>,
) -> Element {
    let id = conv.id;
    let title = if conv.title.trim().is_empty() {
        t!("ai-untitled")
    } else {
        conv.title.clone()
    };
    let date: String = conv.last_message_at.chars().take(10).collect();
    let current = CURRENT() == Some(id);
    rsx! {
        div { class: if current { "flex items-center gap-2 rounded-md px-2 py-1.5 bg-blue-50" } else { "flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-gray-50" },
            if let Some(text) = editing {
                input {
                    class: "flex-1 border border-gray-300 rounded px-2 py-1 text-sm",
                    value: "{text}",
                    oninput: move |e| on_edit_input.call(e.value()),
                    onkeydown: move |e: Event<KeyboardData>| {
                        if e.key() == Key::Enter {
                            on_edit_commit.call(());
                        }
                    },
                    onblur: move |_| on_edit_commit.call(()),
                }
            } else {
                button {
                    class: "flex-1 min-w-0 text-left",
                    onclick: move |_| on_open.call(id),
                    p { class: "text-sm text-gray-900 truncate", {title.clone()} }
                    p { class: "text-[11px] text-gray-400", {date} }
                }
                button {
                    class: "text-xs text-gray-400 hover:text-gray-700",
                    aria_label: t!("ai-rename"),
                    onclick: move |_| on_edit.call((id, title.clone())),
                    {"✎"}
                }
                button {
                    class: "text-xs text-gray-400 hover:text-red-600",
                    aria_label: t!("ai-delete"),
                    onclick: move |_| on_delete.call(id),
                    {"🗑"}
                }
            }
        }
    }
}

#[component]
fn Bubble(bubble: ChatBubble) -> Element {
    let navigator = use_navigator();
    match bubble {
        ChatBubble::User(text) => rsx! {
            div { class: "flex justify-end",
                div { class: "max-w-[80%] rounded-lg bg-blue-600 text-white px-3 py-2 text-sm whitespace-pre-wrap",
                    {text}
                }
            }
        },
        ChatBubble::Assistant { text, trace } => {
            // Entries touchant un flow, clonées (closures 'static dans rsx).
            let flow_entries: Vec<AiToolTrace> = trace
                .iter()
                .filter(|e| e.ok && e.flow_id.is_some())
                .cloned()
                .collect();
            rsx! {
                div { class: "flex justify-start",
                    div { class: "max-w-[90%] space-y-2",
                        if !trace.is_empty() {
                            details { class: "text-xs text-gray-600 border border-gray-200 rounded-md",
                                summary { class: "px-2 py-1 cursor-pointer select-none",
                                    {t!("ai-tool-trace")}
                                }
                                ul { class: "px-2 py-1 space-y-0.5",
                                    for entry in trace.iter() {
                                        li { class: "flex items-start gap-1",
                                            span { class: if entry.ok { "text-green-600" } else { "text-red-600" },
                                                {if entry.ok { "✓" } else { "✗" }}
                                            }
                                            span { class: "font-mono", {entry.name.clone()} }
                                            span { class: "text-gray-500",
                                                {
                                                    crate::api::error_i18n::localize_tool_summary(
                                                        entry.code.as_deref(),
                                                        entry.args.as_ref(),
                                                        &entry.summary,
                                                    )
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if !text.is_empty() {
                            if super::markdown::looks_like_markdown(&text) {
                                div {
                                    class: "ai-md rounded-lg bg-gray-100 text-gray-900 px-3 py-2 text-sm",
                                    dangerous_inner_html: super::markdown::to_html(&text),
                                }
                            } else {
                                div { class: "rounded-lg bg-gray-100 text-gray-900 px-3 py-2 text-sm whitespace-pre-wrap",
                                    {text}
                                }
                            }
                        }
                        // Carte flow : l'outil a touché un flow → deep link.
                        for entry in flow_entries {
                            button {
                                key: "{entry.flow_id:?}",
                                class: "px-3 py-1.5 rounded-md border border-blue-600 text-blue-600 text-sm hover:bg-blue-50",
                                onclick: move |_| {
                                    if let Some(flow_id) = entry.flow_id {
                                        OPEN_FLOW.with_mut(|f| *f = Some(flow_id));
                                        navigator.push(crate::app::Route::Flows {});
                                    }
                                },
                                {t!("ai-open-in-editor")}
                            }
                        }
                    }
                }
            }
        }
    }
}
