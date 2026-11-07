//! Skills tab of the Agents page (agent-environments.md D711): texts the agents read at each
//! run. Every save is a draft; a person holding `agents:write` activates a version after
//! comparing it in the common versions drawer (D221, D222). The editor is the common shell.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::error::ApiError;
use crate::api::error_i18n::localize;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::editor_shell::{Chip, EditorShell, EditorStatus, StatusTone};
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::components::versions_drawer::{VersionRow, VersionsDrawer};
use crate::state::access;
use pnex_core::agent_skills::{
    AgentSkill, AgentSkillActivate, AgentSkillCreate, AgentSkillUpdate, SkillPin,
};
use pnex_core::agents::{Agent, AgentUpdate};

#[component]
pub fn SkillsTab(reload: Signal<u32>) -> Element {
    let page = use_signal(|| 0i64);
    let mut adding = use_signal(|| false);
    let mut open = use_signal(|| Option::<String>::None);
    let mut error = use_signal(|| Option::<ApiError>::None);
    let mut confirm_delete = use_signal(|| Option::<String>::None);
    let can_write = access::can("agents:write");
    let can_delete = access::can("agents:delete");
    let list = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            api::agent_skills::list(PAGE_SIZE, offset).await
        }
    });
    let (state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, 0, Vec::new()),
    };
    let columns = vec![
        Column::new(t!("agents-skill-col-name").to_string(), |s: &AgentSkill| {
            let description = s.description.clone();
            rsx! {
                div { class: "text-sm font-medium text-ink font-mono", {s.name.clone()} }
                if !description.is_empty() {
                    div { class: "text-xs text-slate mt-0.5", {description} }
                }
            }
        }),
        Column::new(
            t!("agents-skill-col-active").to_string(),
            |s: &AgentSkill| {
                let label = match s.active_number {
                    Some(n) => format!("v{n}"),
                    None => t!("agents-skill-status-draft").to_string(),
                };
                rsx! {
                    span { class: "text-sm text-slate", {label} }
                }
            },
        ),
        Column::new(
            t!("agents-skill-col-latest").to_string(),
            |s: &AgentSkill| {
                rsx! {
                    span { class: "text-sm text-slate", "v{s.latest_number}" }
                }
            },
        )
        .secondary(),
        Column::new(t!("common-actions").to_string(), move |s: &AgentSkill| {
            let id = s.id.clone();
            let confirm_id = id.clone();
            rsx! {
                if can_delete {
                    div { onclick: move |e| e.stop_propagation(),
                        if confirm_delete() == Some(confirm_id.clone()) {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| {
                                    let id = id.clone();
                                    spawn(async move {
                                        match api::agent_skills::delete(&id).await {
                                            Ok(()) => {
                                                error.set(None);
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(e) => error.set(Some(e)),
                                        }
                                        confirm_delete.set(None);
                                    });
                                },
                                {t!("agents-skill-confirm-delete")}
                            }
                        } else {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| confirm_delete.set(Some(confirm_id.clone())),
                                {t!("common-delete")}
                            }
                        }
                    }
                }
            }
        })
        .actions(),
    ];
    rsx! {
        if adding() {
            NewSkill { adding, open, reload }
        }
        if let Some(id) = open() {
            div { class: "fixed inset-0 z-30 bg-white",
                SkillEditor {
                    key: "{id}",
                    id,
                    open,
                    reload,
                }
            }
        }
        if can_write {
            div { class: "mb-4 flex justify-end",
                button { class: BTN_PRIMARY_SM, onclick: move |_| adding.set(true),
                    {t!("agents-skill-new")}
                }
            }
        }
        if let Some(err) = error() {
            div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700 mb-6",
                {localize(&err)}
            }
        }
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-skill-empty").to_string(),
            empty_detail: rsx! {
                p { class: "text-sm text-slate mt-2", {t!("agents-skill-empty-hint")} }
            },
            div { class: "space-y-4",
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|s: &AgentSkill| s.id.clone()),
                    on_row_click: move |id: String| open.set(Some(id)),
                }
                ListPager { count, page }
            }
        }
    }
}

/// A file read as the text of a skill.
async fn file_text(evt: Event<FormData>) -> Option<String> {
    let file = evt.files().first().cloned()?;
    file.read_string().await.ok()
}

#[component]
fn NewSkill(adding: Signal<bool>, open: Signal<Option<String>>, reload: Signal<u32>) -> Element {
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut body = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let valid = !name().trim().is_empty() && !body().trim().is_empty();
    let submit = move |_| {
        let input = AgentSkillCreate {
            name: name().trim().to_string(),
            description: description().trim().to_string(),
            body: body(),
        };
        busy.set(true);
        spawn(async move {
            let result = api::agent_skills::create(&input).await;
            busy.set(false);
            match result {
                Ok(s) => {
                    adding.set(false);
                    reload.with_mut(|r| *r += 1);
                    open.set(Some(s.id));
                }
                Err(e) => form_error.set(Some(e)),
            }
        });
    };
    let import = move |evt: Event<FormData>| async move {
        if let Some(text) = file_text(evt).await {
            body.set(text);
        }
    };
    rsx! {
        FormDialog {
            title: t!("agents-skill-new").to_string(),
            submit_label: t!("common-create").to_string(),
            on_close: move |_| adding.set(false),
            busy: busy(),
            valid,
            max_width: "max-w-2xl".to_string(),
            on_submit: submit,
            div { class: "space-y-4",
                div {
                    label { r#for: "skill-name", class: FIELD_LABEL, {t!("agents-skill-field-name")} }
                    input {
                        id: "skill-name",
                        class: "{INPUT} w-full font-mono",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                    p { class: "text-xs text-slate mt-1", {t!("agents-skill-field-name-hint")} }
                }
                div {
                    label { r#for: "skill-description", class: FIELD_LABEL,
                        {t!("agents-skill-field-description")}
                    }
                    input {
                        id: "skill-description",
                        class: "{INPUT} w-full",
                        value: "{description}",
                        oninput: move |e| description.set(e.value()),
                    }
                }
                div {
                    label { r#for: "skill-body", class: FIELD_LABEL, {t!("agents-skill-field-body")} }
                    textarea {
                        id: "skill-body",
                        class: "{INPUT} h-64 w-full font-mono",
                        value: "{body}",
                        oninput: move |e| body.set(e.value()),
                    }
                    label { class: "{BTN_SECONDARY_SM} mt-2 cursor-pointer",
                        {t!("agents-skill-import")}
                        input {
                            class: "hidden",
                            r#type: "file",
                            accept: ".md,.markdown,.txt,text/markdown,text/plain",
                            onchange: import,
                        }
                    }
                }
                p { class: "text-xs text-slate", {t!("agents-skill-draft-hint")} }
                if let Some(e) = form_error() {
                    p { class: "text-sm text-red-700", {localize(&e)} }
                }
            }
        }
    }
}

#[component]
fn SkillEditor(id: String, open: Signal<Option<String>>, reload: Signal<u32>) -> Element {
    let can_write = access::can("agents:write");
    let mut tick = use_signal(|| 0u32);
    let mut text = use_signal(String::new);
    let mut dirty = use_signal(|| false);
    let mut versions_open = use_signal(|| false);
    let mut pinning = use_signal(|| false);
    let mut message = use_signal(|| Option::<String>::None);
    let mut restore_error = use_signal(|| Option::<String>::None);
    let skill_id = id.clone();
    let skill = use_resource(move || {
        let id = skill_id.clone();
        async move {
            let _ = tick();
            api::agent_skills::get(&id).await
        }
    });
    let versions_id = id.clone();
    let versions = use_resource(move || {
        let id = versions_id.clone();
        async move {
            let _ = tick();
            api::agent_skills::versions(&id).await
        }
    });
    // The newest text fills the editor, unless a change is being made.
    use_effect(move || {
        let newest = match &*versions.read() {
            Some(Ok(p)) => p.results.first().map(|v| v.body.clone()),
            _ => None,
        };
        if let Some(body) = newest {
            if !*dirty.peek() {
                text.set(body);
            }
        }
    });
    let current: Option<AgentSkill> = skill.read().as_ref().and_then(|r| r.as_ref().ok()).cloned();
    let rows: Option<Result<Vec<VersionRow>, ApiError>> = versions.read().as_ref().map(|r| {
        r.as_ref()
            .map(|p| {
                p.results
                    .iter()
                    .map(|v| VersionRow {
                        version: v.number,
                        yaml: v.body.clone(),
                        created_at: v.created_at.clone(),
                        author: None,
                    })
                    .collect()
            })
            .map_err(Clone::clone)
    });
    let status = match current.as_ref() {
        Some(s) => match s.active_number {
            Some(n) if n == s.latest_number => {
                EditorStatus::new(StatusTone::Green, t!("agents-skill-status-active", v : n))
            }
            Some(n) => EditorStatus::new(
                StatusTone::Amber,
                t!("agents-skill-status-review", latest : s.latest_number, active : n),
            ),
            None => EditorStatus::new(StatusTone::Slate, t!("agents-skill-status-draft")),
        },
        None => EditorStatus::new(StatusTone::Slate, t!("agents-skill-status-draft")),
    };
    let title = current.as_ref().map(|s| s.name.clone()).unwrap_or_default();
    let expected = current.as_ref().map(|s| s.version);
    let active = current.as_ref().and_then(|s| s.active_number);
    let save_id = id.clone();
    let save = move |_| {
        let Some(expected_version) = expected else {
            return;
        };
        let id = save_id.clone();
        let input = AgentSkillUpdate {
            expected_version,
            description: None,
            body: Some(text()),
        };
        spawn(async move {
            match api::agent_skills::update(&id, &input).await {
                Ok(_) => {
                    dirty.set(false);
                    message.set(None);
                    tick.with_mut(|n| *n += 1);
                    reload.with_mut(|n| *n += 1);
                }
                Err(e) => message.set(Some(localize(&e))),
            }
        });
    };
    let activate_id = id.clone();
    let on_activate = move |number: i32| {
        let Some(expected_version) = expected else {
            return;
        };
        let id = activate_id.clone();
        spawn(async move {
            let input = AgentSkillActivate {
                number,
                expected_version,
            };
            match api::agent_skills::activate(&id, &input).await {
                Ok(_) => {
                    restore_error.set(None);
                    versions_open.set(false);
                    tick.with_mut(|n| *n += 1);
                    reload.with_mut(|n| *n += 1);
                }
                Err(e) => restore_error.set(Some(localize(&e))),
            }
        });
    };
    let on_open = move |v: VersionRow| {
        versions_open.set(false);
        text.set(v.yaml);
        dirty.set(true);
    };
    let import = move |evt: Event<FormData>| async move {
        if let Some(body) = file_text(evt).await {
            text.set(body);
            dirty.set(true);
        }
    };
    rsx! {
        EditorShell {
            on_back: move |_| open.set(None),
            title,
            status,
            version: current.as_ref().map(|s| i64::from(s.latest_number)),
            extra_chips: rsx! {
                if dirty() {
                    Chip {
                        tone: StatusTone::Amber,
                        label: t!("agents-skill-unsaved").to_string(),
                    }
                }
            },
            actions: rsx! {
                if can_write {
                    label { class: "{BTN_SECONDARY_SM} cursor-pointer",
                        {t!("agents-skill-import")}
                        input {
                            class: "hidden",
                            r#type: "file",
                            accept: ".md,.markdown,.txt,text/markdown,text/plain",
                            onchange: import,
                        }
                    }
                }
                button { class: BTN_SECONDARY_SM, onclick: move |_| versions_open.set(true),
                    {t!("agents-skill-versions")}
                }
                if can_write && active.is_some() {
                    button { class: BTN_SECONDARY_SM, onclick: move |_| pinning.set(true), {t!("agents-skill-pin")} }
                }
                button { class: BTN_PRIMARY_SM, disabled: !can_write || !dirty(), onclick: save,
                    {t!("agents-skill-save")}
                }
            },
            banner: rsx! {
                div { class: "border-b border-line bg-amber-50 px-4 py-2 text-sm text-amber-800",
                    {t!("agents-skill-draft-hint")}
                    if let Some(m) = message() {
                        span { class: "ml-2 text-red-700", "{m}" }
                    }
                }
            },
            canvas: rsx! {
                textarea {
                    class: "h-full w-full resize-none bg-surface p-4 font-mono text-sm text-ink focus:outline-none",
                    readonly: !can_write,
                    value: "{text}",
                    oninput: move |e| {
                        text.set(e.value());
                        dirty.set(true);
                    },
                }
            },
        }
        if versions_open() {
            VersionsDrawer {
                rows,
                active_version: active.unwrap_or_default(),
                can_write,
                error: restore_error(),
                restore_label: Some(t!("agents-skill-activate").to_string()),
                on_close: move |_| versions_open.set(false),
                on_open,
                on_restore: on_activate,
            }
        }
        if pinning() {
            if let (Some(s), Some(v)) = (current.clone(), active) {
                PinDialog { skill_id: s.id, version: v, pinning }
            }
        }
    }
}

/// Pins the active version of a skill on an agent: a new version of its record.
#[component]
fn PinDialog(skill_id: String, version: i32, pinning: Signal<bool>) -> Element {
    let mut picked = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let agents = use_resource(move || async move { api::hosted_agents::agents(100, 0).await });
    let list: Vec<Agent> = match &*agents.value().read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    let chosen = list.iter().find(|a| a.id == picked()).cloned();
    let valid = chosen.is_some();
    let submit = move |_| {
        let Some(agent) = chosen.clone() else {
            return;
        };
        let mut spec = agent.spec.clone();
        spec.skills.retain(|p| p.skill_id != skill_id);
        spec.skills.push(SkillPin {
            skill_id: skill_id.clone(),
            version,
        });
        let input = AgentUpdate {
            expected_version: agent.version,
            description: None,
            spec,
        };
        busy.set(true);
        spawn(async move {
            let result = api::agent_skills::update_agent(&agent.id, &input).await;
            busy.set(false);
            match result {
                Ok(_) => pinning.set(false),
                Err(e) => form_error.set(Some(e)),
            }
        });
    };
    rsx! {
        FormDialog {
            title: t!("agents-skill-pin").to_string(),
            submit_label: t!("agents-skill-pin").to_string(),
            on_close: move |_| pinning.set(false),
            busy: busy(),
            valid,
            on_submit: submit,
            div { class: "space-y-4",
                div {
                    label { r#for: "skill-pin-agent", class: FIELD_LABEL,
                        {t!("agents-skill-pin-agent")}
                    }
                    select {
                        id: "skill-pin-agent",
                        class: "{INPUT} w-full",
                        onchange: move |e| picked.set(e.value()),
                        option { value: "", {t!("agents-skill-pin-pick")} }
                        for a in list.iter() {
                            option { key: "{a.id}", value: "{a.id}", {a.name.clone()} }
                        }
                    }
                }
                p { class: "text-xs text-slate", {t!("agents-skill-pin-hint", v : version)} }
                if let Some(e) = form_error() {
                    p { class: "text-sm text-red-700", {localize(&e)} }
                }
            }
        }
    }
}
