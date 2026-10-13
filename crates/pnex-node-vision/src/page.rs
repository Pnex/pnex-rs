//! `pnex-page-write` node (media-vision-studio.md D209) — the Markdown of
//! each message becomes a page of the org (or is appended to one) through
//! the backend (`/internal/flow/page-write`), which freezes a named version
//! stamped with the flow and the AI provenance of the text
//! (`payload.ai_generated`, D210).
//!
//! Output (port 0): `payload = {page_id, title, version, ai_generated}`,
//! `topic` kept. A refused write is logged and shown on the node status.
//!
//! `pnex-page-pdf` (D209, D210) renders a page version as a PDF stored in
//! the media library (`/internal/flow/page-pdf`): port 0 = ready
//! `{asset_id, filename, page_id, version, title}`, port 1 = an AI version
//! waits for approval `{page_id, version, title}`; nothing new = silence.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::pages::{
    PagePdfConfig, PagePdfRequest, PagePdfResponse, PagePdfSource, PagePdfStatus, PageWriteConfig,
    PageWriteMode, PageWriteRequest, PageWriteResponse,
};
use pnex_core::vision::{NodeStatus, NodeStatusLevel};
use pnex_node_camera::status::publish_status;

use crate::caption::post_ai;

const NODE: &str = "pnex-page-write";

/// Explicit fields, no `flatten` (arbitrary_precision, see llm_text).
#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(default)]
    mode: PageWriteMode,
    #[serde(default)]
    title: String,
    #[serde(default)]
    page_id: String,
    #[serde(default)]
    text_field: String,
    #[serde(default)]
    version_label: String,
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_node_id: String,
}

/// Markdown and AI flag of a message (`None` = no text at `field`).
pub fn markdown_of(payload: &Value, field: &str) -> Option<(String, bool)> {
    let text = pnex_core::resolve_msg_path(payload, field.trim())?
        .as_str()?
        .to_string();
    let ai = payload
        .get("ai_generated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some((text, ai))
}

#[derive(Default)]
struct Stats {
    received: AtomicU64,
    written: AtomicU64,
    failed: AtomicU64,
}

#[flow_node("pnex-page-write", red_name = "pnex-page-write")]
struct PageWriteNode {
    base: BaseFlowNodeState,
    cfg: PageWriteConfig,
    org_id: i64,
    flow_id: i64,
    node_id: String,
    http: reqwest::Client,
    url: String,
    token: String,
    stats: Stats,
    last_error: std::sync::Mutex<Option<String>>,
}

impl PageWriteNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let raw = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let cfg = PageWriteConfig {
            mode: raw.mode,
            title: raw.title,
            page_id: raw.page_id,
            text_field: raw.text_field,
            version_label: raw.version_label,
        };
        if let Some((code, message)) = cfg.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if raw.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let env = |k: &str| {
            std::env::var(k).map_err(|_| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} : {k} is not set in the runtime environment"
                ))
            })
        };
        let url = env("PNEX_FLOW_PAGE_URL")?;
        let token = env("PNEX_FLOW_WRITE_TOKEN")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(PageWriteNode {
            base: base_node,
            cfg,
            org_id: raw.pnex_org_id,
            flow_id: raw.pnex_flow_id,
            node_id: raw.pnex_node_id,
            http,
            url,
            token,
            stats: Stats::default(),
            last_error: std::sync::Mutex::new(None),
        }))
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match self.last_error.lock().expect("page error").clone() {
            Some(e) => NodeStatus::new(NodeStatusLevel::Error, "page-write-failed").detail(e),
            None => NodeStatus::new(NodeStatusLevel::Ok, "vision-running"),
        };
        [
            ("received", &s.received),
            ("written", &s.written),
            ("failed", &s.failed),
        ]
        .into_iter()
        .fold(base, |st, (k, v)| st.stat(k, v.load(Ordering::Relaxed)))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .unwrap_or(Value::Null)
        };
        let Some((markdown, ai_generated)) = markdown_of(&payload, &self.cfg.text_field) else {
            log::warn!(
                "{NODE} [{}] : message ignored — no text at \"{}\"",
                self.name(),
                self.cfg.text_field
            );
            return Ok(());
        };
        let req = PageWriteRequest {
            org_id: self.org_id,
            flow_id: self.flow_id,
            mode: self.cfg.mode,
            title: self.cfg.title.clone(),
            page_id: self.cfg.page_id.clone(),
            markdown,
            version_label: self.cfg.version_label.clone(),
            ai_generated,
        };
        let res: std::result::Result<PageWriteResponse, String> =
            post_ai(&self.http, &self.url, &self.token, &req).await;
        let res = match res {
            Ok(r) => {
                *self.last_error.lock().expect("page error") = None;
                self.stats.written.fetch_add(1, Ordering::Relaxed);
                r
            }
            Err(e) => {
                log::warn!("{NODE} [{}] : write refused: {e}", self.name());
                self.stats.failed.fetch_add(1, Ordering::Relaxed);
                *self.last_error.lock().expect("page error") = Some(e);
                publish_status(self, &self.node_id, &self.status());
                return Ok(());
            }
        };
        publish_status(self, &self.node_id, &self.status());
        let out = json!({
            "page_id": res.page_id,
            "title": res.title,
            "version": res.version,
            "ai_generated": ai_generated,
        });
        msg.write()
            .await
            .set("payload".to_string(), Variant::from(out));
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PageWriteNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PageWriteNode, msg: MsgHandle| async move {
                    let res = node.execute(msg, cancel.child_token()).await;
                    if let Err(e) = &res {
                        log::warn!("{NODE} [{}] : message rejected : {e}", node.name());
                    }
                    res
                },
            )
            .await;
        }
    }
}

const PDF_NODE: &str = "pnex-page-pdf";

#[derive(Debug, Deserialize)]
struct PdfNodeConfig {
    #[serde(default)]
    source: PagePdfSource,
    #[serde(default)]
    page_id: String,
    #[serde(default = "yes")]
    require_approval: bool,
    #[serde(default)]
    language: String,
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_node_id: String,
}

fn yes() -> bool {
    true
}

/// Page and version named by a message (`message` source).
pub fn version_of(payload: &Value) -> Option<(String, i64)> {
    let page = payload.get("page_id")?.as_str()?.to_string();
    let version = payload.get("version")?.as_i64()?;
    Some((page, version))
}

#[flow_node("pnex-page-pdf", red_name = "pnex-page-pdf")]
struct PagePdfNode {
    base: BaseFlowNodeState,
    cfg: PagePdfConfig,
    org_id: i64,
    flow_id: i64,
    node_id: String,
    http: reqwest::Client,
    url: String,
    token: String,
    stats: Stats,
    last_error: std::sync::Mutex<Option<String>>,
}

impl PagePdfNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let raw = PdfNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("{PDF_NODE} : invalid config : {e}"))
        })?;
        let cfg = PagePdfConfig {
            source: raw.source,
            page_id: raw.page_id,
            require_approval: raw.require_approval,
            language: raw.language,
        };
        if let Some((code, message)) = cfg.check() {
            return Err(
                EdgelinkError::BadFlowsJson(format!("{PDF_NODE} [{code}] : {message}")).into(),
            );
        }
        if raw.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{PDF_NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let env = |k: &str| {
            std::env::var(k).map_err(|_| {
                EdgelinkError::InvalidOperation(format!(
                    "{PDF_NODE} : {k} is not set in the runtime environment"
                ))
            })
        };
        // Same host and token as the page writes.
        let url = env("PNEX_FLOW_PAGE_URL")?.replace("/page-write", "/page-pdf");
        let token = env("PNEX_FLOW_WRITE_TOKEN")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| {
                EdgelinkError::InvalidOperation(format!("{PDF_NODE} : http client: {e}"))
            })?;
        Ok(Box::new(PagePdfNode {
            base: base_node,
            cfg,
            org_id: raw.pnex_org_id,
            flow_id: raw.pnex_flow_id,
            node_id: raw.pnex_node_id,
            http,
            url,
            token,
            stats: Stats::default(),
            last_error: std::sync::Mutex::new(None),
        }))
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match self.last_error.lock().expect("pdf error").clone() {
            Some(e) => NodeStatus::new(NodeStatusLevel::Error, "page-pdf-failed").detail(e),
            None => NodeStatus::new(NodeStatusLevel::Ok, "vision-running"),
        };
        [
            ("received", &s.received),
            ("written", &s.written),
            ("failed", &s.failed),
        ]
        .into_iter()
        .fold(base, |st, (k, v)| st.stat(k, v.load(Ordering::Relaxed)))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .unwrap_or(Value::Null)
        };
        let (page_id, version) = match self.cfg.source {
            PagePdfSource::Message => match version_of(&payload) {
                Some((p, v)) => (p, Some(v)),
                None => {
                    log::warn!(
                        "{PDF_NODE} [{}] : message ignored — expected payload.page_id and payload.version",
                        self.name()
                    );
                    return Ok(());
                }
            },
            PagePdfSource::LatestSendable => (self.cfg.page_id.clone(), None),
        };
        let req = PagePdfRequest {
            org_id: self.org_id,
            flow_id: self.flow_id,
            node_id: self.node_id.clone(),
            source: self.cfg.source,
            page_id,
            version,
            require_approval: self.cfg.require_approval,
            language: self.cfg.language.clone(),
        };
        let res: std::result::Result<PagePdfResponse, String> =
            post_ai(&self.http, &self.url, &self.token, &req).await;
        let res = match res {
            Ok(r) => {
                *self.last_error.lock().expect("pdf error") = None;
                r
            }
            Err(e) => {
                log::warn!("{PDF_NODE} [{}] : export refused: {e}", self.name());
                self.stats.failed.fetch_add(1, Ordering::Relaxed);
                *self.last_error.lock().expect("pdf error") = Some(e);
                publish_status(self, &self.node_id, &self.status());
                return Ok(());
            }
        };
        let port = match res.status {
            PagePdfStatus::NothingNew => return Ok(()),
            PagePdfStatus::Ready => {
                self.stats.written.fetch_add(1, Ordering::Relaxed);
                0
            }
            PagePdfStatus::NeedsApproval => 1,
        };
        publish_status(self, &self.node_id, &self.status());
        let out = serde_json::to_value(&res).unwrap_or(Value::Null);
        msg.write()
            .await
            .set("payload".to_string(), Variant::from(out));
        self.fan_out_one(Envelope { port, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PagePdfNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PagePdfNode, msg: MsgHandle| async move {
                    let res = node.execute(msg, cancel.child_token()).await;
                    if let Err(e) = &res {
                        log::warn!("{PDF_NODE} [{}] : message rejected : {e}", node.name());
                    }
                    res
                },
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_named_by_a_message() {
        let p = json!({"page_id": "p", "version": 3, "title": "x"});
        assert_eq!(version_of(&p), Some(("p".into(), 3)));
        assert_eq!(version_of(&json!({"page_id": "p"})), None);
    }

    #[test]
    fn markdown_and_ai_flag_of_a_message() {
        let p = json!({"text": "# Report", "ai_generated": true});
        assert_eq!(markdown_of(&p, "text"), Some(("# Report".into(), true)));
        let q = json!({"out": {"md": "x"}});
        assert_eq!(markdown_of(&q, "out.md"), Some(("x".into(), false)));
        assert_eq!(markdown_of(&q, "text"), None);
    }
}
