//! LLM gateway of hosted agents (autonomous-agents.md D385, agent-environments.md D732).
//! A sandbox speaks the Anthropic or the OpenAI protocol to PneX with its run token; PneX
//! calls the provider the org routes to the `agent` usage, with a key that never enters the
//! sandbox. Each call is counted against the run and the org, and journaled in O2 with the
//! fingerprints of its request and answer, never their content (D709).

use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use chrono::Utc;
use loco_rs::app::AppContext;
use pnex_core::agent_runs::LlmUsageCount;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::services::agent_runs::{self, RunPrincipal};
use crate::services::ai::config::ResolvedAiConfig;
use crate::services::ai::provider::Provider;
use crate::services::openobserve::{ensure_org_credentials, Client, OpenobserveSettings};

pub const CALLS_STREAM: &str = "agent_llm_calls";
const AGENT_DAILY_CALLS: u64 = 5000;
const ANSWER_MAX: usize = 32 * 1024 * 1024;
const LINE_MAX: usize = 1024 * 1024;
/// Request headers an engine may need passed on; anything else, its key first, stays here.
const FORWARDED: [&str; 2] = ["anthropic-version", "anthropic-beta"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Anthropic,
    OpenAi,
}

impl Protocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }
}

#[derive(Debug)]
pub enum GatewayError {
    /// No provider for the `agent` usage, or AI switched off.
    NotConfigured,
    /// The provider speaks the other protocol.
    ProtocolMismatch,
    /// The run spent its calls.
    RunBudget,
    /// The org spent its daily calls for agents.
    OrgQuota(u64),
    Invalid,
    Upstream(String),
}

/// `settings.ai.agent_daily_calls`, else [`AGENT_DAILY_CALLS`].
fn daily_cap(ctx: &AppContext) -> u64 {
    ctx.config
        .settings
        .as_ref()
        .and_then(|s| s.get("ai"))
        .and_then(|ai| ai.get("agent_daily_calls"))
        .and_then(Value::as_u64)
        .unwrap_or(AGENT_DAILY_CALLS)
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// What one call leaves behind: counted on the run, one event in O2.
struct Record {
    ctx: AppContext,
    run: RunPrincipal,
    protocol: Protocol,
    provider: String,
    model: String,
    status: u16,
    started: Instant,
    request_sha256: String,
}

impl Record {
    fn finish(self, usage: LlmUsageCount, response_sha256: String) {
        tokio::spawn(async move {
            if let Err(e) = agent_runs::add_tokens(&self.ctx.db, self.run, usage).await {
                tracing::warn!(error = %e, run = %self.run.id, "run tokens not counted");
            }
            let Some(settings) = OpenobserveSettings::from_config(&self.ctx.config) else {
                return;
            };
            let client = Client::new(&settings);
            let event = json!({
                "_timestamp": Utc::now().timestamp_micros(),
                "run_id": self.run.id.to_string(),
                "protocol": self.protocol.as_str(),
                "provider": self.provider,
                "model": self.model,
                "status": self.status,
                "input_tokens": usage.input,
                "output_tokens": usage.output,
                "latency_ms": self.started.elapsed().as_millis() as u64,
                "request_sha256": self.request_sha256,
                "response_sha256": response_sha256,
            });
            let written = async {
                let creds = ensure_org_credentials(&self.ctx.db, &client, self.run.org_id).await?;
                client
                    .ingest_json(&creds.o2_org, CALLS_STREAM, &[event], &creds.email_passcode)
                    .await
            };
            if let Err(e) = written.await {
                tracing::warn!(error = %e, run = %self.run.id, "gateway call not journaled");
            }
        });
    }
}

/// Reads a streamed answer as it passes: its fingerprint and the usage of its events.
/// Dropped at the end of the stream or when the engine hangs up; either way the call is
/// recorded with what went through.
struct Tap {
    upstream: reqwest::Response,
    hasher: Sha256,
    usage: LlmUsageCount,
    line: Vec<u8>,
    record: Option<Record>,
}

impl Tap {
    fn observe(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
        for &b in chunk {
            if b != b'\n' {
                // An event line is small; a longer one is not read, only passed on.
                if self.line.len() < LINE_MAX {
                    self.line.push(b);
                }
                continue;
            }
            let line = std::mem::take(&mut self.line);
            if let Some(data) = line.strip_prefix(b"data:") {
                if let Ok(event) = serde_json::from_slice::<Value>(data.trim_ascii()) {
                    self.usage.read_event(&event);
                }
            }
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        if let Some(record) = self.record.take() {
            let sha = format!("sha256:{:x}", std::mem::take(&mut self.hasher).finalize());
            record.finish(self.usage, sha);
        }
    }
}

/// The request as sent upstream: the org's model in place of the engine's, and the usage
/// asked for in an OpenAI stream.
fn rewrite(
    protocol: Protocol,
    mut body: Value,
    model: &str,
) -> Result<(Value, bool), GatewayError> {
    let Some(obj) = body.as_object_mut() else {
        return Err(GatewayError::Invalid);
    };
    obj.insert("model".into(), json!(model));
    let stream = obj.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream && protocol == Protocol::OpenAi {
        obj.insert("stream_options".into(), json!({ "include_usage": true }));
    }
    Ok((body, stream))
}

fn upstream_url(protocol: Protocol, cfg: &ResolvedAiConfig) -> String {
    let base = cfg.base_url.trim_end_matches('/');
    match protocol {
        Protocol::Anthropic => {
            let root = base
                .strip_suffix("/v1/messages")
                .or_else(|| base.strip_suffix("/v1"))
                .unwrap_or(base);
            format!("{root}/v1/messages")
        }
        Protocol::OpenAi => format!("{base}/chat/completions"),
    }
}

/// One call of a run through the gateway.
pub async fn forward(
    ctx: &AppContext,
    run: RunPrincipal,
    protocol: Protocol,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response, GatewayError> {
    let cfg = match run.provider_id {
        Some(id) => {
            crate::services::ai::config::resolve_provider(&ctx.db, &ctx.config, run.org_id, id)
                .await
        }
        None => {
            crate::services::ai::config::resolve(
                &ctx.db,
                &ctx.config,
                run.org_id,
                pnex_core::LlmUsage::Agent,
            )
            .await
        }
    }
    .ok_or(GatewayError::NotConfigured)?;
    let speaks = match cfg.provider {
        Provider::Anthropic => Protocol::Anthropic,
        Provider::OpenAiCompat => Protocol::OpenAi,
    };
    if speaks != protocol {
        return Err(GatewayError::ProtocolMismatch);
    }
    let parsed: Value = serde_json::from_slice(&body).map_err(|_| GatewayError::Invalid)?;
    let (request, stream) = rewrite(protocol, parsed, &cfg.model)?;
    if !agent_runs::take_call(&ctx.db, run)
        .await
        .map_err(|e| GatewayError::Upstream(e.to_string()))?
    {
        return Err(GatewayError::RunBudget);
    }
    let cap = daily_cap(ctx);
    if let Err(crate::services::flow_ai::FlowAiError::Quota(n)) =
        crate::services::flow_ai::count_daily(ctx, "llm-agent", run.org_id, cap).await
    {
        return Err(GatewayError::OrgQuota(n));
    }
    let mut call = crate::services::ai::agent::shared_http()
        .post(upstream_url(protocol, &cfg))
        .timeout(cfg.timeout)
        .json(&request);
    call = match protocol {
        Protocol::Anthropic => {
            let mut call = call.header("x-api-key", &cfg.api_key);
            for name in FORWARDED {
                if let Some(v) = headers.get(name) {
                    call = call.header(name, v);
                }
            }
            if !headers.contains_key("anthropic-version") {
                call = call.header("anthropic-version", "2023-06-01");
            }
            call
        }
        Protocol::OpenAi => call.bearer_auth(&cfg.api_key),
    };
    let started = Instant::now();
    let upstream = call
        .send()
        .await
        .map_err(|e| GatewayError::Upstream(e.without_url().to_string()))?;
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let record = Record {
        ctx: ctx.clone(),
        run,
        protocol,
        provider: cfg.name.clone(),
        model: cfg.model.clone(),
        status: status.as_u16(),
        started,
        request_sha256: sha256(request.to_string().as_bytes()),
    };
    let mut response = if stream && status.is_success() {
        let tap = Tap {
            upstream,
            hasher: Sha256::new(),
            usage: LlmUsageCount::default(),
            line: Vec::new(),
            record: Some(record),
        };
        let chunks = futures_util::stream::unfold(Some(tap), |tap| async move {
            let mut tap = tap?;
            match tap.upstream.chunk().await {
                Ok(Some(chunk)) => {
                    tap.observe(&chunk);
                    Some((Ok::<Bytes, std::io::Error>(chunk), Some(tap)))
                }
                Ok(None) => None,
                Err(e) => Some((Err(std::io::Error::other(e.without_url())), None)),
            }
        });
        Response::new(Body::from_stream(chunks))
    } else {
        // The provider address is the org's: its answer is read within a bound.
        let mut upstream = upstream;
        let mut answer = Vec::new();
        while let Some(chunk) = upstream
            .chunk()
            .await
            .map_err(|e| GatewayError::Upstream(e.without_url().to_string()))?
        {
            if answer.len() + chunk.len() > ANSWER_MAX {
                return Err(GatewayError::Upstream("answer too large".into()));
            }
            answer.extend_from_slice(&chunk);
        }
        let usage = serde_json::from_slice::<Value>(&answer)
            .map(|v| LlmUsageCount::of_body(&v))
            .unwrap_or_default();
        record.finish(usage, sha256(&answer));
        Response::new(Body::from(answer))
    };
    *response.status_mut() =
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if let Some(ct) = content_type {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, ct);
    }
    Ok(response.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_org_model_replaces_the_engine_model() {
        let (body, stream) = rewrite(
            Protocol::Anthropic,
            json!({ "model": "whatever", "stream": true, "messages": [] }),
            "org-model",
        )
        .unwrap();
        assert_eq!(body["model"], "org-model");
        assert!(stream);
        assert!(body.get("stream_options").is_none());
        let (body, _) = rewrite(
            Protocol::OpenAi,
            json!({ "model": "x", "stream": true }),
            "m",
        )
        .unwrap();
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(rewrite(Protocol::OpenAi, json!([1]), "m").is_err());
    }
}
