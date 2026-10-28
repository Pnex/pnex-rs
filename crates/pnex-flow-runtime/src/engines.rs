//! The engine farm: **one edgelinkd `Engine` per deployed flow** — 1 tab
//! of the artifact = 1 engine in the same process.
//!
//! Why a farm: the vendor loads its array all-or-nothing
//! (`Flow::new(..)?` short-circuits the loading loop) and
//! `redeploy_flows` **empties its maps before** reloading — one invalid tab
//! rejected the whole artifact → exit(1) → supervisor crash-loop, all
//! flows down. The farm isolates: an invalid tab only fails its own
//! engine (`flow_error` on stdout), the others keep running; reloading a
//! tab does not stop the others (no more global ingestion pause).
//!
//! Added stdout contract: `flow_started {flow, rev}`, `flow_error {flow,
//! error}` — the deploy acknowledgement goes through these per-flow events
//! (the backend supervisor subscribes to them; runtime.json stays process
//! health only). `--check` reuses `build_engine` without `start()` for the
//! deploy pre-flight.
//!
//! Semantic delta (accepted, see flow-engine.md §5): "global" context,
//! link call and the http-response registry become **intra-flow** — each
//! engine is self-contained (own ContextManager, registries, channels,
//! CancellationToken), zero mutable global state in the vendor.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgelink_core::runtime::engine::Engine;
use edgelink_core::runtime::registry::RegistryHandle;
use serde_json::Value;

/// A tab of the artifact isolated with its nodes — the unit of deployment.
pub struct TabInput {
    /// `pnex_flow_id` of the tab (fallback: `pnexflow{id}` prefix) — farm
    /// key. Tabs without a resolvable identifier are ignored (warn).
    pub flow_id: i64,
    pub tab_id: String,
    /// Node-RED array of the tab (tab first, then `z` entries).
    pub array: Value,
    /// SHA-256 hex of the canonical array — the reload diff unit.
    pub hash: String,
}

/// Outcome of a boot/reload cycle for a flow.
pub enum FlowOutcome {
    /// Engine loaded + started — or unchanged (idempotent confirmation, it
    /// resolves a pending deploy acknowledgement without touching the engine).
    Started {
        flow_id: i64,
        tab_id: String,
        rev: String,
    },
    /// Build (or start) failed. If a last-good was running, it keeps going
    /// (ingestion preserved) — the backend chip will show "error".
    Failed {
        flow_id: i64,
        tab_id: String,
        error: String,
    },
    /// Tab gone from the artifact: engine stopped and removed.
    Stopped { flow_id: i64, tab_id: String },
}

/// Report of a reload cycle. **Per-flow** outcomes are emitted on the
/// fly via the callback passed to [`EngineFarm::reload`] (a slow tab must
/// not delay the deploy acknowledgement of the others) — the report only
/// carries the aggregates.
pub struct ReloadReport {
    /// Number of changes applied (swaps + removals).
    pub changed: u32,
    /// Removed flows (debug pumps to abort in main).
    pub removed: Vec<i64>,
}

/// An engine + the metadata the runtime needs (debug, reload
/// diff). Mutations serialised by the signal loop; `lifecycle`
/// protects `start`/`stop` (the vendor uses `try_write` — a concurrent access
/// would return "already started"/"not started").
pub struct FlowEngine {
    pub flow_id: i64,
    pub tab_id: String,
    pub engine: Engine,
    pub tab_hash: String,
    /// hex(node `ElementId`) → editor id, to stamp the debug.
    pub node_by_hex: HashMap<String, String>,
    pub lifecycle: Arc<tokio::sync::Mutex<()>>,
}

/// The farm: `engines` = live flows (running or last-good), `failed` =
/// last build failure per flow (faulty hash + error). An unchanged tab
/// is not retried on every SIGUSR1: retry = modified tab or process
/// restart.
pub struct EngineFarm {
    reg: RegistryHandle,
    engines: HashMap<i64, FlowEngine>,
    failed: HashMap<i64, (String, String)>,
}

impl EngineFarm {
    pub fn new(reg: RegistryHandle) -> Self {
        Self {
            reg,
            engines: HashMap::new(),
            failed: HashMap::new(),
        }
    }

    /// Boot: builds + starts each engine independently — one tab's
    /// failure does not affect the following ones. Each outcome is emitted **as
    /// soon as it happens** via `on_outcome` (stdout → deploy acknowledgement):
    /// a tab slow at boot must not delay the announcements of the following ones.
    pub async fn boot(&mut self, tabs: Vec<TabInput>, on_outcome: &mut dyn FnMut(FlowOutcome)) {
        for tab in tabs {
            match self.build_and_start(&tab).await {
                Ok(engine) => {
                    self.engines.insert(tab.flow_id, engine);
                    on_outcome(FlowOutcome::Started {
                        flow_id: tab.flow_id,
                        tab_id: tab.tab_id,
                        rev: tab.hash,
                    });
                }
                Err(e) => {
                    self.failed
                        .insert(tab.flow_id, (tab.hash.clone(), e.clone()));
                    on_outcome(FlowOutcome::Failed {
                        flow_id: tab.flow_id,
                        tab_id: tab.tab_id,
                        error: e,
                    });
                }
            }
        }
    }

    /// Reload cycle: diff by `tab_hash`, swap limited to the flow concerned,
    /// last-good preserved on build failure. Outcomes are emitted
    /// **on the fly** via `on_outcome` — the `flow_started` of a tab whose
    /// build+start just succeeded goes out **before** the removal stops
    /// (potentially slow) that follow: the deploy acknowledgement can no
    /// longer lose the race against a full cycle.
    pub async fn reload(
        &mut self,
        tabs: Vec<TabInput>,
        on_outcome: &mut dyn FnMut(FlowOutcome),
    ) -> ReloadReport {
        let mut report = ReloadReport {
            changed: 0,
            removed: Vec::new(),
        };
        let new_ids: HashSet<i64> = tabs.iter().map(|t| t.flow_id).collect();

        for tab in tabs {
            let (fid, hash) = (tab.flow_id, tab.hash.clone());

            // 1) Unchanged (running or last-good with current hash):
            //    idempotent confirmation, the engine is not touched.
            if self.engines.get(&fid).is_some_and(|fe| fe.tab_hash == hash) {
                on_outcome(FlowOutcome::Started {
                    flow_id: fid,
                    tab_id: tab.tab_id,
                    rev: hash,
                });
                continue;
            }
            // 2) Already failed for THIS hash: no retry on an unchanged tab
            //    (retry = modified tab or process restart).
            if let Some((h, err)) = self.failed.get(&fid) {
                if *h == hash {
                    on_outcome(FlowOutcome::Failed {
                        flow_id: fid,
                        tab_id: tab.tab_id,
                        error: err.clone(),
                    });
                    continue;
                }
            }

            // 3) Build the new engine BEFORE touching the old one
            //    (last-good preserved on build failure).
            match build_engine(&self.reg, &tab) {
                Ok(engine) => {
                    // Stop the old one BEFORE starting the new one: two engines
                    // at once on the same graph would double the injects
                    // (each engine has its own timers). Outage limited to this
                    // flow (same semantics as the old redeploy_flows).
                    if let Some(old) = self.engines.remove(&fid) {
                        let _guard = old.lifecycle.lock().await;
                        if let Err(e) = old.engine.stop().await {
                            log::warn!("flow {fid}: stopping the last-good: {e}");
                        }
                    }
                    match engine.start().await {
                        Ok(()) => {
                            self.failed.remove(&fid);
                            self.engines.insert(
                                fid,
                                FlowEngine {
                                    flow_id: fid,
                                    tab_id: tab.tab_id.clone(),
                                    engine,
                                    tab_hash: hash.clone(),
                                    node_by_hex: crate::attrib::node_map(
                                        tab.array.as_array().unwrap_or(&Vec::new()),
                                    ),
                                    lifecycle: Arc::new(tokio::sync::Mutex::new(())),
                                },
                            );
                            report.changed += 1;
                            on_outcome(FlowOutcome::Started {
                                flow_id: fid,
                                tab_id: tab.tab_id,
                                rev: hash,
                            });
                        }
                        Err(e) => {
                            // Built but not startable: flow down until the
                            // next modified reload (rare case — build OK).
                            let msg = format!("start: {e}");
                            self.failed.insert(fid, (hash.clone(), msg.clone()));
                            on_outcome(FlowOutcome::Failed {
                                flow_id: fid,
                                tab_id: tab.tab_id,
                                error: msg,
                            });
                        }
                    }
                }
                Err(e) => {
                    // Build failure: the last-good (if any) keeps
                    // running with the old graph — ingestion preserved.
                    self.failed.insert(fid, (hash.clone(), e.clone()));
                    on_outcome(FlowOutcome::Failed {
                        flow_id: fid,
                        tab_id: tab.tab_id,
                        error: e,
                    });
                    // changed unchanged: nothing was applied for this flow.
                }
            }
        }

        // Removals: flows missing from the new artifact. Each stop emits its
        // `flow_stopped` as soon as it is done — a slow stop (node in the middle
        // of a bounded network read) does not hold up the following announcements.
        let stale: Vec<i64> = self
            .engines
            .keys()
            .copied()
            .chain(self.failed.keys().copied())
            .filter(|fid| !new_ids.contains(fid))
            .collect();
        for fid in stale {
            if let Some(old) = self.engines.remove(&fid) {
                let _guard = old.lifecycle.lock().await;
                if let Err(e) = old.engine.stop().await {
                    log::warn!("flow {fid}: stop on removal: {e}");
                }
                on_outcome(FlowOutcome::Stopped {
                    flow_id: fid,
                    tab_id: old.tab_id,
                });
            } else if self.failed.remove(&fid).is_some() {
                // Failed without engine: nothing to stop, just leave the state.
                on_outcome(FlowOutcome::Stopped {
                    flow_id: fid,
                    tab_id: String::new(),
                });
            }
        }

        report
    }

    /// Iteration over the live engines (debug pumps to (re)start).
    pub fn engines(&self) -> impl Iterator<Item = &FlowEngine> {
        self.engines.values()
    }

    /// Clean stop of all engines (stable order by flow_id).
    pub async fn stop_all(&self) {
        let mut ids: Vec<i64> = self.engines.keys().copied().collect();
        ids.sort_unstable();
        for fid in ids {
            if let Some(fe) = self.engines.get(&fid) {
                let _guard = fe.lifecycle.lock().await;
                if let Err(e) = fe.engine.stop().await {
                    log::warn!("flow {fid}: stop: {e}");
                }
            }
        }
    }

    async fn build_and_start(&self, tab: &TabInput) -> Result<FlowEngine, String> {
        let engine = build_engine(&self.reg, tab)?;
        engine.start().await.map_err(|e| format!("start: {e}"))?;
        Ok(FlowEngine {
            flow_id: tab.flow_id,
            tab_id: tab.tab_id.clone(),
            engine,
            tab_hash: tab.hash.clone(),
            node_by_hex: crate::attrib::node_map(tab.array.as_array().unwrap_or(&Vec::new())),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

/// Reads + parses the artifact into tabs (**file** failures: read, JSON,
/// non-array root). Also returns the SHA-256 fingerprint of the raw file —
/// the `flow_rev` of runtime.json (process health, never a version).
pub async fn read_tabs(flows_path: &str) -> Result<(Vec<TabInput>, String), String> {
    let raw = tokio::fs::read_to_string(flows_path)
        .await
        .map_err(|e| format!("read: {e}"))?;
    let rev = sha256_hex(raw.as_bytes());
    let value: Value = serde_json::from_str(&raw).map_err(|e| format!("JSON : {e}"))?;
    let tabs = parse_tabs(&value)?;
    Ok((tabs, rev))
}

/// Groups the artifact into tabs — replicates the classification of the vendor
/// deserialiser (`deser.rs`): `type == "tab"` → tab; entries with `z` → nodes/
/// groups of the tab; `comment` ignored; entries without `z` (subflow templates,
/// global config nodes) → **warn + ignored** (never produced by the PNEX
/// projection — an orphan subflow instance will fail its tab, which
/// is the intended isolation). Tabs without a resolvable `pnex_flow_id` are
/// ignored (warn) — the projection always stamps them.
pub fn parse_tabs(root: &Value) -> Result<Vec<TabInput>, String> {
    let entries = root
        .as_array()
        .ok_or_else(|| "the artifact must be an array".to_string())?;

    // Tabs in file order + entry buffers per z (a tab may
    // appear AFTER its nodes — unconditional buffering per z).
    let mut order: Vec<String> = Vec::new();
    let mut tab_entries: HashMap<String, Value> = HashMap::new();
    let mut by_z: HashMap<String, Vec<Value>> = HashMap::new();
    for e in entries {
        let Some(obj) = e.as_object() else {
            return Err("each entry must be an object".to_string());
        };
        let id = obj.get("id").and_then(|v| v.as_str());
        let type_ = obj.get("type").and_then(|v| v.as_str());
        match (id, type_) {
            (Some(id), Some("tab")) => {
                if tab_entries.contains_key(id) {
                    return Err(format!("duplicate tab: {id}"));
                }
                order.push(id.to_string());
                tab_entries.insert(id.to_string(), e.clone());
            }
            (Some(_), Some("comment")) => {} // ignored by the engine too
            (Some(_), Some("subflow")) => {
                log::warn!("subflow template ignored (not supported in the farm): {id:?}");
            }
            (Some(_), Some(t)) if t.starts_with("subflow:") => {
                log::warn!("subflow node ignored: template missing in the farm");
            }
            (Some(_), _) => match obj.get("z").and_then(|v| v.as_str()) {
                Some(z) => by_z.entry(z.to_string()).or_default().push(e.clone()),
                None => log::warn!("global config node ignored: {id:?}"),
            },
            // Entries without id or type: the engine skips them too.
            (None, _) => {}
        }
    }

    let mut tabs = Vec::new();
    for id in order {
        let Some(tab_entry) = tab_entries.get(&id) else {
            continue;
        };
        let flow_id = tab_entry
            .get("pnex_flow_id")
            .and_then(|v| v.as_i64())
            .or_else(|| id.strip_prefix("pnexflow").and_then(|s| s.parse().ok()));
        let Some(flow_id) = flow_id else {
            log::warn!("tab without resolvable pnex_flow_id, ignored: {id}");
            continue;
        };
        let mut array = vec![tab_entry.clone()];
        if let Some(buf) = by_z.remove(&id) {
            array.extend(buf);
        }
        let hash = sha256_hex(
            serde_json::to_string(&array)
                .expect("serialisable")
                .as_bytes(),
        );
        tabs.push(TabInput {
            flow_id,
            tab_id: id,
            array: Value::Array(array),
            hash,
        });
    }
    Ok(tabs)
}

/// Builds a tab's engine **without starting it** — shared by the farm
/// (boot/reload) and the `--check` mode (deploy pre-flight).
pub(crate) fn build_engine(reg: &RegistryHandle, tab: &TabInput) -> Result<Engine, String> {
    Engine::with_json(reg, tab.array.clone(), None).map_err(|e| e.to_string())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn two_tab_artifact() -> Value {
        json!([
            { "id": "pnexflow1", "type": "tab", "label": "f1", "pnex_flow_id": 1 },
            { "id": "pnexflow1_n1", "z": "pnexflow1", "type": "inject", "payload": "1" },
            { "id": "pnexflow2", "type": "tab", "label": "f2", "pnex_flow_id": 2 },
            { "id": "pnexflow2_n1", "z": "pnexflow2", "type": "debug" },
            { "id": "global_conf", "type": "mqtt-broker", "name": "b" },
            { "type": "malformed" }
        ])
    }

    #[test]
    fn parse_tabs_groups_by_z_and_ignores_orphans() {
        let tabs = parse_tabs(&two_tab_artifact()).expect("tabs");
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].flow_id, 1);
        assert_eq!(tabs[0].tab_id, "pnexflow1");
        // tab first + its node.
        let arr = tabs[0].array.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "tab");
        assert_eq!(arr[1]["id"], "pnexflow1_n1");
        // deterministic hash.
        assert_eq!(tabs[0].hash, tabs[0].hash);
        assert_ne!(tabs[0].hash, tabs[1].hash);
    }

    #[test]
    fn parse_tabs_ignores_tabs_without_flow_id() {
        let artifact = json!([
            { "id": "pnexflow1", "type": "tab", "pnex_flow_id": 1 },
            { "id": "sans-id", "type": "tab", "label": "étranger" },
        ]);
        let tabs = parse_tabs(&artifact).expect("tabs");
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].flow_id, 1);
    }

    #[test]
    fn non_array_root_is_a_file_error() {
        assert!(parse_tabs(&json!({})).is_err());
    }

    #[test]
    fn per_tab_hash_changes_with_the_content() {
        let a = parse_tabs(&two_tab_artifact()).unwrap();
        let mut b_artifact = two_tab_artifact();
        b_artifact[1]["payload"] = json!("autre");
        let b = parse_tabs(&b_artifact).unwrap();
        assert_ne!(a[0].hash, b[0].hash);
        assert_eq!(a[1].hash, b[1].hash);
    }
}
