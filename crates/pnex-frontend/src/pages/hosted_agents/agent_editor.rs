//! Editor of an agent's record (agent-environments.md D757, D710, D714, D717, D730, D744,
//! D775): every field of its spec, a save makes a new version on the version read, and the
//! common version history. The role is set at creation only; skills are pinned from the
//! skill editor and only shown here.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::error::ApiError;
use crate::components::icons;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::components::versions_drawer::{VersionRow, VersionsDrawer};
use crate::state::access;
use crate::state::toasts;
use pnex_core::agent_runs::{ArtifactOutput, RunCache, RunRepo, RunService};
use pnex_core::agent_skills::SkillPin;
use pnex_core::agents::{
    Agent, AgentLevel, AgentRole, AgentSpec, AgentToolInfo, AgentUpdate, AgentVersion, GateMode,
};

mod sections;

use sections::{
    CachesSection, KvList, OutputsSection, ReposSection, ServicesSection, SkillsSection,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ServiceDraft {
    pub name: String,
    pub image: String,
    pub env: Vec<(String, String)>,
    /// One argument per line.
    pub ready: String,
    /// Comma separated.
    pub data_paths: String,
    pub run_as_user: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CacheDraft {
    pub name: String,
    pub path: String,
    /// Comma separated.
    pub key_files: String,
}

/// The record as the form edits it: lists of text kept raw while typing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Draft {
    pub description: String,
    pub level: AgentLevel,
    pub environment_id: String,
    /// One argument per line, so an argument may hold spaces.
    pub command: String,
    pub env: Vec<(String, String)>,
    pub timeout: String,
    pub calls: String,
    /// Comma separated.
    pub inputs: String,
    pub outputs: Vec<ArtifactOutput>,
    pub repos: Vec<RunRepo>,
    pub services: Vec<ServiceDraft>,
    pub caches: Vec<CacheDraft>,
    /// Empty: the org's provider for hosted agents.
    pub provider_id: String,
    pub skills: Vec<SkillPin>,
    /// Internal only (D785): the assistant tools it may call.
    pub tools: Vec<String>,
    /// Internal only; empty: the default.
    pub max_steps: String,
    /// Internal only (D833): who decides each write tool; absent = a person.
    pub gate: std::collections::BTreeMap<String, GateMode>,
    /// Internal only (D836); empty: none.
    pub validator_id: String,
}

/// Non-empty trimmed lines.
pub(super) fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Non-empty trimmed comma separated names.
pub(super) fn names(text: &str) -> Vec<String> {
    super::names(text)
}

fn pairs(env: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    env.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn map(rows: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    rows.iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), v.clone()))
        .collect()
}

impl Draft {
    pub(super) fn of(description: &str, s: &AgentSpec) -> Self {
        Self {
            description: description.to_string(),
            level: s.level,
            environment_id: s.environment_id.clone().unwrap_or_default(),
            command: s.command.join("\n"),
            env: pairs(&s.env),
            timeout: s.timeout_secs.to_string(),
            calls: s.llm_calls_max.map(|n| n.to_string()).unwrap_or_default(),
            inputs: s.inputs.join(", "),
            outputs: s.outputs.clone(),
            repos: s.repos.clone(),
            services: s
                .services
                .iter()
                .map(|v| ServiceDraft {
                    name: v.name.clone(),
                    image: v.image.clone(),
                    env: pairs(&v.env),
                    ready: v.ready_command.join("\n"),
                    data_paths: v.data_paths.join(", "),
                    run_as_user: v.run_as_user,
                })
                .collect(),
            caches: s
                .caches
                .iter()
                .map(|c| CacheDraft {
                    name: c.name.clone(),
                    path: c.path.clone(),
                    key_files: c.key_files.join(", "),
                })
                .collect(),
            provider_id: s.provider_id.clone().unwrap_or_default(),
            skills: s.skills.clone(),
            tools: s.tools.clone(),
            max_steps: s.max_steps.map(|n| n.to_string()).unwrap_or_default(),
            gate: s.gate.clone(),
            validator_id: s.validator_id.clone().unwrap_or_default(),
        }
    }

    /// Some tool goes through the validator agent.
    pub(super) fn needs_validator(&self) -> bool {
        self.gate
            .iter()
            .any(|(t, m)| m.validated() && self.tools.contains(t))
    }

    /// The spec to send; a number that does not parse goes as an invalid one, refused by
    /// the server with its field.
    pub(super) fn spec(&self) -> AgentSpec {
        let calls = self.calls.trim();
        let steps = self.max_steps.trim();
        let common = AgentSpec {
            level: self.level,
            timeout_secs: self.timeout.trim().parse().unwrap_or(0),
            llm_calls_max: (!calls.is_empty()).then(|| calls.parse().unwrap_or(0)),
            inputs: names(&self.inputs),
            provider_id: (!self.provider_id.is_empty()).then(|| self.provider_id.clone()),
            skills: self.skills.clone(),
            ..Default::default()
        };
        // Only the fields of the level go: the others stay in the form, not in the record.
        if self.level == AgentLevel::Internal {
            // Only the modes of ticked tools, and never the default: the record stays small.
            let gate = self
                .gate
                .iter()
                .filter(|(t, m)| **m != GateMode::Human && self.tools.contains(t))
                .map(|(t, m)| (t.clone(), *m))
                .collect();
            return AgentSpec {
                tools: self.tools.clone(),
                max_steps: (!steps.is_empty()).then(|| steps.parse().unwrap_or(0)),
                gate,
                validator_id: (self.needs_validator() && !self.validator_id.is_empty())
                    .then(|| self.validator_id.clone()),
                ..common
            };
        }
        AgentSpec {
            environment_id: (!self.environment_id.is_empty()).then(|| self.environment_id.clone()),
            command: lines(&self.command),
            env: map(&self.env),
            outputs: self
                .outputs
                .iter()
                .filter(|o| !o.name.trim().is_empty())
                .map(|o| ArtifactOutput {
                    name: o.name.trim().to_string(),
                    required: o.required,
                })
                .collect(),
            repos: self
                .repos
                .iter()
                .filter(|r| !r.url.trim().is_empty())
                .map(|r| RunRepo {
                    url: r.url.trim().to_string(),
                    git_ref: r.git_ref.trim().to_string(),
                    path: r.path.trim().to_string(),
                    ..r.clone()
                })
                .collect(),
            services: self
                .services
                .iter()
                .filter(|v| !v.name.trim().is_empty())
                .map(|v| RunService {
                    name: v.name.trim().to_string(),
                    image: v.image.trim().to_string(),
                    env: map(&v.env),
                    ready_command: lines(&v.ready),
                    data_paths: names(&v.data_paths),
                    run_as_user: v.run_as_user,
                })
                .collect(),
            caches: self
                .caches
                .iter()
                .filter(|c| !c.name.trim().is_empty())
                .map(|c| RunCache {
                    name: c.name.trim().to_string(),
                    path: c.path.trim().to_string(),
                    key_files: names(&c.key_files),
                })
                .collect(),
            ..common
        }
    }
}

/// Label of a field the server refused, as the form names it.
fn field_label(field: &str) -> String {
    let key = match field.strip_prefix("spec.").unwrap_or(field) {
        "environment_id" => "agents-field-environment",
        "command" => "agents-field-command",
        "timeout_secs" => "agents-field-timeout",
        "llm_calls_max" => "agents-field-calls",
        "inputs" => "agents-field-inputs",
        "env" => "agents-edit-env",
        "outputs" => "agents-edit-outputs",
        "repos" => "agents-edit-repos",
        "services" => "agents-edit-services",
        "caches" => "agents-edit-caches",
        "provider_id" => "agents-edit-provider",
        "skills" => "agents-edit-skills",
        "level" => "agents-field-level",
        "tools" => "agents-edit-tools",
        "max_steps" => "agents-field-max-steps",
        "gate" => "agents-gate-title",
        "validator_id" => "agents-field-validator",
        "description" => "agents-edit-description",
        other => return other.to_string(),
    };
    t!(key).to_string()
}

fn error_text(err: &ApiError) -> String {
    let message = crate::api::error_i18n::localize(err);
    match err.fields().and_then(|f| f.keys().next().cloned()) {
        Some(field) => {
            t!("agents-edit-field-error", field : field_label(&field), message : message)
                .to_string()
        }
        None => message,
    }
}

#[component]
pub(super) fn AgentEditor(
    agent_id: String,
    on_back: Callback<()>,
    on_skills: Callback<()>,
) -> Element {
    let can_write = access::can("agents:write");
    let mut generation = use_signal(|| 0u32);
    let load_id = agent_id.clone();
    let detail = use_resource(move || {
        let id = load_id.clone();
        let gen = generation();
        async move { (gen, api::hosted_agents::agent(&id).await) }
    });
    let mut agent = use_signal(|| Option::<Agent>::None);
    let mut draft = use_signal(Draft::default);
    let mut init_done = use_signal(|| None::<u32>);
    use_effect(move || {
        let Some((gen, Ok(a))) = &*detail.read() else {
            return;
        };
        let gen = *gen;
        if gen != generation() || init_done() == Some(gen) {
            return;
        }
        draft.set(Draft::of(&a.description, &a.spec));
        agent.set(Some(a.clone()));
        init_done.set(Some(gen));
    });
    let load_error = match &*detail.read() {
        Some((_, Err(e))) => Some(crate::api::error_i18n::localize(e)),
        _ => None,
    };
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let mut busy = use_signal(|| false);
    let mut versions_open = use_signal(|| false);

    let mut save = move |spec: AgentSpec| {
        let Some(a) = agent() else { return };
        let input = AgentUpdate {
            expected_version: a.version,
            description: Some(draft.peek().description.trim().to_string()),
            spec,
        };
        busy.set(true);
        spawn(async move {
            let result = api::agent_skills::update_agent(&a.id, &input).await;
            busy.set(false);
            match result {
                Ok(saved) => {
                    toasts::success(t!("agents-edit-saved", v : saved.version).to_string());
                    form_error.set(None);
                    versions_open.set(false);
                    draft.set(Draft::of(&saved.description, &saved.spec));
                    agent.set(Some(saved));
                }
                Err(e) if e.status == Some(409) => {
                    toasts::error(t!("agents-edit-conflict").to_string());
                    versions_open.set(false);
                    generation.with_mut(|g| *g += 1);
                }
                Err(e) => form_error.set(Some(e)),
            }
        });
    };

    let history_id = agent_id.clone();
    let history = use_resource(move || {
        let id = history_id.clone();
        let open = versions_open();
        let _ = agent();
        async move {
            if !open {
                return None;
            }
            Some(
                api::hosted_agents::agent_versions(&id)
                    .await
                    .map(|p| p.results),
            )
        }
    });
    let history_versions: Vec<AgentVersion> = match &*history.value().read() {
        Some(Some(Ok(v))) => v.clone(),
        _ => Vec::new(),
    };
    let rows = match &*history.value().read() {
        Some(Some(Ok(v))) => Some(Ok(v
            .iter()
            .map(|v| VersionRow {
                version: v.number,
                yaml: serde_json::to_string_pretty(&v.spec).unwrap_or_default(),
                created_at: v.created_at.clone(),
                author: None,
            })
            .collect())),
        Some(Some(Err(e))) => Some(Err(e.clone())),
        _ => None,
    };

    let Some(current) = agent() else {
        return rsx! {
            div { class: "space-y-3",
                button {
                    class: BTN_SECONDARY_SM,
                    aria_label: t!("agents-edit-back").to_string(),
                    onclick: move |_| on_back.call(()),
                    icons::ArrowLeft { class: "h-4 w-4" }
                }
                if let Some(e) = load_error {
                    p { class: "text-sm text-alarm", "{e}" }
                }
            }
        };
    };
    let dirty = {
        let d = draft();
        d.spec() != current.spec || d.description.trim() != current.description
    };
    let role = match current.role {
        AgentRole::Viewer => t!("agents-role-viewer"),
        AgentRole::Member => t!("agents-role-member"),
    };
    let open_list = history_versions.clone();
    let restore_list = history_versions;
    rsx! {
        div { class: "space-y-4",
            div { class: "flex items-start justify-between flex-wrap gap-3",
                div { class: "flex items-start gap-3 min-w-0",
                    button {
                        class: "{BTN_SECONDARY_SM} shrink-0",
                        aria_label: t!("agents-edit-back").to_string(),
                        onclick: move |_| on_back.call(()),
                        icons::ArrowLeft { class: "h-4 w-4" }
                    }
                    div { class: "min-w-0",
                        h2 { class: "text-xl font-bold text-ink", {current.name.clone()} }
                        div { class: "flex items-center gap-2 mt-1 flex-wrap text-sm text-slate",
                            span { "v{current.version}" }
                            span { {t!("agents-edit-role", role : role)} }
                            if dirty {
                                span { class: "text-xs text-amber-600", {t!("agents-edit-dirty")} }
                            }
                        }
                    }
                }
                div { class: "flex items-center gap-2 shrink-0",
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| versions_open.set(true),
                        icons::History { class: "h-4 w-4 inline mr-1" }
                        {t!("agents-edit-history")}
                    }
                    if can_write {
                        button {
                            class: BTN_PRIMARY,
                            disabled: !dirty || busy(),
                            onclick: move |_| save(draft.peek().spec()),
                            icons::Save { class: "h-4 w-4 inline mr-1" }
                            {t!("agents-edit-save")}
                        }
                    }
                }
            }
            if !can_write {
                p { class: "text-sm text-slate", {t!("agents-edit-read-only")} }
            }
            if let Some(reason) = current.validator_tripped.clone() {
                TrippedBanner {
                    agent_id: current.id.clone(),
                    reason,
                    can_write,
                    on_rearmed: move |a: Agent| agent.set(Some(a)),
                }
            }
            if let Some(err) = form_error() {
                div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                    {error_text(&err)}
                }
            }
            fieldset { class: "min-w-0 space-y-6", disabled: !can_write,
                GeneralSection { draft }
                if draft().level == AgentLevel::Internal {
                    ToolsSection { draft, role: current.role }
                    if draft().needs_validator() {
                        ValidatorField { draft, self_id: current.id.clone() }
                    }
                } else {
                    div {
                        h3 { class: FIELD_LABEL, {t!("agents-edit-env")} }
                        KvList {
                            rows: draft().env,
                            generated: false,
                            on_change: move |rows| draft.write().env = rows,
                        }
                        p { class: "text-xs text-slate mt-1", {t!("agents-edit-env-hint")} }
                    }
                    OutputsSection { draft }
                    ReposSection { draft }
                    ServicesSection { draft }
                    CachesSection { draft }
                }
            }
            SkillsSection { pins: current.spec.skills.clone(), on_skills }
            if versions_open() {
                VersionsDrawer {
                    rows,
                    active_version: current.version,
                    can_write,
                    on_close: move |_| versions_open.set(false),
                    on_open: move |row: VersionRow| {
                        if let Some(v) = open_list.iter().find(|v| v.number == row.version) {
                            let description = draft.peek().description.clone();
                            draft.set(Draft::of(&description, &v.spec));
                            versions_open.set(false);
                        }
                    },
                    on_restore: move |n: i32| {
                        if let Some(v) = restore_list.iter().find(|v| v.number == n) {
                            save(v.spec.clone());
                        }
                    },
                }
            }
        }
    }
}

/// Description, environment, command, limits, inputs and own provider.
#[component]
fn GeneralSection(draft: Signal<Draft>) -> Element {
    let mut draft = draft;
    let environments =
        use_resource(move || async move { api::hosted_agents::environments().await });
    let envs: Vec<(String, String)> = match &*environments.value().read() {
        Some(Ok(p)) => p
            .results
            .iter()
            .map(|e| (e.id.clone(), e.name.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let providers = use_resource(move || async move {
        if !access::can("ai.providers:read") {
            return Vec::new();
        }
        api::ai::providers().await.unwrap_or_default()
    });
    let provider_options: Vec<(String, String)> = providers
        .value()
        .read()
        .as_ref()
        .map(|list| {
            list.iter()
                .map(|p| (p.id.to_string(), format!("{} · {}", p.name, p.model)))
                .collect()
        })
        .unwrap_or_default();
    let d = draft();
    let hosted = d.level == AgentLevel::Hosted;
    rsx! {
        div { class: "grid grid-cols-1 md:grid-cols-2 gap-4",
            div { class: "md:col-span-2",
                label { r#for: "agent-edit-description", class: FIELD_LABEL,
                    {t!("agents-edit-description")}
                }
                input {
                    id: "agent-edit-description",
                    class: INPUT,
                    value: "{d.description}",
                    oninput: move |e| draft.write().description = e.value(),
                }
            }
            LevelField { draft }
            div { hidden: !hosted,
                label { r#for: "agent-edit-env", class: FIELD_LABEL, {t!("agents-field-environment")} }
                select {
                    id: "agent-edit-env",
                    class: INPUT,
                    onchange: move |e| draft.write().environment_id = e.value(),
                    option { value: "", {t!("agents-field-environment-pick")} }
                    for (id, label) in envs {
                        option {
                            key: "{id}",
                            value: "{id}",
                            selected: id == d.environment_id,
                            {label}
                        }
                    }
                }
            }
            div {
                label { r#for: "agent-edit-provider", class: FIELD_LABEL,
                    {t!("agents-edit-provider")}
                }
                select {
                    id: "agent-edit-provider",
                    class: INPUT,
                    onchange: move |e| draft.write().provider_id = e.value(),
                    option { value: "", {t!("agents-edit-provider-default")} }
                    for (id, label) in provider_options {
                        option {
                            key: "{id}",
                            value: "{id}",
                            selected: id == d.provider_id,
                            {label}
                        }
                    }
                }
            }
            div { hidden: !hosted,
                label { r#for: "agent-edit-command", class: FIELD_LABEL, {t!("agents-field-command")} }
                textarea {
                    id: "agent-edit-command",
                    class: "{INPUT} font-mono text-xs h-24",
                    spellcheck: "false",
                    value: "{d.command}",
                    oninput: move |e| draft.write().command = e.value(),
                }
                p { class: "text-xs text-slate mt-1", {t!("agents-edit-command-hint")} }
            }
            div { class: "grid grid-cols-2 gap-3 content-start",
                div {
                    label { r#for: "agent-edit-timeout", class: FIELD_LABEL,
                        {t!("agents-field-timeout")}
                    }
                    input {
                        id: "agent-edit-timeout",
                        class: INPUT,
                        r#type: "number",
                        value: "{d.timeout}",
                        oninput: move |e| draft.write().timeout = e.value(),
                    }
                }
                div {
                    label { r#for: "agent-edit-calls", class: FIELD_LABEL, {t!("agents-field-calls")} }
                    input {
                        id: "agent-edit-calls",
                        class: INPUT,
                        r#type: "number",
                        value: "{d.calls}",
                        oninput: move |e| draft.write().calls = e.value(),
                    }
                }
                div { class: "col-span-2",
                    label { r#for: "agent-edit-inputs", class: FIELD_LABEL,
                        {t!("agents-field-inputs")}
                    }
                    input {
                        id: "agent-edit-inputs",
                        class: "{INPUT} font-mono",
                        value: "{d.inputs}",
                        oninput: move |e| draft.write().inputs = e.value(),
                    }
                    p { class: "text-xs text-slate mt-1", {t!("agents-field-names-hint")} }
                }
            }
        }
    }
}

/// Where the loop of the agent runs (D378, D785).
#[component]
fn LevelField(draft: Signal<Draft>) -> Element {
    let mut draft = draft;
    let level = draft().level;
    rsx! {
        div {
            label { r#for: "agent-edit-level", class: FIELD_LABEL, {t!("agents-field-level")} }
            select {
                id: "agent-edit-level",
                class: INPUT,
                onchange: move |e| {
                    draft.write().level = if e.value() == "internal" {
                        AgentLevel::Internal
                    } else {
                        AgentLevel::Hosted
                    };
                },
                option { value: "hosted", selected: level == AgentLevel::Hosted,
                    {t!("agents-level-hosted")}
                }
                option {
                    value: "internal",
                    selected: level == AgentLevel::Internal,
                    {t!("agents-level-internal")}
                }
            }
            p { class: "text-xs text-slate mt-1", {t!("agents-field-level-hint")} }
        }
    }
}

/// The assistant tools an internal agent may call, and its most steps (D785).
#[component]
fn ToolsSection(draft: Signal<Draft>, role: AgentRole) -> Element {
    let mut draft = draft;
    let tools = use_resource(move || async move { api::hosted_agents::agent_tools().await });
    let d = draft();
    let list = match &*tools.value().read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let viewer = role == AgentRole::Viewer;
    rsx! {
        div { class: "space-y-3",
            div { class: "max-w-xs",
                label { r#for: "agent-edit-max-steps", class: FIELD_LABEL,
                    {t!("agents-field-max-steps")}
                }
                input {
                    id: "agent-edit-max-steps",
                    class: INPUT,
                    r#type: "number",
                    value: "{d.max_steps}",
                    oninput: move |e| draft.write().max_steps = e.value(),
                }
                p { class: "text-xs text-slate mt-1", {t!("agents-field-max-steps-hint")} }
            }
            h3 { class: FIELD_LABEL, {t!("agents-edit-tools")} }
            p { class: "text-xs text-slate", {t!("agents-edit-tools-hint")} }
            if viewer {
                p { class: "text-xs text-slate", {t!("agents-edit-tools-viewer")} }
            }
            div { class: "max-h-80 overflow-auto rounded border border-gray-200 divide-y divide-gray-100",
                for tool in list {
                    ToolRow {
                        key: "{tool.name}",
                        checked: d.tools.contains(&tool.name),
                        disabled: viewer && tool.write,
                        tool,
                        draft,
                    }
                }
            }
        }
    }
}

fn mode_label(m: GateMode) -> String {
    match m {
        GateMode::Human => t!("agents-gate-human"),
        GateMode::Autonomous => t!("agents-gate-autonomous"),
        GateMode::Validator => t!("agents-gate-validator"),
        GateMode::Chain => t!("agents-gate-chain"),
    }
    .to_string()
}

const MODES: [GateMode; 4] = [
    GateMode::Human,
    GateMode::Validator,
    GateMode::Chain,
    GateMode::Autonomous,
];

fn mode_of(value: &str) -> GateMode {
    MODES
        .into_iter()
        .find(|m| m.as_str() == value)
        .unwrap_or_default()
}

#[component]
fn ToolRow(tool: AgentToolInfo, checked: bool, disabled: bool, draft: Signal<Draft>) -> Element {
    let mut draft = draft;
    let name = tool.name.clone();
    let gate_name = tool.name.clone();
    let mode = draft().gate.get(&tool.name).copied().unwrap_or_default();
    let select_id = format!("agent-gate-{}", tool.name);
    rsx! {
        div { class: "flex flex-wrap items-start gap-x-3 gap-y-2 p-2 text-sm",
            label { class: "flex min-w-0 flex-1 basis-64 items-start gap-2",
                input {
                    r#type: "checkbox",
                    class: "mt-1",
                    disabled,
                    checked,
                    onchange: move |e: FormEvent| toggle(&mut draft.write().tools, &name, e.checked()),
                }
                span { class: "min-w-0",
                    span { class: "font-mono text-ink break-words", {tool.name.clone()} }
                    span { class: "block text-xs text-slate", {tool.summary.clone()} }
                }
            }
            // Who decides a write (D833); a never-delegated one stays with a person (D838).
            if tool.write && checked {
                div { class: "flex shrink-0 items-center gap-2",
                    label { r#for: "{select_id}", class: "text-xs text-slate",
                        {t!("agents-gate-who")}
                    }
                    select {
                        id: "{select_id}",
                        class: "rounded border border-gray-300 bg-white px-2 py-1 text-xs",
                        disabled: tool.never_delegated,
                        onchange: move |e| {
                            draft.write().gate.insert(gate_name.clone(), mode_of(&e.value()));
                        },
                        for m in MODES {
                            option {
                                key: "{m.as_str()}",
                                value: "{m.as_str()}",
                                selected: mode == m,
                                {mode_label(m)}
                            }
                        }
                    }
                    if tool.never_delegated {
                        span { class: "text-xs text-slate", {t!("agents-gate-never")} }
                    }
                }
            }
        }
    }
}

/// The agent that validates the requests of `validator` and `chain` tools (D836): another
/// internal agent of the org.
#[component]
fn ValidatorField(draft: Signal<Draft>, self_id: String) -> Element {
    let mut draft = draft;
    let agents = use_resource(move || async move { api::hosted_agents::agents(200, 0).await });
    let choices: Vec<(String, String)> = match &*agents.read() {
        Some(Ok(p)) => p
            .results
            .iter()
            .filter(|a| a.spec.level == AgentLevel::Internal && a.id != self_id)
            .map(|a| (a.id.clone(), a.name.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let chosen = draft().validator_id;
    rsx! {
        div { class: "max-w-md",
            label { r#for: "agent-edit-validator", class: FIELD_LABEL, {t!("agents-field-validator")} }
            select {
                id: "agent-edit-validator",
                class: INPUT,
                onchange: move |e| draft.write().validator_id = e.value(),
                option { value: "", selected: chosen.is_empty(), {t!("gov-choose")} }
                for (id, name) in choices {
                    option { key: "{id}", value: "{id}", selected: chosen == id, "{name}" }
                }
            }
            p { class: "text-xs text-slate mt-1", {t!("agents-field-validator-hint")} }
        }
    }
}

/// D839: a validator that tripped hands its requests to a person until re-armed.
#[component]
fn TrippedBanner(
    agent_id: String,
    reason: String,
    can_write: bool,
    on_rearmed: Callback<Agent>,
) -> Element {
    let mut busy = use_signal(|| false);
    let why = if reason == "rate" {
        t!("agents-tripped-rate", count : RATE_PER_HOUR)
    } else {
        t!("agents-tripped-all-accepted", count : ALL_ACCEPTED)
    };
    let rearm = move |_| {
        let id = agent_id.clone();
        busy.set(true);
        spawn(async move {
            match api::hosted_agents::rearm_validator(&id).await {
                Ok(a) => {
                    toasts::success(t!("agents-rearmed").to_string());
                    on_rearmed.call(a);
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        div {
            class: "flex flex-wrap items-center gap-3 rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900",
            role: "status",
            p { class: "min-w-0 flex-1 basis-64",
                strong { class: "block font-semibold", {t!("agents-tripped-title")} }
                "{why}"
            }
            if can_write {
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    disabled: busy(),
                    onclick: rearm,
                    {t!("agents-rearm")}
                }
            }
        }
    }
}

/// D839: limits of a validator, as the server applies them.
pub(super) const RATE_PER_HOUR: i64 = 30;
pub(super) const ALL_ACCEPTED: i64 = 20;

/// Adds or removes `name`, keeping the order of the list.
fn toggle(tools: &mut Vec<String>, name: &str, on: bool) {
    tools.retain(|t| t != name);
    if on {
        tools.push(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::agent_runs::GENERATED;

    #[test]
    fn a_spec_survives_the_form() {
        let spec = AgentSpec {
            environment_id: Some("e1".into()),
            command: vec!["sh".into(), "-c".into(), "echo a b".into()],
            env: [("MODE".to_string(), "fast".to_string())].into(),
            timeout_secs: 600,
            llm_calls_max: Some(50),
            inputs: vec!["ticket".into(), "notes".into()],
            outputs: vec![ArtifactOutput {
                name: "patch".into(),
                required: true,
            }],
            repos: vec![RunRepo {
                url: "https://github.com/acme/app.git".into(),
                git_ref: "main".into(),
                path: "app".into(),
                secret_id: Some("s1".into()),
                push_secret_id: Some("s2".into()),
            }],
            services: vec![RunService {
                name: "db".into(),
                image: "postgres@sha256:00".into(),
                env: [("PASSWORD".to_string(), GENERATED.to_string())].into(),
                ready_command: vec!["pg_isready".into(), "-h".into(), "localhost".into()],
                data_paths: vec!["/var/lib/postgresql/data".into()],
                run_as_user: Some(70),
            }],
            caches: vec![RunCache {
                name: "cargo".into(),
                path: "/root/.cargo".into(),
                key_files: vec!["Cargo.lock".into()],
            }],
            provider_id: Some("p1".into()),
            skills: vec![SkillPin {
                skill_id: "k1".into(),
                version: 2,
            }],
            ..Default::default()
        };
        assert_eq!(Draft::of("d", &spec).spec(), spec);
        // Blank rows added in the form are left out, not sent.
        let mut d = Draft::of("d", &spec);
        d.env.push((String::new(), String::new()));
        d.repos.push(RunRepo {
            url: String::new(),
            git_ref: "main".into(),
            path: String::new(),
            secret_id: None,
            push_secret_id: None,
        });
        assert_eq!(d.spec(), spec);
        // A number that does not parse is sent invalid, never dropped silently.
        d.calls = "abc".into();
        assert_eq!(d.spec().llm_calls_max, Some(0));
        d.calls = String::new();
        assert_eq!(d.spec().llm_calls_max, None);
    }

    #[test]
    fn only_the_modes_of_ticked_writes_are_sent() {
        let mut d = Draft {
            level: AgentLevel::Internal,
            tools: vec!["create_object".into()],
            validator_id: "v1".into(),
            ..Default::default()
        };
        d.gate.insert("create_object".into(), GateMode::Human);
        d.gate.insert("update_object".into(), GateMode::Autonomous);
        let spec = d.spec();
        assert!(
            spec.gate.is_empty(),
            "a person is the default; an unticked tool goes away"
        );
        assert_eq!(spec.validator_id, None, "no validator when none is needed");
        d.gate.insert("create_object".into(), GateMode::Chain);
        let spec = d.spec();
        assert_eq!(spec.gate.get("create_object"), Some(&GateMode::Chain));
        assert_eq!(spec.validator_id.as_deref(), Some("v1"));
        assert_eq!(Draft::of("", &spec).spec(), spec);
    }

    #[test]
    fn an_internal_spec_sends_its_tools_and_none_of_the_sandbox() {
        let spec = AgentSpec {
            level: AgentLevel::Internal,
            timeout_secs: 600,
            inputs: vec!["task".into()],
            tools: vec!["list_flows".into(), "create_object".into()],
            max_steps: Some(12),
            ..Default::default()
        };
        assert_eq!(Draft::of("d", &spec).spec(), spec);
        // Fields typed while hosted stay in the form, never in an internal record.
        let mut d = Draft::of("d", &spec);
        d.command = "sh".into();
        d.environment_id = "e1".into();
        assert_eq!(d.spec(), spec);
        d.level = AgentLevel::Hosted;
        assert!(d.spec().tools.is_empty() && d.spec().max_steps.is_none());
        let mut tools = vec!["a".to_string()];
        toggle(&mut tools, "b", true);
        toggle(&mut tools, "a", false);
        assert_eq!(tools, ["b"]);
    }
}
