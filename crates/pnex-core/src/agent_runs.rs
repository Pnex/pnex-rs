//! Runs of hosted agents (autonomous-agents.md D382, agent-environments.md D731, D732): one
//! sandbox job on the active image of an environment, a run token that opens the LLM
//! gateway only while the job runs, and usage counted by the gateway.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::runners::{RunnerJobOutcome, RunnerJobState};

pub const RUN_TOKEN_PREFIX: &str = "pnagr_";
pub const LLM_CALLS_MAX_DEFAULT: i32 = 200;
pub const LLM_CALLS_MAX: i32 = 10_000;
pub const ENV_MAX: usize = 64;
pub const ENV_VALUE_MAX: usize = 4096;
/// Largest LLM request body the gateway forwards.
pub const GATEWAY_BODY_MAX: usize = 8 * 1024 * 1024;

/// Variables the run gets from PneX: the agent may not set them itself.
pub const RESERVED_ENV: [&str; 6] = [
    "PNEX_RUN_TOKEN",
    "PNEX_GATEWAY_URL",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_API_KEY",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentRunCreate {
    /// The run uses the active build of this environment.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub environment_id: String,
    /// Command of the agent; empty runs the entrypoint of the image.
    #[serde(default)]
    pub command: Vec<String>,
    /// Plain variables; a value that looks like a secret is refused (D705).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub timeout_secs: u32,
    /// Calls through the gateway this run may make; [`LLM_CALLS_MAX_DEFAULT`] when absent.
    #[serde(default)]
    pub llm_calls_max: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentRun {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub environment_id: String,
    /// Image by digest, as run.
    pub image: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub job_id: String,
    pub state: RunnerJobState,
    pub outcome: Option<RunnerJobOutcome>,
    pub exit_code: Option<i32>,
    pub llm_calls: i32,
    pub llm_calls_max: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub started_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
}

/// Tokens of one LLM answer, read from its body or its stream.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LlmUsageCount {
    pub input: i64,
    pub output: i64,
}

impl LlmUsageCount {
    fn add_anthropic(&mut self, usage: &serde_json::Value) {
        let n = |k: &str| usage[k].as_i64().unwrap_or(0);
        self.input +=
            n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
        self.output += n("output_tokens");
    }

    /// From a whole JSON answer, Anthropic or OpenAI.
    pub fn of_body(body: &serde_json::Value) -> Self {
        let mut out = Self::default();
        let usage = &body["usage"];
        if usage.get("prompt_tokens").is_some() {
            out.input = usage["prompt_tokens"].as_i64().unwrap_or(0);
            out.output = usage["completion_tokens"].as_i64().unwrap_or(0);
        } else {
            out.add_anthropic(usage);
        }
        out
    }

    /// From one `data:` event of a stream. Anthropic gives the input in `message_start` and
    /// the running output total in `message_delta`; OpenAI gives both in its last chunk.
    pub fn read_event(&mut self, event: &serde_json::Value) {
        match event["type"].as_str() {
            Some("message_start") => self.add_anthropic(&event["message"]["usage"]),
            Some("message_delta") => {
                if let Some(n) = event["usage"]["output_tokens"].as_i64() {
                    self.output = n;
                }
            }
            _ => {
                if let Some(n) = event["usage"]["prompt_tokens"].as_i64() {
                    self.input = n;
                    self.output = event["usage"]["completion_tokens"].as_i64().unwrap_or(0);
                }
            }
        }
    }
}

/// Field and token of the first problem of a create body.
pub fn create_error(body: &AgentRunCreate) -> Option<(&'static str, &'static str)> {
    if body.timeout_secs == 0 || body.timeout_secs > crate::runners::TIMEOUT_MAX_SECS {
        return Some(("timeout_secs", "invalid"));
    }
    if body.command.len() > crate::runners::COMMAND_ARGS_MAX
        || body
            .command
            .iter()
            .any(|a| a.len() > crate::runners::COMMAND_ARG_MAX || a.contains('\0'))
    {
        return Some(("command", "invalid"));
    }
    if let Some(n) = body.llm_calls_max {
        if !(1..=LLM_CALLS_MAX).contains(&n) {
            return Some(("llm_calls_max", "invalid"));
        }
    }
    if body.env.len() > ENV_MAX {
        return Some(("env", "too_many"));
    }
    for (k, v) in &body.env {
        let name_ok = !k.is_empty()
            && k.len() <= 128
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !k.starts_with(|c: char| c.is_ascii_digit());
        if !name_ok || RESERVED_ENV.contains(&k.as_str()) {
            return Some(("env", "invalid"));
        }
        if v.len() > ENV_VALUE_MAX || v.contains('\0') {
            return Some(("env", "invalid"));
        }
        if crate::environments::holds_a_secret(v) {
            return Some(("env", "secret"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_is_read_from_bodies_and_streams() {
        let anthropic = json!({ "usage": { "input_tokens": 10, "cache_read_input_tokens": 5, "output_tokens": 7 } });
        assert_eq!(
            LlmUsageCount::of_body(&anthropic),
            LlmUsageCount {
                input: 15,
                output: 7
            }
        );
        let openai = json!({ "usage": { "prompt_tokens": 3, "completion_tokens": 4 } });
        assert_eq!(
            LlmUsageCount::of_body(&openai),
            LlmUsageCount {
                input: 3,
                output: 4
            }
        );
        let mut s = LlmUsageCount::default();
        s.read_event(&json!({ "type": "message_start", "message": { "usage": { "input_tokens": 12, "output_tokens": 1 } } }));
        s.read_event(&json!({ "type": "content_block_delta" }));
        s.read_event(&json!({ "type": "message_delta", "usage": { "output_tokens": 40 } }));
        assert_eq!(
            s,
            LlmUsageCount {
                input: 12,
                output: 40
            }
        );
        let mut o = LlmUsageCount::default();
        o.read_event(&json!({ "choices": [] }));
        o.read_event(
            &json!({ "choices": [], "usage": { "prompt_tokens": 9, "completion_tokens": 2 } }),
        );
        assert_eq!(
            o,
            LlmUsageCount {
                input: 9,
                output: 2
            }
        );
    }

    #[test]
    fn the_gateway_variables_stay_with_pnex() {
        let mut body = AgentRunCreate {
            environment_id: "x".into(),
            timeout_secs: 60,
            ..Default::default()
        };
        assert_eq!(create_error(&body), None);
        body.env.insert("ANTHROPIC_API_KEY".into(), "mine".into());
        assert_eq!(create_error(&body), Some(("env", "invalid")));
        body.env.clear();
        body.env.insert("1BAD".into(), "x".into());
        assert_eq!(create_error(&body), Some(("env", "invalid")));
        body.env.clear();
        body.timeout_secs = 0;
        assert_eq!(create_error(&body), Some(("timeout_secs", "invalid")));
    }
}
