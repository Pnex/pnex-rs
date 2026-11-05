use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::pipelines::DatasetTriggerConfig;

// ─────────────── dataset-trigger (data-pipelines.md D578) ───────────────

/// Table trigger inspector: the table by name, since a pipeline creates its table at
/// its first run, and which event.
#[component]
pub(super) fn DatasetTriggerForm(
    mut cx: EditorCx,
    initial: DatasetTriggerConfig,
    can_write: bool,
) -> Element {
    let mut dataset = use_signal(|| initial.dataset.clone());
    let mut on = use_signal(|| initial.on.clone());
    let mut save = move || {
        let next = DatasetTriggerConfig {
            dataset: dataset(),
            on: on(),
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::DatasetTrigger { config } = &mut node.kind {
                *config = next;
            }
        });
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-dataset-trigger-help")} }
            label { class: FIELD_LABEL, r#for: "dataset-trigger-table",
                {t!("flows-dataset-trigger-table")}
            }
            input {
                id: "dataset-trigger-table",
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                "data-testid": "dataset-trigger-table",
                disabled: !can_write,
                value: "{dataset}",
                oninput: move |e| dataset.set(e.value()),
                onchange: move |_| save(),
            }
            label { class: FIELD_LABEL, r#for: "dataset-trigger-on", {t!("flows-dataset-trigger-on")} }
            select {
                id: "dataset-trigger-on",
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                "data-testid": "dataset-trigger-on",
                disabled: !can_write,
                onchange: move |e| {
                    on.set(e.value());
                    save();
                },
                option { value: "committed", selected: on() != "synced",
                    {t!("flows-dataset-trigger-committed")}
                }
                option { value: "synced", selected: on() == "synced",
                    {t!("flows-dataset-trigger-synced")}
                }
            }
        }
    }
}
