//! Abstraction fournisseur LLM — un seul format interne normalisé
//! (`Msg`/`ToolCall`/`ToolSpec`/`AiTurn`) ; chaque fournisseur mappe
//! depuis/vers son protocole wire. Zéro nouvelle dépendance : reqwest
//! (rustls + json, déjà dans l'arbre).
//!
//! ## Anthropic natif
//! `POST {base}/v1/messages`, en-têtes `x-api-key` +
//! `anthropic-version: 2023-06-01`. Les `tool_result` consécutifs sont
//! regroupés dans **un** message `role=user` (sinon l'API renvoie 400).
//!
//! ## OpenAI-compatible (OpenAI, Ollama, vLLM, OpenRouter…)
//! `POST {base}/chat/completions` où `base_url` pointe la racine de
//! version (ex. `https://api.openai.com/v1`). `arguments` des tool_calls
//! est une **chaîne JSON** à parser ; chaque `tool_result` devient un
//! message `role=tool` (ordre préservé).

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::config::ResolvedAiConfig;
use super::error::AiError;

/// LLM protocol; its name is also the stored value
/// (`llm_providers.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAiCompat,
}

impl Provider {
    /// Parsing tolérant (défaut `anthropic`, valeur DB par défaut).
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("openai_compat") {
            Provider::OpenAiCompat
        } else {
            Provider::Anthropic
        }
    }

    /// Stored value (`llm_providers.kind`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAiCompat => "openai_compat",
        }
    }
}

// ───────────────── Types normalisés (le loop d'agent ne voit que ça) ─────────────────

/// Message de conversation, format interne neutre.
#[derive(Clone, Debug)]
pub enum Msg {
    User {
        text: String,
    },
    Assistant {
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    /// Résultat d'outil — re-représenté par le protocole du fournisseur.
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
}

/// Appel d'outil demandé par le modèle.
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Déclaration d'un outil (JSON Schema du corps).
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Requête d'un tour de conversation.
pub struct AiChatRequest<'a> {
    pub system: String,
    pub messages: &'a [Msg],
    pub tools: &'a [ToolSpec],
    pub max_tokens: u32,
}

/// Raison d'arrêt normalisée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other(String),
}

/// Consommation d'un appel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Un tour du modèle : texte et/ou appels d'outils.
#[derive(Clone, Debug, Default)]
pub struct AiTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

impl Provider {
    /// Un tour de conversation → un bloc assistant (texte et/ou tool_use).
    pub async fn chat(
        &self,
        http: &Client,
        cfg: &ResolvedAiConfig,
        req: &AiChatRequest<'_>,
    ) -> Result<AiTurn, AiError> {
        match self {
            Provider::Anthropic => anthropic_chat(http, cfg, req).await,
            Provider::OpenAiCompat => openai_chat(http, cfg, req).await,
        }
    }
}

// ─────────────────────────── Côté Anthropic ───────────────────────────

/// Blocs du body de réponse Anthropic (text | tool_use).
#[derive(Clone, Debug, Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}
#[derive(Serialize)]
struct AnthropicTool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a Value,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Value,
}

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool<'a>>,
}

/// Construit les messages wire Anthropic. Les `ToolResult` consécutifs sont
/// regroupés dans **un** message `role=user` (contrat de l'API : deux
/// messages user d'affilée ou un tool_result isolé hors regroupement = 400).
fn build_anthropic_messages(msgs: &[Msg]) -> Vec<AnthropicMessage> {
    let mut out: Vec<AnthropicMessage> = Vec::new();
    let mut i = 0;
    while i < msgs.len() {
        match &msgs[i] {
            Msg::User { text } => {
                out.push(AnthropicMessage {
                    role: "user",
                    content: json!(text),
                });
                i += 1;
            }
            Msg::Assistant { text, tool_calls } => {
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(t) = text {
                    if !t.is_empty() {
                        blocks.push(json!({ "type": "text", "text": t }));
                    }
                }
                for call in tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    }));
                }
                out.push(AnthropicMessage {
                    role: "assistant",
                    content: json!(blocks),
                });
                i += 1;
            }
            Msg::ToolResult { .. } => {
                let mut blocks: Vec<Value> = Vec::new();
                while let Some(Msg::ToolResult {
                    id,
                    content,
                    is_error,
                    ..
                }) = msgs.get(i)
                {
                    blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": content,
                        "is_error": is_error,
                    }));
                    i += 1;
                }
                out.push(AnthropicMessage {
                    role: "user",
                    content: json!(blocks),
                });
            }
        }
    }
    out
}

/// Tour Anthropic : requête, appel HTTP, mapping réponse → `AiTurn`.
async fn anthropic_chat(
    http: &Client,
    cfg: &ResolvedAiConfig,
    req: &AiChatRequest<'_>,
) -> Result<AiTurn, AiError> {
    let body = AnthropicRequest {
        model: &cfg.model,
        max_tokens: req.max_tokens,
        system: &req.system,
        messages: build_anthropic_messages(req.messages),
        tools: req
            .tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name,
                description: t.description,
                input_schema: &t.input_schema,
            })
            .collect(),
    };
    let url = format!("{}/v1/messages", anthropic_base(&cfg.base_url));
    egress_check(&url)?;
    let resp = http
        .post(&url)
        .header("x-api-key", &cfg.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(anthropic_request_error)?;
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(map_http_error(status.as_u16(), &text, retry_after));
    }
    let parsed: AnthropicResponse = resp
        .json()
        .await
        .map_err(|e| AiError::Deserialize(e.to_string()))?;
    let mut turn = AiTurn {
        usage: Usage {
            input_tokens: parsed.usage.as_ref().map_or(0, |u| u.input_tokens),
            output_tokens: parsed.usage.as_ref().map_or(0, |u| u.output_tokens),
        },
        stop_reason: parsed.stop_reason.as_deref().map(|r| match r {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            other => StopReason::Other(other.into()),
        }),
        ..Default::default()
    };
    for block in parsed.content {
        match block.kind.as_str() {
            "text" => {
                if !block.text.as_deref().unwrap_or_default().is_empty() {
                    turn.text = Some(block.text.unwrap_or_default());
                }
            }
            "tool_use" => {
                if let (Some(id), Some(name)) = (block.id.clone(), block.name.clone()) {
                    turn.tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: block.input.unwrap_or_else(|| json!({})),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(turn)
}

/// Erreur au niveau de la requête (réseau/timeout) — avant toute réponse HTTP.
/// Anthropic API root: tolerates a base URL pasted with `/v1` or the full
/// `/v1/messages` endpoint (both would otherwise 404 on `/v1/v1/messages`).
fn anthropic_base(base_url: &str) -> &str {
    let base = base_url.trim_end_matches('/');
    base.strip_suffix("/v1/messages")
        .or_else(|| base.strip_suffix("/v1"))
        .unwrap_or(base)
}

fn anthropic_request_error(e: reqwest::Error) -> AiError {
    request_error(e)
}

/// Egress guard of a provider URL (R8, SEC-14): an IP literal skips the
/// guarded resolver, so it is checked here.
fn egress_check(url: &str) -> Result<(), AiError> {
    let refused = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| pnex_core::egress::check_url(&u));
    match refused {
        Some(code) => Err(AiError::Network(code.to_string())),
        None => Ok(()),
    }
}

/// Mapping commun réseau/timeout → `AiError`.
fn request_error(e: reqwest::Error) -> AiError {
    if e.is_timeout() {
        AiError::Timeout
    } else {
        AiError::Network(e.to_string())
    }
}

/// Mapping commun des réponses HTTP en échec (401/403, 429, autres).
fn map_http_error(status: u16, body: &str, retry_after: Option<u64>) -> AiError {
    let message = extract_error_message(body);
    match status {
        401 | 403 => AiError::AuthRejected(message),
        429 => AiError::RateLimited(retry_after),
        _ => AiError::Upstream(status, message),
    }
}

/// Extrait un message d'erreur lisible d'un body JSON fournisseur —
/// Anthropic `{"error":{"message"}}`, OpenAI `{"error":{"message"|"code"}}`,
/// sinon le corps brut tronqué.
fn extract_error_message(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "(corps vide)".into();
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if let Some(msg) = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
        {
            return msg.to_string();
        }
        if let Some(msg) = v.get("message").and_then(Value::as_str) {
            return msg.to_string();
        }
    }
    trimmed.chars().take(300).collect()
}

// ─────────────────────── Côté OpenAI-compatible ───────────────────────

#[derive(Serialize)]
struct OpenAiMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCallOut>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OpenAiToolCallOut {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiFunctionOut,
}

#[derive(Serialize)]
struct OpenAiFunctionOut {
    name: String,
    /// Chaîne JSON (contrat OpenAI) — sérialisé depuis les arguments Value.
    arguments: String,
}

#[derive(Serialize)]
struct OpenAiTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiToolDef<'a>,
}

#[derive(Serialize)]
struct OpenAiToolDef<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    #[serde(default)]
    message: Option<OpenAiMessageIn>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiMessageIn {
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallIn>>,
}

#[derive(Deserialize)]
struct OpenAiToolCallIn {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionIn>,
}

#[derive(Deserialize)]
struct OpenAiFunctionIn {
    #[serde(default)]
    name: Option<String>,
    /// Chaîne JSON (contrat OpenAI) — parsée en Value, échec = erreur claire.
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool<'a>>,
    max_tokens: u32,
}

/// Construit les messages wire OpenAI : un message `role=tool` par
/// `ToolResult` (ordre préservé), `arguments` re-sérialisés en chaîne JSON.
fn build_openai_messages(msgs: &[Msg]) -> Vec<OpenAiMessage> {
    let mut out: Vec<OpenAiMessage> = Vec::new();
    for msg in msgs {
        match msg {
            Msg::User { text } => out.push(OpenAiMessage {
                role: "user",
                content: Some(text.clone()),
                tool_calls: None,
                tool_call_id: None,
            }),
            Msg::Assistant { text, tool_calls } => out.push(OpenAiMessage {
                role: "assistant",
                content: text.clone(),
                tool_calls: Some(
                    tool_calls
                        .iter()
                        .map(|c| OpenAiToolCallOut {
                            id: c.id.clone(),
                            kind: "function",
                            function: OpenAiFunctionOut {
                                name: c.name.clone(),
                                arguments: c.arguments.to_string(),
                            },
                        })
                        .collect(),
                ),
                tool_call_id: None,
            }),
            Msg::ToolResult { id, content, .. } => out.push(OpenAiMessage {
                role: "tool",
                content: Some(content.clone()),
                tool_calls: None,
                tool_call_id: Some(id.clone()),
            }),
        }
    }
    out
}

/// Tour OpenAI-compatible : requête, appel HTTP, mapping réponse → `AiTurn`.
async fn openai_chat(
    http: &Client,
    cfg: &ResolvedAiConfig,
    req: &AiChatRequest<'_>,
) -> Result<AiTurn, AiError> {
    let body = OpenAiRequest {
        model: &cfg.model,
        messages: build_openai_messages(req.messages),
        tools: req
            .tools
            .iter()
            .map(|t| OpenAiTool {
                kind: "function",
                function: OpenAiToolDef {
                    name: t.name,
                    description: t.description,
                    parameters: &t.input_schema,
                },
            })
            .collect(),
        max_tokens: req.max_tokens,
    };
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    egress_check(&url)?;
    let resp = http
        .post(&url)
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .send()
        .await
        .map_err(request_error)?;
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(map_http_error(status.as_u16(), &text, retry_after));
    }
    let parsed: OpenAiResponse = resp
        .json()
        .await
        .map_err(|e| AiError::Deserialize(e.to_string()))?;
    let Some(choice) = parsed.choices.into_iter().next() else {
        return Err(AiError::Deserialize("aucun choix dans la réponse".into()));
    };
    let finish = choice.finish_reason.as_deref().unwrap_or("stop");
    let mut turn = AiTurn {
        usage: Usage {
            input_tokens: parsed.usage.as_ref().map_or(0, |u| u.prompt_tokens),
            output_tokens: parsed.usage.as_ref().map_or(0, |u| u.completion_tokens),
        },
        stop_reason: Some(match finish {
            "tool_calls" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            "stop" | "end_turn" => StopReason::EndTurn,
            other => StopReason::Other(other.into()),
        }),
        ..Default::default()
    };
    if let Some(message) = choice.message {
        if let Some(Value::String(text)) = message.content {
            if !text.is_empty() {
                turn.text = Some(text);
            }
        }
        for call in message.tool_calls.unwrap_or_default() {
            let (Some(id), Some(function)) = (call.id, call.function) else {
                continue;
            };
            let (Some(name), Some(arguments)) = (function.name, function.arguments) else {
                continue;
            };
            let arguments: Value = serde_json::from_str(&arguments)
                .map_err(|e| AiError::Deserialize(format!("arguments d'outil illisibles: {e}")))?;
            turn.tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }
    Ok(turn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Msg {
        Msg::User { text: text.into() }
    }

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn anthropic_outil_simple_et_regroupement_tool_result() {
        // user → assistant(tool_use ×2) → tool_result ×2 : le contrat
        // Anthropic exige UN message user avec les 2 tool_result.
        let msgs = vec![
            user("lis A0"),
            Msg::Assistant {
                text: Some("je regarde".into()),
                tool_calls: vec![
                    call("t1", "list_devices", json!({})),
                    call("t2", "get_flow", json!({"flow_id": 3})),
                ],
            },
            Msg::ToolResult {
                id: "t1".into(),
                name: "list_devices".into(),
                content: "{}".into(),
                is_error: false,
            },
            Msg::ToolResult {
                id: "t2".into(),
                name: "get_flow".into(),
                content: "{\"x\":1}".into(),
                is_error: false,
            },
        ];
        let wire = build_anthropic_messages(&msgs);
        assert_eq!(
            wire.len(),
            3,
            "user, assistant, user(tool_results regroupés)"
        );
        assert_eq!(wire[0].role, "user");
        assert_eq!(wire[1].role, "assistant");
        assert_eq!(wire[2].role, "user");
        let blocks = wire[2].content.as_array().expect("content = tableau");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "t1");
        assert_eq!(blocks[1]["tool_use_id"], "t2");
    }

    #[test]
    fn anthropic_assistant_sans_texte_et_erreurs_is_error() {
        let msgs = vec![
            Msg::Assistant {
                text: None,
                tool_calls: vec![call("t9", "query_telemetry", json!({"metric": "a0"}))],
            },
            Msg::ToolResult {
                id: "t9".into(),
                name: "query_telemetry".into(),
                content: "métrique inconnue".into(),
                is_error: true,
            },
        ];
        let wire = build_anthropic_messages(&msgs);
        assert_eq!(wire.len(), 2);
        let blocks = wire[0].content.as_array().expect("assistant = blocks");
        assert_eq!(blocks.len(), 1, "pas de bloc text vide");
        assert_eq!(blocks[0]["type"], "tool_use");
        let results = wire[1].content.as_array().expect("tool_results");
        assert_eq!(results[0]["is_error"], true);
    }

    #[test]
    fn openai_tool_result_un_message_par_outil() {
        let msgs = vec![
            user("crée le flow"),
            Msg::Assistant {
                text: None,
                tool_calls: vec![call("c1", "create_flow", json!({"name": "n"}))],
            },
            Msg::ToolResult {
                id: "c1".into(),
                name: "create_flow".into(),
                content: "{\"flow_id\":7}".into(),
                is_error: false,
            },
        ];
        let wire = build_openai_messages(&msgs);
        assert_eq!(wire.len(), 3);
        assert_eq!(wire[1].role, "assistant");
        assert_eq!(
            wire[1].tool_calls.as_ref().unwrap()[0].function.arguments,
            r#"{"name":"n"}"#
        );
        assert_eq!(wire[2].role, "tool");
        assert_eq!(wire[2].tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn reponse_anthropic_deserialisee() {
        let raw = r#"{
            "content": [
                {"type": "text", "text": "voici le flow"},
                {"type": "tool_use", "id": "t1", "name": "create_flow", "input": {"name": "n"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 12, "output_tokens": 34}
        }"#;
        let parsed: AnthropicResponse = serde_json::from_str(raw).expect("désérialisable");
        assert_eq!(parsed.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(parsed.content.len(), 2);
        assert_eq!(parsed.usage.unwrap().input_tokens, 12);
    }

    #[test]
    fn reponse_openai_arguments_chaine_json() {
        let raw = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "validate_flow_graph", "arguments": "{\"graph\":{\"nodes\":[]}}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 6}
        }"#;
        let parsed: OpenAiResponse = serde_json::from_str(raw).expect("désérialisable");
        let choice = &parsed.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        let f = choice
            .message
            .as_ref()
            .unwrap()
            .tool_calls
            .as_ref()
            .unwrap()[0]
            .function
            .as_ref()
            .unwrap();
        assert_eq!(f.name.as_deref(), Some("validate_flow_graph"));
        let args: Value =
            serde_json::from_str(f.arguments.as_deref().unwrap_or_default()).expect("JSON valide");
        assert_eq!(args["graph"]["nodes"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn anthropic_base_strips_version_suffix() {
        assert_eq!(
            anthropic_base("https://api.anthropic.com"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            anthropic_base("https://api.anthropic.com/"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            anthropic_base("https://api.anthropic.com/v1"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            anthropic_base("https://api.anthropic.com/v1/"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            anthropic_base("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn erreurs_http_mappees() {
        let e = map_http_error(401, r#"{"error":{"message":"invalid x-api-key"}}"#, None);
        assert!(matches!(e, AiError::AuthRejected(ref m) if m.contains("invalid x-api-key")));
        let e = map_http_error(429, "{}", Some(17));
        assert!(matches!(e, AiError::RateLimited(Some(17))));
        let e = map_http_error(529, r#"{"error":{"message":"overloaded"}}"#, None);
        assert!(matches!(e, AiError::Upstream(529, ref m) if m.contains("overloaded")));
        let e = map_http_error(500, "", None);
        assert!(matches!(e, AiError::Upstream(500, _)));
    }

    #[test]
    fn message_erreur_extrait_de_toutes_les_formes() {
        assert_eq!(
            extract_error_message(r#"{"error":{"message":"boom"}}"#),
            "boom"
        );
        assert_eq!(extract_error_message(r#"{"message":"plat"}"#), "plat");
        assert_eq!(extract_error_message("  "), "(corps vide)");
        let brut = extract_error_message("not json");
        assert!(brut.contains("not json"));
    }

    #[test]
    fn provider_parse_et_noms_db() {
        assert_eq!(Provider::parse("anthropic"), Provider::Anthropic);
        assert_eq!(Provider::parse("openai_compat"), Provider::OpenAiCompat);
        assert_eq!(Provider::parse("OPENAI_COMPAT"), Provider::OpenAiCompat);
        assert_eq!(
            Provider::parse("inconnu"),
            Provider::Anthropic,
            "défaut = anthropic (valeur DB par défaut)"
        );
        assert_eq!(Provider::Anthropic.as_str(), "anthropic");
        assert_eq!(Provider::OpenAiCompat.as_str(), "openai_compat");
    }
}
