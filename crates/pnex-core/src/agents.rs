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

/// What an agent runs, versioned: every change is a new version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentSpec {
    /// Its runs use the active build of this environment.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub environment_id: String,
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
}

/// `(field, token)` of the first problem of a record.
pub fn spec_error(spec: &AgentSpec) -> Option<(&'static str, &'static str)> {
    // Same rules as a run: a record is a run waiting for its files.
    let as_run = AgentRunCreate {
        environment_id: spec.environment_id.clone(),
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
    None
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
        environment_id: spec.environment_id.clone(),
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
            environment_id: uuid::Uuid::nil().to_string(),
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
}
