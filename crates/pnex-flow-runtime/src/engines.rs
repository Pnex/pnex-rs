//! La ferme d'engines : **un `Engine` edgelinkd par flow déployé** — 1 tab
//! de l'artefact = 1 engine dans le même process.
//!
//! Pourquoi une ferme : le vendor charge son tableau en tout-ou-rien
//! (`Flow::new(..)?` court-circuite la boucle de chargement) et
//! `redeploy_flows` **vide ses maps avant** de recharger — un tab invalide
//! rejetait tout l'artefact → exit(1) → crash-loop du superviseur, tous les
//! flows down. La ferme isole : un tab invalide ne fait échouer que son
//! engine (`flow_error` sur stdout), les autres continuent ; le reload d'un
//! tab n'arrête pas les autres (plus de pause d'ingestion globale).
//!
//! Contrat stdout ajouté : `flow_started {flow, rev}`, `flow_error {flow,
//! error}` — l'acquittement du deploy passe par ces événements par flow
//! (le superviseur backend y souscrit ; runtime.json reste la santé process
//! uniquement). `--check` réutilise `build_engine` sans `start()` pour le
//! pré-flight du deploy.
//!
//! Delta sémantique (accepté, cf. flow-engine.md §5) : context « global »,
//! link call et registre http-response deviennent **intra-flow** — chaque
//! engine est autonome (ContextManager, registres, canaux, CancellationToken
//! propres), zéro état global mutable dans le vendor.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgelink_core::runtime::engine::Engine;
use edgelink_core::runtime::registry::RegistryHandle;
use serde_json::Value;

/// Un tab de l'artefact isolé avec ses nœuds — l'unité de déploiement.
pub struct TabInput {
    /// `pnex_flow_id` du tab (fallback : préfixe `pnexflow{id}`) — clé de la
    /// ferme. Les tabs sans identifiant résolvable sont ignorés (warn).
    pub flow_id: i64,
    pub tab_id: String,
    /// Tableau Node-RED du tab (tab en tête, puis entrées `z`).
    pub array: Value,
    /// SHA-256 hex du tableau canonique — l'unité de diff du reload.
    pub hash: String,
}

/// Issue d'un cycle boot/reload pour un flow.
pub enum FlowOutcome {
    /// Engine chargé + démarré — ou inchangé (confirmation idempotente, elle
    /// résout un acquittement deploy en attente sans toucher l'engine).
    Started {
        flow_id: i64,
        tab_id: String,
        rev: String,
    },
    /// Build (ou démarrage) en échec. Si un last-good tournait, il continue
    /// (ingestion préservée) — le chip backend montrera « error ».
    Failed {
        flow_id: i64,
        tab_id: String,
        error: String,
    },
    /// Tab disparu de l'artefact : engine arrêté et retiré.
    Stopped { flow_id: i64, tab_id: String },
}

/// Rapport d'un cycle de reload. Les issues **par flow** sont émises à la
/// volée via le callback passé à [`EngineFarm::reload`] (un tab lent ne doit
/// pas retarder l'acquittement deploy des autres) — le rapport ne porte plus
/// que les agrégats.
pub struct ReloadReport {
    /// Nombre de changements appliqués (swaps + retraits).
    pub changed: u32,
    /// Flows retirés (pompes debug à aborter côté main).
    pub removed: Vec<i64>,
}

/// Un engine + les métadonnées nécessaires au runtime (debug, diff de
/// reload). Mutations sérialisées par la boucle signaux ; `lifecycle`
/// protège `start`/`stop` (le vendor utilise `try_write` — un accès concurrent
/// retournerait « already started »/« not started »).
pub struct FlowEngine {
    pub flow_id: i64,
    pub tab_id: String,
    pub engine: Engine,
    pub tab_hash: String,
    /// hex(`ElementId` nœud) → id éditeur, pour estampiller le debug.
    pub node_by_hex: HashMap<String, String>,
    pub lifecycle: Arc<tokio::sync::Mutex<()>>,
}

/// La ferme : `engines` = flows vivants (running ou last-good), `failed` =
/// dernier échec de build par flow (hash fautif + erreur). Un tab inchangé
/// n'est pas retenté à chaque SIGUSR1 : retry = tab modifié ou redémarrage
/// du process.
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

    /// Boot : construit + démarre chaque engine indépendamment — l'échec
    /// d'un tab n'affecte pas les suivants. Chaque issue est émise **dès
    /// qu'elle survient** via `on_outcome` (stdout → acquittement deploy) :
    /// un tab lent au boot ne doit pas retarder les annonces des suivants.
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

    /// Cycle de reload : diff par `tab_hash`, swap limité au flow concerné,
    /// last-good préservé en cas d'échec de build. Les issues sont émises
    /// **à la volée** via `on_outcome` — le `flow_started` d'un tab dont le
    /// build+start vient de réussir part **avant** les arrêts de retraits
    /// (potentiellement lents) qui suivent : l'acquittement deploy ne peut
    /// plus perdre la course contre un cycle au complet.
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

            // 1) Inchangé (running ou last-good avec hash courant) :
            //    confirmation idempotente, l'engine n'est pas touché.
            if self.engines.get(&fid).is_some_and(|fe| fe.tab_hash == hash) {
                on_outcome(FlowOutcome::Started {
                    flow_id: fid,
                    tab_id: tab.tab_id,
                    rev: hash,
                });
                continue;
            }
            // 2) Déjà en échec pour CE hash : pas de retry sur tab inchangé
            //    (retry = tab modifié ou redémarrage du process).
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

            // 3) Build du nouvel engine AVANT de toucher l'ancien
            //    (last-good préservé en cas d'échec de build).
            match build_engine(&self.reg, &tab) {
                Ok(engine) => {
                    // Stop l'ancien AVANT le start du nouveau : deux engines
                    // simultanés sur le même graphe doubleraient les injects
                    // (timers propres à chaque engine). Coupure limitée à ce
                    // flow (même sémantique que l'ancien redeploy_flows).
                    if let Some(old) = self.engines.remove(&fid) {
                        let _guard = old.lifecycle.lock().await;
                        if let Err(e) = old.engine.stop().await {
                            log::warn!("flow {fid} : arrêt du last-good : {e}");
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
                            // Construit mais pas démarrable : flow down jusqu'au
                            // prochain reload modifié (cas rare — build OK).
                            let msg = format!("démarrage : {e}");
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
                    // Échec de build : le last-good (s'il existe) continue de
                    // tourner avec l'ancien graphe — ingestion préservée.
                    self.failed.insert(fid, (hash.clone(), e.clone()));
                    on_outcome(FlowOutcome::Failed {
                        flow_id: fid,
                        tab_id: tab.tab_id,
                        error: e,
                    });
                    // changed inchangé : rien n'a été appliqué pour ce flow.
                }
            }
        }

        // Retraits : flows absents du nouvel artefact. Chaque arrêt émet son
        // `flow_stopped` dès qu'il est fait — un arrêt lent (node en pleine
        // lecture réseau bornée) n'immobilise pas les annonces des suivants.
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
                    log::warn!("flow {fid} : arrêt au retrait : {e}");
                }
                on_outcome(FlowOutcome::Stopped {
                    flow_id: fid,
                    tab_id: old.tab_id,
                });
            } else if self.failed.remove(&fid).is_some() {
                // En échec sans engine : rien à arrêter, juste sortir de l'état.
                on_outcome(FlowOutcome::Stopped {
                    flow_id: fid,
                    tab_id: String::new(),
                });
            }
        }

        report
    }

    /// Itération sur les engines vivants (pompes debug à (re)lancer).
    pub fn engines(&self) -> impl Iterator<Item = &FlowEngine> {
        self.engines.values()
    }

    /// Arrêt propre de tous les engines (ordre stable par flow_id).
    pub async fn stop_all(&self) {
        let mut ids: Vec<i64> = self.engines.keys().copied().collect();
        ids.sort_unstable();
        for fid in ids {
            if let Some(fe) = self.engines.get(&fid) {
                let _guard = fe.lifecycle.lock().await;
                if let Err(e) = fe.engine.stop().await {
                    log::warn!("flow {fid} : arrêt : {e}");
                }
            }
        }
    }

    async fn build_and_start(&self, tab: &TabInput) -> Result<FlowEngine, String> {
        let engine = build_engine(&self.reg, tab)?;
        engine
            .start()
            .await
            .map_err(|e| format!("démarrage : {e}"))?;
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

/// Lit + parse l'artefact en tabs (échecs **fichier** : lecture, JSON,
/// racine non-tableau). Retourne aussi l'empreinte SHA-256 du fichier brut —
/// le `flow_rev` du runtime.json (santé process, jamais une version).
pub async fn read_tabs(flows_path: &str) -> Result<(Vec<TabInput>, String), String> {
    let raw = tokio::fs::read_to_string(flows_path)
        .await
        .map_err(|e| format!("lecture : {e}"))?;
    let rev = sha256_hex(raw.as_bytes());
    let value: Value = serde_json::from_str(&raw).map_err(|e| format!("JSON : {e}"))?;
    let tabs = parse_tabs(&value)?;
    Ok((tabs, rev))
}

/// Regroupe l'artefact en tabs — réplique le classement du désérialiseur
/// vendor (`deser.rs`) : `type == "tab"` → tab ; entrées avec `z` → nœuds/
/// groupes du tab ; `comment` ignorés ; entrées sans `z` (subflow templates,
/// global config nodes) → **warn + ignorées** (jamais produites par la
/// projection PNEX — un subflow instance orphelin fera échouer son tab, ce
/// qui est l'isolation voulue). Les tabs sans `pnex_flow_id` résolvable sont
/// ignorés (warn) — la projection les estampille toujours.
pub fn parse_tabs(root: &Value) -> Result<Vec<TabInput>, String> {
    let entries = root
        .as_array()
        .ok_or_else(|| "l'artefact doit être un tableau".to_string())?;

    // Tabs dans l'ordre du fichier + buffers d'entrées par z (un tab peut
    // apparaître APRÈS ses nœuds — bufferisation inconditionnelle par z).
    let mut order: Vec<String> = Vec::new();
    let mut tab_entries: HashMap<String, Value> = HashMap::new();
    let mut by_z: HashMap<String, Vec<Value>> = HashMap::new();
    for e in entries {
        let Some(obj) = e.as_object() else {
            return Err("chaque entrée doit être un objet".to_string());
        };
        let id = obj.get("id").and_then(|v| v.as_str());
        let type_ = obj.get("type").and_then(|v| v.as_str());
        match (id, type_) {
            (Some(id), Some("tab")) => {
                if tab_entries.contains_key(id) {
                    return Err(format!("tab dupliqué : {id}"));
                }
                order.push(id.to_string());
                tab_entries.insert(id.to_string(), e.clone());
            }
            (Some(_), Some("comment")) => {} // ignoré par le moteur aussi
            (Some(_), Some("subflow")) => {
                log::warn!("subflow template ignoré (non supporté en ferme) : {id:?}");
            }
            (Some(_), Some(t)) if t.starts_with("subflow:") => {
                log::warn!("nœud subflow ignoré : template absent en ferme");
            }
            (Some(_), _) => match obj.get("z").and_then(|v| v.as_str()) {
                Some(z) => by_z.entry(z.to_string()).or_default().push(e.clone()),
                None => log::warn!("global config node ignoré : {id:?}"),
            },
            // Entrées sans id ou sans type : le moteur les saute aussi.
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
            log::warn!("tab sans pnex_flow_id résolvable, ignoré : {id}");
            continue;
        };
        let mut array = vec![tab_entry.clone()];
        if let Some(buf) = by_z.remove(&id) {
            array.extend(buf);
        }
        let hash = sha256_hex(
            serde_json::to_string(&array)
                .expect("sérialisable")
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

/// Construit l'engine d'un tab **sans le démarrer** — partagé par la ferme
/// (boot/reload) et le mode `--check` (pré-flight du deploy).
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
    fn parse_tabs_groupe_par_z_et_ignore_les_orphelins() {
        let tabs = parse_tabs(&two_tab_artifact()).expect("tabs");
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].flow_id, 1);
        assert_eq!(tabs[0].tab_id, "pnexflow1");
        // tab en tête + son nœud.
        let arr = tabs[0].array.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "tab");
        assert_eq!(arr[1]["id"], "pnexflow1_n1");
        // hash déterministe.
        assert_eq!(tabs[0].hash, tabs[0].hash);
        assert_ne!(tabs[0].hash, tabs[1].hash);
    }

    #[test]
    fn parse_tabs_ignore_les_tabs_sans_flow_id() {
        let artifact = json!([
            { "id": "pnexflow1", "type": "tab", "pnex_flow_id": 1 },
            { "id": "sans-id", "type": "tab", "label": "étranger" },
        ]);
        let tabs = parse_tabs(&artifact).expect("tabs");
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].flow_id, 1);
    }

    #[test]
    fn racine_non_tableau_erreur_fichier() {
        assert!(parse_tabs(&json!({})).is_err());
    }

    #[test]
    fn hash_par_tab_change_avec_le_contenu() {
        let a = parse_tabs(&two_tab_artifact()).unwrap();
        let mut b_artifact = two_tab_artifact();
        b_artifact[1]["payload"] = json!("autre");
        let b = parse_tabs(&b_artifact).unwrap();
        assert_ne!(a[0].hash, b[0].hash);
        assert_eq!(a[1].hash, b[1].hash);
    }
}
