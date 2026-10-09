//! Inspecteur de configuration du nœud sélectionné — un composant par kind.
//!
//! Remonté par `key` à chaque changement de sélection : les champs repartent
//! de la config du nœud, état local propre (pattern `MetadataEditor` des
//! devices : texte local + drapeau d'invalidité pour les JSON).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{
    CalcConfig, CameraSourceConfig, CoolPropConfig, DebugConfig, DeviceReadConfig,
    DeviceWriteConfig, FlowNode, FlowNodeKind, FunctionNodeConfig, HttpFetchAuth, HttpFetchHeader,
    HttpFetchMethod, HttpFetchNodeConfig, HttpFetchOnError, HttpFetchProxy, InjectConfig,
    JsonMergeConfig, JsonSplitConfig, MetricConfig, NotifyNodeConfig, RegPidConfig, RegTtConfig,
    SafeState, ValueConfig, VideoRecordConfig,
};

use super::{geometry, state, EditorCx};
use crate::api;
use crate::components::code_highlight::JsonEditor;
use crate::components::confirm::ConfirmDialog;

#[component]
pub(crate) fn InspectorBody(mut cx: EditorCx, can_write: bool, flow_id: i64) -> Element {
    let Some(node_id) = cx.selected_node.cloned() else {
        return rsx! {};
    };
    let graph = cx.graph.cloned();
    let Some(node) = graph.nodes.iter().find(|n| n.id == node_id).cloned() else {
        return rsx! {};
    };

    // Node violations — messages resolved at render via machine code
    // (`err-<kebab>`), verbatim fallback for unknown codes.
    let node_violations = cx.violations_of(&node_id);
    let mut confirm_delete = use_signal(|| false);

    rsx! {
        div { class: "space-y-3",
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-node-name")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm disabled:bg-gray-50 disabled:text-gray-400",
                    r#type: "text",
                    value: "{node.name.clone().unwrap_or_default()}",
                    disabled: !can_write,
                    oninput: move |event| patch_selected(
                        &mut cx,
                        move |node: &mut FlowNode| {
                            let value = event.value();
                            node.name = if value.trim().is_empty() { None } else { Some(value) };
                        },
                    ),
                }
            }
            {kind_form(&node, cx, can_write, flow_id)}

            if !node_violations.is_empty() {
                div { class: "rounded-lg bg-red-50 border border-red-200 p-2 space-y-1",
                    for violation in node_violations {
                        p { class: "text-xs text-red-700",
                            {crate::api::error_i18n::localize_violation(&violation)}
                        }
                    }
                }
            }

            if can_write {
                button {
                    class: "w-full px-3 py-2 text-sm text-red-600 border border-red-200 rounded-lg hover:bg-red-50 transition-colors",
                    onclick: move |_| confirm_delete.set(true),
                    {t!("flows-node-delete")}
                }
            }

            if confirm_delete() {
                ConfirmDialog {
                    title: t!("flows-node-delete"),
                    message: format!("#{}", node.id),
                    confirm_label: t!("flows-node-delete"),
                    on_confirm: move |_| {
                        confirm_delete.set(false);
                        remove_selected(&mut cx);
                        cx.selected_node.set(None);
                    },
                    on_cancel: move |_| confirm_delete.set(false),
                }
            }
        }
    }
}

/// Formulaire propre au kind — un composant par variante (hooks isolés).
fn kind_form(node: &FlowNode, cx: EditorCx, can_write: bool, flow_id: i64) -> Element {
    match &node.kind {
        FlowNodeKind::Inject { config } => rsx! {
            InjectForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::DeviceRead { config } => rsx! {
            DeviceReadForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::DeviceWrite { config } => rsx! {
            DeviceWriteForm {
                cx,
                initial: config.clone(),
                can_write,
                flow_id,
            }
        },
        FlowNodeKind::Calc { config } => rsx! {
            CalcForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Value { config } => rsx! {
            ValueForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Metric { config } => rsx! {
            MetricForm {
                cx,
                initial: config.clone(),
                can_write,
                flow_id,
            }
        },
        FlowNodeKind::CoolProp { config } => rsx! {
            CoolPropForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::PnexNotify { config } => rsx! {
            NotifyForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::HttpFetch { config } => rsx! {
            HttpFetchForm { cx, initial: config.clone(), can_write }
        },
        // Remonté par `key` à chaque changement de fonction/version épinglée :
        // le formulaire repart de la config (école MetadataEditor).
        FlowNodeKind::PnexFunction { config } => rsx! {
            FunctionForm {
                key: "{config.function_id}-{config.version_number}",
                cx,
                initial: config.clone(),
                can_write,
            }
        },
        FlowNodeKind::JsonSplit { config } => rsx! {
            JsonSplitForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::JsonMerge { config } => rsx! {
            JsonMergeForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::CameraSource { config } => rsx! {
            CameraSourceForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::VideoRecord { config } => rsx! {
            VideoRecordForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::VisionDetect { config } => rsx! {
            VisionDetectForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::EventLog { config } => rsx! {
            EventLogForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::MemoryWrite { config } => rsx! {
            MemoryWriteForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::MemoryRead { config } => rsx! {
            MemoryReadForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::ControlSource { config } => rsx! {
            ControlSourceForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Weather { config } => rsx! {
            WeatherForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Anomaly { config } => rsx! {
            AnomalyForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Forecast { config } => rsx! {
            ForecastForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Debug { config } => rsx! {
            DebugForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Display { .. } => rsx! {
            DisplayForm { can_write }
        },
        FlowNodeKind::RegTtHeat { config } => rsx! {
            RegTtForm {
                cx,
                initial: config.clone(),
                can_write,
                heat: true,
                flow_id,
            }
        },
        FlowNodeKind::RegTtCool { config } => rsx! {
            RegTtForm {
                cx,
                initial: config.clone(),
                can_write,
                heat: false,
                flow_id,
            }
        },
        FlowNodeKind::RegPid { config } => rsx! {
            RegPidForm {
                cx,
                initial: config.clone(),
                can_write,
                flow_id,
            }
        },
        FlowNodeKind::Red { type_name, config } => rsx! {
            RedForm {
                cx,
                initial_type: type_name.clone(),
                initial_config: config.clone(),
                can_write,
            }
        },
    }
}

mod camera_source;
mod control_source;
mod coolprop;
mod device;
mod event_log;
mod function;
mod helpers;
mod http_fetch;
mod inject;
mod json;
mod memory;
mod notify;
mod predict;
mod red;
mod reg;
mod simple;
mod value;
mod video_record;
mod vision_detect;
mod weather;

use camera_source::CameraSourceForm;
use control_source::ControlSourceForm;
use coolprop::CoolPropForm;
use device::{DeviceReadForm, DeviceWriteForm};
use event_log::EventLogForm;
use function::FunctionForm;
use helpers::{patch_selected, remove_selected};
use http_fetch::HttpFetchForm;
use inject::InjectForm;
use json::{JsonMergeForm, JsonSplitForm};
use memory::{MemoryReadForm, MemoryWriteForm};
use notify::NotifyForm;
use predict::{AnomalyForm, ForecastForm};
use red::RedForm;
use reg::{RegPidForm, RegTtForm};
use simple::{DebugForm, DisplayForm, MetricForm};
use value::{CalcForm, ValueForm};
use video_record::VideoRecordForm;
use vision_detect::VisionDetectForm;
use weather::WeatherForm;
