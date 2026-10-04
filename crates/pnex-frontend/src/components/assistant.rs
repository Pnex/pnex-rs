//! Panneau assistant IA — bouton flottant global (toutes pages, mode
//! authentifié) + drawer de chat. La trace d'outils est repliable ; une
//! carte « Ouvrir dans l'éditeur » apparaît quand un outil a touché un
//! flow (deep-link via `state::flows::OPEN_FLOW`).
//!
//! Monté dans `pages/shell.rs` (branche Authenticated), à côté du
//! ToastContainer. Invisible tant que le statut n'est pas hydraté ou que
//! l'assistant est désactivé/non configuré (kill-switch env).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{AiChatMessage, AiChatRequest, AiToolTrace};

use crate::api;
use crate::state::flows::OPEN_FLOW;
use crate::state::{ai, toasts};

#[derive(Clone, Debug, PartialEq)]
enum ChatBubble {
    User(String),
    Assistant {
        text: String,
        trace: Vec<AiToolTrace>,
    },
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

    let mut send = move |text: String| {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() || thinking() {
            return;
        }
        // Historique = texte des bulles précédentes + le message courant
        // (les erreurs ne sont pas renvoyées au serveur).
        let mut history: Vec<AiChatMessage> = messages()
            .iter()
            .map(|bubble| match bubble {
                ChatBubble::User(text) => AiChatMessage {
                    role: "user".into(),
                    content: text.clone(),
                },
                ChatBubble::Assistant { text, .. } => AiChatMessage {
                    role: "assistant".into(),
                    content: text.clone(),
                },
            })
            .collect();
        history.push(AiChatMessage {
            role: "user".into(),
            content: trimmed.clone(),
        });
        messages.with_mut(|m| m.push(ChatBubble::User(trimmed.clone())));
        input.set(String::new());
        thinking.set(true);
        spawn(async move {
            let language = crate::i18n::current_tag();
            let req = AiChatRequest {
                messages: history,
                language: Some(language),
                page: None,
            };
            match api::ai::chat(req).await {
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

    let close = move |_| on_close.call(());
    let bubbles = messages().clone();

    rsx! {
        div { class: "fixed inset-0 z-40",
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                header { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("ai-title")} }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: close,
                        {"✕"}
                    }
                }
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
                    div { class: "flex justify-end mt-2",
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
