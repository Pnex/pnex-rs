//! Nœud custom EdgeLinkd `pnex-http-fetch` (C1a) — requête HTTP client
//! configurable « type curl » (amendement edge-model 2026-09-15 : la
//! collecte web courante est un simple bloc de flow, pas un domaine).
//!
//! Sémantique : la réponse **remplace** le payload (`payload` = corps auto-
//! parsé JSON si le content-type en contient, sinon texte ; `statusCode` =
//! code final après redirects). Le moteur ETL standard prend le relais.
//! Body POST : `config.body` littéral, sinon le payload entrant (chaîne →
//! tel quel, autre → sérialisé JSON + `content-type: application/json`
//! surchargeable par header explicite). Pas de templating d'URL v1 — le
//! calcul passe par un nœud calc amont.
//!
//! Erreurs (`on_error`, école notify D53) : `reject` (défaut) → erreur
//! réseau/timeout/HTTP ≥ 400/corps trop gros rejetée via `flow.handle_error`
//! (flow_error isolé, le flow survit, jamais de crash-loop) ; `passthrough`
//! → `payload` null + `statusCode` si dispo + `http_error` (raison), le
//! graphe décide (switch/range).
//!
//! Secrets (bearer, basic/proxy passwords, key header value): vault
//! references in flows.json (secrets.md D115, lot S5). They are fetched
//! from `PNEX_FLOW_SECRET_URL` on the first request (the backend serves a
//! secret only once the flow is marked deployed, right after the runtime
//! acknowledged the artifact), then kept in memory until the next deploy.
//! A legacy plaintext value is used as is. Providers de
//! scraping : mode proxy (`http://host:port` + user/pass) OU idiome « clé
//! dans l'URL cible » (ScraperAPI/ZenRows) — pas besoin de proxy. Limites
//! reqwest v1 : pas de SOCKS (feature off), pas de décompression gzip
//! (aucun `Accept-Encoding` annoncé → réponse identity), redirects suivis
//! (≤ 10, défaut reqwest).

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

/// Point d'ancrage référencé par le binaire `pnex-flow-runtime` : garantit que
/// l'édition de liens conserve les soumissions `inventory` de ce crate.
pub fn registered() {}

/// Cap de taille du corps de réponse — un scraping qui dérape ne doit pas
/// saturer la mémoire du runtime.
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Header carrying the flow runtime service token.
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";

/// Client HTTP du nœud (timeout total par requête + proxy éventuel), built
/// once its secrets are known. `proxy_password` = resolved proxy password.
/// Fail-fast au pré-flight : proxy invalide = artefact refusé
/// (`BadFlowsJson`), jamais un nœud silencieux.
fn build_client(cfg: &HttpFetchNodeConfig, proxy_password: &str) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent("pnex-http-fetch")
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .connect_timeout(Duration::from_secs(10));
    match &cfg.proxy {
        // « Direct » = direct : on ignore les HTTP_PROXY/HTTPS_PROXY de
        // l'environnement (prévisible, et les tests tournent sans env).
        pnex_core::HttpFetchProxy::None => {
            builder = builder.no_proxy();
        }
        pnex_core::HttpFetchProxy::Custom { url, username, .. } => {
            let mut proxy = reqwest::Proxy::all(url).map_err(|e| {
                EdgelinkError::BadFlowsJson(format!(
                    "pnex-http-fetch : proxy « {url} » invalide : {e}"
                ))
            })?;
            if let Some(u) = username {
                proxy = proxy.basic_auth(u, proxy_password);
            }
            builder = builder.proxy(proxy);
        }
    }
    Ok(builder.build().map_err(|e| {
        EdgelinkError::BadFlowsJson(format!("pnex-http-fetch : client HTTP impossible : {e}"))
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
            EdgelinkError::BadFlowsJson(format!("pnex-http-fetch : config invalide : {e}"))
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

    /// Raisonne l'échec selon `on_error` : reject → erreur rejetée (routée
    /// vers `flow.handle_error`) ; passthrough → payload null + statusCode
    /// si dispo + `http_error`, sortie normale.
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

    /// Un message entrant → requête HTTP → payload remplacé par la réponse.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        log::debug!(
            "pnex-http-fetch [{}] : {:?} {}",
            self.name(),
            self.config.method,
            self.config.url
        );

        // 1) Body : littéral de config, sinon payload entrant (POST only).
        let body: Option<(String, bool)> = match (&self.config.body, &self.config.method) {
            (Some(b), _) => Some((b.clone(), false)),
            (None, HttpFetchMethod::Get) => None,
            (None, HttpFetchMethod::Post) => {
                let payload = msg.read().await.get("payload").cloned();
                let json = match payload {
                    Some(v) => serde_json::to_value(&v).map_err(|e| {
                        EdgelinkError::InvalidOperation(format!(
                            "pnex-http-fetch [{}] : payload non sérialisable : {e}",
                            self.name()
                        ))
                    })?,
                    None => serde_json::Value::Null,
                };
                match json {
                    // Chaîne → corps brut ; null → pas de corps ; autre → JSON.
                    serde_json::Value::String(s) => Some((s, false)),
                    serde_json::Value::Null => None,
                    other => Some((
                        serde_json::to_string(&other).map_err(|e| {
                            EdgelinkError::InvalidOperation(format!(
                                "pnex-http-fetch [{}] : corps JSON impossible : {e}",
                                self.name()
                            ))
                        })?,
                        true,
                    )),
                }
            }
        };

        // 2) Requête : headers statiques, auth, body éventuel.
        let method = reqwest::Method::from_bytes(match self.config.method {
            HttpFetchMethod::Get => b"GET",
            HttpFetchMethod::Post => b"POST",
        })
        .map_err(|e| {
            EdgelinkError::InvalidOperation(format!(
                "pnex-http-fetch [{}] : méthode invalide : {e}",
                self.name()
            ))
        })?;
        let ready = match self.ready().await {
            Ok(ready) => ready,
            Err(reason) => return self.fail(reason, None, msg, cancel).await,
        };
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

        // 3) Envoi — erreur réseau/timeout → fail(reason, status=None).
        let mut resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return self
                    .fail(format!("requête échouée : {e}"), None, msg, cancel)
                    .await;
            }
        };
        let status = resp.status();

        // 4) HTTP ≥ 400 → fail(status=code). Passthrough : le graphe branche
        // sur statusCode ; reject : flow_error isolé.
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

        // 5) Corps borné (cap 10 Mo) puis payload = auto-parse JSON si le
        // content-type en contient, sinon texte (fallback silencieux).
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_RESPONSE_BYTES {
                return self
                    .fail(
                        format!("corps trop gros ({len} o > {MAX_RESPONSE_BYTES} o)"),
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
                "pnex-http-fetch [{}] : lecture du corps interrompue : {e}",
                self.name()
            ))
        })? {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return self
                    .fail(
                        format!("corps trop gros (> {MAX_RESPONSE_BYTES} o)"),
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

/// Frontière serde_json ↔ Variant du modèle edgelink (école pnex-node-device
/// calc.rs) — jamais de panic sur une valeur inattendue.
fn variant_from_json(json: serde_json::Value) -> Result<Variant> {
    Ok(serde_json::from_value(json).map_err(|e| {
        EdgelinkError::InvalidOperation(format!("pnex-http-fetch : valeur non convertible : {e}"))
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
                    // `with_uow` route les erreurs vers `flow.handle_error` sans
                    // les logger : on journalise ici pour l'exploitation.
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-http-fetch [{}] : message rejeté : {e}", node.name());
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
    fn node_enregistre_dans_le_registre() {
        let reg = RegistryBuilder::default().build().expect("registre");
        let meta = reg
            .get("pnex-http-fetch")
            .expect("nœud pnex-http-fetch absent du registre");
        assert_eq!(meta.type_, "pnex-http-fetch");
    }

    #[test]
    fn registered_ne_panique_pas() {
        registered();
    }

    /// La config projetée (champs aplatis, modes taggés) est désérialisable
    /// telle quelle par le nœud — le contrat projection → runtime.
    #[test]
    fn config_accepte_projection_aplatie() {
        let cfg: HttpFetchNodeConfig = serde_json::from_str(
            r#"{
                "url": "https://api.exemple.dev/x",
                "method": "post",
                "headers": [{"name": "Accept", "value": "application/json"}],
                "auth": {"mode": "bearer", "token": "t"},
                "proxy": {"mode": "none"},
                "timeout_secs": 15,
                "body": null,
                "on_error": "passthrough"
            }"#,
        )
        .expect("config projetée désérialisable");
        assert_eq!(cfg.auth, HttpFetchAuth::Bearer { token: "t".into() });
        assert_eq!(cfg.timeout_secs, 15);
    }

    /// Mode taggé inconnu → rejet explicite au build (pré-flight 400
    /// engine_load avec l'erreur réelle).
    #[test]
    fn config_rejete_mode_auth_inconnu() {
        let err = serde_json::from_str::<HttpFetchNodeConfig>(
            r#"{"url": "https://x", "auth": {"mode": "digest", "user": "u"}}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown variant"), "{err}");
    }

    /// Défauts : timeout 30 (pas 0 — sinon violation parasite côté éditeur
    /// et timeout désactivé côté runtime).
    #[test]
    fn config_defaut_timeout_trente() {
        let cfg: HttpFetchNodeConfig = serde_json::from_str(r#"{"url": "https://x"}"#).unwrap();
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(HttpFetchNodeConfig::default().timeout_secs, 30);
    }

    /// Proxy invalide → client refusé au build (fail-fast pré-flight).
    #[test]
    fn client_rejete_proxy_invalide() {
        let cfg = HttpFetchNodeConfig {
            proxy: HttpFetchProxy::Custom {
                url: "://pas-une-url".into(),
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

    /// Le header `HttpFetchHeader` de config reste serde-only (wasm-safe).
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
