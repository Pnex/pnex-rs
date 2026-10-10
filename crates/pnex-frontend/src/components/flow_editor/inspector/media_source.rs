use super::helpers::*;
use super::*;

use pnex_core::media_ingest::{MediaSourceConfig, MediaSourceEmit};

// ─────────────── media-source (media-ingest.md D163) ───────────────

/// Media source inspector: the org's streams as checkboxes, emission mode
/// (segment | sentence) and an optional minimum confidence.
#[component]
pub(super) fn MediaSourceForm(
    mut cx: EditorCx,
    initial: MediaSourceConfig,
    can_write: bool,
) -> Element {
    let streams_res = use_resource(move || async move {
        api::media_streams::list(None, None)
            .await
            .map(|page| page.results)
            .unwrap_or_default()
    });
    let mut picked = use_signal(move || initial.streams.clone());
    let emit = initial.emit;
    let mut conf_raw =
        use_signal(move || initial.min_confidence.map(v_to_string).unwrap_or_default());
    let mut conf_invalid = use_signal(|| false);

    let loaded = streams_res.value().read().is_some();
    // Org streams + configured slugs that are not (or no longer) streams of
    // the org: kept visible so the user unchecks them (the deploy refuses them).
    let mut rows: Vec<(String, String)> = streams_res
        .value()
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.slug, s.name))
        .collect();
    let current = picked();
    for slug in &current {
        if !rows.iter().any(|(s, _)| s == slug) {
            rows.push((slug.clone(), slug.clone()));
        }
    }

    let mut toggle = move |slug: String| {
        let mut next = picked.peek().clone();
        if let Some(pos) = next.iter().position(|s| *s == slug) {
            next.remove(pos);
        } else {
            next.push(slug);
        }
        picked.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::MediaSource { config } = &mut node.kind {
                config.streams = next;
            }
        });
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-media-source-help")} }
            div {
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-media-source-streams")}
                }
                if loaded && rows.is_empty() {
                    p { class: "text-xs text-amber-700", {t!("flows-media-source-no-stream")} }
                }
                ul { class: "space-y-1",
                    for (slug, name) in rows {
                        li { key: "{slug}",
                            label { class: "flex items-center gap-2 text-sm",
                                input {
                                    r#type: "checkbox",
                                    checked: current.contains(&slug),
                                    disabled: !can_write,
                                    onchange: {
                                        let slug = slug.clone();
                                        move |_| toggle(slug.clone())
                                    },
                                }
                                span { "{name}" }
                                span { class: "text-xs text-gray-400", "{slug}" }
                            }
                        }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-media-source-emit")}
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = if event.value() == "sentence" {
                            MediaSourceEmit::Sentence
                        } else {
                            MediaSourceEmit::Segment
                        };
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::MediaSource { config } = &mut node.kind {
                                    config.emit = next;
                                }
                            },
                        );
                    },
                    option {
                        value: "segment",
                        selected: emit == MediaSourceEmit::Segment,
                        {t!("flows-media-source-emit-segment")}
                    }
                    option {
                        value: "sentence",
                        selected: emit == MediaSourceEmit::Sentence,
                        {t!("flows-media-source-emit-sentence")}
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-media-source-min-confidence")}
                }
                input {
                    class: if conf_invalid() { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                    r#type: "number",
                    min: "0",
                    max: "1",
                    step: "0.05",
                    value: "{conf_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        conf_raw.set(raw.clone());
                        // Empty = no threshold.
                        let parsed = if raw.trim().is_empty() {
                            Some(None)
                        } else {
                            parse_secs(&raw).filter(|v| (0.0..=1.0).contains(v)).map(Some)
                        };
                        conf_invalid.set(parsed.is_none());
                        if let Some(v) = parsed {
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::MediaSource { config } = &mut node.kind {
                                        config.min_confidence = v;
                                    }
                                },
                            );
                        }
                    },
                }
                span { class: "text-xs text-gray-400 mt-1 block",
                    {t!("flows-media-source-min-confidence-hint")}
                }
            }
        }
    }
}
