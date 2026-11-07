//! Flow editor loading: initial graph adoption and canvas subtitle caches.

use dioxus::prelude::*;
use pnex_core::{Flow, FlowGraph, FlowViolation};

use super::{geometry, state};
use crate::api;
use crate::api::error::ApiError;

/// Fills the caches behind the canvas subtitles, once per editor mount.
pub(super) fn use_subtitle_caches() {
    // Vision model names for the `vision-detect` canvas subtitles — one
    // fetch per editor mount (scoped task, render-safe global write).
    use_hook(|| {
        spawn(async move {
            if let Ok(page) = api::ml_models::list(Some(100), None).await {
                crate::state::vision::remember(&page.results);
            }
            // Control keys for the `control-source` subtitles (O30: the raw
            // id showed until the inspector was opened); `list` fills the
            // key cache.
            let _ = api::controls::list().await;
        });
    });
}

/// Adopts the flow detail into the editor signals the first time it loads.
pub(super) fn use_initial_load(
    detail: Resource<Result<Flow, ApiError>>,
    mut graph: Signal<FlowGraph>,
    mut saved_graph: Signal<FlowGraph>,
    mut saved_version: Signal<i64>,
    mut violations: Signal<Vec<FlowViolation>>,
) {
    // Initial load guard (never a `.set()` during render).
    let mut loaded = use_signal(|| false);
    use_effect(move || {
        let resource = detail.value();
        let value = resource.read();
        let Some(Ok(flow)) = &*value else {
            return;
        };
        if loaded() {
            return;
        }
        let mut fresh = flow.graph.clone();
        geometry::ensure_positions(&mut fresh);
        // Auto tracking from load: a split in auto mode regenerates its
        // keys (hence its ports) from its upstream without waiting for an edit.
        state::sync_split_keys_auto(&mut fresh);
        graph.set(fresh.clone());
        saved_graph.set(fresh);
        saved_version.set(flow.latest_version_number);
        // Stored nodes the server left out (retired kind…): shown until saved.
        violations.set(flow.unreadable_nodes.clone());
        loaded.set(true);
    });
}
