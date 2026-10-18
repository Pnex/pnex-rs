//! Page `/edges/browser-agents` (extension-collector.md D254–D270): the
//! browser agents of the org (paired from the extension, one per computer)
//! and their missions.
//!
//! Two tabs over the CRUD base (ListLayout + DataTable + ConfirmDialog).
//! Agents are never created here: the header's download button gets the
//! extension, which pairs itself after the user signs in. Missions are
//! edited on the version read (409 when someone saved in between).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::browser_agent::{
    BrowserAgent, CollectPageParams, CollectValueParams, FollowAccountParams, MediaPolicy, Mission,
    MissionCreate, MissionPatch, MissionSpec, MissionState, SocialPlatform, SocialVideo,
    VideoState, ATTR_TEXT, MIN_INTERVAL_SECS,
};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{page_slice, ListPager};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{
    BTN_SECONDARY, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT, PANEL, TAB, TAB_ACTIVE,
    TAB_BAR,
};
use crate::state::{org, session, toasts};

fn can_write() -> bool {
    let (Some(user), Some(org_id)) = (session::user(), org::current()) else {
        return false;
    };
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .is_some_and(|m| org::role_can_write(m.role))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Agents,
    Missions,
    Videos,
}

#[component]
pub fn BrowserAgents() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut tab = use_signal(|| Tab::Agents);
    let mut creating = use_signal(|| false);
    let write = can_write();
    let agents = use_resource(move || async move {
        let _ = reload();
        api::extension::agents().await
    });
    let add_label = (tab() == Tab::Missions).then(|| t!("browser-missions-new").to_string());
    let download = api::extension::download_url();
    rsx! {
        ListLayout {
            title: t!("browser-agents-title").to_string(),
            subtitle: Some(t!("browser-agents-subtitle").to_string()),
            tabs: rsx! {
                div { class: TAB_BAR, role: "tablist",
                    for (value, key) in [
                        (Tab::Agents, "browser-agents-tab-agents"),
                        (Tab::Missions, "browser-agents-tab-missions"),
                        (Tab::Videos, "browser-agents-tab-videos"),
                    ]
                    {
                        button {
                            key: "{key}",
                            class: if tab() == value { TAB_ACTIVE } else { TAB },
                            r#type: "button",
                            role: "tab",
                            aria_selected: tab() == value,
                            onclick: move |_| tab.set(value),
                            {t!(key)}
                        }
                    }
                }
            },
            actions: rsx! {
                a {
                    class: BTN_SECONDARY,
                    href: "{download}",
                    download: "pnex-browser-agent.zip",
                    {t!("browser-agents-download")}
                }
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: write,
            add_label,
            on_add: move |_| creating.set(true),
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else if tab() == Tab::Agents {
                AgentsTab { agents, can_write: write, reload }
            } else if tab() == Tab::Videos {
                VideosTab { reload }
            } else {
                MissionsTab {
                    agents,
                    can_write: write,
                    reload,
                    creating,
                }
            }
        }
    }
}

type AgentsResource = Resource<Result<Vec<BrowserAgent>, api::error::ApiError>>;

#[derive(Clone, PartialEq)]
enum AgentDialog {
    Rename(BrowserAgent),
    Delete(BrowserAgent),
}

#[component]
fn AgentsTab(agents: AgentsResource, can_write: bool, reload: Signal<u32>) -> Element {
    let mut reload = reload;
    let mut dialog = use_signal(|| None::<AgentDialog>);
    let page = use_signal(|| 0i64);
    let (state, count, rows) = match &*agents.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(all)) => (Some(Ok(())), all.len() as i64, page_slice(all, page())),
        Some(Err(err)) => (Some(Err(err.clone())), 0, Vec::new()),
    };
    let columns = vec![
        Column::new(t!("browser-agents-col-name").to_string(), |a: &BrowserAgent| {
            rsx! {
                p { class: "font-medium text-gray-900", "{a.name}" }
                p { class: "text-xs text-gray-500 font-mono", "{a.device_id}" }
            }
        }),
        Column::new(t!("browser-agents-col-browser").to_string(), |a: &BrowserAgent| {
            let fp = &a.fingerprint;
            let line = [fp.browser.as_str(), fp.browser_version.as_str(), fp.os.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            rsx! { "{line}" }
        })
        .with_td_class("text-gray-600")
        .secondary(),
        Column::new(t!("browser-agents-col-status").to_string(), |a: &BrowserAgent| {
            rsx! {
                if a.online {
                    span { class: "rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-800",
                        {t!("browser-agents-online")}
                    }
                } else {
                    span { class: "rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-600",
                        {t!("browser-agents-offline")}
                    }
                }
            }
        }),
        Column::new(t!("browser-agents-col-paired").to_string(), |a: &BrowserAgent| {
            rsx! { {date_label(&a.created_at)} }
        })
        .with_td_class("text-gray-600")
        .secondary(),
        Column::new(t!("common-actions").to_string(), move |a: &BrowserAgent| {
            let (to_rename, to_delete) = (a.clone(), a.clone());
            rsx! {
                if can_write {
                    div { class: "flex justify-end gap-2",
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: move |_| dialog.set(Some(AgentDialog::Rename(to_rename.clone()))),
                            {t!("browser-agents-rename")}
                        }
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| dialog.set(Some(AgentDialog::Delete(to_delete.clone()))),
                            {t!("browser-agents-delete")}
                        }
                    }
                }
            }
        })
        .actions(),
    ];
    rsx! {
        div { class: "space-y-4",
            InstallSteps {}
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("browser-agents-empty").to_string(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|a: &BrowserAgent| a.id.to_string()),
                }
                ListPager { count, page }
            }
        }
        match dialog() {
            Some(AgentDialog::Rename(agent)) => rsx! {
                RenameAgent {
                    key: "rename-{agent.id}",
                    agent: agent.clone(),
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(AgentDialog::Delete(agent)) => rsx! {
                ConfirmDialog {
                    title: t!("browser-agents-delete-title"),
                    message: t!("browser-agents-delete-message", name : agent.name.clone()),
                    confirm_label: t!("browser-agents-delete"),
                    on_confirm: {
                        let id = agent.id;
                        move |_| {
                            dialog.set(None);
                            spawn(async move {
                                match api::devices::delete(id).await {
                                    Ok(()) => reload.with_mut(|r| *r += 1),
                                    Err(err) => toasts::error(err),
                                }
                            });
                        }
                    },
                    on_cancel: move |_| dialog.set(None),
                }
            },
            None => rsx! {},
        }
    }
}

/// How to install the extension (D270): download, load unpacked, sign in.
#[component]
fn InstallSteps() -> Element {
    rsx! {
        div { class: "{PANEL} p-4 text-sm text-gray-700 space-y-2",
            p { class: "font-medium text-gray-900", {t!("browser-agents-install-title")} }
            ol { class: "list-decimal pl-5 space-y-1",
                li { {t!("browser-agents-install-step-download")} }
                li { {t!("browser-agents-install-step-load")} }
                li { {t!("browser-agents-install-step-signin")} }
                li { {t!("browser-agents-install-step-allow")} }
            }
            p { class: "text-xs text-gray-500", {t!("browser-agents-install-one-per-computer")} }
        }
    }
}

#[component]
fn RenameAgent(agent: BrowserAgent, on_close: Callback<()>, on_saved: Callback<()>) -> Element {
    let mut name = use_signal(|| agent.name.clone());
    let mut busy = use_signal(|| false);
    let save = move |_| {
        let (id, value) = (agent.id, name());
        busy.set(true);
        spawn(async move {
            let res = api::extension::rename_agent(id, value).await;
            busy.set(false);
            match res {
                Ok(_) => on_saved.call(()),
                Err(err) => toasts::error(err),
            }
        });
    };
    rsx! {
        FormDialog {
            title: t!("browser-agents-rename-title").to_string(),
            submit_label: t!("browser-agents-save").to_string(),
            on_close,
            on_submit: save,
            busy: busy(),
            valid: !name().trim().is_empty(),
            div {
                label { r#for: "agent-name", class: FIELD_LABEL, {t!("browser-agents-col-name")} }
                input {
                    id: "agent-name",
                    class: INPUT,
                    value: "{name}",
                    maxlength: 120,
                    oninput: move |e| name.set(e.value()),
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum MissionDialog {
    Edit(Mission),
    Delete(Mission),
}

/// Label of a mission state, with its agent code as a sentence.
fn state_label(m: &Mission) -> String {
    let state = match m.state {
        MissionState::Idle => t!("browser-missions-state-idle"),
        MissionState::Running => t!("browser-missions-state-running"),
        MissionState::Blocked => t!("browser-missions-state-blocked"),
        MissionState::Failed => t!("browser-missions-state-failed"),
    };
    match &m.state_code {
        Some(code) => format!(
            "{state} — {}",
            crate::api::error_i18n::localize_code(code, None, code)
        ),
        None => state.to_string(),
    }
}

#[component]
fn MissionsTab(
    agents: AgentsResource,
    can_write: bool,
    reload: Signal<u32>,
    creating: Signal<bool>,
) -> Element {
    let mut reload = reload;
    let mut creating = creating;
    let mut dialog = use_signal(|| None::<MissionDialog>);
    let missions = use_resource(move || async move {
        let _ = reload();
        api::extension::missions().await
    });
    let agent_list: Vec<BrowserAgent> = agents
        .value()
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned()
        .unwrap_or_default();
    let names = agent_list.clone();
    let page = use_signal(|| 0i64);
    let (state, count, rows) = match &*missions.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(all)) => (Some(Ok(())), all.len() as i64, page_slice(all, page())),
        Some(Err(err)) => (Some(Err(err.clone())), 0, Vec::new()),
    };
    let toggle = move |m: Mission| {
        let patch = MissionPatch {
            expected_version: m.version,
            enabled: Some(!m.enabled),
            ..Default::default()
        };
        spawn(async move {
            match api::extension::update_mission(&m.id, &patch).await {
                Ok(_) => reload.with_mut(|r| *r += 1),
                Err(err) => toasts::error(err),
            }
        });
    };
    let columns = vec![
        Column::new(t!("browser-missions-col-name").to_string(), |m: &Mission| {
            rsx! {
                p { class: "font-medium text-gray-900", "{m.name}" }
                p { class: "text-xs text-gray-500 font-mono break-all", "{m.target}" }
            }
        }),
        Column::new(t!("browser-missions-col-interval").to_string(), |m: &Mission| {
            let secs = m.interval_secs.to_string();
            rsx! { {t!("browser-missions-interval-value", secs : secs)} }
        })
        .with_td_class("text-gray-600")
        .secondary(),
        Column::new(t!("browser-missions-col-agent").to_string(), move |m: &Mission| {
            let label = match m.agent_id {
                Some(id) => names.iter().find(|a| a.id == id).map_or_else(|| id.to_string(), |a| a.name.clone()),
                None => t!("browser-missions-any-agent").to_string(),
            };
            rsx! { "{label}" }
        })
        .with_td_class("text-gray-600")
        .secondary(),
        Column::new(t!("browser-missions-col-state").to_string(), |m: &Mission| {
            let label = if m.enabled { state_label(m) } else { t!("browser-missions-disabled").to_string() };
            rsx! { "{label}" }
        })
        .with_td_class("text-gray-600"),
        Column::new(t!("common-actions").to_string(), move |m: &Mission| {
            let (to_toggle, to_edit, to_delete) = (m.clone(), m.clone(), m.clone());
            let toggle_key = if m.enabled { "browser-missions-disable" } else { "browser-missions-enable" };
            rsx! {
                if can_write {
                    div { class: "flex justify-end gap-2",
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: move |_| toggle(to_toggle.clone()),
                            {t!(toggle_key)}
                        }
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: move |_| dialog.set(Some(MissionDialog::Edit(to_edit.clone()))),
                            {t!("browser-missions-edit")}
                        }
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| dialog.set(Some(MissionDialog::Delete(to_delete.clone()))),
                            {t!("browser-agents-delete")}
                        }
                    }
                }
            }
        })
        .actions(),
    ];
    rsx! {
        div { class: "space-y-2",
            p { class: "text-xs text-gray-500", {t!("browser-missions-help")} }
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("browser-missions-empty").to_string(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|m: &Mission| m.id.clone()),
                }
                ListPager { count, page }
            }
        }
        if creating() {
            MissionForm {
                mission: None,
                agents: agent_list.clone(),
                on_close: move |_| creating.set(false),
                on_saved: move |_| {
                    creating.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        match dialog() {
            Some(MissionDialog::Edit(m)) => rsx! {
                MissionForm {
                    key: "edit-{m.id}",
                    mission: Some(m.clone()),
                    agents: agent_list.clone(),
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(MissionDialog::Delete(m)) => rsx! {
                ConfirmDialog {
                    title: t!("browser-missions-delete-title"),
                    message: t!("browser-missions-delete-message", name : m.name.clone()),
                    confirm_label: t!("browser-agents-delete"),
                    on_confirm: {
                        let id = m.id.clone();
                        move |_| {
                            let id = id.clone();
                            dialog.set(None);
                            spawn(async move {
                                match api::extension::delete_mission(&id).await {
                                    Ok(_) => reload.with_mut(|r| *r += 1),
                                    Err(err) => toasts::error(err),
                                }
                            });
                        }
                    },
                    on_cancel: move |_| dialog.set(None),
                }
            },
            None => rsx! {},
        }
    }
}

/// Mission kinds offered by the form (D256), with their label key.
const KINDS: [(&str, &str); 3] = [
    ("collect_value", "browser-missions-kind-value"),
    ("collect_page", "browser-missions-kind-page"),
    ("follow_account", "browser-missions-kind-account"),
];

/// Video policies of a followed account (D267), with their label key.
const POLICIES: [(&str, &str); 3] = [
    (
        "if_no_transcript",
        "browser-missions-policy-if-no-transcript",
    ),
    ("always", "browser-missions-policy-always"),
    ("metadata_only", "browser-missions-policy-metadata-only"),
];

fn policy_of(wire: &str) -> MediaPolicy {
    match wire {
        "always" => MediaPolicy::Always,
        "metadata_only" => MediaPolicy::MetadataOnly,
        _ => MediaPolicy::IfNoTranscript,
    }
}

/// Create (`mission: None`) or edit form of a mission.
#[component]
fn MissionForm(
    mission: Option<Mission>,
    agents: Vec<BrowserAgent>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let value = match mission.as_ref().map(|m| &m.spec) {
        Some(MissionSpec::CollectValue(p)) => Some(p.clone()),
        _ => None,
    };
    let initial_url = match mission.as_ref().map(|m| &m.spec) {
        Some(MissionSpec::CollectValue(p)) => p.url.clone(),
        Some(MissionSpec::CollectPage(p)) => p.url.clone(),
        Some(MissionSpec::FollowAccount(_)) | None => String::new(),
    };
    let account = match mission.as_ref().map(|m| &m.spec) {
        Some(MissionSpec::FollowAccount(p)) => Some(p.clone()),
        _ => None,
    };
    let initial_policy = match account.as_ref().map(|a| a.media_policy) {
        Some(MediaPolicy::Always) => "always",
        Some(MediaPolicy::MetadataOnly) => "metadata_only",
        _ => "if_no_transcript",
    };
    let mut handle = use_signal(|| {
        account
            .as_ref()
            .map(|a| a.handle.clone())
            .unwrap_or_default()
    });
    let mut policy = use_signal(|| initial_policy.to_string());
    let initial_kind = mission.as_ref().map_or("collect_value", |m| m.spec.kind());
    let mut kind = use_signal(|| initial_kind.to_string());
    let mut name = use_signal(|| mission.as_ref().map(|m| m.name.clone()).unwrap_or_default());
    let mut url = use_signal(|| initial_url);
    let mut selector = use_signal(|| {
        value
            .as_ref()
            .map(|p| p.selector.clone())
            .unwrap_or_default()
    });
    let mut attr = use_signal(|| {
        value
            .as_ref()
            .map_or_else(|| ATTR_TEXT.to_string(), |p| p.attr.clone())
    });
    let mut metric = use_signal(|| value.as_ref().map(|p| p.metric.clone()).unwrap_or_default());
    let mut interval = use_signal(|| {
        mission
            .as_ref()
            .map_or(300, |m| m.interval_secs)
            .to_string()
    });
    let mut agent = use_signal(|| {
        mission
            .as_ref()
            .and_then(|m| m.agent_id)
            .map(|id| id.to_string())
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let editing = mission.clone();
    let save = move |_| {
        let spec = match kind().as_str() {
            "collect_page" => MissionSpec::CollectPage(CollectPageParams { url: url() }),
            "follow_account" => MissionSpec::FollowAccount(FollowAccountParams {
                platform: SocialPlatform::X,
                handle: handle(),
                media_policy: policy_of(&policy()),
            }),
            _ => MissionSpec::CollectValue(CollectValueParams {
                url: url(),
                selector: selector(),
                attr: attr(),
                metric: metric(),
            }),
        };
        let interval_secs = interval().trim().parse::<i32>().unwrap_or(0);
        let agent_id = agent().parse::<i64>().ok();
        let editing = editing.clone();
        busy.set(true);
        spawn(async move {
            let res = match editing {
                Some(m) => {
                    let patch = MissionPatch {
                        expected_version: m.version,
                        name: Some(name()),
                        spec: Some(spec),
                        interval_secs: Some(interval_secs),
                        agent_id: Some(agent_id),
                        enabled: None,
                    };
                    api::extension::update_mission(&m.id, &patch).await
                }
                None => {
                    let input = MissionCreate {
                        name: name(),
                        spec,
                        interval_secs,
                        agent_id,
                        enabled: true,
                    };
                    api::extension::create_mission(&input).await
                }
            };
            busy.set(false);
            match res {
                Ok(_) => on_saved.call(()),
                Err(err) => toasts::error(err),
            }
        });
    };
    let title = if mission.is_some() {
        t!("browser-missions-edit-title")
    } else {
        t!("browser-missions-create-title")
    };
    let is_value = kind() == "collect_value";
    let is_account = kind() == "follow_account";
    let target_ok = if is_account {
        !handle().trim().is_empty()
    } else {
        !url().trim().is_empty()
    };
    let valid = !name().trim().is_empty()
        && target_ok
        && (!is_value || (!selector().trim().is_empty() && !metric().trim().is_empty()));
    let min = MIN_INTERVAL_SECS.to_string();
    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("browser-agents-save").to_string(),
            on_close,
            on_submit: save,
            busy: busy(),
            valid,
            div { class: "space-y-3",
                div {
                    label { r#for: "mission-kind", class: FIELD_LABEL,
                        {t!("browser-missions-field-kind")}
                    }
                    select {
                        id: "mission-kind",
                        class: INPUT,
                        value: "{kind}",
                        onchange: move |e| kind.set(e.value()),
                        for (value, key) in KINDS {
                            option {
                                key: "{value}",
                                value: "{value}",
                                selected: kind() == value,
                                {t!(key)}
                            }
                        }
                    }
                }
                div {
                    label { r#for: "mission-name", class: FIELD_LABEL,
                        {t!("browser-missions-field-name")}
                    }
                    input {
                        id: "mission-name",
                        class: INPUT,
                        value: "{name}",
                        maxlength: 120,
                        oninput: move |e| name.set(e.value()),
                    }
                }
                if is_account {
                    div {
                        label { r#for: "mission-handle", class: FIELD_LABEL,
                            {t!("browser-missions-field-handle")}
                        }
                        input {
                            id: "mission-handle",
                            class: "{INPUT} font-mono",
                            placeholder: "account",
                            value: "{handle}",
                            oninput: move |e| handle.set(e.value()),
                        }
                        p { class: FIELD_HELP, {t!("browser-missions-field-handle-help")} }
                    }
                    div {
                        label { r#for: "mission-policy", class: FIELD_LABEL,
                            {t!("browser-missions-field-policy")}
                        }
                        select {
                            id: "mission-policy",
                            class: INPUT,
                            value: "{policy}",
                            onchange: move |e| policy.set(e.value()),
                            for (value, key) in POLICIES {
                                option {
                                    key: "{value}",
                                    value: "{value}",
                                    selected: policy() == value,
                                    {t!(key)}
                                }
                            }
                        }
                    }
                } else {
                    div {
                        label { r#for: "mission-url", class: FIELD_LABEL,
                            {t!("browser-missions-field-url")}
                        }
                        input {
                            id: "mission-url",
                            class: INPUT,
                            r#type: "url",
                            placeholder: "https://",
                            value: "{url}",
                            oninput: move |e| url.set(e.value()),
                        }
                    }
                }
                if is_value {
                    div {
                        label { r#for: "mission-selector", class: FIELD_LABEL,
                            {t!("browser-missions-field-selector")}
                        }
                        input {
                            id: "mission-selector",
                            class: "{INPUT} font-mono",
                            placeholder: ".price",
                            value: "{selector}",
                            oninput: move |e| selector.set(e.value()),
                        }
                        p { class: FIELD_HELP, {t!("browser-missions-field-selector-help")} }
                    }
                    div {
                        label { r#for: "mission-attr", class: FIELD_LABEL,
                            {t!("browser-missions-field-attr")}
                        }
                        input {
                            id: "mission-attr",
                            class: "{INPUT} font-mono",
                            value: "{attr}",
                            oninput: move |e| attr.set(e.value()),
                        }
                        p { class: FIELD_HELP, {t!("browser-missions-field-attr-help")} }
                    }
                    div {
                        label { r#for: "mission-metric", class: FIELD_LABEL,
                            {t!("browser-missions-field-metric")}
                        }
                        input {
                            id: "mission-metric",
                            class: "{INPUT} font-mono",
                            placeholder: "price",
                            value: "{metric}",
                            oninput: move |e| metric.set(e.value()),
                        }
                        p { class: FIELD_HELP, {t!("browser-missions-field-metric-help")} }
                    }
                }
                div {
                    label { r#for: "mission-interval", class: FIELD_LABEL,
                        {t!("browser-missions-field-interval")}
                    }
                    input {
                        id: "mission-interval",
                        class: INPUT,
                        r#type: "number",
                        min: "{min}",
                        value: "{interval}",
                        oninput: move |e| interval.set(e.value()),
                    }
                    p { class: FIELD_HELP,
                        {t!("browser-missions-field-interval-help", min : min.clone())}
                    }
                }
                div {
                    label { r#for: "mission-agent", class: FIELD_LABEL,
                        {t!("browser-missions-field-agent")}
                    }
                    select {
                        id: "mission-agent",
                        class: INPUT,
                        value: "{agent}",
                        onchange: move |e| agent.set(e.value()),
                        option { value: "", selected: agent().is_empty(),
                            {t!("browser-missions-any-agent")}
                        }
                        for a in agents.iter() {
                            option {
                                key: "{a.id}",
                                value: "{a.id}",
                                selected: agent() == a.id.to_string(),
                                "{a.name}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The video registry (D259): what the agents saw on followed accounts,
/// paginated by the server (it grows).
#[component]
fn VideosTab(reload: Signal<u32>) -> Element {
    let page = use_signal(|| 0i64);
    let videos = use_resource(move || async move {
        let _ = reload();
        let size = crate::components::crud::pager::PAGE_SIZE;
        api::extension::videos(size, page() * size).await
    });
    let (state, count, rows) = match &*videos.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count, p.results.clone()),
        Some(Err(err)) => (Some(Err(err.clone())), 0, Vec::new()),
    };
    let columns = vec![
        Column::new(
            t!("browser-videos-col-post").to_string(),
            |v: &SocialVideo| {
                let text: String = v.text.chars().take(140).collect();
                rsx! {
                    a {
                        class: "font-medium text-gray-900 hover:underline",
                        href: "{v.url}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "@{v.author}"
                    }
                    p { class: "text-xs text-gray-500 break-words", "{text}" }
                }
            },
        ),
        Column::new(
            t!("browser-videos-col-published").to_string(),
            |v: &SocialVideo| {
                let date = v
                    .published_at
                    .as_deref()
                    .map(date_label)
                    .unwrap_or_default();
                rsx! { "{date}" }
            },
        )
        .with_td_class("text-gray-600")
        .secondary(),
        Column::new(
            t!("browser-videos-col-state").to_string(),
            |v: &SocialVideo| {
                let label = match (v.state, v.has_video) {
                    (VideoState::Captured, _) => t!("browser-videos-captured"),
                    (VideoState::Seen, true) => t!("browser-videos-seen-video"),
                    (VideoState::Seen, false) => t!("browser-videos-seen-text"),
                };
                rsx! { "{label}" }
            },
        )
        .with_td_class("text-gray-600"),
    ];
    rsx! {
        div { class: "space-y-2",
            p { class: "text-xs text-gray-500", {t!("browser-videos-help")} }
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("browser-videos-empty").to_string(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|v: &SocialVideo| v.platform_id.clone()),
                }
                ListPager { count, page }
            }
        }
    }
}
