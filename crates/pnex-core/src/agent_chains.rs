//! Agent runs chained by flows (agent-environments.md D713, D716, D718, D765–D768).
//!
//! A deployed flow starts a run with the `agent_run` node, which answers at once with the
//! run and chain ids; when a run ends, the backend publishes an [`AgentRunFinishedEvent`]
//! on the org channel where the `agent_run_finished` nodes listen. No flow waits in memory:
//! the state is in `agent_runs` and `agent_chains`. A chain shares one budget and holds one
//! subject at most.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ontology::flow::FlowNodeRef;

/// Valkey pub/sub channel of the ended runs of an org.
pub fn finished_channel(org_id: i64) -> String {
    format!("pnex:agentrun:v1:{org_id}")
}

pub const RUNS_MAX_DEFAULT: i32 = 10;
pub const LLM_CALLS_MAX_DEFAULT: i32 = 500;
pub const ROUNDS_MAX_DEFAULT: i32 = 3;
const RUNS_CAP: i32 = 100;
const LLM_CALLS_CAP: i32 = 100_000;
const ROUNDS_CAP: i32 = 20;
/// Bytes of a subject key: `(repository, ticket)` or an object id.
pub const SUBJECT_MAX: usize = 200;

/// Ports of the `agent_run` node: started (or joined), refused.
pub const NODE_PORT_STARTED: usize = 0;
pub const NODE_PORT_REFUSED: usize = 1;
pub const NODE_PORT_COUNT: usize = 2;

fn runs_default() -> i32 {
    RUNS_MAX_DEFAULT
}
fn llm_calls_default() -> i32 {
    LLM_CALLS_MAX_DEFAULT
}
fn rounds_default() -> i32 {
    ROUNDS_MAX_DEFAULT
}

/// Bounds of a chain, set when it opens (D716). A round is one more run of an agent that
/// already ran in the chain, such as a fix after a review (D765).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChainLimits {
    #[serde(default = "runs_default")]
    pub runs_max: i32,
    /// Model calls of all the runs of the chain together.
    #[serde(default = "llm_calls_default")]
    pub llm_calls_max: i32,
    #[serde(default = "rounds_default")]
    pub rounds_max: i32,
}

impl Default for ChainLimits {
    fn default() -> Self {
        Self {
            runs_max: RUNS_MAX_DEFAULT,
            llm_calls_max: LLM_CALLS_MAX_DEFAULT,
            rounds_max: ROUNDS_MAX_DEFAULT,
        }
    }
}

impl ChainLimits {
    /// `(field, token)` of the first bound out of range.
    pub fn error(&self) -> Option<(&'static str, &'static str)> {
        let out = |v: i32, cap: i32| !(1..=cap).contains(&v);
        if out(self.runs_max, RUNS_CAP) {
            return Some(("runs_max", "out_of_range"));
        }
        if out(self.llm_calls_max, LLM_CALLS_CAP) {
            return Some(("llm_calls_max", "out_of_range"));
        }
        if out(self.rounds_max, ROUNDS_CAP) {
            return Some(("rounds_max", "out_of_range"));
        }
        None
    }
}

/// A subject is a printable key of bounded length.
pub fn subject_is_valid(subject: &str) -> bool {
    !subject.trim().is_empty()
        && subject.len() <= SUBJECT_MAX
        && !subject.chars().any(char::is_control)
}

/// Closed set of chain states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ChainState {
    /// Takes runs while its budget lasts; holds its subject.
    Active,
    /// Closed by a person: no more runs, subject free.
    Closed,
}

impl ChainState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Active, Self::Closed]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

/// Read model of a chain, with what it used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentChain {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub subject: Option<String>,
    pub state: ChainState,
    pub limits: ChainLimits,
    pub runs: i64,
    pub llm_calls: i64,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub closed_at: Option<String>,
}

/// Why a chain takes no more runs (`agent-chain-exhausted`, `args.reason`).
pub const EXHAUSTED_RUNS: &str = "runs";
pub const EXHAUSTED_ROUNDS: &str = "rounds";
pub const EXHAUSTED_LLM_CALLS: &str = "llm_calls";

fn bad(code: &'static str, message: &str) -> Option<(&'static str, String)> {
    Some((code, message.into()))
}

fn uuid_or_empty(s: &str) -> bool {
    s.is_empty() || uuid::Uuid::parse_str(s).is_ok()
}

/// Configuration of the `agent_run` node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunNodeConfig {
    /// Agent of the org to run (UUID).
    #[serde(default)]
    pub agent_id: String,
    /// Input name of the agent → dotted path in the message holding an artifact id,
    /// e.g. `"ticket": "payload.ticket"` or `"patch": "run.outputs.patch"`.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Dotted path of the subject key in the message; empty = no subject (D718).
    #[serde(default)]
    pub subject_from: String,
    /// Bounds of the chain this node opens when the message carries no `chain_id`.
    #[serde(default)]
    pub limits: ChainLimits,
}

fn path_is_valid(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 200
        && p.split('.').all(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

impl AgentRunNodeConfig {
    /// Save validation: an empty node is a valid draft.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if !uuid_or_empty(&self.agent_id) {
            return bad("agent_run_invalid", "the agent id is not a UUID");
        }
        for (name, path) in &self.inputs {
            if !crate::agent_runs::artifact_name_is_valid(name) || !path_is_valid(path) {
                return bad("agent_run_invalid", "invalid input name or message path");
            }
        }
        if !self.subject_from.is_empty() && !path_is_valid(&self.subject_from) {
            return bad("agent_run_invalid", "invalid subject path");
        }
        if self.limits.error().is_some() {
            return bad("agent_run_invalid", "chain bounds out of range");
        }
        None
    }

    /// Deploy gate and runtime build: an agent is picked.
    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.agent_id.is_empty() {
            return bad("agent_run_unset", "pick an agent");
        }
        self.check()
    }
}

/// Configuration of the `agent_run_finished` node: which ended runs it emits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunFinishedConfig {
    /// Runs of this agent only; empty = any agent.
    #[serde(default)]
    pub agent_id: String,
    /// Runs that belong to a chain only.
    #[serde(default)]
    pub chained_only: bool,
}

impl AgentRunFinishedConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        if !uuid_or_empty(&self.agent_id) {
            return bad("agent_run_invalid", "the agent id is not a UUID");
        }
        None
    }
}

/// Pub/sub frame of one ended run, published once (D398 anti-storm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunFinishedEvent {
    pub run_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub chain_id: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    /// Job state (`succeeded`, `failed`…), outcome and exit code as the runner reported.
    pub state: String,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Why PneX failed it (`agent-run-output-missing`).
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub llm_calls: i32,
    /// Output name → id of the artifact it sent: the inputs of the next run (D729).
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
    pub ts_ms: i64,
}

impl AgentRunFinishedEvent {
    pub fn matches(&self, cfg: &AgentRunFinishedConfig) -> bool {
        (cfg.agent_id.is_empty() || self.agent_id.as_deref() == Some(cfg.agent_id.as_str()))
            && (!cfg.chained_only || self.chain_id.is_some())
    }
}

/// `POST /internal/flow/agent-run`: the values the runtime read from the message. The
/// agent and the paths come from the deployed config, never from here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunNodeRequest {
    pub node: FlowNodeRef,
    /// Input name → artifact id.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// `msg.chain_id`: the run joins that chain.
    #[serde(default)]
    pub chain_id: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
}

/// Reply of `POST /internal/flow/agent-run`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AgentRunNodeReply {
    pub port: usize,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub chain_id: Option<String>,
    /// The subject already had an active chain: no second run (D718).
    #[serde(default)]
    pub joined: bool,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub args: Value,
}

/// The value at a dotted path of a message, as a string.
pub fn msg_string(msg: &Value, path: &str) -> Option<String> {
    let v = path.split('.').try_fold(msg, |v, k| v.get(k))?;
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_configs_check_drafts_and_deploys() {
        let draft = AgentRunNodeConfig::default();
        assert!(draft.check().is_none());
        assert_eq!(draft.check_deployable().unwrap().0, "agent_run_unset");
        let mut cfg = AgentRunNodeConfig {
            agent_id: uuid::Uuid::nil().to_string(),
            inputs: BTreeMap::from([("ticket".into(), "payload.ticket".into())]),
            subject_from: "payload.subject".into(),
            limits: ChainLimits::default(),
        };
        assert!(cfg.check_deployable().is_none());
        cfg.inputs.insert("bad/name".into(), "payload".into());
        assert_eq!(cfg.check().unwrap().0, "agent_run_invalid");
        cfg.inputs.remove("bad/name");
        cfg.limits.rounds_max = 0;
        assert_eq!(cfg.check().unwrap().0, "agent_run_invalid");
        let finished = AgentRunFinishedConfig {
            agent_id: "x".into(),
            chained_only: false,
        };
        assert_eq!(finished.check().unwrap().0, "agent_run_invalid");
    }

    #[test]
    fn finished_events_match_agent_and_chain() {
        let ev = AgentRunFinishedEvent {
            run_id: "r".into(),
            agent_id: Some("a".into()),
            chain_id: None,
            subject: None,
            state: "succeeded".into(),
            outcome: None,
            exit_code: Some(0),
            error: None,
            llm_calls: 0,
            outputs: BTreeMap::new(),
            ts_ms: 0,
        };
        assert!(ev.matches(&AgentRunFinishedConfig::default()));
        let cfg = |agent: &str, chained_only| AgentRunFinishedConfig {
            agent_id: agent.into(),
            chained_only,
        };
        assert!(ev.matches(&cfg("a", false)));
        assert!(!ev.matches(&cfg("b", false)));
        assert!(!ev.matches(&cfg("", true)));
    }

    #[test]
    fn message_paths_read_strings_only() {
        let msg = serde_json::json!({ "payload": { "ticket": "t1", "n": 3, "o": {} } });
        assert_eq!(msg_string(&msg, "payload.ticket").as_deref(), Some("t1"));
        assert_eq!(msg_string(&msg, "payload.n").as_deref(), Some("3"));
        assert_eq!(msg_string(&msg, "payload.o"), None);
        assert_eq!(msg_string(&msg, "payload.none"), None);
        assert!(subject_is_valid("repo#12"));
        assert!(!subject_is_valid(" "));
    }
}
