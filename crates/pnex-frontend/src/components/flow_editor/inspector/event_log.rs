use super::helpers::*;
use super::*;

use pnex_core::events::{event_stream_name, EventLevel, EventLogConfig, EVENT_STREAM_MAX_LEN};

use crate::pages::events::level_label;

// ─────────────── event-log (camera-video.md D84) ───────────────

/// Max message length (mirror of `EventLogConfig::check`).
const MESSAGE_MAX: usize = 500;

/// Event log inspector: stream label with a live preview of the normalized
/// O2 stream name (`ev_…`), level and optional message.
#[component]
pub(super) fn EventLogForm(mut cx: EditorCx, initial: EventLogConfig, can_write: bool) -> Element {
    let mut stream = use_signal(move || initial.stream.clone());
    let mut message = use_signal(move || initial.message.clone());
    let level = initial.level;
    let normalized = event_stream_name(&stream());
    let stream_bad = normalized.is_none();
    let message_len = message.read().chars().count();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-event-log-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-event-log-stream")} }
                input {
                    class: if stream_bad { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                    r#type: "text",
                    value: "{stream}",
                    placeholder: "events",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        stream.set(raw.clone());
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::EventLog { config } = &mut node.kind {
                                config.stream = raw;
                            }
                        });
                    },
                }
                match normalized {
                    Some(name) => rsx! {
                        span { class: "text-xs text-gray-500 mt-1 block",
                            {t!("flows-event-log-stream-preview")}
                            code { class: "ml-1 px-1 rounded bg-gray-100", "{name}" }
                        }
                    },
                    None => rsx! {
                        span { class: "text-xs text-red-600 mt-1 block",
                            {t!("flows-event-log-stream-invalid", max: EVENT_STREAM_MAX_LEN)}
                        }
                    },
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-event-log-level")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        if let Some(next) = EventLevel::from_wire(&event.value()) {
                            patch_selected(&mut cx, move |node: &mut FlowNode| {
                                if let FlowNodeKind::EventLog { config } = &mut node.kind {
                                    config.level = next;
                                }
                            });
                        }
                    },
                    for l in EventLevel::ALL {
                        option { key: "{l.wire()}", value: l.wire(), selected: l == level, {level_label(l)} }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-event-log-message")} }
                textarea {
                    class: if message_len > MESSAGE_MAX { "w-full h-16 px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full h-16 px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                    value: "{message}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        message.set(raw.clone());
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::EventLog { config } = &mut node.kind {
                                config.message = raw;
                            }
                        });
                    },
                }
                span { class: "text-xs text-gray-400 mt-1 block",
                    {t!("flows-event-log-message-hint", count: message_len, max: MESSAGE_MAX)}
                }
            }
        }
    }
}
