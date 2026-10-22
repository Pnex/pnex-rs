//! Ontology nodes (partner-watch.md D312): `pnex-ontology-query`,
//! `pnex-object-upsert`, `pnex-link-upsert`. The runtime has no database
//! access: each node calls `/internal/flow/ontology/{query,upsert,link}`
//! naming itself (org, flow, version, node); the backend reads the node
//! config from the deployed version and writes through the ontology
//! services (same rules as the UI). A refused call is logged and the
//! message dropped.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::ontology::api::{PropertyFilter, QueryResult};
use pnex_core::ontology::flow::{
    resolve_value, FlowLinkRequest, FlowNodeRef, FlowQueryRequest, FlowUpsertAck,
    FlowUpsertRequest, QueryOutput,
};

const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";

/// Fields every ontology node carries in the artifact. Explicit fields,
/// no `flatten` (arbitrary_precision).
#[derive(Debug, Deserialize)]
struct Common {
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_version: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

/// HTTP side shared by the three nodes.
struct Backend {
    node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl Backend {
    fn build(name: &str, c: Common, path: &str) -> Result<Self> {
        if c.pnex_org_id <= 0 || c.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{name} : pnex_org_id / pnex_flow_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let base = std::env::var("PNEX_FLOW_ONTOLOGY_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{name} : PNEX_FLOW_ONTOLOGY_URL is not set in the runtime environment"
            ))
        })?;
        let token = std::env::var("PNEX_FLOW_WRITE_TOKEN").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{name} : PNEX_FLOW_WRITE_TOKEN is not set in the runtime environment"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{name} : http client: {e}")))?;
        Ok(Self {
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: format!("{}/{path}", base.trim_end_matches('/')),
            token,
        })
    }

    /// POSTs `body`; `None` (logged) when refused or unreachable.
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        who: &str,
        body: &impl serde::Serialize,
    ) -> Option<T> {
        let res = self
            .http
            .post(&self.url)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .json(body)
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => match r.json::<T>().await {
                Ok(v) => Some(v),
                Err(e) => {
                    log::warn!("{who} : unreadable answer: {e}");
                    None
                }
            },
            Ok(r) => {
                let status = r.status();
                let text = r.text().await.unwrap_or_default();
                log::warn!("{who} : refused ({status}): {text}");
                None
            }
            Err(e) => {
                log::warn!("{who} : call failed: {e}");
                None
            }
        }
    }
}

fn to_json(v: Option<&Variant>) -> Value {
    v.map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
        .unwrap_or(Value::Null)
}

/// The whole message as JSON (for `$msg.<path>` values).
async fn msg_json(msg: &MsgHandle) -> Value {
    let m = msg.read().await;
    serde_json::to_value(&*m).unwrap_or(Value::Null)
}

fn path<'a>(v: &'a Value, p: &str) -> Option<&'a Value> {
    p.split('.').try_fold(v, |cur, seg| match cur {
        Value::Object(o) => o.get(seg),
        Value::Array(a) => a.get(seg.parse::<usize>().ok()?),
        _ => None,
    })
}

/// Filter values with the runtime tokens resolved for this message.
fn values_for(filters: &[PropertyFilter], msg: &Value) -> Vec<Value> {
    let now = chrono::Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let now = now.to_rfc3339();
    let read = |p: &str| path(msg, p).cloned();
    filters
        .iter()
        .map(|f| resolve_value(&f.value, &today, &now, &read))
        .collect()
}

// ───────────────────────────── ontology-query ─────────────────────────────

#[derive(Debug, Deserialize)]
struct QueryConfig {
    #[serde(default)]
    filters: Vec<PropertyFilter>,
    #[serde(default)]
    output: QueryOutput,
}

#[flow_node("pnex-ontology-query", red_name = "pnex-ontology-query")]
struct OntologyQueryNode {
    base: BaseFlowNodeState,
    config: QueryConfig,
    backend: Backend,
}

impl OntologyQueryNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let name = "pnex-ontology-query";
        let cfg = QueryConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{name} : invalid config : {e}")))?;
        let common = Common::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{name} : invalid config : {e}")))?;
        Ok(Box::new(OntologyQueryNode {
            base: base_node,
            config: cfg,
            backend: Backend::build(name, common, "query")?,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let req = FlowQueryRequest {
            node: self.backend.node.clone(),
            values: values_for(&self.config.filters, &msg_json(&msg).await),
        };
        let who = format!("pnex-ontology-query [{}]", self.name());
        let Some(result) = self.backend.call::<QueryResult>(&who, &req).await else {
            return Ok(());
        };
        // The day the query ran, for the functions downstream (no clock in
        // Starlark).
        let now = chrono::Utc::now();
        let (today, now) = (now.format("%Y-%m-%d").to_string(), now.to_rfc3339());
        let objects: Vec<Value> = result
            .rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.object.id, "type": r.object.type_key, "title": r.object.title,
                    "version": r.object.version, "properties": r.object.properties,
                })
            })
            .collect();
        match self.config.output {
            QueryOutput::Array => {
                {
                    let mut m = msg.write().await;
                    m.set("payload".into(), Variant::from(Value::Array(objects)));
                    m.set("count".into(), Variant::from(result.count));
                    m.set("today".into(), Variant::from(today.clone()));
                    m.set("now".into(), Variant::from(now.clone()));
                }
                self.fan_out_one(Envelope { port: 0, msg }, cancel).await
            }
            QueryOutput::Each => {
                for o in objects {
                    let out = msg.deep_clone(false).await;
                    {
                        let mut m = out.write().await;
                        m.set("payload".into(), Variant::from(o["properties"].clone()));
                        m.set("today".into(), Variant::from(today.clone()));
                        m.set("now".into(), Variant::from(now.clone()));
                        m.set(
                            "object".into(),
                            Variant::from(serde_json::json!({
                                "id": o["id"], "type": o["type"], "title": o["title"], "version": o["version"],
                            })),
                        );
                    }
                    self.fan_out_one(Envelope { port: 0, msg: out }, cancel.child_token())
                        .await?;
                }
                Ok(())
            }
        }
    }
}

// ───────────────────────────── object-upsert ─────────────────────────────

#[flow_node("pnex-object-upsert", red_name = "pnex-object-upsert")]
struct ObjectUpsertNode {
    base: BaseFlowNodeState,
    backend: Backend,
}

impl ObjectUpsertNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let name = "pnex-object-upsert";
        let common = Common::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{name} : invalid config : {e}")))?;
        Ok(Box::new(ObjectUpsertNode {
            base: base_node,
            backend: Backend::build(name, common, "upsert")?,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let req = {
            let m = msg.read().await;
            let Value::Object(properties) = to_json(m.get("payload")) else {
                log::warn!(
                    "pnex-object-upsert [{}] : message dropped — payload is not an object of properties",
                    self.name()
                );
                return Ok(());
            };
            let object = to_json(m.get("object"));
            FlowUpsertRequest {
                node: self.backend.node.clone(),
                object_id: to_json(m.get("object_id"))
                    .as_str()
                    .or_else(|| object.get("id").and_then(Value::as_str))
                    .map(str::to_string),
                title: to_json(m.get("title")).as_str().map(str::to_string),
                properties,
                expected_version: to_json(m.get("expected_version"))
                    .as_i64()
                    .map(|v| v as i32),
            }
        };
        let who = format!("pnex-object-upsert [{}]", self.name());
        let Some(ack) = self.backend.call::<FlowUpsertAck>(&who, &req).await else {
            return Ok(());
        };
        {
            let mut m = msg.write().await;
            m.set(
                "object".into(),
                Variant::from(serde_json::to_value(&ack.object).unwrap_or(Value::Null)),
            );
            m.set("created".into(), Variant::from(ack.created));
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

// ───────────────────────────── link-upsert ─────────────────────────────

#[flow_node("pnex-link-upsert", red_name = "pnex-link-upsert")]
struct LinkUpsertNode {
    base: BaseFlowNodeState,
    backend: Backend,
}

impl LinkUpsertNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let name = "pnex-link-upsert";
        let common = Common::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{name} : invalid config : {e}")))?;
        Ok(Box::new(LinkUpsertNode {
            base: base_node,
            backend: Backend::build(name, common, "link")?,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload = to_json(msg.read().await.get("payload"));
        let Some(req) = link_request(self.backend.node.clone(), &payload) else {
            log::warn!(
                "pnex-link-upsert [{}] : message dropped — payload needs source_id and target_id",
                self.name()
            );
            return Ok(());
        };
        let who = format!("pnex-link-upsert [{}]", self.name());
        let Some(link) = self.backend.call::<Value>(&who, &req).await else {
            return Ok(());
        };
        msg.write().await.set("link".into(), Variant::from(link));
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

fn link_request(node: FlowNodeRef, p: &Value) -> Option<FlowLinkRequest> {
    let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
    Some(FlowLinkRequest {
        node,
        source_id: s("source_id")?,
        target_id: s("target_id")?,
        attributes: p
            .get("attributes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        valid_from: s("valid_from"),
        confidence: p.get("confidence").and_then(Value::as_f64),
        evidence: p.get("evidence").cloned().unwrap_or_default(),
    })
}

macro_rules! run_each {
    ($t:ty) => {
        #[async_trait]
        impl FlowNodeBehavior for $t {
            fn get_base(&self) -> &BaseFlowNodeState {
                &self.base
            }

            async fn run(self: Arc<Self>, stop_token: CancellationToken) {
                while !stop_token.is_cancelled() {
                    let cancel = stop_token.child_token();
                    with_uow(
                        self.as_ref(),
                        cancel.child_token(),
                        |node: &$t, msg: MsgHandle| async move {
                            node.execute(msg, cancel.child_token()).await
                        },
                    )
                    .await;
                }
            }
        }
    };
}

run_each!(OntologyQueryNode);
run_each!(ObjectUpsertNode);
run_each!(LinkUpsertNode);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filter_values_resolve_per_message() {
        let filters: Vec<PropertyFilter> = serde_json::from_value(json!([
            {"key": "prochaine_echeance", "op": "lte", "value": "$today"},
            {"key": "siren", "op": "eq", "value": "$msg.payload.siren"},
            {"key": "etat", "op": "in", "value": ["ouverte"]}
        ]))
        .unwrap();
        let v = values_for(&filters, &json!({"payload": {"siren": "123456789"}}));
        assert_eq!(v[0].as_str().unwrap().len(), 10);
        assert_eq!(v[1], json!("123456789"));
        assert_eq!(v[2], json!(["ouverte"]));
    }

    #[test]
    fn link_needs_both_ends() {
        let node = FlowNodeRef {
            org_id: 1,
            flow_id: 2,
            version: 3,
            node_id: "n".into(),
        };
        let r = link_request(
            node.clone(),
            &json!({"source_id": "a", "target_id": "b", "attributes": {"role": "x"}}),
        )
        .unwrap();
        assert_eq!(r.attributes["role"], json!("x"));
        assert!(link_request(node, &json!({"source_id": "a"})).is_none());
    }
}
