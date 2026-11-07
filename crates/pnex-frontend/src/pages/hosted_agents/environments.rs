//! Environments tab (agent-environments.md D702, D704, D748, D763): the list, a new
//! environment, the scan policy of the org; an environment opens on its editor.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::env_editor::EnvironmentEditor;
use crate::api;
use crate::api::error::ApiError;
use crate::components::crud::form::FormDialog;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::access;
use crate::state::toasts;
use pnex_core::environments::{
    Environment, EnvironmentInput, Isolation, IsolationInput, ScanMode, ScanPolicy, ScanThreshold,
};

/// Where the runs of an environment execute: an Admin picks, others read (D763).
#[component]
fn IsolationCell(env: Environment, reload: Signal<u32>) -> Element {
    let label = |i: Isolation| match i {
        Isolation::Pod => t!("agents-isolation-pod").to_string(),
        Isolation::Vm => t!("agents-isolation-vm").to_string(),
    };
    if !access::can("environments:build") {
        return rsx! {
            span { class: "text-sm text-slate", {label(env.isolation)} }
        };
    }
    let (id, version, current) = (env.id.clone(), env.version, env.isolation);
    let save = move |value: String| {
        let id = id.clone();
        let mut reload = reload;
        spawn(async move {
            if let Some(isolation) = Isolation::parse(&value) {
                let input = IsolationInput {
                    isolation,
                    expected_version: version,
                };
                match api::hosted_agents::set_isolation(&id, &input).await {
                    Ok(_) => {
                        toasts::info(t!("agents-isolation-saved").to_string());
                        reload += 1;
                    }
                    Err(e) => toasts::error(e),
                }
            }
        });
    };
    rsx! {
        select {
            class: "{INPUT}",
            aria_label: t!("agents-col-isolation").to_string(),
            onchange: move |e| save(e.value()),
            for i in [Isolation::Pod, Isolation::Vm] {
                option {
                    key: "{i.as_str()}",
                    value: i.as_str(),
                    selected: i == current,
                    {label(i)}
                }
            }
        }
    }
}

/// Environments of the org with their active image; one opens on its editor.
#[component]
pub(super) fn EnvironmentsTab(reload: Signal<u32>) -> Element {
    let mut editing = use_signal(|| Option::<String>::None);
    let mut adding = use_signal(|| false);
    let list = use_resource(move || async move {
        let _ = reload();
        api::hosted_agents::environments().await
    });
    if let Some(env_id) = editing() {
        return rsx! {
            EnvironmentEditor {
                key: "{env_id}",
                env_id,
                on_back: move |_| {
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        };
    }
    let (state, is_empty, rows) = match &*list.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let columns = vec![
        Column::new(t!("agents-col-name").to_string(), |e: &Environment| {
            let description = e.description.clone();
            rsx! {
                div { class: "text-sm font-medium text-ink", {e.name.clone()} }
                if !description.is_empty() {
                    div { class: "text-xs text-slate mt-0.5", {description} }
                }
            }
        }),
        Column::new(t!("agents-col-image").to_string(), |e: &Environment| {
            let image = e.active_image.clone();
            rsx! {
                if let Some(image) = image {
                    span { class: "font-mono text-xs text-slate break-all", {image} }
                } else {
                    span { class: "text-sm text-slate", {t!("agents-no-image")} }
                }
            }
        }),
        Column::new(
            t!("agents-col-isolation").to_string(),
            move |e: &Environment| {
                rsx! {
                    IsolationCell { key: "{e.id}-{e.version}", env: e.clone(), reload }
                }
            },
        ),
        Column::new(t!("common-actions").to_string(), move |e: &Environment| {
            let id = e.id.clone();
            rsx! {
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| editing.set(Some(id.clone())),
                    {t!("agents-env-open")}
                }
            }
        })
        .actions(),
    ];
    rsx! {
        if adding() {
            NewEnvironment { adding, editing }
        }
        div { class: "flex flex-wrap items-end justify-between gap-3 mb-4",
            ScanPolicyCard {}
            if access::can("environments:write") {
                button { class: BTN_PRIMARY, onclick: move |_| adding.set(true),
                    {t!("agents-env-new")}
                }
            }
        }
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-environments-empty").to_string(),
            empty_detail: rsx! {
                p { class: "text-sm text-slate mt-2", {t!("agents-env-empty-hint")} }
            },
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|e: &Environment| e.id.clone()),
            }
        }
    }
}

/// Name, description and registry; the new environment opens on its editor.
#[component]
fn NewEnvironment(adding: Signal<bool>, editing: Signal<Option<String>>) -> Element {
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut registry = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let registries = use_resource(move || async move { api::hosted_agents::registries().await });
    let options: Vec<(String, String)> = match &*registries.value().read() {
        Some(Ok(p)) => p
            .results
            .iter()
            .map(|r| {
                (
                    r.id.clone(),
                    format!("{} · {}/{}", r.name, r.host, r.repository),
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let no_registry = matches!(&*registries.value().read(), Some(Ok(p)) if p.count == 0)
        || matches!(&*registries.value().read(), Some(Err(_)));
    let valid = !name().trim().is_empty() && !registry().is_empty();
    rsx! {
        FormDialog {
            title: t!("agents-env-new").to_string(),
            submit_label: t!("common-create").to_string(),
            on_close: move |_| adding.set(false),
            busy: busy(),
            valid,
            on_submit: move |_| {
                let input = EnvironmentInput {
                    name: name().trim().to_string(),
                    description: description().trim().to_string(),
                    registry_id: registry(),
                    expected_version: None,
                };
                busy.set(true);
                spawn(async move {
                    let result = api::hosted_agents::create_environment(&input).await;
                    busy.set(false);
                    match result {
                        Ok(env) => {
                            adding.set(false);
                            editing.set(Some(env.id));
                        }
                        Err(e) => form_error.set(Some(e)),
                    }
                });
            },
            div { class: "space-y-4",
                div {
                    label { r#for: "env-name", class: FIELD_LABEL, {t!("agents-field-name")} }
                    input {
                        id: "env-name",
                        class: "{INPUT} w-full",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label { r#for: "env-description", class: FIELD_LABEL,
                        {t!("agents-env-field-description")}
                    }
                    input {
                        id: "env-description",
                        class: "{INPUT} w-full",
                        value: "{description}",
                        oninput: move |e| description.set(e.value()),
                    }
                }
                div {
                    label { r#for: "env-registry", class: FIELD_LABEL,
                        {t!("agents-env-field-registry")}
                    }
                    select {
                        id: "env-registry",
                        class: "{INPUT} w-full",
                        onchange: move |e| registry.set(e.value()),
                        option { value: "", {t!("agents-env-field-registry-pick")} }
                        for (id, label) in options {
                            option { key: "{id}", value: "{id}", {label} }
                        }
                    }
                    if no_registry {
                        p { class: "text-xs text-slate mt-1", {t!("agents-env-no-registry")} }
                    }
                }
                if let Some(err) = form_error() {
                    div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                        {crate::api::error_i18n::localize(&err)}
                    }
                }
            }
        }
    }
}

/// The scan policy of the org (D748): read by all, changed with `environments:build`.
#[component]
fn ScanPolicyCard() -> Element {
    let mut generation = use_signal(|| 0u32);
    let policy = use_resource(move || async move {
        let _ = generation();
        api::hosted_agents::scan_policy().await
    });
    let Some(Ok(current)) = policy.value().read().clone() else {
        return rsx! {};
    };
    let can_edit = access::can("environments:build");
    let save = move |next: ScanPolicy| {
        spawn(async move {
            match api::hosted_agents::set_scan_policy(&next).await {
                Ok(_) => toasts::info(t!("agents-env-policy-saved").to_string()),
                Err(e) => toasts::error(e),
            }
            generation.with_mut(|g| *g += 1);
        });
    };
    let thresholds = [
        (
            ScanThreshold::Critical,
            "critical",
            t!("agents-env-threshold-critical"),
        ),
        (ScanThreshold::High, "high", t!("agents-env-threshold-high")),
        (ScanThreshold::None, "none", t!("agents-env-threshold-none")),
    ];
    let modes = [
        (ScanMode::Signal, "signal", t!("agents-env-mode-signal")),
        (ScanMode::Refuse, "refuse", t!("agents-env-mode-refuse")),
    ];
    rsx! {
        div { class: "flex flex-wrap items-end gap-3",
            div {
                label { r#for: "scan-threshold", class: FIELD_LABEL,
                    {t!("agents-env-policy-threshold")}
                }
                select {
                    id: "scan-threshold",
                    class: "{INPUT}",
                    disabled: !can_edit,
                    onchange: move |e| {
                        let threshold = match e.value().as_str() {
                            "high" => ScanThreshold::High,
                            "none" => ScanThreshold::None,
                            _ => ScanThreshold::Critical,
                        };
                        save(ScanPolicy { threshold, ..current });
                    },
                    for (value, key, label) in thresholds {
                        option {
                            key: "{key}",
                            value: key,
                            selected: value == current.threshold,
                            {label}
                        }
                    }
                }
            }
            div {
                label { r#for: "scan-mode", class: FIELD_LABEL, {t!("agents-env-policy-mode")} }
                select {
                    id: "scan-mode",
                    class: "{INPUT}",
                    disabled: !can_edit,
                    onchange: move |e| {
                        let mode = if e.value() == "refuse" {
                            ScanMode::Refuse
                        } else {
                            ScanMode::Signal
                        };
                        save(ScanPolicy { mode, ..current });
                    },
                    for (value, key, label) in modes {
                        option {
                            key: "{key}",
                            value: key,
                            selected: value == current.mode,
                            {label}
                        }
                    }
                }
            }
        }
    }
}
