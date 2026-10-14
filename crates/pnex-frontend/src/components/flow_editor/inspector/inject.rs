use super::helpers::*;
use super::*;
use crate::components::ui::FIELD_LABEL;

/// ─────────────── inject ───────────────

#[component]
pub(super) fn InjectForm(mut cx: EditorCx, initial: InjectConfig, can_write: bool) -> Element {
    let mut repeat = use_signal(move || initial.repeat_secs.map(v_to_string).unwrap_or_default());
    let mut cron = use_signal(move || initial.cron.clone());
    let mut once = use_signal(move || initial.once_delay_secs.map(v_to_string).unwrap_or_default());
    let mut topic = use_signal(move || initial.topic.clone().unwrap_or_default());
    let initial_payload = initial.payload.clone();

    rsx! {
        div { class: "space-y-3",
            {
                text_field(
                    t!("flows-inject-repeat"),
                    repeat,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        repeat.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Inject { config } = &mut node.kind {
                                    config.repeat_secs = parse_secs(&raw);
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-inject-cron"),
                    cron,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        cron.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Inject { config } = &mut node.kind {
                                    config.cron = raw;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-inject-once-delay"),
                    once,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        once.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Inject { config } = &mut node.kind {
                                    config.once_delay_secs = parse_secs(&raw);
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-inject-topic"),
                    topic,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        topic.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Inject { config } = &mut node.kind {
                                    config.topic = Some(raw).filter(|t| !t.is_empty());
                                }
                            },
                        );
                    },
                )
            }
            InjectPayloadField { cx, initial: initial_payload, can_write }
        }
    }
}

/// What the inject sends, picked by type (O70): the raw JSON stays one
/// choice among others, never the default way in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PayloadMode {
    Timestamp,
    Number,
    Bool,
    Text,
    Json,
}

impl PayloadMode {
    const ALL: [PayloadMode; 5] = [
        PayloadMode::Timestamp,
        PayloadMode::Number,
        PayloadMode::Bool,
        PayloadMode::Text,
        PayloadMode::Json,
    ];

    fn of(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Timestamp,
            serde_json::Value::Number(_) => Self::Number,
            serde_json::Value::Bool(_) => Self::Bool,
            serde_json::Value::String(_) => Self::Text,
            _ => Self::Json,
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::Timestamp => "timestamp",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::Text => "text",
            Self::Json => "json",
        }
    }

    fn from_wire(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.wire() == raw)
    }

    fn label(self) -> String {
        match self {
            Self::Timestamp => t!("flows-inject-kind-timestamp").to_string(),
            Self::Number => t!("flows-inject-kind-number").to_string(),
            Self::Bool => t!("flows-inject-kind-bool").to_string(),
            Self::Text => t!("flows-inject-kind-text").to_string(),
            Self::Json => t!("flows-inject-kind-json").to_string(),
        }
    }

    /// Value set when the user switches to this mode.
    fn default_value(self) -> serde_json::Value {
        match self {
            Self::Timestamp => serde_json::Value::Null,
            Self::Number => serde_json::json!(1),
            Self::Bool => serde_json::json!(true),
            Self::Text => serde_json::json!(""),
            Self::Json => serde_json::json!({}),
        }
    }
}

/// Edit text of a payload in its mode (a text payload is shown unquoted).
fn mode_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => payload_text(other),
    }
}

fn set_payload(cx: &mut EditorCx, value: serde_json::Value) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::Inject { config } = &mut node.kind {
            config.payload = value;
        }
    });
}

#[component]
fn InjectPayloadField(mut cx: EditorCx, initial: serde_json::Value, can_write: bool) -> Element {
    let mut mode = use_signal(|| PayloadMode::of(&initial));
    let mut text = use_signal(move || mode_text(&initial));
    let mut invalid = use_signal(|| false);
    let field_class = if invalid() {
        "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm font-mono"
    } else {
        "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono"
    };

    rsx! {
        label { class: "block",
            span { class: FIELD_LABEL, {t!("flows-inject-payload")} }
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled: !can_write,
                onchange: move |event| {
                    if let Some(next) = PayloadMode::from_wire(&event.value()) {
                        let value = next.default_value();
                        mode.set(next);
                        text.set(mode_text(&value));
                        invalid.set(false);
                        set_payload(&mut cx, value);
                    }
                },
                for m in PayloadMode::ALL {
                    option { value: m.wire(), selected: mode() == m, {m.label()} }
                }
            }
        }
        if mode() == PayloadMode::Timestamp {
            p { class: "text-xs text-gray-500", {t!("flows-inject-kind-timestamp-help")} }
        }
        if mode() == PayloadMode::Bool {
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled: !can_write,
                onchange: move |event| {
                    let on = event.value() == "true";
                    text.set(on.to_string());
                    set_payload(&mut cx, serde_json::json!(on));
                },
                option { value: "true", selected: text() == "true", {t!("flows-inject-bool-true")} }
                option { value: "false", selected: text() == "false", {t!("flows-inject-bool-false")} }
            }
        }
        if mode() == PayloadMode::Number {
            input {
                class: field_class,
                r#type: "number",
                step: "any",
                value: "{text}",
                disabled: !can_write,
                oninput: move |event| {
                    let raw = event.value();
                    text.set(raw.clone());
                    let parsed = raw
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .and_then(serde_json::Number::from_f64);
                    invalid.set(parsed.is_none());
                    if let Some(n) = parsed {
                        set_payload(&mut cx, serde_json::Value::Number(n));
                    }
                },
            }
        }
        if mode() == PayloadMode::Text {
            input {
                class: field_class,
                r#type: "text",
                value: "{text}",
                disabled: !can_write,
                oninput: move |event| {
                    let raw = event.value();
                    text.set(raw.clone());
                    set_payload(&mut cx, serde_json::Value::String(raw));
                },
            }
        }
        if mode() == PayloadMode::Json {
            textarea {
                class: "{field_class} h-20",
                value: "{text}",
                disabled: !can_write,
                oninput: move |event| {
                    let raw = event.value();
                    text.set(raw.clone());
                    // Intermediate invalid input: text kept, graph intact.
                    match serde_json::from_str::<serde_json::Value>(&raw) {
                        Ok(value) => {
                            invalid.set(false);
                            set_payload(&mut cx, value);
                        }
                        Err(_) => invalid.set(true),
                    }
                },
            }
        }
        if invalid() {
            span { class: "text-xs text-red-600", {t!("flows-inject-payload-invalid")} }
        }
    }
}
