use super::helpers::*;
use super::*;

/// ─────────────── inject ───────────────

#[component]
pub(super) fn InjectForm(mut cx: EditorCx, initial: InjectConfig, can_write: bool) -> Element {
    let mut repeat = use_signal(move || initial.repeat_secs.map(v_to_string).unwrap_or_default());
    let mut cron = use_signal(move || initial.cron.clone());
    let mut once = use_signal(move || initial.once_delay_secs.map(v_to_string).unwrap_or_default());
    let mut topic = use_signal(move || initial.topic.clone().unwrap_or_default());
    let mut payload = use_signal(move || payload_text(&initial.payload));
    let mut payload_invalid = use_signal(|| false);

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
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-inject-payload")}
                }
                textarea {
                    class: if payload_invalid() { "w-full h-20 px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm font-mono" } else { "w-full h-20 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono" },
                    value: "{payload}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        match serde_json::from_str::<serde_json::Value>(&raw) {
                            Ok(value) => {
                                payload_invalid.set(false);
                                payload.set(raw.clone());
                                patch_selected(
                                    &mut cx,
                                    move |node: &mut FlowNode| {
                                        if let FlowNodeKind::Inject { config } = &mut node.kind {
                                            config.payload = value;
                                        }
                                    },
                                );
                            }
                            Err(_) => {
                                // Saisie intermédiaire invalide : texte conservé,
                                // graphe intact (pattern MetadataEditor).
                                payload_invalid.set(true);
                                payload.set(raw);
                            }
                        }
                    },
                }
                if payload_invalid() {
                    span { class: "text-xs text-red-600 mt-1", {t!("flows-inject-payload-invalid")} }
                }
            }
        }
    }
}
