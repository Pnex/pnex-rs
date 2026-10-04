//! Inspecteur d'item d'annotation (D59) — type de cible (le `kind` suit),
//! device via `ResourcePicker` onglet Device, gpio pour une cible pin,
//! texte pour une note, label, couleur, suppression.
//! École closures dioxus : un clone local nommé par closure.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{
    AnnotationTarget, ANNOTATION_KIND_CONTROL, ANNOTATION_KIND_DEVICE, ANNOTATION_KIND_PIN,
    ANNOTATION_KIND_READING, ANNOTATION_KIND_STATUS,
};

use crate::components::annotation_editor::state::{
    set_item_color, set_item_label, set_item_target, AnnotationEditorCx,
};
use crate::components::annotation_editor::surface_targets::{
    ControlTargetEditor, ReadingTargetEditor,
};
use crate::components::resource_picker::{PickerTab, ResourcePick, ResourcePicker};

/// Cible par défaut d'un nouveau type (le slug/gpio utiles sont reportés).
fn target_for_kind(new_kind: &str, current: &AnnotationTarget) -> AnnotationTarget {
    let slug = current_slug(current);
    let gpio = match current {
        AnnotationTarget::Pin { pin_gpio, .. } => *pin_gpio,
        _ => 4,
    };
    match new_kind {
        ANNOTATION_KIND_PIN => AnnotationTarget::Pin {
            device_id: slug.unwrap_or_default(),
            pin_gpio: gpio,
        },
        ANNOTATION_KIND_STATUS => AnnotationTarget::Status {
            device_id: slug.unwrap_or_default(),
        },
        ANNOTATION_KIND_DEVICE => AnnotationTarget::Device {
            device_id: slug.unwrap_or_default(),
        },
        // Own control, provisioned by the server at save (D131); the kind
        // and an optional link to an existing control are set below.
        ANNOTATION_KIND_CONTROL => AnnotationTarget::Control {
            control_id: uuid::Uuid::nil(),
            kind: Some(pnex_core::ui_control::ControlKind::Switch),
        },
        ANNOTATION_KIND_READING => AnnotationTarget::Reading {
            source: pnex_core::SourceRef {
                role: "primary".into(),
                metric: String::new(),
                device_id: slug.unwrap_or_default(),
                window: "1h".into(),
                memory: None,
            },
            spark: false,
        },
        _ => AnnotationTarget::Note {
            text: String::new(),
        },
    }
}

fn current_slug(target: &AnnotationTarget) -> Option<String> {
    match target {
        AnnotationTarget::Device { device_id }
        | AnnotationTarget::Pin { device_id, .. }
        | AnnotationTarget::Status { device_id } => Some(device_id.clone()),
        AnnotationTarget::Note { .. }
        | AnnotationTarget::Control { .. }
        | AnnotationTarget::Reading { .. } => None,
    }
}

fn kind_of(target: &AnnotationTarget) -> &'static str {
    crate::components::annotation_editor::state::target_kind(target)
}

/// Panneau d'inspection de l'item sélectionné (rien rendu si aucun).
#[component]
pub fn AnnotationInspector(cx: AnnotationEditorCx, can_write: bool) -> Element {
    let item_id = cx.selected.cloned();
    let Some(item_id) = item_id else {
        return rsx! {};
    };
    let doc = cx.doc.cloned();
    let Some(item) = doc.items.iter().find(|it| it.id == item_id).cloned() else {
        return rsx! {};
    };
    let mut picker_open = use_signal(|| false);
    let kind_now = kind_of(&item.target).to_string();
    let color_now = item.color.clone().unwrap_or_else(|| "#2563eb".to_string());
    let _ = can_write;

    let pos = match &item.geometry {
        pnex_core::AnnotationGeometry::Equirect { yaw, pitch } => {
            format!("yaw {:.1} / pitch {:.1}", yaw, pitch)
        }
        _ => String::new(),
    };
    let slug_now =
        current_slug(&item.target).unwrap_or_else(|| t!("annot-inspector-none").to_string());
    let gpio_now = match &item.target {
        AnnotationTarget::Pin { pin_gpio, .. } => *pin_gpio,
        _ => 0,
    };
    let note_now = match &item.target {
        AnnotationTarget::Note { text } => text.clone(),
        _ => String::new(),
    };

    // Un clone nommé par closure (école closures dioxus : deux closures
    // `move` ne peuvent pas partager le même String).
    let kind_target = item.target.clone();
    let id_del = item.id.clone();
    let id_kind = item.id.clone();
    let id_gpio = item.id.clone();
    let id_note = item.id.clone();
    let id_label = item.id.clone();
    let id_color = item.id.clone();
    let id_color_reset = item.id.clone();
    let id_picker = item.id.clone();

    rsx! {
        div { class: "border-t border-gray-200 p-3 space-y-3 bg-gray-50",
            div { class: "flex items-center justify-between",
                span { class: "text-xs font-semibold text-gray-500 uppercase tracking-wide",
                    {t!("annot-inspector-title")}
                }
                button {
                    class: "text-xs text-red-600 hover:text-red-700 font-medium",
                    onclick: move |_| {
                        let id = id_del.clone();
                        cx.update_doc(move |doc| crate::components::annotation_editor::state::remove_item(
                            doc,
                            &id,
                        ));
                        cx.selected.set(None);
                    },
                    {t!("annot-inspector-delete")}
                }
            }
            label { class: "block text-xs font-medium text-gray-600",
                {t!("annot-inspector-kind")}
                select {
                    class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                    value: "{kind_now}",
                    onchange: move |e| {
                        let target = target_for_kind(&e.value(), &kind_target);
                        let id = id_kind.clone();
                        cx.update_doc(move |doc| set_item_target(doc, &id, target));
                    },
                    option { value: "note", {t!("annot-kind-note")} }
                    option { value: "device", {t!("annot-kind-device")} }
                    option { value: "pin", {t!("annot-kind-pin")} }
                    option { value: "status", {t!("annot-kind-status")} }
                    option { value: "control", {t!("annot-kind-control")} }
                    option { value: "reading", {t!("annot-kind-reading")} }
                }
            }
            if let AnnotationTarget::Control { control_id, kind } = item.target.clone() {
                ControlTargetEditor {
                    key: "{item.id}",
                    cx,
                    item_id: item.id.clone(),
                    current: control_id,
                    kind,
                }
            }
            if let AnnotationTarget::Reading { source, spark } = item.target.clone() {
                ReadingTargetEditor {
                    key: "{item.id}",
                    cx,
                    item_id: item.id.clone(),
                    source,
                    spark,
                }
            }
            if matches!(kind_now.as_str(), "device" | "pin" | "status") {
                div { class: "text-xs text-gray-600",
                    {t!("annot-inspector-target")}
                    span { class: "ml-1 font-mono text-gray-900", "{slug_now}" }
                    button {
                        class: "ml-2 px-2 py-0.5 text-xs text-blue-700 bg-blue-50 border border-blue-200 rounded-lg hover:bg-blue-100",
                        onclick: move |_| picker_open.set(true),
                        {t!("annot-inspector-pick")}
                    }
                    if matches!(item.target, AnnotationTarget::Pin { .. }) {
                        div { class: "mt-2 flex items-center gap-2",
                            label { class: "text-xs text-gray-500", {t!("annot-inspector-gpio")} }
                            input {
                                class: "w-20 rounded-lg border-gray-300 text-sm",
                                r#type: "number",
                                min: "0",
                                value: "{gpio_now}",
                                onchange: move |e| {
                                    if let Ok(gpio) = e.value().parse::<u32>() {
                                        let id = id_gpio.clone();
                                        cx.update_doc(move |doc| {
                                            if let Some(it) = doc.items.iter_mut().find(|it| it.id == id) {
                                                if let AnnotationTarget::Pin { pin_gpio, .. } = &mut it.target {
                                                    *pin_gpio = gpio;
                                                }
                                            }
                                        });
                                    }
                                },
                            }
                        }
                    }
                }
            }
            if kind_now == "note" {
                label { class: "block text-xs font-medium text-gray-600",
                    {t!("annot-inspector-text")}
                    textarea {
                        class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                        value: "{note_now}",
                        onchange: move |e| {
                            let text = e.value();
                            let id = id_note.clone();
                            cx.update_doc(move |doc| {
                                if let Some(it) = doc.items.iter_mut().find(|it| it.id == id) {
                                    if let AnnotationTarget::Note { text: t } = &mut it.target {
                                        *t = text;
                                    }
                                }
                            });
                        },
                    }
                }
            }

            // Label
            label { class: "block text-xs font-medium text-gray-600",
                {t!("annot-inspector-label")}
                input {
                    class: "mt-1 w-full rounded-lg border-gray-300 text-sm",
                    value: "{item.label}",
                    onchange: move |e| {
                        let label = e.value();
                        let id = id_label.clone();
                        cx.update_doc(move |doc| set_item_label(doc, &id, label));
                    },
                }
            }
            // Couleur
            div { class: "flex items-center gap-3",
                label { class: "text-xs font-medium text-gray-600", {t!("annot-inspector-color")} }
                input {
                    r#type: "color",
                    class: "h-7 w-10 rounded border border-gray-300",
                    value: "{color_now}",
                    onchange: move |e| {
                        let color = e.value();
                        let id = id_color.clone();
                        cx.update_doc(move |doc| set_item_color(doc, &id, Some(color)));
                    },
                }
                button {
                    class: "text-xs text-gray-500 hover:text-gray-700",
                    onclick: move |_| {
                        let id = id_color_reset.clone();
                        cx.update_doc(move |doc| set_item_color(doc, &id, None));
                    },
                    {t!("annot-inspector-color-reset")}
                }
            }
            // Position (lecture — ajustée au clic/drag pano)
            div { class: "text-xs text-gray-500 font-mono", "{pos}" }
        }
        // Picker device (modal)
        if picker_open() {
            ResourcePicker {
                initial_tab: PickerTab::Device,
                on_picked: move |pick: ResourcePick| {
                    picker_open.set(false);
                    if let ResourcePick::Device { slug } = pick {
                        let id = id_picker.clone();
                        cx.update_doc(move |doc| {
                            if let Some(it) = doc.items.iter_mut().find(|it| it.id == id) {
                                let new_target = match &it.target {
                                    AnnotationTarget::Pin { pin_gpio, .. } => {
                                        AnnotationTarget::Pin {
                                            device_id: slug.clone(),
                                            pin_gpio: *pin_gpio,
                                        }
                                    }
                                    AnnotationTarget::Status { .. } => {
                                        AnnotationTarget::Status {
                                            device_id: slug.clone(),
                                        }
                                    }
                                    _ => {
                                        AnnotationTarget::Device {
                                            device_id: slug.clone(),
                                        }
                                    }
                                };
                                it.target = new_target.clone();
                                it.kind = crate::components::annotation_editor::state::target_kind(
                                        &new_target,
                                    )
                                    .to_string();
                            }
                        });
                    }
                },
                on_close: move |_| picker_open.set(false),
            }
        }
    }
}
