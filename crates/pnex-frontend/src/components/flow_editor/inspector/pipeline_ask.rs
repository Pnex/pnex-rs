use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::pipeline::ask::{PipelineAskConfig, PER_DAY_DEFAULT, PER_DAY_MAX};

// ─────────────── pipeline-ask (pipeline-engine.md D999) ───────────────

const INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";

/// Pipeline ask inspector: the pipeline as `<pack>/<key>` and the rows per day.
#[component]
pub(super) fn PipelineAskForm(
    mut cx: EditorCx,
    initial: PipelineAskConfig,
    can_write: bool,
) -> Element {
    let mut pipeline = use_signal(|| initial.pipeline.clone());
    let mut per_day = use_signal(|| initial.per_day().to_string());
    let mut save = move || {
        let next = PipelineAskConfig {
            pipeline: pipeline().trim().to_string(),
            per_day: per_day().trim().parse().unwrap_or(PER_DAY_DEFAULT),
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::PipelineAsk { config } = &mut node.kind {
                *config = next;
            }
        });
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-pipeline-ask-help")} }
            label { class: FIELD_LABEL, r#for: "pipeline-ask-pipeline",
                {t!("flows-pipeline-ask-pipeline")}
            }
            input {
                id: "pipeline-ask-pipeline",
                class: INPUT,
                "data-testid": "pipeline-ask-pipeline",
                placeholder: "pack/pipeline",
                disabled: !can_write,
                value: "{pipeline}",
                oninput: move |e| pipeline.set(e.value()),
                onchange: move |_| save(),
            }
            label { class: FIELD_LABEL, r#for: "pipeline-ask-per-day",
                {t!("flows-pipeline-ask-per-day", max : PER_DAY_MAX)}
            }
            input {
                id: "pipeline-ask-per-day",
                class: INPUT,
                "data-testid": "pipeline-ask-per-day",
                r#type: "number",
                min: "1",
                max: "{PER_DAY_MAX}",
                disabled: !can_write,
                value: "{per_day}",
                oninput: move |e| per_day.set(e.value()),
                onchange: move |_| save(),
            }
        }
    }
}
