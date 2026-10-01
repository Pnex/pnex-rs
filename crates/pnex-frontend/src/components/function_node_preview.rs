//! Mini rendering of the flow node a function will produce (mockup "Dans un
//! flow, ce nœud ressemblera à ça"): teal input ports on the left with
//! name + default labels, the node box in the middle (mono name +
//! "{lang} v{N}" subtitle), amber output ports on the right. Light theme —
//! same card styling as the port grids next to it. Derived live from the
//! parsed directives.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{FunctionInput, FunctionLanguage, FunctionOutput};

#[component]
pub fn FunctionNodePreview(
    name: String,
    language: FunctionLanguage,
    version: i64,
    inputs: Vec<FunctionInput>,
    outputs: Vec<FunctionOutput>,
) -> Element {
    let lang_label = match language {
        FunctionLanguage::Js => t!("functions-lang-js"),
        FunctionLanguage::Starlark => t!("functions-lang-starlark"),
    };
    rsx! {
        section { class: "rounded-xl border border-gray-200 bg-white p-4",
            div { class: "mb-3 text-[13px] text-gray-400", {t!("functions-preview-hint")} }
            div { class: "flex items-center justify-center gap-2",
                div { class: "flex flex-col gap-3 items-end",
                    for input in inputs {
                        div { class: "flex items-center gap-2 font-mono text-[13px]",
                            span {
                                span { class: "text-gray-800", "{input.name} " }
                                if let Some(d) = &input.default {
                                    span { class: "text-gray-400", "{d}" }
                                }
                            }
                            span { class: "h-0.5 w-5 bg-teal-400" }
                            span { class: "h-3 w-3 rounded-full border-2 border-teal-500" }
                        }
                    }
                }
                div { class: "w-38 h-24 min-w-36 rounded-xl bg-sky-50 border border-sky-300 shadow-sm flex flex-col justify-center items-center gap-1 px-3",
                    span { class: "font-mono text-sm font-medium text-gray-900 truncate max-w-full", "{name}" }
                    span { class: "text-xs text-gray-500", "{lang_label} v{version}" }
                }
                div { class: "flex flex-col gap-3",
                    for output in outputs {
                        div { class: "flex items-center gap-2 font-mono text-[13px]",
                            span { class: "h-3 w-3 rounded-full border-2 border-amber-500" }
                            span { class: "h-0.5 w-5 bg-amber-400" }
                            span { class: "text-gray-800", "{output.name}" }
                        }
                    }
                }
            }
        }
    }
}
