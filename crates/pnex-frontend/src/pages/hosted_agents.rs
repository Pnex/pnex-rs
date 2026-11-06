//! Agents page (agent-environments.md D757, D761): the agents of the org and their runs.
//! A run opens on its log and its outputs; an output is only downloaded, never shown
//! (R11), and its download needs `agent-artifacts:read`.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::error::ApiError;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{FIELD_LABEL, INPUT, TAB, TAB_ACTIVE, TAB_BAR};
use crate::state::access;
use crate::state::toasts;
use pnex_core::agent_runs::{
    AgentArtifact, AgentRun, ArtifactInput, ArtifactOutput, ArtifactState,
};
use pnex_core::agents::{Agent, AgentCreate, AgentRole, AgentRunStart, AgentSpec};
use pnex_core::environments::{
    Environment, EnvironmentBuild, EnvironmentBuildState, Isolation, IsolationInput,
};
use pnex_core::runners::{RunnerJobOutcome, RunnerJobState, RunnerView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Agents,
    Runs,
    Environments,
    Runners,
}

/// `2026-10-01T13:44:11.000Z` → `2026-10-01 13:44`.
fn short_date(at: &str) -> String {
    at.get(..16).unwrap_or(at).replace('T', " ")
}

/// Names of a comma-separated field, trimmed, empty ones left out.
fn names(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn state_label(run: &AgentRun) -> String {
    match (run.state, run.outcome) {
        (RunnerJobState::Queued, _) => t!("agents-run-queued").to_string(),
        (RunnerJobState::Running, _) => t!("agents-run-running").to_string(),
        (_, Some(RunnerJobOutcome::Succeeded)) => t!("agents-run-succeeded").to_string(),
        (_, Some(RunnerJobOutcome::Stopped)) => t!("agents-run-stopped").to_string(),
        (_, Some(RunnerJobOutcome::Timeout)) => t!("agents-run-timeout").to_string(),
        _ => t!("agents-run-failed").to_string(),
    }
}

fn output_state(a: &AgentArtifact) -> String {
    match a.state {
        ArtifactState::Pending => t!("agents-output-pending").to_string(),
        ArtifactState::Ready => t!("agents-output-ready").to_string(),
        ArtifactState::Refused => t!("agents-output-refused").to_string(),
        ArtifactState::Missing => t!("agents-output-missing").to_string(),
    }
}

#[component]
pub fn HostedAgents() -> Element {
    let mut tab = use_signal(|| Tab::Agents);
    let mut reload = use_signal(|| 0u32);
    let mut adding = use_signal(|| false);
    let can_write = access::can("agents:write");
    let tabs = [
        (Tab::Agents, "agents-tab-agents"),
        (Tab::Runs, "agents-tab-runs"),
        (Tab::Environments, "agents-tab-environments"),
        (Tab::Runners, "agents-tab-runners"),
    ];
    rsx! {
        ListLayout {
            title: t!("agents-title").to_string(),
            subtitle: Some(t!("agents-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: can_write && tab() == Tab::Agents,
            add_label: Some(t!("agents-new").to_string()),
            on_add: move |_| adding.set(true),
            tabs: rsx! {
                div { class: TAB_BAR, role: "tablist",
                    for (id, key) in tabs {
                        button {
                            key: "{key}",
                            class: if tab() == id { TAB_ACTIVE } else { TAB },
                            r#type: "button",
                            role: "tab",
                            aria_selected: tab() == id,
                            onclick: move |_| tab.set(id),
                            {t!(key)}
                        }
                    }
                }
            },
            match tab() {
                Tab::Agents => rsx! {
                    AgentsTab { reload, adding }
                },
                Tab::Runs => rsx! {
                    RunsTab { reload }
                },
                Tab::Environments => rsx! {
                    EnvironmentsTab { reload }
                },
                Tab::Runners => rsx! {
                    RunnersTab { reload }
                },
            }
        }
    }
}

#[component]
fn AgentsTab(reload: Signal<u32>, adding: Signal<bool>) -> Element {
    let page = use_signal(|| 0i64);
    let mut error = use_signal(|| Option::<ApiError>::None);
    let mut confirm_delete = use_signal(|| Option::<String>::None);
    let mut starting = use_signal(|| Option::<Agent>::None);
    let can_delete = access::can("agents:delete");
    let can_run = access::can("agents:run");
    let list = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            api::hosted_agents::agents(PAGE_SIZE, offset).await
        }
    });
    let (state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, 0, Vec::new()),
    };
    let mut columns = vec![
        Column::new(t!("agents-col-name").to_string(), |a: &Agent| {
            let description = a.description.clone();
            rsx! {
                div { class: "text-sm font-medium text-ink", {a.name.clone()} }
                if !description.is_empty() {
                    div { class: "text-xs text-slate mt-0.5", {description} }
                }
            }
        }),
        Column::new(t!("agents-col-role").to_string(), |a: &Agent| {
            let role = match a.role {
                AgentRole::Viewer => t!("agents-role-viewer"),
                AgentRole::Member => t!("agents-role-member"),
            };
            rsx! {
                span { class: "text-sm text-slate", {role} }
            }
        }),
        Column::new(t!("agents-col-files").to_string(), |a: &Agent| {
            let ins = a.spec.inputs.join(", ");
            let outs = a
                .spec
                .outputs
                .iter()
                .map(|o| o.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            rsx! {
                div { class: "text-xs text-slate", {t!("agents-inputs-list", list : ins)} }
                div { class: "text-xs text-slate", {t!("agents-outputs-list", list : outs)} }
            }
        })
        .secondary(),
        Column::new(t!("agents-col-version").to_string(), |a: &Agent| {
            rsx! {
                span { class: "text-sm text-slate", "v{a.version}" }
            }
        })
        .secondary(),
    ];
    columns.push(
        Column::new(t!("common-actions").to_string(), move |a: &Agent| {
            let agent = a.clone();
            let id = a.id.clone();
            let confirm_id = id.clone();
            rsx! {
                div { class: "flex gap-1",
                    if can_run {
                        button {
                            class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                            onclick: move |_| starting.set(Some(agent.clone())),
                            {t!("agents-start")}
                        }
                    }
                    if can_delete {
                        if confirm_delete() == Some(confirm_id.clone()) {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| {
                                    let id = id.clone();
                                    spawn(async move {
                                        match api::hosted_agents::delete_agent(&id).await {
                                            Ok(()) => {
                                                error.set(None);
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(e) => error.set(Some(e)),
                                        }
                                        confirm_delete.set(None);
                                    });
                                },
                                {t!("agents-confirm-delete")}
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
    );
    rsx! {
        if adding() {
            NewAgent { adding, reload }
        }
        if let Some(agent) = starting() {
            StartRun { agent, starting, reload }
        }
        if let Some(err) = error() {
            div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700 mb-6",
                {crate::api::error_i18n::localize(&err)}
            }
        }
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-empty").to_string(),
            empty_detail: rsx! {
                p { class: "text-sm text-slate mt-2", {t!("agents-empty-hint")} }
            },
            div { class: "space-y-4",
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|a: &Agent| a.id.clone()),
                }
                ListPager { count, page }
            }
        }
    }
}

#[component]
fn NewAgent(adding: Signal<bool>, reload: Signal<u32>) -> Element {
    let mut name = use_signal(String::new);
    let mut environment = use_signal(String::new);
    let mut command = use_signal(String::new);
    let mut timeout = use_signal(|| "1800".to_string());
    let mut calls = use_signal(|| "200".to_string());
    let mut inputs = use_signal(String::new);
    let mut required = use_signal(String::new);
    let mut optional = use_signal(String::new);
    let mut member = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let environments =
        use_resource(move || async move { api::hosted_agents::environments().await });
    let options: Vec<(String, String)> = match &*environments.value().read() {
        Some(Ok(p)) => p
            .results
            .iter()
            .map(|e| (e.id.clone(), e.name.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let valid = !name().trim().is_empty() && !environment().is_empty();
    rsx! {
        FormDialog {
            title: t!("agents-new").to_string(),
            submit_label: t!("common-create").to_string(),
            on_close: move |_| adding.set(false),
            busy: busy(),
            valid,
            on_submit: move |_| {
                let mut outputs: Vec<ArtifactOutput> = names(&required())
                    .into_iter()
                    .map(|name| ArtifactOutput {
                        name,
                        required: true,
                    })
                    .collect();
                outputs
                    .extend(
                        names(&optional())
                            .into_iter()
                            .map(|name| ArtifactOutput {
                                name,
                                required: false,
                            }),
                    );
                let input = AgentCreate {
                    name: name().trim().to_string(),
                    description: String::new(),
                    role: if member() { AgentRole::Member } else { AgentRole::Viewer },
                    spec: AgentSpec {
                        environment_id: environment(),
                        command: command().split_whitespace().map(str::to_string).collect(),
                        timeout_secs: timeout().trim().parse().unwrap_or(0),
                        llm_calls_max: calls().trim().parse().ok(),
                        inputs: names(&inputs()),
                        outputs,
                        ..Default::default()
                    },
                };
                busy.set(true);
                spawn(async move {
                    let result = api::hosted_agents::create_agent(&input).await;
                    busy.set(false);
                    match result {
                        Ok(_) => {
                            adding.set(false);
                            reload.with_mut(|r| *r += 1);
                        }
                        Err(e) => form_error.set(Some(e)),
                    }
                });
            },
            div { class: "space-y-4",
                div {
                    label { r#for: "agent-name", class: FIELD_LABEL, {t!("agents-field-name")} }
                    input {
                        id: "agent-name",
                        class: "{INPUT} w-full",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label { r#for: "agent-env", class: FIELD_LABEL, {t!("agents-field-environment")} }
                    select {
                        id: "agent-env",
                        class: "{INPUT} w-full",
                        onchange: move |e| environment.set(e.value()),
                        option { value: "", {t!("agents-field-environment-pick")} }
                        for (id, label) in options {
                            option { key: "{id}", value: "{id}", {label} }
                        }
                    }
                }
                div {
                    label { r#for: "agent-command", class: FIELD_LABEL, {t!("agents-field-command")} }
                    input {
                        id: "agent-command",
                        class: "{INPUT} w-full font-mono",
                        value: "{command}",
                        oninput: move |e| command.set(e.value()),
                    }
                    p { class: "text-xs text-slate mt-1", {t!("agents-field-command-hint")} }
                }
                div { class: "grid grid-cols-2 gap-3",
                    div {
                        label { r#for: "agent-timeout", class: FIELD_LABEL,
                            {t!("agents-field-timeout")}
                        }
                        input {
                            id: "agent-timeout",
                            class: "{INPUT} w-full",
                            r#type: "number",
                            value: "{timeout}",
                            oninput: move |e| timeout.set(e.value()),
                        }
                    }
                    div {
                        label { r#for: "agent-calls", class: FIELD_LABEL, {t!("agents-field-calls")} }
                        input {
                            id: "agent-calls",
                            class: "{INPUT} w-full",
                            r#type: "number",
                            value: "{calls}",
                            oninput: move |e| calls.set(e.value()),
                        }
                    }
                }
                div {
                    label { r#for: "agent-inputs", class: FIELD_LABEL, {t!("agents-field-inputs")} }
                    input {
                        id: "agent-inputs",
                        class: "{INPUT} w-full font-mono",
                        value: "{inputs}",
                        oninput: move |e| inputs.set(e.value()),
                    }
                }
                div {
                    label { r#for: "agent-required", class: FIELD_LABEL,
                        {t!("agents-field-required-outputs")}
                    }
                    input {
                        id: "agent-required",
                        class: "{INPUT} w-full font-mono",
                        value: "{required}",
                        oninput: move |e| required.set(e.value()),
                    }
                }
                div {
                    label { r#for: "agent-optional", class: FIELD_LABEL,
                        {t!("agents-field-optional-outputs")}
                    }
                    input {
                        id: "agent-optional",
                        class: "{INPUT} w-full font-mono",
                        value: "{optional}",
                        oninput: move |e| optional.set(e.value()),
                    }
                    p { class: "text-xs text-slate mt-1", {t!("agents-field-names-hint")} }
                }
                label { class: "flex items-center gap-2 text-sm text-ink",
                    input {
                        r#type: "checkbox",
                        checked: member(),
                        onchange: move |e| member.set(e.checked()),
                    }
                    {t!("agents-field-member")}
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

/// Picks one file per declared input, uploads them, then starts the run.
#[component]
fn StartRun(agent: Agent, starting: Signal<Option<Agent>>, reload: Signal<u32>) -> Element {
    let mut files = use_signal(Vec::<(String, Vec<u8>)>::new);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let declared = agent.spec.inputs.clone();
    let valid = declared
        .iter()
        .all(|name| files().iter().any(|(n, _)| n == name));
    let agent_id = agent.id.clone();
    rsx! {
        FormDialog {
            title: t!("agents-start-title", name : agent.name.clone()).to_string(),
            submit_label: t!("agents-start").to_string(),
            on_close: move |_| starting.set(None),
            busy: busy(),
            valid,
            on_submit: move |_| {
                let agent_id = agent_id.clone();
                let picked = files();
                busy.set(true);
                spawn(async move {
                    let mut inputs = Vec::new();
                    for (name, bytes) in picked {
                        match api::hosted_agents::upload_artifact(&name, bytes).await {
                            Ok(a) => {
                                inputs
                                    .push(ArtifactInput {
                                        name,
                                        artifact_id: a.id,
                                    })
                            }
                            Err(e) => {
                                busy.set(false);
                                form_error.set(Some(e));
                                return;
                            }
                        }
                    }
                    let start = AgentRunStart {
                        inputs,
                        ..Default::default()
                    };
                    let result = api::hosted_agents::start_run(&agent_id, &start).await;
                    busy.set(false);
                    match result {
                        Ok(_) => {
                            toasts::info(t!("agents-started").to_string());
                            starting.set(None);
                            reload.with_mut(|r| *r += 1);
                        }
                        Err(e) => form_error.set(Some(e)),
                    }
                });
            },
            div { class: "space-y-4",
                if declared.is_empty() {
                    p { class: "text-sm text-slate", {t!("agents-start-no-input")} }
                }
                for name in declared.clone() {
                    div { key: "{name}",
                        label { class: FIELD_LABEL, {t!("agents-start-input", name : name.clone())} }
                        input {
                            r#type: "file",
                            class: "text-sm",
                            onchange: move |evt: FormEvent| {
                                let name = name.clone();
                                async move {
                                    let Some(file) = evt.files().first().cloned() else {
                                        return;
                                    };
                                    let bytes = file.read_bytes().await.map(|b| b.to_vec()).unwrap_or_default();
                                    files
                                        .with_mut(|f| {
                                            f.retain(|(n, _)| n != &name);
                                            f.push((name, bytes));
                                        });
                                }
                            },
                        }
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

#[component]
fn RunsTab(reload: Signal<u32>) -> Element {
    let page = use_signal(|| 0i64);
    let mut open = use_signal(|| Option::<AgentRun>::None);
    let list = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            api::hosted_agents::runs(PAGE_SIZE, offset).await
        }
    });
    let (state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, 0, Vec::new()),
    };
    let columns = vec![
        Column::new(t!("agents-col-started").to_string(), |r: &AgentRun| {
            rsx! {
                span { class: "text-sm text-ink", {short_date(&r.created_at)} }
            }
        }),
        Column::new(t!("agents-col-state").to_string(), |r: &AgentRun| {
            let error = r.error.clone();
            rsx! {
                div { class: "text-sm text-ink", {state_label(r)} }
                if let Some(code) = error {
                    div { class: "text-xs text-alarm", {crate::api::error_i18n::localize_code(&code, None, &code)} }
                }
            }
        }),
        Column::new(t!("agents-col-usage").to_string(), |r: &AgentRun| {
            rsx! {
                span { class: "text-sm text-slate",
                    {t!("agents-usage", calls : r.llm_calls, max : r.llm_calls_max, input : r.input_tokens, output : r.output_tokens)}
                }
            }
        })
        .secondary(),
        Column::new(t!("common-actions").to_string(), move |r: &AgentRun| {
            let run = r.clone();
            rsx! {
                button {
                    class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                    onclick: move |_| open.set(Some(run.clone())),
                    {t!("agents-open-run")}
                }
            }
        })
        .actions(),
    ];
    rsx! {
        if let Some(run) = open() {
            RunDetail { run, open, reload }
        }
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-runs-empty").to_string(),
            empty_detail: rsx! {},
            div { class: "space-y-4",
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|r: &AgentRun| r.id.clone()),
                }
                ListPager { count, page }
            }
        }
    }
}

#[component]
fn RunDetail(run: AgentRun, open: Signal<Option<AgentRun>>, reload: Signal<u32>) -> Element {
    let id = run.id.clone();
    let logs_id = id.clone();
    let outputs_id = id.clone();
    let logs = use_resource(move || {
        let id = logs_id.clone();
        async move { api::hosted_agents::run_logs(&id, 500, 0).await }
    });
    let outputs = use_resource(move || {
        let id = outputs_id.clone();
        async move { api::hosted_agents::run_outputs(&id).await }
    });
    let can_stop = access::can("agents:stop")
        && matches!(run.state, RunnerJobState::Queued | RunnerJobState::Running);
    let can_download = access::can("agent-artifacts:read");
    let lines = match &*logs.value().read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    let files = match &*outputs.value().read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    rsx! {
        crate::components::modal::Modal {
            title: t!("agents-run-title", date : short_date(& run.created_at)).to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close: move |_| open.set(None),
            div { class: "space-y-4",
                div { class: "text-sm text-ink", {state_label(&run)} }
                if can_stop {
                    button {
                        class: DANGER_BTN,
                        onclick: move |_| {
                            let id = id.clone();
                            spawn(async move {
                                match api::hosted_agents::stop_run(&id).await {
                                    Ok(_) => {
                                        open.set(None);
                                        reload.with_mut(|r| *r += 1);
                                    }
                                    Err(e) => toasts::error(e),
                                }
                            });
                        },
                        {t!("agents-stop")}
                    }
                }
                h3 { class: "text-sm font-semibold text-ink", {t!("agents-outputs")} }
                if files.is_empty() {
                    p { class: "text-sm text-slate", {t!("agents-no-outputs")} }
                }
                for file in files {
                    div {
                        key: "{file.id}",
                        class: "flex items-center justify-between gap-2 text-sm",
                        span { class: "font-mono text-ink", {file.name.clone()} }
                        span { class: "text-slate", {output_state(&file)} }
                        if can_download && file.state == ArtifactState::Ready {
                            DownloadButton { artifact: file.clone() }
                        }
                    }
                }
                h3 { class: "text-sm font-semibold text-ink", {t!("agents-log")} }
                if lines.is_empty() {
                    p { class: "text-sm text-slate", {t!("agents-no-log")} }
                } else {
                    pre { class: "max-h-96 overflow-auto rounded bg-gray-50 p-3 text-xs leading-5 text-ink",
                        for (i, l) in lines.into_iter().enumerate() {
                            div { key: "{i}",
                                span { class: "text-slate", "{l.container} │ " }
                                "{l.line}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Saves an output as a file: never displayed in the app (R11).
#[component]
fn DownloadButton(artifact: AgentArtifact) -> Element {
    let file_name = match artifact.form {
        Some(pnex_core::agent_runs::ArtifactForm::Tar) => format!("{}.tar", artifact.name),
        _ => artifact.name.clone(),
    };
    rsx! {
        button {
            class: "px-2 py-0.5 text-xs border border-gray-300 rounded hover:bg-gray-50",
            onclick: move |_| {
                let id = artifact.id.clone();
                let file_name = file_name.clone();
                spawn(async move {
                    match api::hosted_agents::artifact_content(&id).await {
                        Ok(bytes) => {
                            crate::util::trigger_download(
                                &file_name,
                                "application/octet-stream",
                                &bytes,
                            );
                        }
                        Err(e) => toasts::error(e),
                    }
                });
            },
            {t!("agents-download")}
        }
    }
}

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

/// Environments of the org with their active image; a build opens on its log.
#[component]
fn EnvironmentsTab(reload: Signal<u32>) -> Element {
    let mut open = use_signal(|| Option::<Environment>::None);
    let list = use_resource(move || async move {
        let _ = reload();
        api::hosted_agents::environments().await
    });
    let (state, is_empty, rows) = match &*list.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let columns =
        vec![
        Column::new(t!("agents-col-name").to_string(), |e: &Environment| {
            rsx! {
                span { class: "text-sm font-medium text-ink", {e.name.clone()} }
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
        Column::new(t!("agents-col-isolation").to_string(), move |e: &Environment| {
            rsx! {
                IsolationCell { key: "{e.id}-{e.version}", env: e.clone(), reload }
            }
        }),
        Column::new(t!("common-actions").to_string(), move |e: &Environment| {
            let env = e.clone();
            rsx! {
                button {
                    class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                    onclick: move |_| open.set(Some(env.clone())),
                    {t!("agents-builds")}
                }
            }
        })
        .actions(),
    ];
    rsx! {
        if let Some(env) = open() {
            BuildsDialog { env, open }
        }
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-environments-empty").to_string(),
            empty_detail: rsx! {},
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|e: &Environment| e.id.clone()),
            }
        }
    }
}

fn build_state(b: &EnvironmentBuild) -> String {
    match b.state {
        EnvironmentBuildState::Building => t!("agents-build-building").to_string(),
        EnvironmentBuildState::Ready => t!("agents-build-ready").to_string(),
        EnvironmentBuildState::Failed => t!("agents-build-failed").to_string(),
    }
}

#[component]
fn BuildsDialog(env: Environment, open: Signal<Option<Environment>>) -> Element {
    let env_id = env.id.clone();
    let mut log_of = use_signal(|| Option::<String>::None);
    let builds = use_resource(move || {
        let id = env_id.clone();
        async move { api::hosted_agents::builds(&id).await }
    });
    let logs_env = env.id.clone();
    let lines = use_resource(move || {
        let env = logs_env.clone();
        async move {
            match log_of() {
                Some(build) => api::hosted_agents::build_logs(&env, &build)
                    .await
                    .map(|p| p.results)
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
    });
    let rows = match &*builds.value().read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    let shown = lines.value().read().clone().unwrap_or_default();
    rsx! {
        crate::components::modal::Modal {
            title: t!("agents-builds-title", name : env.name.clone()).to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close: move |_| open.set(None),
            div { class: "space-y-3",
                if rows.is_empty() {
                    p { class: "text-sm text-slate", {t!("agents-no-builds")} }
                }
                for b in rows {
                    div {
                        key: "{b.id}",
                        class: "flex items-center justify-between gap-2 text-sm",
                        span { class: "text-ink", "v{b.number}" }
                        span { class: "text-slate", {build_state(&b)} }
                        span { class: "text-slate", {short_date(&b.created_at)} }
                        button {
                            class: "px-2 py-0.5 text-xs border border-gray-300 rounded hover:bg-gray-50",
                            onclick: move |_| log_of.set(Some(b.id.clone())),
                            {t!("agents-log")}
                        }
                    }
                }
                if log_of().is_some() {
                    pre { class: "max-h-96 overflow-auto rounded bg-gray-50 p-3 text-xs leading-5 text-ink",
                        if shown.is_empty() {
                            {t!("agents-no-log")}
                        }
                        for (i, l) in shown.into_iter().enumerate() {
                            div { key: "{i}", "{l.line}" }
                        }
                    }
                }
            }
        }
    }
}

/// Runners of the org: where runs and builds go.
#[component]
fn RunnersTab(reload: Signal<u32>) -> Element {
    let list = use_resource(move || async move {
        let _ = reload();
        api::hosted_agents::runners().await
    });
    let (state, is_empty, rows) = match &*list.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let columns = vec![
        Column::new(t!("agents-col-name").to_string(), |r: &RunnerView| {
            rsx! {
                span { class: "text-sm font-medium text-ink", {r.name.clone()} }
            }
        }),
        Column::new(t!("agents-col-state").to_string(), |r: &RunnerView| {
            let label = if r.online {
                t!("agents-runner-online")
            } else {
                t!("agents-runner-offline")
            };
            rsx! {
                span { class: "text-sm text-slate", {label} }
            }
        }),
        Column::new(
            t!("agents-col-capabilities").to_string(),
            |r: &RunnerView| {
                let caps = r.capabilities.join(" · ");
                rsx! {
                    span { class: "font-mono text-xs text-slate", {caps} }
                }
            },
        )
        .secondary(),
    ];
    rsx! {
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-runners-empty").to_string(),
            empty_detail: rsx! {},
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|r: &RunnerView| r.id.clone()),
            }
        }
    }
}
