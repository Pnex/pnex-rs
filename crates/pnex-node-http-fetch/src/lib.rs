//! Custom EdgeLinkd node `pnex-http-fetch` (C1a) — configurable "curl-like"
//! HTTP client request (edge-model amendment 2026-09-15: everyday
//! web collection is a simple flow block, not a domain).
//!
//! Semantics: the response **replaces** the payload (`payload` = body auto-
//! parsed as JSON if the content-type contains it, otherwise text; `statusCode` =
//! final code after redirects). The standard ETL engine takes over.
//! POST body: literal `config.body`, otherwise the incoming payload (string →
//! as is, other → serialized JSON + `content-type: application/json`
//! overridable by an explicit header). No URL templating in v1 — the
//! computation goes through an upstream calc node.
//!
//! Errors (`on_error`, notify D53 pattern): `reject` (default) → network/timeout
//! error/HTTP ≥ 400/oversized body rejected via `flow.handle_error`
//! (isolated flow_error, the flow survives, never a crash-loop); `passthrough`
//! → null `payload` + `statusCode` if available + `http_error` (reason), the
//! graph decides (switch/range).
//!
//! Secrets (bearer, basic/proxy passwords, key header value): vault
//! references in flows.json (secrets.md D115, lot S5). They are fetched
//! from `PNEX_FLOW_SECRET_URL` on the first request (the backend serves a
//! secret only once the flow is marked deployed, right after the runtime
//! acknowledged the artifact), then kept in memory until the next deploy.
//! Scraping
//! providers: proxy mode (`http://host:port` + user/pass) OR the "key
//! in the target URL" idiom (ScraperAPI/ZenRows) — no proxy needed. reqwest
//! v1 limits: no SOCKS (feature off), no gzip decompression
//! (no `Accept-Encoding` announced → identity response), redirects followed
//! (≤ 10, reqwest default).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::{
    HttpFetchAuth, HttpFetchMethod, HttpFetchNodeConfig, HttpFetchOnError, SecretSlot,
};

/// Anchor referenced by the `pnex-flow-runtime` binary: guarantees that
/// linking keeps this crate's `inventory` submissions.
pub fn registered() {}

/// Response body size cap — a runaway scraping must not
/// saturate the runtime memory.
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Header carrying the flow runtime service token.
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";

/// The node's HTTP client (total timeout per request + optional proxy), built
/// once its secrets are known. `proxy_password` = resolved proxy password.
/// Fail-fast at pre-flight: invalid proxy = artefact refused
/// (`BadFlowsJson`), never a silent node.
fn build_client(cfg: &HttpFetchNodeConfig, proxy_password: &str) -> Result<reqwest::Client> {
    // Egress guard (R8, SEC-W3): internal addresses refused at resolution,
    // each redirect re-checked.
    let mut builder = pnex_core::egress::guarded(reqwest::Client::builder())
        .redirect(pnex_core::egress::redirect_policy(10))
        .user_agent("pnex-http-fetch")
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .connect_timeout(Duration::from_secs(10));
    match &cfg.proxy {
        // "Direct" = direct: ignore HTTP_PROXY/HTTPS_PROXY from
        // the environment (predictable, and the tests run without env).
        pnex_core::HttpFetchProxy::None => {
            builder = builder.no_proxy();
        }
        pnex_core::HttpFetchProxy::Custom { url, username, .. } => {
            let mut proxy = reqwest::Proxy::all(url).map_err(|e| {
                EdgelinkError::BadFlowsJson(format!(
                    "pnex-http-fetch: invalid proxy \"{url}\": {e}"
                ))
            })?;
            if let Some(u) = username {
                proxy = proxy.basic_auth(u, proxy_password);
            }
            builder = builder.proxy(proxy);
        }
    }
    Ok(builder.build().map_err(|e| {
        EdgelinkError::BadFlowsJson(format!("pnex-http-fetch: cannot build HTTP client: {e}"))
    })?)
}

#[derive(Debug)]
#[flow_node("pnex-http-fetch", red_name = "pnex-http-fetch")]
struct PnexHttpFetchNode {
    base: BaseFlowNodeState,
    config: HttpFetchNodeConfig,
    /// Org stamped at deploy (vault lookups).
    org_id: i64,
    /// Vault endpoint + runtime token, when a secret field is a reference.
    secret_source: Option<(String, String)>,
    /// Client + secret values, resolved once (at build without vault
    /// references, on the first request otherwise).
    ready: tokio::sync::OnceCell<Ready>,
}

/// The node's secrets in plaintext (memory only, never logged).
#[derive(Debug)]
struct Ready {
    http: reqwest::Client,
    /// `auth.password` | `auth.token` | `auth.value` → value.
    auth_secret: String,
}

#[derive(Debug, Default, Deserialize)]
struct Stamps {
    #[serde(default)]
    pnex_org_id: i64,
}

/// One vault secret, retried briefly on 404: right after a first deploy
/// the backend may not have marked the flow deployed yet.
async fn fetch_secret(
    url: &str,
    token: &str,
    org_id: i64,
    id: uuid::Uuid,
) -> std::result::Result<String, String> {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .expect("reqwest client (rustls)")
    });
    let mut attempt = 0u64;
    loop {
        let resp = CLIENT
            .get(format!("{}/{id}", url.trim_end_matches('/')))
            .query(&[("org_id", org_id)])
            .header(FLOW_TOKEN_HEADER, token)
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .send()
            .await
            .map_err(|e| format!("secret endpoint unreachable: {e}"))?;
        let status = resp.status().as_u16();
        if status == 404 && attempt < 3 {
            attempt += 1;
            tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
            continue;
        }
        if status == 404 {
            return Err("secret not available: not referenced by a deployed flow".into());
        }
        if !resp.status().is_success() {
            return Err(format!("secret endpoint answered HTTP {status}"));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("secret endpoint answer unreadable: {e}"))?;
        return body
            .get("value")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| "secret endpoint answered without a value".into());
    }
}

impl PnexHttpFetchNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = HttpFetchNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-http-fetch: invalid config: {e}"))
        })?;
        let org_id = Stamps::deserialize(&config.rest)
            .unwrap_or_default()
            .pnex_org_id;
        // Pre-flight with a placeholder password: an invalid proxy URL is
        // refused at build, whatever the secrets.
        build_client(&cfg, "")?;
        let needs_vault = cfg
            .secret_slots()
            .iter()
            .any(|(_, slot)| slot.secret_id().is_some());
        let secret_source = if needs_vault {
            let url = std::env::var("PNEX_FLOW_SECRET_URL")
                .ok()
                .filter(|v| !v.is_empty());
            let token = std::env::var("PNEX_FLOW_WRITE_TOKEN")
                .ok()
                .filter(|v| !v.is_empty());
            match (url, token) {
                (Some(url), Some(token)) => Some((url, token)),
                _ => {
                    return Err(EdgelinkError::InvalidOperation(
                        "pnex-http-fetch: PNEX_FLOW_SECRET_URL/PNEX_FLOW_WRITE_TOKEN missing \
                         from the runtime environment (runtime_token not configured on the \
                         server?)"
                            .into(),
                    )
                    .into())
                }
            }
        } else {
            None
        };
        let ready = tokio::sync::OnceCell::new();
        if !needs_vault {
            let _ = ready.set(Self::assemble(&cfg, |slot| match slot {
                SecretSlot::Value(v) => v.clone(),
                _ => String::new(),
            })?);
        }
        Ok(Box::new(PnexHttpFetchNode {
            base: base_node,
            config: cfg,
            org_id,
            secret_source,
            ready,
        }))
    }

    /// Builds the client and picks the auth secret, `value_of` giving the
    /// plaintext of each slot.
    fn assemble(
        cfg: &HttpFetchNodeConfig,
        value_of: impl Fn(&SecretSlot) -> String,
    ) -> Result<Ready> {
        let mut auth_secret = String::new();
        let mut proxy_password = String::new();
        for (field, slot) in cfg.secret_slots() {
            if field == "proxy.password" {
                proxy_password = value_of(slot);
            } else {
                auth_secret = value_of(slot);
            }
        }
        Ok(Ready {
            http: build_client(cfg, &proxy_password)?,
            auth_secret,
        })
    }

    /// The resolved client and secrets (vault fetch on first use).
    async fn ready(&self) -> std::result::Result<&Ready, String> {
        self.ready
            .get_or_try_init(|| async {
                let Some((url, token)) = &self.secret_source else {
                    return Err("secret endpoint not configured".to_string());
                };
                let mut values = std::collections::BTreeMap::new();
                for (_, slot) in self.config.secret_slots() {
                    if let SecretSlot::Ref(id) = slot {
                        values.insert(*id, fetch_secret(url, token, self.org_id, *id).await?);
                    }
                }
                Self::assemble(&self.config, |slot| match slot {
                    SecretSlot::Value(v) => v.clone(),
                    SecretSlot::Ref(id) => values.get(id).cloned().unwrap_or_default(),
                    SecretSlot::Unset => String::new(),
                })
                .map_err(|e| e.to_string())
            })
            .await
    }

    /// Handles the failure according to `on_error`: reject → rejected error (routed
    /// to `flow.handle_error`); passthrough → null payload + statusCode
    /// if available + `http_error`, normal output.
    async fn fail(
        &self,
        reason: String,
        status: Option<u16>,
        msg: MsgHandle,
        cancel: CancellationToken,
    ) -> Result<()> {
        match self.config.on_error {
            HttpFetchOnError::Reject => Err(EdgelinkError::InvalidOperation(format!(
                "pnex-http-fetch [{}] : {reason}",
                self.name()
            ))
            .into()),
            HttpFetchOnError::Passthrough => {
                {
                    let mut m = msg.write().await;
                    m.set(
                        "payload".to_string(),
                        variant_from_json(serde_json::Value::Null)?,
                    );
                    if let Some(code) = status {
                        m.set(
                            "statusCode".to_string(),
                            variant_from_json(serde_json::json!(code))?,
                        );
                    }
                    m.set(
                        "http_error".to_string(),
                        variant_from_json(serde_json::json!(reason))?,
                    );
                }
                self.fan_out_one(Envelope { port: 0, msg }, cancel).await
            }
        }
    }

    /// One incoming message → HTTP request → payload replaced by the response.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        log::debug!(
            "pnex-http-fetch [{}] : {:?} {}",
            self.name(),
            self.config.method,
            self.config.url
        );

        // 1) Body: config literal, otherwise incoming payload (POST only).
        let body: Option<(String, bool)> = match (&self.config.body, &self.config.method) {
            (Some(b), _) => Some((b.clone(), false)),
            (None, HttpFetchMethod::Get) => None,
            (None, HttpFetchMethod::Post) => {
                let payload = msg.read().await.get("payload").cloned();
                let json = match payload {
                    Some(v) => serde_json::to_value(&v).map_err(|e| {
                        EdgelinkError::InvalidOperation(format!(
                            "pnex-http-fetch [{}]: payload not serializable: {e}",
                            self.name()
                        ))
                    })?,
                    None => serde_json::Value::Null,
                };
                match json {
                    // String → raw body; null → no body; other → JSON.
                    serde_json::Value::String(s) => Some((s, false)),
                    serde_json::Value::Null => None,
                    other => Some((
                        serde_json::to_string(&other).map_err(|e| {
                            EdgelinkError::InvalidOperation(format!(
                                "pnex-http-fetch [{}]: cannot serialize JSON body: {e}",
                                self.name()
                            ))
                        })?,
                        true,
                    )),
                }
            }
        };

        // 2) Request: static headers, auth, optional body.
        let method = reqwest::Method::from_bytes(match self.config.method {
            HttpFetchMethod::Get => b"GET",
            HttpFetchMethod::Post => b"POST",
        })
        .map_err(|e| {
            EdgelinkError::InvalidOperation(format!(
                "pnex-http-fetch [{}]: invalid method: {e}",
                self.name()
            ))
        })?;
        let ready = match self.ready().await {
            Ok(ready) => ready,
            Err(reason) => return self.fail(reason, None, msg, cancel).await,
        };
        // An IP literal skips the resolver: checked here (R8).
        let refused = reqwest::Url::parse(&self.config.url)
            .ok()
            .and_then(|u| pnex_core::egress::check_url(&u));
        if let Some(code) = refused {
            return self.fail(code.to_string(), None, msg, cancel).await;
        }
        let mut req = ready.http.request(method, &self.config.url);
        let mut explicit_content_type = false;
        for h in &self.config.headers {
            if h.name.eq_ignore_ascii_case("content-type") {
                explicit_content_type = true;
            }
            req = req.header(&h.name, &h.value);
        }
        match &self.config.auth {
            HttpFetchAuth::Basic { username, .. } => {
                req = req.basic_auth(username, Some(&ready.auth_secret));
            }
            HttpFetchAuth::Bearer { .. } => {
                req = req.bearer_auth(&ready.auth_secret);
            }
            HttpFetchAuth::Header { name, .. } => {
                req = req.header(name, &ready.auth_secret);
            }
            HttpFetchAuth::None => {}
        }
        if let Some((body, is_json)) = body {
            if is_json && !explicit_content_type {
                req = req.header(reqwest::header::CONTENT_TYPE, "application/json");
            }
            req = req.body(body);
        }

        // 3) Send — network error/timeout → fail(reason, status=None).
        let mut resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return self
                    .fail(format!("request failed: {e}"), None, msg, cancel)
                    .await;
            }
        };
        let status = resp.status();

        // 4) HTTP ≥ 400 → fail(status=code). Passthrough: the graph branches
        // on statusCode; reject: isolated flow_error.
        if status.is_client_error() || status.is_server_error() {
            return self
                .fail(
                    format!("HTTP {}", status.as_u16()),
                    Some(status.as_u16()),
                    msg,
                    cancel,
                )
                .await;
        }

        // 5) Bounded body (10 MB cap) then payload = JSON auto-parse if the
        // content-type contains it, otherwise text (silent fallback).
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_RESPONSE_BYTES {
                return self
                    .fail(
                        format!("body too large ({len} B > {MAX_RESPONSE_BYTES} B)"),
                        Some(status.as_u16()),
                        msg,
                        cancel,
                    )
                    .await;
            }
        }
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| {
            EdgelinkError::InvalidOperation(format!(
                "pnex-http-fetch [{}]: body read interrupted: {e}",
                self.name()
            ))
        })? {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return self
                    .fail(
                        format!("body too large (> {MAX_RESPONSE_BYTES} B)"),
                        Some(status.as_u16()),
                        msg,
                        cancel,
                    )
                    .await;
            }
            bytes.extend_from_slice(&chunk);
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let payload_json = if bytes.is_empty() {
            serde_json::Value::Null
        } else if content_type.contains("json") {
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_else(|_| {
                serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
            })
        } else {
            serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
        };

        {
            let mut m = msg.write().await;
            m.set("payload".to_string(), variant_from_json(payload_json)?);
            m.set(
                "statusCode".to_string(),
                variant_from_json(serde_json::json!(status.as_u16()))?,
            );
        }

        // 6) Sortie unique (port 0).
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

/// serde_json ↔ Variant boundary of the edgelink model (pnex-node-device
/// calc.rs pattern) — never a panic on an unexpected value.
fn variant_from_json(json: serde_json::Value) -> Result<Variant> {
    Ok(serde_json::from_value(json).map_err(|e| {
        EdgelinkError::InvalidOperation(format!("pnex-http-fetch: value not convertible: {e}"))
    })?)
}

#[async_trait]
impl FlowNodeBehavior for PnexHttpFetchNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PnexHttpFetchNode, msg: MsgHandle| async move {
                    // `with_uow` routes errors to `flow.handle_error` without
                    // logging them: we log here for operations.
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-http-fetch [{}]: message rejected: {e}", node.name());
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgelink_core::runtime::registry::RegistryBuilder;
    use pnex_core::{HttpFetchHeader, HttpFetchProxy};

    #[test]
    fn node_is_in_the_registry() {
        let reg = RegistryBuilder::default().build().expect("registry");
        let meta = reg
            .get("pnex-http-fetch")
            .expect("pnex-http-fetch node missing from the registry");
        assert_eq!(meta.type_, "pnex-http-fetch");
    }

    #[test]
    fn registered_does_not_panic() {
        registered();
    }

    /// The projected config (flattened fields, tagged modes) is deserializable
    /// as is by the node — the projection → runtime contract.
    #[test]
    fn config_accepts_flattened_projection() {
        let cfg: HttpFetchNodeConfig = serde_json::from_str(
            r#"{
                "url": "https://api.exemple.dev/x",
                "method": "post",
                "headers": [{"name": "Accept", "value": "application/json"}],
                "auth": {"mode": "bearer", "token": {"value": "t"}},
                "proxy": {"mode": "none"},
                "timeout_secs": 15,
                "body": null,
                "on_error": "passthrough"
            }"#,
        )
        .expect("projected config deserializable");
        assert_eq!(cfg.auth, HttpFetchAuth::Bearer { token: "t".into() });
        assert_eq!(cfg.timeout_secs, 15);
    }

    /// Unknown tagged mode → explicit rejection at build (pre-flight 400
    /// engine_load with the real error).
    #[test]
    fn config_rejects_unknown_auth_mode() {
        let err = serde_json::from_str::<HttpFetchNodeConfig>(
            r#"{"url": "https://x", "auth": {"mode": "digest", "user": "u"}}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown variant"), "{err}");
    }

    /// Defaults: timeout 30 (not 0 — otherwise a spurious violation in the editor
    /// and timeout disabled in the runtime).
    #[test]
    fn config_default_timeout_thirty() {
        let cfg: HttpFetchNodeConfig = serde_json::from_str(r#"{"url": "https://x"}"#).unwrap();
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(HttpFetchNodeConfig::default().timeout_secs, 30);
    }

    /// Invalid proxy → client refused at build (pre-flight fail-fast).
    #[test]
    fn client_rejects_invalid_proxy() {
        let cfg = HttpFetchNodeConfig {
            proxy: HttpFetchProxy::Custom {
                url: "://not-a-url".into(),
                username: None,
                password: pnex_core::SecretSlot::Unset,
            },
            ..Default::default()
        };
        assert!(build_client(&cfg, "").is_err());
        let ok = HttpFetchNodeConfig {
            proxy: HttpFetchProxy::Custom {
                url: "http://proxy.exemple.dev:8001".into(),
                username: Some("u".into()),
                password: "p".into(),
            },
            ..Default::default()
        };
        assert!(build_client(&ok, "p").is_ok());
    }

    /// The config header `HttpFetchHeader` stays serde-only (wasm-safe).
    #[test]
    fn headers_serde_roundtrip() {
        let hs = vec![
            HttpFetchHeader {
                name: "X-Api-Key".into(),
                value: "k".into(),
            },
            HttpFetchHeader {
                name: "Accept".into(),
                value: String::new(),
            },
        ];
        let json = serde_json::to_string(&hs).unwrap();
        let back: Vec<HttpFetchHeader> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, hs);
    }
}
