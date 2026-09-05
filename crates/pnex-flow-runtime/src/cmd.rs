//! Commandes backend → runtime : `<home>/cmd.json` + SIGUSR2.
//!
//! Contrat (cf. docs/architecture/flow-engine.md §0ter) :
//! - le superviseur écrit `cmd.json` atomiquement (tmp+rename) **avant** le
//!   signal et le purge au spawn de l'enfant (jamais de rejeu d'avant-crash) ;
//! - le runtime lit, **ignore `seq <= last_seq`** (idempotence), supprime le
//!   fichier puis exécute ;
//! - ack stdout corrélé par seq : `run_once_done` / `run_once_failed`.
//!
//! Le nœud inject builtin ne peut pas être déclenché de l'extérieur
//! (`InjectNode` est privé dans le vendor — jamais patché — et son `run()`
//! ne consomme pas son canal d'entrée) : le run-once **relit les wires du
//! flows.json**, reconstruit le msg depuis la config RED de l'inject (même
//! normalisation legacy que le nœud builtin, évaluation via la fonction
//! publique `evaluate_raw_node_property`), puis injecte dans les cibles via
//! `Engine::inject_msg` (timeout par cible).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::engine::Engine;
use edgelink_core::runtime::eval::evaluate_raw_node_property;
use edgelink_core::runtime::model::json::deser::parse_red_id_str;
use edgelink_core::runtime::model::{Msg, MsgHandle, RedPropertyType, Variant};

/// Issue d'une commande run-once (l'appelant émet l'ack stdout).
pub struct RunOnceOutcome {
    pub seq: u64,
    pub flow: String,
    pub nodes: u32,
    pub injected: u32,
    pub error: Option<&'static str>,
}

/// Traite la commande courante (`<home>/cmd.json`, posée avant SIGUSR2).
/// Retourne `None` si la commande est une rejeu ignorée (aucun ack émis).
pub async fn handle_cmd(
    engine: &Engine,
    home: &std::path::Path,
    flows_path: &str,
    last_seq: &mut u64,
) -> Option<RunOnceOutcome> {
    let cmd_path = home.join("cmd.json");
    let Ok(raw) = tokio::fs::read_to_string(&cmd_path).await else {
        return Some(RunOnceOutcome {
            seq: 0,
            flow: String::new(),
            nodes: 0,
            injected: 0,
            error: Some("cmd_illisible"),
        });
    };
    let cmd: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            let _ = tokio::fs::remove_file(&cmd_path).await;
            return Some(RunOnceOutcome {
                seq: 0,
                flow: String::new(),
                nodes: 0,
                injected: 0,
                error: Some("cmd_illisible"),
            });
        }
    };
    let seq = cmd.get("seq").and_then(|s| s.as_u64()).unwrap_or(0);
    let cmd_flow = cmd.get("flow").and_then(|f| f.as_str()).unwrap_or_default().to_string();

    // Rejeu : une seq déjà exécutée ne produit AUCUN ack (idempotence).
    if seq <= *last_seq {
        return None;
    }
    *last_seq = seq;
    let _ = tokio::fs::remove_file(&cmd_path).await;

    // L'artefact est relu à chaque commande : la version en exécution est
    // celle que le moteur vient de charger (post-redeploy).
    let json = match tokio::fs::read_to_string(flows_path).await {
        Ok(j) => j,
        Err(_) => return Some(RunOnceOutcome::absent(seq, cmd_flow)),
    };
    let Ok(artefact) = serde_json::from_str::<Value>(&json) else {
        return Some(RunOnceOutcome::absent(seq, cmd_flow));
    };
    let Some(entries) = artefact.as_array() else {
        return Some(RunOnceOutcome::absent(seq, cmd_flow));
    };

    // Tab absent = flow non déployé dans l'artefact courant.
    let tab_present = entries.iter().any(|e| {
        e.get("type").and_then(|t| t.as_str()) == Some("tab")
            && e.get("id").and_then(|i| i.as_str()) == Some(cmd_flow.as_str())
    });
    if !tab_present {
        return Some(RunOnceOutcome::absent(seq, cmd_flow));
    }

    // Les injects du tab — `nodes` compte même sans cible câblée.
    let injects: Vec<&Value> = entries
        .iter()
        .filter(|e| {
            e.get("z").and_then(|z| z.as_str()) == Some(cmd_flow.as_str())
                && e.get("type").and_then(|t| t.as_str()) == Some("inject")
        })
        .collect();

    let mut injected = 0u32;
    for entry in &injects {
        let msg = match build_msg_from_red(entry).await {
            Ok(m) => m,
            Err(e) => {
                log::warn!(
                    "run-once : inject {} ignoré : {e}",
                    entry.get("id").and_then(|i| i.as_str()).unwrap_or("?")
                );
                continue;
            }
        };
        injected += fire_node(engine, entry, &msg).await;
    }

    Some(RunOnceOutcome { seq, flow: cmd_flow, nodes: injects.len() as u32, injected, error: None })
}

impl RunOnceOutcome {
    fn absent(seq: u64, flow: String) -> Self {
        Self { seq, flow, nodes: 0, injected: 0, error: Some("flow_absent") }
    }
}

/// Un nœud inject RED → ses cibles (tous ports) : chaque cible reçoit un
/// clone profond (`MsgHandle` = Arc mutable — le moteur fait pareil au
/// fan-out). Compte les injections réussies ; cible en échec/timeout 5 s =
/// sautée (les autres cibles poursuivent).
async fn fire_node(engine: &Engine, entry: &Value, msg: &MsgHandle) -> u32 {
    let cancel = CancellationToken::new();
    let mut injected = 0u32;
    let empty = Vec::new();
    let ports = entry.get("wires").and_then(|w| w.as_array()).unwrap_or(&empty);
    for port in ports {
        for target in port.as_array().unwrap_or(&empty) {
            let Some(id_str) = target.as_str() else { continue };
            let Some(eid) = parse_red_id_str(id_str) else {
                log::warn!("run-once : id de cible non résolu : {id_str}");
                continue;
            };
            let m = if injected == 0 { msg.clone() } else { msg.deep_clone(true).await };
            match tokio::time::timeout(
                Duration::from_millis(5_000),
                engine.inject_msg(&eid, m, cancel.child_token()),
            )
            .await
            {
                Ok(Ok(())) => injected += 1,
                Ok(Err(e)) => log::warn!("run-once : cible {id_str} en échec : {e}"),
                Err(_) => log::warn!("run-once : cible {id_str} interrompue (timeout 5 s)"),
            }
        }
    }
    injected
}

/// Reconstruit le msg depuis la config RED d'un inject — **même normalisation
/// legacy que le nœud builtin** (`props` absentes = payload/topic racine ;
/// `p=payload` sans `v` = racine) puis évaluation via la fonction publique du
/// moteur (mêmes sémantiques `vt` : str/num/json/date…). Échec d'évaluation =
/// erreur (l'inject est sauté, les autres poursuivent).
async fn build_msg_from_red(entry: &Value) -> Result<MsgHandle, String> {
    let props: Vec<Value> = match entry.get("props").and_then(|p| p.as_array()) {
        Some(props) => props
            .iter()
            .map(|prop| {
                let mut prop = prop.clone();
                let p = prop.get("p").and_then(|x| x.as_str()).unwrap_or_default();
                if p == "payload" && prop.get("v").is_none() {
                    prop["v"] = entry.get("payload").cloned().unwrap_or(Value::Null);
                    prop["vt"] =
                        entry.get("payloadType").cloned().unwrap_or(Value::String("str".into()));
                } else if p == "topic" && prop.get("v").is_none()
                    && prop.get("vt") == Some(&Value::String("str".into()))
                {
                    prop["v"] = entry.get("topic").cloned().unwrap_or(Value::Null);
                }
                prop
            })
            .collect(),
        None => {
            // Props absentes : synthèse depuis les clés racine (payload avec
            // son `payloadType`, topic en `str`).
            vec![
                serde_json::json!({
                    "p": "payload",
                    "v": entry.get("payload").cloned().unwrap_or(Value::Null),
                    "vt": entry.get("payloadType").cloned().unwrap_or(Value::String("str".into()))
                }),
                serde_json::json!({
                    "p": "topic",
                    "v": entry.get("topic").cloned().unwrap_or(Value::Null),
                    "vt": "str"
                }),
            ]
        }
    };

    let mut body: BTreeMap<String, Variant> = BTreeMap::new();
    for prop in &props {
        let Some(p) = prop.get("p").and_then(|x| x.as_str()) else { continue };
        let Some(v) = prop.get("v").and_then(|x| x.as_str()) else { continue };
        let vt = prop
            .get("vt")
            .map(|x| serde_json::from_value::<RedPropertyType>(x.clone()).unwrap_or_default())
            .unwrap_or_default();
        let value = match evaluate_raw_node_property(v, vt, None, None, None).await {
            Ok(v) => v,
            Err(e) => return Err(format!("évaluation de « {p} » : {e}")),
        };
        body.insert(p.to_string(), value);
    }
    body.insert(
        edgelink_core::runtime::model::wellknown::MSG_ID_PROPERTY.to_string(),
        Variant::String(Msg::generate_id().to_string()),
    );
    Ok(MsgHandle::with_properties(body))
}
