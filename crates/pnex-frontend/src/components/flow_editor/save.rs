//! Flow editor save (validate → PATCH) and 409 conflict resolution.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{validate_graph, UpdateFlow};

use super::cx::EditorCx;
use super::{geometry, state};
use crate::api;
use crate::state::session;
use crate::state::toasts;

/// Save state and callback: `(saving, conflict, propose_deploy, save)`.
#[allow(clippy::type_complexity)]
pub(super) fn use_save(
    flow_id: i64,
    cx: EditorCx,
    mut loaded_from: Signal<Option<i64>>,
    mut reload_meta: Signal<u32>,
    on_changed: Callback<()>,
) -> (
    Signal<bool>,
    Signal<Option<String>>,
    Signal<Option<i64>>,
    Callback<Option<String>>,
) {
    let EditorCx {
        mut graph,
        mut saved_graph,
        mut saved_version,
        mut violations,
        ..
    } = cx;
    // --- Save (validate → PATCH) ---
    let mut saving = use_signal(|| false);
    let mut conflict = use_signal(|| None::<String>);
    // Deploy offer after saving an already deployed flow: the running
    // version stays the old one (an update never reloads the
    // engine) — small "Deploy vN?" box rather than an auto-deploy
    // that would purge the debug feed on every iteration (user choice).
    let mut propose_deploy = use_signal(|| None::<i64>);
    // Save (button, unchanged name) and rename (shell title)
    // share the same PATCH — the graph travels with it: renaming means
    // saving the current state.
    let save = Callback::new(move |name: Option<String>| {
        if saving() {
            return;
        }
        let local = validate_graph(&graph.peek().clone());
        if !local.is_empty() {
            violations.set(local);
            return;
        }
        saving.set(true);
        let renamed = name.is_some();
        let params = UpdateFlow {
            expected_version_number: saved_version(),
            graph: graph(),
            name,
            author: session::user().map(|user| user.username),
            note: None,
        };
        spawn(async move {
            match api::flows::update(flow_id, params).await {
                Ok(flow) => {
                    graph.with_mut(|g| state::adopt_stored_secrets(g, &flow.graph));
                    saved_graph.set(flow.graph.clone());
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                    loaded_from.set(None);
                    toasts::success("toast-flow-saved");
                    // Flow already deployed: the running version stays
                    // the old one — offer the deploy (a later edit
                    // hides the box, the render condition depends on dirty).
                    if flow.status == pnex_core::FlowStatus::Deployed {
                        propose_deploy.set(Some(flow.latest_version_number));
                    }
                    // The header title comes from the detail resource.
                    if renamed {
                        reload_meta.with_mut(|r| *r += 1);
                    }
                    on_changed.call(());
                }
                Err(err) => match api::flows::classify_save_error(&err) {
                    api::flows::SaveError::Conflict { description } => {
                        conflict.set(Some(description));
                    }
                    api::flows::SaveError::Invalid(invalid) => violations.set(invalid),
                    api::flows::SaveError::Other(message) => toasts::error(message),
                },
            }
            saving.set(false);
        });
    });
    (saving, conflict, propose_deploy, save)
}

/// 409 resolution: drop the local graph and reload the server one.
pub(super) fn conflict_reload(
    flow_id: i64,
    cx: EditorCx,
    mut conflict: Signal<Option<String>>,
) -> impl FnMut(()) + Copy {
    let EditorCx {
        mut graph,
        mut saved_graph,
        mut saved_version,
        mut violations,
        ..
    } = cx;
    move |_: ()| {
        conflict.set(None);
        spawn(async move {
            match api::flows::detail(flow_id).await {
                Ok(flow) => {
                    let mut fresh = flow.graph.clone();
                    geometry::ensure_positions(&mut fresh);
                    graph.set(fresh.clone());
                    saved_graph.set(fresh);
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                }
                Err(err) => toasts::error(err),
            }
        });
    }
}

/// 409 resolution: overwrite the server version with the local graph.
pub(super) fn conflict_overwrite(
    flow_id: i64,
    cx: EditorCx,
    mut conflict: Signal<Option<String>>,
    on_changed: Callback<()>,
) -> impl FnMut(MouseEvent) {
    let EditorCx {
        mut graph,
        mut saved_graph,
        mut saved_version,
        mut violations,
        ..
    } = cx;
    move |_: MouseEvent| {
        conflict.set(None);
        spawn(async move {
            let Ok(server) = api::flows::detail(flow_id).await else {
                toasts::error(t!("common-error").to_string());
                return;
            };
            let params = UpdateFlow {
                // Fresh server version: deliberate overwrite.
                expected_version_number: server.latest_version_number,
                graph: graph.peek().clone(),
                name: None,
                author: session::user().map(|user| user.username),
                note: None,
            };
            match api::flows::update(flow_id, params).await {
                Ok(flow) => {
                    graph.with_mut(|g| state::adopt_stored_secrets(g, &flow.graph));
                    saved_graph.set(flow.graph.clone());
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                    toasts::success("toast-flow-saved");
                    on_changed.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    }
}
