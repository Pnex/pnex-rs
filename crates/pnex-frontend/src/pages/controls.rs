//! Page `/controls` — org controls (D125, `docs/architecture/
//! surfaces-controls.md`): the switches, sliders, buttons and numeric
//! inputs that the surfaces operate and the flows listen to
//! (`control-source`). A control is never bound to a pin: the flow decides.
//!
//! CRUD school (ListLayout + DataTable + Modal form + ConfirmDialog).
//! Deleting a control listened to by a deployed flow is refused (409
//! `control-in-use`, the server message names the flows).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{
    check_control_label, valid_control_key, ControlKind, ControlSpec, ControlValue,
    CreateUiControl, UiControl, UpdateUiControl,
};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::RefreshButton;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::surface::{kind_text, spec_summary, suggest_key};
use crate::state::{org, toasts};

#[component]
pub fn Controls() -> Element {
    let mut reload = use_signal(|| 0u32);
    // None = closed, Some(None) = creation, Some(Some(c)) = edition.
    let mut editing = use_signal(|| None::<Option<UiControl>>);
    let mut delete_target = use_signal(|| None::<UiControl>);
    let can_write = org::current_can_write();

    let list = use_resource(move || {
        let _ = reload();
        let has_org = org::current().is_some();
        async move {
            if !has_org {
                return None;
            }
            Some(api::controls::list().await)
        }
    });
    let rows: Vec<UiControl> = match &*list.read() {
        Some(Some(Ok(rows))) => rows.clone(),
        _ => Vec::new(),
    };
    let list_state = match &*list.read() {
        None | Some(None) => None,
        Some(Some(Ok(_))) => Some(Ok(())),
        Some(Some(Err(e))) => Some(Err(e.clone())),
    };
    let ids: Vec<Uuid> = rows.iter().map(|c| c.id).collect();
    let values = use_resource(move || {
        let ids = ids.clone();
        async move { api::controls::values(ids).await.unwrap_or_default() }
    });
    let values: BTreeMap<Uuid, Option<ControlValue>> =
        values.read().as_ref().cloned().unwrap_or_default();
    let is_empty = rows.is_empty();
    let modal = editing.read().clone();
    let deleting = delete_target.read().clone();

    let columns = vec![
        Column::new(t!("controls-col-control").to_string(), |c: &UiControl| {
            rsx! {
                div { class: "text-sm font-medium text-gray-900", "{c.label}" }
                code { class: "text-xs text-gray-500", "{c.key}" }
            }
        }),
        Column::new(t!("controls-col-kind").to_string(), |c: &UiControl| {
            let kind = kind_text(c.spec.kind);
            let domain = spec_summary(&c.spec);
            rsx! {
                div { class: "text-sm text-gray-800", "{kind}" }
                div { class: "text-xs text-gray-500", "{domain}" }
            }
        }),
        Column::new(t!("controls-col-value").to_string(), move |c: &UiControl| {
            let shown = values
                .get(&c.id)
                .cloned()
                .flatten()
                .map(|v| {
                    let unit = c.spec.unit.clone().unwrap_or_default();
                    format!("{} {unit}", v.v).trim().to_string()
                })
                .unwrap_or_else(|| "—".into());
            rsx! {
                span { class: "font-mono text-sm text-gray-800", "{shown}" }
            }
        }),
        Column::new(t!("controls-col-listened").to_string(), |c: &UiControl| {
            let names: Vec<String> = c.listened_by.iter().map(|l| l.flow_name.clone()).collect();
            rsx! {
                if names.is_empty() {
                    span {
                        class: "inline-flex rounded bg-amber-50 px-2 py-0.5 text-xs font-medium text-amber-700",
                        title: t!("controls-idle-help").to_string(),
                        {t!("controls-idle")}
                    }
                } else {
                    span { class: "text-sm text-gray-700", {names.join(", ")} }
                }
            }
        }),
        Column::new(String::new(), move |c: &UiControl| {
            let edit = c.clone();
            let del = c.clone();
            rsx! {
                if can_write {
                    button {
                        class: "inline-flex items-center px-3 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 mr-2",
                        onclick: move |_| editing.set(Some(Some(edit.clone()))),
                        icons::Wrench { class: "h-4 w-4 mr-1" }
                        {t!("controls-edit-short")}
                    }
                    button {
                        class: DANGER_BTN,
                        onclick: move |_| delete_target.set(Some(del.clone())),
                        icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                        {t!("common-delete")}
                    }
                }
            }
        })
        .with_td_class("text-right whitespace-nowrap"),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-controls").to_string(),
            subtitle: Some(t!("controls-subtitle").to_string()),
            can_write,
            actions: rsx! {
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            },
            add_label: Some(t!("controls-new").to_string()),
            on_add: move |_| editing.set(Some(None)),
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                ListStates {
                    state: list_state,
                    is_empty,
                    empty_message: t!("controls-empty").to_string(),
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|c: &UiControl| c.id.to_string()),
                    }
                }
            }
            if let Some(initial) = modal {
                ControlForm {
                    initial,
                    on_close: move |_| editing.set(None),
                    on_saved: move |_| {
                        editing.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some(c) = deleting {
                ConfirmDialog {
                    title: t!("controls-delete").to_string(),
                    message: t!("controls-delete-confirm", label : c.label.clone()).to_string(),
                    confirm_label: t!("common-delete").to_string(),
                    on_confirm: move |_| {
                        let id = c.id;
                        let done = t!("controls-deleted").to_string();
                        spawn(async move {
                            match api::controls::delete(id).await {
                                Ok(()) => {
                                    toasts::success(done);
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(e) => toasts::error(e),
                            }
                            delete_target.set(None);
                        });
                    },
                    on_cancel: move |_| delete_target.set(None),
                }
            }
        }
    }
}

/// Parses an optional numeric field (empty = default of the kind).
fn opt_num(s: &str) -> Result<Option<f64>, ()> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<f64>().map(Some).map_err(|_| ())
}

fn num_text(v: Option<f64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

/// Creation / edition form. The kind is chosen at creation only: the
/// widgets bound to a control are of its kind.
#[component]
fn ControlForm(
    initial: Option<UiControl>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let editing_id = initial.as_ref().map(|c| c.id);
    let init = initial.clone();
    let spec0 = init
        .as_ref()
        .map(|c| c.spec.clone())
        .unwrap_or_else(|| ControlSpec::new(ControlKind::Switch));
    let mut label = use_signal(|| init.as_ref().map(|c| c.label.clone()).unwrap_or_default());
    let mut key = use_signal(|| init.as_ref().map(|c| c.key.clone()).unwrap_or_default());
    // The key follows the label until typed by hand (creation only).
    let mut key_touched = use_signal(|| init.is_some());
    let mut kind = use_signal(|| spec0.kind);
    let on = use_signal(|| num_text(spec0.on));
    let off = use_signal(|| num_text(spec0.off));
    let min = use_signal(|| num_text(spec0.min));
    let max = use_signal(|| num_text(spec0.max));
    let step = use_signal(|| num_text(spec0.step));
    let press = use_signal(|| num_text(spec0.press));
    let mut unit = use_signal(|| spec0.unit.clone().unwrap_or_default());
    let mut confirm = use_signal(|| spec0.confirm);
    let mut busy = use_signal(|| false);

    // Spec built from the fields (None: a number does not parse).
    let build = move || -> Option<ControlSpec> {
        let mut spec = ControlSpec::new(kind());
        spec.on = opt_num(&on()).ok()?;
        spec.off = opt_num(&off()).ok()?;
        spec.min = opt_num(&min()).ok()?;
        spec.max = opt_num(&max()).ok()?;
        spec.step = opt_num(&step()).ok()?;
        spec.press = opt_num(&press()).ok()?;
        let u = unit().trim().to_string();
        spec.unit = (!u.is_empty()).then_some(u);
        spec.confirm = confirm();
        Some(spec)
    };
    let spec = build();
    let key_ok = valid_control_key(key().trim());
    let label_ok = check_control_label(&label()).is_none();
    let spec_error: Option<String> = match &spec {
        None => Some(t!("controls-form-number-invalid").to_string()),
        Some(s) => s.check().map(|(code, _)| spec_error_text(code)),
    };
    let ok = key_ok && label_ok && spec_error.is_none() && !busy();
    let title = if editing_id.is_some() {
        t!("controls-edit").to_string()
    } else {
        t!("controls-new").to_string()
    };
    let k = kind();

    let submit = move |_| {
        let Some(spec) = build() else {
            return;
        };
        busy.set(true);
        let key_v = key.peek().trim().to_string();
        let label_v = label.peek().trim().to_string();
        let saved = t!("controls-saved").to_string();
        spawn(async move {
            let res = match editing_id {
                Some(id) => api::controls::update(
                    id,
                    UpdateUiControl {
                        key: Some(key_v),
                        label: Some(label_v),
                        spec: Some(spec),
                    },
                )
                .await
                .map(|_| ()),
                None => api::controls::create(CreateUiControl {
                    key: key_v,
                    label: label_v,
                    spec,
                })
                .await
                .map(|_| ()),
            };
            busy.set(false);
            match res {
                Ok(()) => {
                    toasts::success(saved);
                    on_saved.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    };

    rsx! {
        Modal {
            title,
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-3",
                FormRow { label: t!("controls-form-label").to_string(),
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                        value: "{label}",
                        placeholder: t!("controls-label-placeholder").to_string(),
                        oninput: move |e| {
                            let v = e.value();
                            if !key_touched() {
                                key.set(suggest_key(&v));
                            }
                            label.set(v);
                        },
                    }
                }
                FormRow { label: t!("controls-form-key").to_string(),
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                        value: "{key}",
                        placeholder: "light.room",
                        oninput: move |e| {
                            key_touched.set(true);
                            key.set(e.value());
                        },
                    }
                    p { class: "mt-1 text-xs text-gray-500", {t!("controls-form-key-help")} }
                }
                FormRow { label: t!("controls-col-kind").to_string(),
                    select {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                        disabled: editing_id.is_some(),
                        onchange: move |e| {
                            if let Some(next) = ControlKind::ALL
                                .into_iter()
                                .find(|k| k.as_str() == e.value())
                            {
                                kind.set(next);
                            }
                        },
                        for kd in ControlKind::ALL {
                            option {
                                key: "{kd.as_str()}",
                                value: "{kd.as_str()}",
                                selected: kd == k,
                                {kind_text(kd)}
                            }
                        }
                    }
                }
                div { class: "grid grid-cols-3 gap-2",
                    if k == ControlKind::Switch {
                        NumField {
                            label: t!("controls-form-on").to_string(),
                            value: on,
                            placeholder: "1",
                        }
                        NumField {
                            label: t!("controls-form-off").to_string(),
                            value: off,
                            placeholder: "0",
                        }
                    }
                    if k == ControlKind::Slider || k == ControlKind::Number {
                        NumField {
                            label: t!("controls-form-min").to_string(),
                            value: min,
                            placeholder: if k == ControlKind::Slider { "0" } else { "" },
                        }
                        NumField {
                            label: t!("controls-form-max").to_string(),
                            value: max,
                            placeholder: if k == ControlKind::Slider { "100" } else { "" },
                        }
                        NumField {
                            label: t!("controls-form-step").to_string(),
                            value: step,
                            placeholder: if k == ControlKind::Slider { "1" } else { "" },
                        }
                    }
                    if k == ControlKind::Button {
                        NumField {
                            label: t!("controls-form-press").to_string(),
                            value: press,
                            placeholder: "1",
                        }
                    }
                    div {
                        label { class: "block text-xs font-medium text-gray-500 mb-1",
                            {t!("controls-form-unit")}
                        }
                        input {
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                            value: "{unit}",
                            placeholder: "%",
                            oninput: move |e| unit.set(e.value()),
                        }
                    }
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    input {
                        r#type: "checkbox",
                        checked: confirm(),
                        onchange: move |e| confirm.set(e.checked()),
                    }
                    {t!("controls-form-confirm")}
                }
                if let Some(err) = spec_error {
                    p { class: "text-xs text-red-600", "{err}" }
                }
                if !key().is_empty() && !key_ok {
                    p { class: "text-xs text-red-600", {t!("controls-form-key-invalid")} }
                }
                div { class: "flex justify-end gap-2 pt-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700 disabled:opacity-40",
                        disabled: !ok,
                        onclick: submit,
                        {t!("common-save")}
                    }
                }
            }
        }
    }
}

/// Localized text of a spec check code (`ControlSpec::check`).
fn spec_error_text(code: &str) -> String {
    match code {
        "control_spec_switch_same" => t!("controls-spec-switch-same").to_string(),
        "control_spec_range" => t!("controls-spec-range").to_string(),
        "control_spec_step" => t!("controls-spec-step").to_string(),
        "control_spec_unit" => t!("controls-spec-unit").to_string(),
        _ => t!("controls-form-number-invalid").to_string(),
    }
}

#[component]
fn FormRow(label: String, children: Element) -> Element {
    rsx! {
        div {
            label { class: "block text-xs font-medium text-gray-500 uppercase mb-1", "{label}" }
            {children}
        }
    }
}

#[component]
fn NumField(label: String, mut value: Signal<String>, placeholder: String) -> Element {
    rsx! {
        div {
            label { class: "block text-xs font-medium text-gray-500 mb-1", "{label}" }
            input {
                class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm",
                r#type: "text",
                inputmode: "decimal",
                value: "{value}",
                placeholder: "{placeholder}",
                oninput: move |e| value.set(e.value()),
            }
        }
    }
}
