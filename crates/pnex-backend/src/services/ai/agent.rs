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
use super::tools::{self, ToolDeps, ToolError, ToolOutcome, ToolTrace};

/// Max number of LLM calls per user turn.
pub const MAX_ITERATIONS: usize = 6;
/// max_tokens per LLM call.
pub const MAX_TOKENS: u32 = 4096;
/// Ceiling of ONE LLM call; each request sets its provider's own timeout
/// (`ResolvedAiConfig::timeout`), which overrides this one.
pub const REQUEST_TIMEOUT_SECS: u64 = pnex_core::LLM_TIMEOUT_MAX_SECS as u64;

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
    // Set once the turn read untrusted outside content (SEC-71).
    let mut tainted = false;

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
            if tainted && is_write_tool(&call.name) {
                // Held, not run: the human confirms it in the chat (SEC-71).
                // A refused permission is still reported as a refusal.
                if let Err(err) = tools::authorize_tool(deps, &call.name) {
                    let (entry, content) = traced(&call.name, &call.arguments, Err(err));
                    trace.push(entry);
                    messages.push(Msg::ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        content,
                        is_error: true,
                    });
                    continue;
                }
                tracing::info!(tool = %call.name, "assistant write held for confirmation");
                trace.push(held(&call.name, &call.arguments));
                messages.push(Msg::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    content: HELD_NOTE.to_string(),
                    is_error: false,
                });
                continue;
            }
            // Tainted as soon as it is called: even its error may carry
            // outside content.
            tainted |= UNTRUSTED_TOOLS.contains(&call.name.as_str());
            let outcome = tools::execute(deps, &call.name, &call.arguments).await;
            let is_error = outcome.is_err();
            let (entry, content) = traced(&call.name, &call.arguments, outcome);
            trace.push(entry);
            messages.push(Msg::ToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                content,
                is_error,
            });
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

/// Tools whose output carries content from outside the platform: mail
/// bodies and subjects, website pages and extracts, uploaded documents.
/// Once one ran, the turn is tainted: its write tools wait for the human
/// (SEC-71).
pub const UNTRUSTED_TOOLS: &[&str] = &[
    "list_mails",
    "read_mail",
    "try_site_recipe",
    "get_site_recipe_run",
    "search_docs",
    "read_chunk",
    "open_page",
];

/// What the model reads instead of the result of a held write.
const HELD_NOTE: &str = "NOT RUN: this turn read untrusted outside content, so this write waits for the user's confirmation. It is shown to the user with a Confirm button; tell the user what you prepared and ask them to check and confirm it. Do not retry it.";

/// A write tool: its registry permission writes. Proposals are not: they
/// run nothing, the human acts from their button.
pub fn is_write_tool(name: &str) -> bool {
    matches!(pnex_api_contract::iam::tool(name), Some(Some(p)) if p.ends_with(":write"))
        && !name.starts_with("propose_")
}

/// Trace entry of a held write: the full arguments are what the human
/// confirms (`conversations::pending_action`).
fn held(name: &str, arguments: &serde_json::Value) -> ToolTrace {
    ToolTrace {
        name: name.to_string(),
        arguments: arguments.clone(),
        ok: false,
        summary: format!("{name} not run: waiting for the user's confirmation"),
        summary_key: None,
        flow_id: None,
        code: Some(pnex_core::err_codes::AI_CONFIRM_REQUIRED),
        args: Some(serde_json::json!({ "tool": name })),
    }
}

/// Trace entry and model-facing content of one executed call.
fn traced(
    name: &str,
    arguments: &serde_json::Value,
    outcome: Result<ToolOutcome, ToolError>,
) -> (ToolTrace, String) {
    match outcome {
        Ok(outcome) => {
            tracing::info!(tool = %name, "assistant tool executed");
            let summary = tools::summarize(name, &outcome);
            let entry = ToolTrace {
                name: name.to_string(),
                arguments: arguments.clone(),
                ok: true,
                summary: summary.text,
                summary_key: Some(summary.key),
                flow_id: outcome.flow_id,
                code: None,
                args: Some(summary.args),
            };
            (entry, outcome.value.to_string())
        }
        Err(err) => {
            tracing::info!(tool = %name, error = %err.message, "assistant tool failed");
            let entry = ToolTrace {
                name: name.to_string(),
                arguments: arguments.clone(),
                ok: false,
                summary: err.message.clone(),
                summary_key: None,
                flow_id: None,
                code: err.code,
                args: err.args,
            };
            (entry, err.message)
        }
    }
}

/// Runs a held write the user confirmed, with the arguments the human saw:
/// the same `execute` path, so the caller's rights, the org scoping and
/// `expected_version` are checked again. No LLM call.
pub async fn run_confirmed(
    deps: &ToolDeps<'_>,
    name: &str,
    arguments: &serde_json::Value,
) -> AgentReply {
    let outcome = tools::execute(deps, name, arguments).await;
    let (entry, _) = traced(name, arguments, outcome);
    AgentReply {
        answer: String::new(),
        tool_trace: vec![entry],
        usage: Usage::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_are_held_reads_and_proposals_are_not() {
        for name in [
            "update_flow",
            "create_object",
            "try_site_recipe",
            "update_notification_template",
        ] {
            assert!(is_write_tool(name), "{name}");
        }
        for name in [
            "read_mail",
            "get_flow",
            "propose_action",
            "propose_site_recipe",
            "unknown",
        ] {
            assert!(!is_write_tool(name), "{name}");
        }
        for name in UNTRUSTED_TOOLS {
            assert!(
                pnex_api_contract::iam::tool(name).is_some(),
                "{name} not a registry tool"
            );
        }
    }
}
