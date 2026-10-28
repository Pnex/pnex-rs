//! Bounded agent loop: messages → tool_calls → execution → … until
//! a final text answer or the iteration bound (in which case an answer
//! without tools is forced).
//!
//! Bounds: `MAX_ITERATIONS` LLM calls max per user turn (cost/latency
//! budget bounded by construction), `MAX_TOKENS` per call, HTTP timeout
//! per call. History is short (the front only sends the text of
//! previous turns), no context window management.

use std::time::Duration;

use super::config::ResolvedAiConfig;
use super::error::AiError;
use super::provider::{AiChatRequest, Msg, Provider, Usage};
use super::tools::{self, ToolDeps, ToolOutcome, ToolTrace};

/// Max number of LLM calls per user turn.
pub const MAX_ITERATIONS: usize = 6;
/// max_tokens per LLM call.
pub const MAX_TOKENS: u32 = 4096;
/// Timeout of ONE LLM call.
pub const REQUEST_TIMEOUT_SECS: u64 = 90;

/// Process-wide HTTP client of the agent loop (connection pool + TLS
/// session reuse across turns; cheap `Arc` clone).
pub fn shared_http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        // Egress guard (R8, SEC-14): internal addresses refused at
        // resolution.
        pnex_core::egress::guarded(reqwest::Client::builder())
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            // An LLM API never redirects: following one would let an org's
            // provider URL bounce the platform onto an internal host (R8).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

/// Process-wide HTTP client of the connector probe (short timeout).
pub fn shared_probe_http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .timeout(Duration::from_secs(30))
            // An LLM API never redirects: following one would let an org's
            // provider URL bounce the platform onto an internal host (R8).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

/// Answer of a full user turn.
pub struct AgentReply {
    pub answer: String,
    /// Trace of the executed tools (for the collapsible UI bubble).
    pub tool_trace: Vec<ToolTrace>,
    /// Sum of the tokens of all the turn's calls.
    pub usage: Usage,
}

/// Runs a user turn: loops until a text answer or
/// the iteration bound (last call **without tools** to force the
/// conclusion).
pub async fn run_turn(
    deps: &ToolDeps<'_>,
    cfg: &ResolvedAiConfig,
    provider: Provider,
    system: String,
    history: Vec<Msg>,
) -> Result<AgentReply, AiError> {
    let http = shared_http();
    let specs = tools::tool_specs();
    let mut messages = history;
    let mut trace: Vec<ToolTrace> = Vec::new();
    let mut usage_total = Usage::default();

    // Tool-enabled calls; the forced final answer below is the last of the
    // `MAX_ITERATIONS` calls (O48).
    for _iteration in 0..MAX_ITERATIONS - 1 {
        let req = AiChatRequest {
            system: system.clone(),
            messages: &messages,
            tools: &specs,
            max_tokens: MAX_TOKENS,
        };
        let turn = provider.chat(http, cfg, &req).await?;
        usage_total = Usage {
            input_tokens: usage_total.input_tokens + turn.usage.input_tokens,
            output_tokens: usage_total.output_tokens + turn.usage.output_tokens,
        };

        // Final text answer → turn done.
        if turn.tool_calls.is_empty() {
            return Ok(AgentReply {
                answer: turn
                    .text
                    .unwrap_or_else(|| "(the model returned an empty answer)".to_string()),
                tool_trace: trace,
                usage: usage_total,
            });
        }

        // ── Sequential execution of the requested tools ──
        messages.push(Msg::Assistant {
            text: turn.text.clone(),
            tool_calls: turn.tool_calls.clone(),
        });
        for call in turn.tool_calls {
            let outcome: Result<ToolOutcome, tools::ToolError> =
                tools::execute(deps, &call.name, &call.arguments).await;
            match outcome {
                Ok(outcome) => {
                    tracing::info!(tool = %call.name, "assistant tool executed");
                    let summary = tools::summarize(&call.name, &outcome);
                    trace.push(ToolTrace {
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        ok: true,
                        summary: summary.text,
                        summary_key: Some(summary.key),
                        flow_id: outcome.flow_id,
                        code: None,
                        args: Some(summary.args),
                    });
                    messages.push(Msg::ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        content: outcome.value.to_string(),
                        is_error: false,
                    });
                }
                Err(err) => {
                    tracing::info!(tool = %call.name, erreur = %err.message, "assistant tool failed");
                    trace.push(ToolTrace {
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        ok: false,
                        summary: err.message.clone(),
                        summary_key: None,
                        flow_id: None,
                        code: err.code,
                        args: err.args,
                    });
                    messages.push(Msg::ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        content: err.message,
                        is_error: true,
                    });
                }
            }
        }
    }

    // Iteration bound reached: last call WITHOUT tools to force
    // a text conclusion (the agent budget stays bounded).
    messages.push(Msg::User {
        text: "(tool limit reached for this turn — answer the user now without calling a tool)"
            .into(),
    });
    let req = AiChatRequest {
        system,
        messages: &messages,
        tools: &[],
        max_tokens: MAX_TOKENS,
    };
    let turn = provider.chat(http, cfg, &req).await?;
    usage_total = Usage {
        input_tokens: usage_total.input_tokens + turn.usage.input_tokens,
        output_tokens: usage_total.output_tokens + turn.usage.output_tokens,
    };
    Ok(AgentReply {
        answer: turn
            .text
            .unwrap_or_else(|| "(the model returned an empty answer)".to_string()),
        tool_trace: trace,
        usage: usage_total,
    })
}
