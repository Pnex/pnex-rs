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
use pnex_core::agents::{Agent, AgentRole, AgentSpec, AgentUpdate, AgentVersion};

mod sections;

use sections::{CachesSection, KvList, OutputsSection, ReposSection, ServicesSection, SkillsSection};

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
            environment_id: s.environment_id.clone(),
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
        }
    }

    /// The spec to send; a number that does not parse goes as an invalid one, refused by
    /// the server with its field.
    pub(super) fn spec(&self) -> AgentSpec {
        let calls = self.calls.trim();
        AgentSpec {
            environment_id: self.environment_id.clone(),
            command: lines(&self.command),
            env: map(&self.env),
            timeout_secs: self.timeout.trim().parse().unwrap_or(0),
            llm_calls_max: (!calls.is_empty()).then(|| calls.parse().unwrap_or(0)),
            inputs: names(&self.inputs),
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
            provider_id: (!self.provider_id.is_empty()).then(|| self.provider_id.clone()),
            skills: self.skills.clone(),
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
        "description" => "agents-edit-description",
        other => return other.to_string(),
    };
    t!(key).to_string()
}

fn error_text(err: &ApiError) -> String {
    let message = crate::api::error_i18n::localize(err);
    match err.fields().and_then(|f| f.keys().next().cloned()) {
        Some(field) => t!("agents-edit-field-error", field : field_label(&field), message : message)
            .to_string(),
        None => message,
    }
}

#[component]
pub(super) fn AgentEditor(agent_id: String, on_back: Callback<()>, on_skills: Callback<()>) -> Element {
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
            Some(api::hosted_agents::agent_versions(&id).await.map(|p| p.results))
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
            if let Some(err) = form_error() {
                div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                    {error_text(&err)}
                }
            }
            fieldset { class: "space-y-6", disabled: !can_write,
                GeneralSection { draft }
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
            div {
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
            div {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::agent_runs::GENERATED;

    #[test]
    fn a_spec_survives_the_form() {
        let spec = AgentSpec {
            environment_id: "e1".into(),
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
}
