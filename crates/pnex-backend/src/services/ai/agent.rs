//! Boucle d'agent bornée : messages → tool_calls → exécution → … jusqu'à
//! une réponse textuelle finale ou la borne d'itérations (auquel cas on
//! force une réponse sans outils).
//!
//! Bornes : `MAX_ITERATIONS` appels LLM max par tour utilisateur (budget
//! coût/latence borné par construction), `MAX_TOKENS` par appel, timeout
//! HTTP par appel. L'historique est court (front n'envoie que le texte des
//! tours précédents), pas de gestion de fenêtre de contexte.

use std::time::Duration;

use super::config::ResolvedAiConfig;
use super::error::AiError;
use super::provider::{AiChatRequest, Msg, Provider, Usage};
use super::tools::{self, ToolDeps, ToolOutcome, ToolTrace};

/// Nombre max d'appels LLM par tour utilisateur.
pub const MAX_ITERATIONS: usize = 6;
/// max_tokens par appel LLM.
pub const MAX_TOKENS: u32 = 4096;
/// Timeout d'UN appel LLM.
pub const REQUEST_TIMEOUT_SECS: u64 = 90;

/// Process-wide HTTP client of the agent loop (connection pool + TLS
/// session reuse across turns; cheap `Arc` clone).
pub fn shared_http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
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
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            // An LLM API never redirects: following one would let an org's
            // provider URL bounce the platform onto an internal host (R8).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

/// Réponse d'un tour utilisateur complet.
pub struct AgentReply {
    pub answer: String,
    /// Trace des outils exécutés (pour la bulle UI repliable).
    pub tool_trace: Vec<ToolTrace>,
    /// Somme des tokens de tous les appels du tour.
    pub usage: Usage,
}

/// Exécute un tour utilisateur : boucle jusqu'à une réponse textuelle ou
/// à la borne d'itérations (dernier appel **sans outils** pour forcer la
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

    for _iteration in 0..=MAX_ITERATIONS {
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

        // Réponse textuelle finale → tour terminé.
        if turn.tool_calls.is_empty() {
            return Ok(AgentReply {
                answer: turn
                    .text
                    .unwrap_or_else(|| "(the model returned an empty answer)".to_string()),
                tool_trace: trace,
                usage: usage_total,
            });
        }

        // ── Exécution séquentielle des outils demandés ──
        messages.push(Msg::Assistant {
            text: turn.text.clone(),
            tool_calls: turn.tool_calls.clone(),
        });
        for call in turn.tool_calls {
            let outcome: Result<ToolOutcome, tools::ToolError> =
                tools::execute(deps, &call.name, &call.arguments).await;
            match outcome {
                Ok(outcome) => {
                    tracing::info!(tool = %call.name, "outil assistant exécuté");
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
                    tracing::info!(tool = %call.name, erreur = %err.message, "outil assistant en échec");
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

    // Borne d'itérations atteinte : dernier appel SANS outils pour forcer
    // une conclusion textuelle (le budget de l'agent reste borné).
    messages.push(Msg::User {
        text: "(borne d'outils atteinte pour ce tour — réponds maintenant à l'utilisateur sans appeler d'outil)"
            .into(),
    });
    let req = AiChatRequest {
        system,
        messages: &messages,
        tools: &[],
        max_tokens: MAX_TOKENS,
    };
    let turn = provider.chat(http, cfg, &req).await?;
    Ok(AgentReply {
        answer: turn
            .text
            .unwrap_or_else(|| "(the model returned an empty answer)".to_string()),
        tool_trace: trace,
        usage: usage_total,
    })
}

/// Constructeur de la trace pour le contrôleur (name/arguments/ok/summary/
/// flow_id) — le front n'a pas besoin du JSON complet des résultats.
pub fn trace_entry(
    name: String,
    arguments: serde_json::Value,
    ok: bool,
    summary: String,
    flow_id: Option<i64>,
) -> ToolTrace {
    ToolTrace {
        name,
        arguments,
        ok,
        summary,
        summary_key: None,
        flow_id,
        code: None,
        args: None,
    }
}
