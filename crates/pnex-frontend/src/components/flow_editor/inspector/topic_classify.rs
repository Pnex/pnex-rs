use super::helpers::*;
use super::*;

use pnex_core::taxonomy::TopicClassifyConfig;

// ─────────────── topic-classify (media-ingest.md D168) ───────────────

/// Topic classifier inspector: taxonomy of the org, its version pinned at
/// pick time (a newer one is offered, never followed silently), text path.
#[component]
pub(super) fn TopicClassifyForm(
    mut cx: EditorCx,
    initial: TopicClassifyConfig,
    can_write: bool,
) -> Element {
    let list =
        use_resource(move || async move { api::taxonomies::list().await.unwrap_or_default() });
    let mut picked = use_signal(move || (initial.taxonomy_id.clone(), initial.version));
    let mut text_field = use_signal(move || initial.text_field.clone());

    let taxonomies = list.value().read().clone().unwrap_or_default();
    let loaded = list.value().read().is_some();
    let (picked_id, pinned) = picked();
    let current = taxonomies
        .iter()
        .find(|t| t.id == picked_id)
        .map(|t| t.current_version);
    let unknown = loaded && !picked_id.is_empty() && current.is_none();

    let mut pin = move |id: String, version: i32| {
        picked.set((id.clone(), version));
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::TopicClassify { config } = &mut node.kind {
                config.taxonomy_id = id;
                config.version = version;
            }
        });
    };
    let options = taxonomies.clone();
    let on_pick = move |event: Event<FormData>| {
        let id = event.value();
        let version = options
            .iter()
            .find(|t| t.id == id)
            .map_or(0, |t| t.current_version);
        pin(id, version);
    };
    let pinned_label = pinned.to_string();
    let newer = current.filter(|c| *c > pinned);
    let pin_id = picked_id.clone();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-topic-classify-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-topic-classify-taxonomy")}
                }
                if loaded && taxonomies.is_empty() {
                    p { class: "text-xs text-amber-700", {t!("flows-topic-classify-no-taxonomy")} }
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: on_pick,
                    option { value: "", selected: picked_id.is_empty(), "—" }
                    for tx in taxonomies.iter() {
                        option {
                            key: "{tx.id}",
                            value: "{tx.id}",
                            selected: tx.id == picked_id,
                            "{tx.name}"
                        }
                    }
                }
                if unknown {
                    p { class: "text-xs text-red-700 mt-1", {t!("flows-topic-classify-unknown")} }
                }
            }
            if !picked_id.is_empty() && !unknown {
                div { class: "text-sm text-gray-700",
                    if pinned > 0 {
                        {t!("flows-topic-classify-pinned", version : pinned_label)}
                    } else {
                        span { class: "text-xs text-amber-700",
                            {t!("flows-topic-classify-no-version")}
                        }
                    }
                    if let Some(latest) = newer {
                        button {
                            class: "ml-2 px-2 py-0.5 text-xs border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                            r#type: "button",
                            disabled: !can_write,
                            onclick: move |_| pin(pin_id.clone(), latest),
                            {t!("flows-topic-classify-use-latest", version : latest.to_string())}
                        }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-topic-classify-text-field")}
                }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                    r#type: "text",
                    value: "{text_field}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        text_field.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::TopicClassify { config } = &mut node.kind {
                                    config.text_field = raw;
                                }
                            },
                        );
                    },
                }
                span { class: "text-xs text-gray-400 mt-1 block",
                    {t!("flows-topic-classify-text-field-hint")}
                }
            }
        }
    }
}
