//! Agents of an org (agent-environments.md D757, D761, autonomous-agents.md D379): each one
//! is a service account of the org with a versioned record of what it runs. A run starts
//! from that record and may only tighten it (D380).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent_runs::{
    AgentRunCreate, ArtifactInput, ArtifactOutput, RunCache, RunRepo, RunService,
};

pub const NAME_MAX: usize = 80;
pub const DESCRIPTION_MAX: usize = 1000;

/// Default of `max_steps` of an internal agent, and its bounds (autonomous-agents.md D380).
pub const MAX_STEPS_DEFAULT: i32 = 20;
pub const MAX_STEPS_MAX: i32 = 50;
/// Most tools one internal agent may be given.
pub const TOOLS_MAX: usize = 64;
/// The output every internal run leaves: its final answer (D785).
pub const REPORT_OUTPUT: &str = "report";
/// The same tool call (name and arguments) this many times ends the run (`loop_guard`).
pub const LOOP_GUARD_REPEATS: u32 = 3;

/// Where the loop of an agent runs (autonomous-agents.md D378, D785).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AgentLevel {
    /// In a sandbox a runner creates, from an environment's image.
    #[default]
    Hosted,
    /// In the backend, with a closed subset of the assistant's tools; every write becomes
    /// a proposal a person decides.
    Internal,
}

/// What an agent runs, versioned: every change is a new version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentSpec {
    /// `hosted` when absent.
    #[serde(default)]
    pub level: AgentLevel,
    /// Hosted only, required there: its runs use the active build of this environment.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The longest a run may last; a run may ask for less.
    pub timeout_secs: u32,
    /// Most LLM calls of a run; a run may ask for fewer.
    #[serde(default)]
    pub llm_calls_max: Option<i32>,
    /// Names of the files each run must be given, at `/artifacts/in/<name>`.
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<ArtifactOutput>,
    #[serde(default)]
    pub repos: Vec<RunRepo>,
    #[serde(default)]
    pub services: Vec<RunService>,
    #[serde(default)]
    pub caches: Vec<RunCache>,
    /// Provider of its own, with its own key (D710); else the org's `agent` usage.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub provider_id: Option<String>,
    /// Skills its runs get under `/artifacts/in/skills/`, each at an activated version (D711).
    #[serde(default)]
    pub skills: Vec<crate::agent_skills::SkillPin>,
    /// Internal only: the assistant tools it may call, by name (a closed subset).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Internal only: most model calls of one run; 20 when absent.
    #[serde(default)]
    pub max_steps: Option<i32>,
}

/// One tool an internal agent may be given (`GET /agents/tools`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentToolInfo {
    pub name: String,
    /// What the model is told the tool does (English).
    pub summary: String,
    /// A write: on an internal agent, each call becomes a proposal.
    pub write: bool,
}

/// Role of the service account of an agent: never `admin` (D293), never above its author.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    #[default]
    Viewer,
    Member,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentCreate {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `viewer` when absent (D761).
    #[serde(default)]
    pub role: AgentRole,
    pub spec: AgentSpec,
}

/// A new version of the record, written on the version that was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentUpdate {
    pub expected_version: i32,
    #[serde(default)]
    pub description: Option<String>,
    pub spec: AgentSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Agent {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub description: String,
    /// Its identity in the org: listed with the service accounts, never a member.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub service_account_id: String,
    pub role: AgentRole,
    pub version: i32,
    pub spec: AgentSpec,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentVersion {
    pub number: i32,
    pub spec: AgentSpec,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

/// A run started from an agent: the files for its declared inputs, and limits that may only
/// be lower than the record's.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentRunStart {
    #[serde(default)]
    pub inputs: Vec<ArtifactInput>,
    #[serde(default)]
    pub timeout_secs: Option<u32>,
    #[serde(default)]
    pub llm_calls_max: Option<i32>,
    /// The run joins this active chain of the org and spends its budget (D716).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub chain_id: Option<String>,
    /// Without `chain_id`: opens a chain holding this subject; refused while another
    /// active chain holds it (D718).
    #[serde(default)]
    pub subject: Option<String>,
    /// Without `chain_id`, `subject` or these bounds open a chain for this run (defaults
    /// when only `subject` is given).
    #[serde(default)]
    pub chain_limits: Option<crate::agent_chains::ChainLimits>,
}

/// `(field, token)` of the first problem of a record.
pub fn spec_error(spec: &AgentSpec) -> Option<(&'static str, &'static str)> {
    match spec.level {
        AgentLevel::Hosted => {
            if spec.environment_id.is_none() {
                return Some(("environment_id", "required"));
            }
            if !spec.tools.is_empty() {
                return Some(("tools", "internal_only"));
            }
            if spec.max_steps.is_some() {
                return Some(("max_steps", "internal_only"));
            }
        }
        AgentLevel::Internal => {
            // Nothing of a sandbox reaches an internal agent: no image, env, repo or secret.
            let hosted_only = [
                ("environment_id", spec.environment_id.is_some()),
                ("command", !spec.command.is_empty()),
                ("env", !spec.env.is_empty()),
                ("outputs", !spec.outputs.is_empty()),
                ("repos", !spec.repos.is_empty()),
                ("services", !spec.services.is_empty()),
                ("caches", !spec.caches.is_empty()),
            ];
            if let Some((field, _)) = hosted_only.into_iter().find(|(_, set)| *set) {
                return Some((field, "hosted_only"));
            }
            if spec.tools.len() > TOOLS_MAX {
                return Some(("tools", "too_many"));
            }
            let mut seen = std::collections::BTreeSet::new();
            if !spec.tools.iter().all(|t| seen.insert(t.as_str())) {
                return Some(("tools", "duplicate"));
            }
            if spec
                .max_steps
                .is_some_and(|n| !(1..=MAX_STEPS_MAX).contains(&n))
            {
                return Some(("max_steps", "invalid"));
            }
        }
    }
    // Same rules as a run: a record is a run waiting for its files.
    let as_run = AgentRunCreate {
        environment_id: spec
            .environment_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::nil().to_string()),
        command: spec.command.clone(),
        env: spec.env.clone(),
        timeout_secs: spec.timeout_secs,
        llm_calls_max: spec.llm_calls_max,
        inputs: spec
            .inputs
            .iter()
            .map(|name| ArtifactInput {
                name: name.clone(),
                artifact_id: uuid::Uuid::nil().to_string(),
            })
            .collect(),
        outputs: spec.outputs.clone(),
        repos: spec.repos.clone(),
        services: spec.services.clone(),
        caches: spec.caches.clone(),
    };
    if let Some(found) = crate::agent_runs::create_error(&as_run) {
        return Some(found);
    }
    if let Some(p) = &spec.provider_id {
        if uuid::Uuid::parse_str(p).is_err() {
            return Some(("provider_id", "invalid"));
        }
    }
    if let Some(found) = crate::agent_skills::pins_error(&spec.skills) {
        return Some(found);
    }
    // The skills arrive under this input name.
    if !spec.skills.is_empty()
        && spec
            .inputs
            .iter()
            .any(|i| i == crate::agent_skills::SKILLS_INPUT)
    {
        return Some(("inputs", "reserved"));
    }
    None
}

/// `loop_guard` (D380): counts each call by tool and normalized arguments.
#[derive(Debug, Default)]
pub struct LoopGuard {
    seen: std::collections::HashMap<String, u32>,
}

impl LoopGuard {
    /// Records one call; `true` once the same call reaches [`LOOP_GUARD_REPEATS`].
    pub fn repeated(&mut self, tool: &str, arguments: &serde_json::Value) -> bool {
        let key = format!("{tool}|{}", canonical(arguments));
        let n = self.seen.entry(key).or_insert(0);
        *n += 1;
        *n >= LOOP_GUARD_REPEATS
    }
}

/// Arguments with object keys sorted at every depth: the same call reads the same.
pub fn canonical(v: &serde_json::Value) -> String {
    fn walk(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String(k.clone()).to_string());
                    out.push(':');
                    walk(&m[k], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    walk(x, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    walk(v, &mut out);
    out
}

pub fn name_error(name: &str) -> Option<&'static str> {
    let n = name.trim();
    if n.is_empty() {
        return Some("required");
    }
    if n.chars().count() > NAME_MAX {
        return Some("max_length:80");
    }
    None
}

/// The run a start asks for, or `(field, token)` when it would widen the record (D380): a
/// longer timeout, more LLM calls, an undeclared input or a declared one left out.
pub fn run_of(
    spec: &AgentSpec,
    start: &AgentRunStart,
) -> Result<AgentRunCreate, (&'static str, &'static str)> {
    let timeout_secs = match start.timeout_secs {
        Some(t) if t > spec.timeout_secs => return Err(("timeout_secs", "above_agent")),
        Some(t) => t,
        None => spec.timeout_secs,
    };
    let record_calls = spec
        .llm_calls_max
        .unwrap_or(crate::agent_runs::LLM_CALLS_MAX_DEFAULT);
    let llm_calls_max = match start.llm_calls_max {
        Some(n) if n > record_calls => return Err(("llm_calls_max", "above_agent")),
        Some(n) => n,
        None => record_calls,
    };
    if start.inputs.iter().any(|i| !spec.inputs.contains(&i.name)) {
        return Err(("inputs", "undeclared"));
    }
    if spec
        .inputs
        .iter()
        .any(|name| !start.inputs.iter().any(|i| &i.name == name))
    {
        return Err(("inputs", "missing"));
    }
    Ok(AgentRunCreate {
        environment_id: spec.environment_id.clone().unwrap_or_default(),
        command: spec.command.clone(),
        env: spec.env.clone(),
        timeout_secs,
        llm_calls_max: Some(llm_calls_max),
        inputs: start.inputs.clone(),
        outputs: spec.outputs.clone(),
        repos: spec.repos.clone(),
        services: spec.services.clone(),
        caches: spec.caches.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AgentSpec {
        AgentSpec {
            environment_id: Some(uuid::Uuid::nil().to_string()),
            timeout_secs: 600,
            llm_calls_max: Some(50),
            inputs: vec!["ticket".into()],
            outputs: vec![ArtifactOutput {
                name: "patch".into(),
                required: true,
            }],
            ..Default::default()
        }
    }

    fn input(name: &str) -> ArtifactInput {
        ArtifactInput {
            name: name.into(),
            artifact_id: uuid::Uuid::nil().to_string(),
        }
    }

    #[test]
    fn a_run_may_tighten_its_agent_never_widen_it() {
        let s = spec();
        let start = AgentRunStart {
            inputs: vec![input("ticket")],
            timeout_secs: Some(60),
            llm_calls_max: Some(5),
            ..Default::default()
        };
        let run = run_of(&s, &start).unwrap();
        assert_eq!((run.timeout_secs, run.llm_calls_max), (60, Some(5)));
        assert_eq!(run.outputs, s.outputs, "outputs come from the record");

        let inherit = run_of(
            &s,
            &AgentRunStart {
                inputs: vec![input("ticket")],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (inherit.timeout_secs, inherit.llm_calls_max),
            (600, Some(50))
        );

        let mut wide = start.clone();
        wide.timeout_secs = Some(601);
        assert_eq!(
            run_of(&s, &wide).unwrap_err(),
            ("timeout_secs", "above_agent")
        );
        let mut wide = start.clone();
        wide.llm_calls_max = Some(51);
        assert_eq!(
            run_of(&s, &wide).unwrap_err(),
            ("llm_calls_max", "above_agent")
        );
        let mut extra = start.clone();
        extra.inputs.push(input("other"));
        assert_eq!(run_of(&s, &extra).unwrap_err(), ("inputs", "undeclared"));
        let none = AgentRunStart::default();
        assert_eq!(run_of(&s, &none).unwrap_err(), ("inputs", "missing"));
    }

    #[test]
    fn a_record_follows_the_rules_of_a_run() {
        let mut s = spec();
        assert_eq!(spec_error(&s), None);
        s.inputs = vec!["Bad Name".into()];
        assert_eq!(spec_error(&s), Some(("inputs", "invalid")));
        let mut s = spec();
        s.env.insert("ANTHROPIC_API_KEY".into(), "x".into());
        assert_eq!(spec_error(&s), Some(("env", "invalid")));
        let mut s = spec();
        s.provider_id = Some("nope".into());
        assert_eq!(spec_error(&s), Some(("provider_id", "invalid")));
        assert_eq!(name_error("  "), Some("required"));
        assert_eq!(name_error("triage"), None);
    }

    #[test]
    fn an_internal_agent_has_tools_and_nothing_of_a_sandbox() {
        let internal = AgentSpec {
            level: AgentLevel::Internal,
            timeout_secs: 600,
            inputs: vec!["task".into()],
            tools: vec!["list_flows".into()],
            max_steps: Some(10),
            ..Default::default()
        };
        assert_eq!(spec_error(&internal), None);
        let mut s = internal.clone();
        s.environment_id = Some(uuid::Uuid::nil().to_string());
        assert_eq!(spec_error(&s), Some(("environment_id", "hosted_only")));
        let mut s = internal.clone();
        s.env.insert("MODE".into(), "x".into());
        assert_eq!(spec_error(&s), Some(("env", "hosted_only")));
        let mut s = internal.clone();
        s.repos.push(RunRepo {
            url: "https://github.com/a/b.git".into(),
            git_ref: "main".into(),
            path: "b".into(),
            secret_id: None,
            push_secret_id: None,
        });
        assert_eq!(spec_error(&s), Some(("repos", "hosted_only")));
        let mut s = internal.clone();
        s.max_steps = Some(51);
        assert_eq!(spec_error(&s), Some(("max_steps", "invalid")));
        let mut s = internal.clone();
        s.tools.push("list_flows".into());
        assert_eq!(spec_error(&s), Some(("tools", "duplicate")));

        let mut hosted = spec();
        hosted.tools = vec!["list_flows".into()];
        assert_eq!(spec_error(&hosted), Some(("tools", "internal_only")));
        let mut hosted = spec();
        hosted.environment_id = None;
        assert_eq!(spec_error(&hosted), Some(("environment_id", "required")));
        // A record saved before levels existed reads as hosted.
        let old: AgentSpec = serde_json::from_value(serde_json::json!({
            "environment_id": uuid::Uuid::nil().to_string(), "timeout_secs": 60
        }))
        .unwrap();
        assert_eq!(old.level, AgentLevel::Hosted);
    }

    #[test]
    fn the_loop_guard_stops_the_third_same_call() {
        let mut guard = LoopGuard::default();
        let a = serde_json::json!({"b": 1, "a": [1, {"y": 2, "x": 1}]});
        let same = serde_json::json!({"a": [1, {"x": 1, "y": 2}], "b": 1});
        assert!(!guard.repeated("get_flow", &a));
        assert!(!guard.repeated("get_flow", &same));
        assert!(!guard.repeated("get_flow", &serde_json::json!({"b": 2})));
        assert!(!guard.repeated("list_flows", &a));
        assert!(guard.repeated("get_flow", &a), "third same call");
    }
}
