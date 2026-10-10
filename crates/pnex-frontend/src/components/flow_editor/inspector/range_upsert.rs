use super::helpers::*;
use super::*;

use crate::pages::streams::origin_label;
use pnex_core::time_range::{RangeOrigin, RangeUpsertConfig, ScopeKind};

// ─────────────── range-upsert (media-ingest.md D169) ───────────────

/// Time range writer inspector: scope (a stream of the org, or the org)
/// and the origin written on each range.
#[component]
pub(super) fn RangeUpsertForm(
    mut cx: EditorCx,
    initial: RangeUpsertConfig,
    can_write: bool,
) -> Element {
    let streams_res = use_resource(move || async move {
        api::media_streams::list(None, None)
            .await
            .map(|page| page.results)
            .unwrap_or_default()
    });
    let mut config = use_signal(move || initial.clone());
    let streams = streams_res.value().read().clone().unwrap_or_default();
    let loaded = streams_res.value().read().is_some();
    let current = config();
    let on_stream = current.scope_kind == ScopeKind::Stream.wire();
    let unknown = loaded
        && on_stream
        && !current.scope_id.is_empty()
        && !streams.iter().any(|s| s.id == current.scope_id);

    let mut set = move |next: RangeUpsertConfig| {
        config.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::RangeUpsert { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let on_scope = move |event: Event<FormData>| {
        let mut next = config.peek().clone();
        next.scope_kind = event.value();
        next.scope_id.clear();
        set(next);
    };
    let on_stream_pick = move |event: Event<FormData>| {
        let mut next = config.peek().clone();
        next.scope_id = event.value();
        set(next);
    };
    let on_origin = move |event: Event<FormData>| {
        let mut next = config.peek().clone();
        next.origin = event.value();
        set(next);
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-range-upsert-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("ranges-scope")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: on_scope,
                    option { value: ScopeKind::Stream.wire(), selected: on_stream,
                        {t!("ranges-scope-stream")}
                    }
                    option { value: ScopeKind::Org.wire(), selected: !on_stream,
                        {t!("ranges-scope-org")}
                    }
                }
            }
            if on_stream {
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block",
                        {t!("ranges-stream")}
                    }
                    if loaded && streams.is_empty() {
                        p { class: "text-xs text-amber-700", {t!("flows-media-source-no-stream")} }
                    }
                    select {
                        class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        disabled: !can_write,
                        onchange: on_stream_pick,
                        option { value: "", selected: current.scope_id.is_empty(), "—" }
                        for s in streams.iter() {
                            option {
                                key: "{s.id}",
                                value: "{s.id}",
                                selected: s.id == current.scope_id,
                                "{s.name}"
                            }
                        }
                    }
                    if unknown {
                        p { class: "text-xs text-red-700 mt-1", {t!("flows-range-upsert-unknown")} }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("ranges-col-origin")}
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: on_origin,
                    for o in RangeOrigin::FLOW {
                        option {
                            key: "{o.wire()}",
                            value: o.wire(),
                            selected: current.origin == o.wire(),
                            {origin_label(o)}
                        }
                    }
                }
            }
        }
    }
}
