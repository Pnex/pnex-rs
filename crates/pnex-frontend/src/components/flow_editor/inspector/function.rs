use super::device::patch_selected_function_and_rewire;
use super::*;

/// Form of the `PnexFunction` node: function picker (fills the snapshot:
/// id/name/current version/language/inputs/outputs) and client-only
/// staleness ("newer version available") with adoption of the current
/// version. Declared inputs/outputs are rendered **on the canvas**
/// (labelled ports, one wire per input) — no more bindings: at run time
/// each input implicitly reads `payload.<name>`.
///
/// Remounted by `key` (function + version): the form restarts from the
/// config — same mechanism as the other forms.
#[component]
pub(super) fn FunctionForm(
    mut cx: EditorCx,
    initial: FunctionNodeConfig,
    can_write: bool,
) -> Element {
    // Detail of the pinned function (single fetch on mount — the component
    // is remounted by key on every function/version change): displayed
    // name + staleness detection.
    let pinned_id = initial.function_id;
    let pinned_version = initial.version_number;
    let detail = use_resource(move || async move {
        if pinned_id == 0 {
            None
        } else {
            api::functions::detail(pinned_id).await.ok()
        }
    });
    let detail_value = detail.read().clone().flatten();

    // Version history (pin picker: rollback **and** forward) — each entry
    // carries the version's interface snapshot.
    let versions = use_resource(move || async move {
        if pinned_id == 0 {
            Vec::new()
        } else {
            api::functions::versions_all(pinned_id)
                .await
                .unwrap_or_default()
        }
    });
    let versions_value: Vec<pnex_core::FunctionVersionSummary> =
        versions.read().clone().unwrap_or_default();

    // Pinning any version (go back to an old version that worked, or move
    // forward): the version's interface snapshot is set, wires follow the
    // names (rewire/prune as on pick).
    let mut pin_version = {
        move |summary: pnex_core::FunctionVersionSummary| {
            patch_selected_function_and_rewire(&mut cx, move |node: &mut FlowNode| {
                if let FlowNodeKind::PnexFunction { config } = &mut node.kind {
                    config.version_number = summary.version_number;
                    config.inputs = summary.inputs;
                    config.outputs = summary.outputs;
                }
            });
        }
    };

    // Adopting the current version: rebased snapshot (wires follow the
    // names of the surviving inputs/outputs, via
    // `patch_selected_function_and_rewire`).
    let rebase = {
        let detail = detail_value.clone();
        move |_| {
            let Some(d) = detail.clone() else { return };
            patch_selected_function_and_rewire(&mut cx, move |node: &mut FlowNode| {
                if let FlowNodeKind::PnexFunction { config } = &mut node.kind {
                    config.version_number = d.current_version_number;
                    config.language = d.language;
                    config.inputs = d.inputs.clone();
                    config.outputs = d.outputs.clone();
                }
            });
        }
    };

    let stale = detail_value
        .as_ref()
        .map(|d| d.current_version_number > pinned_version)
        .unwrap_or(false);
    let current_version = detail_value
        .as_ref()
        .map(|d| d.current_version_number)
        .unwrap_or(pinned_version);

    rsx! {
        div { class: "space-y-3",
            crate::components::flow_editor::function_picker::FunctionPicker {
                can_write,
                current_id: pinned_id,
                on_pick: move |pick: crate::components::flow_editor::function_picker::FunctionPick| {
                    patch_selected_function_and_rewire(
                        &mut cx,
                        move |node: &mut FlowNode| {
                            if let FlowNodeKind::PnexFunction { config } = &mut node.kind {
                                config.function_id = pick.id;
                                config.function_name = pick.name;
                                config.version_number = pick.version_number;
                                config.language = pick.language;
                                config.inputs = pick.inputs;
                                config.outputs = pick.outputs;
                            }
                        },
                    );
                },
            }

            if pinned_id != 0 {
                div { class: "flex items-center gap-2 text-xs flex-wrap",
                    span { class: "text-gray-500", {t!("flows-inspector-function-version")} }
                    span { class: "font-mono text-gray-700", {format!("v{pinned_version}")} }
                    span { class: "text-gray-500", "·" }
                    span { class: "text-gray-500", {t!("flows-inspector-function-current")} }
                    span { class: "font-mono text-gray-700", {format!("v{current_version}")} }
                    if stale {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-amber-100 text-amber-800",
                            {t!("flows-inspector-function-stale")}
                        }
                        if can_write {
                            button {
                                class: "px-2 py-0.5 text-xs text-signal-700 bg-signal-50 border border-signal-200 rounded-lg hover:bg-signal-100 transition-colors",
                                onclick: rebase,
                                {t!("flows-inspector-function-rebase")}
                            }
                        }
                    }
                }

                // Pin any version (rollback included) — the version's
                // interface snapshot, wires follow the names.
                if !versions_value.is_empty() {
                    div { class: "space-y-1",
                        span { class: "text-xs font-medium text-gray-500 block",
                            {t!("flows-inspector-function-pin")}
                        }
                        select {
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm disabled:bg-gray-50 disabled:text-gray-400",
                            value: "{pinned_version}",
                            disabled: !can_write,
                            onchange: move |event| {
                                let picked: i64 = event.value().parse().unwrap_or(pinned_version);
                                if let Some(summary) = versions_value
                                    .iter()
                                    .find(|v| v.version_number == picked)
                                    .cloned()
                                {
                                    pin_version(summary);
                                }
                            },
                            for summary in versions_value.clone() {
                                option {
                                    key: "{summary.version_number}",
                                    value: "{summary.version_number}",
                                    selected: summary.version_number == pinned_version,
                                    {
                                        format!(
                                            "v{} — {}",
                                            summary.version_number,
                                            summary
                                                .note
                                                .clone()
                                                .unwrap_or_else(|| summary.created_at.chars().take(10).collect()),
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
