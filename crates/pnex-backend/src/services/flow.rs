//! Réglages du moteur de flow ETL (D18) — `settings.flow` de la config Loco,
//! champ par champ avec défauts (pattern `FirmwareSettings`).
//!
//! Le runtime (`pnex-flow-runtime`) est un **process enfant supervisé** (mode
//! B) : le backend ne lie jamais les crates EdgeLinkd — isolation du stade
//! alpha. `enabled` (défaut `false`) coupe tout : sans lui ni process ni
//! déploiement (les flows restent éditables en base).

use crate::services::openobserve::OpenobserveSettings;
use loco_rs::config::Config;
use serde::Deserialize;

/// Réglages résolus du superviseur de flows.
///
/// Debug manuel : l'allowlist d'env ne sont que des *noms* de variables
/// (les valeurs ne transitent jamais ici).
#[derive(Clone)]
pub struct FlowSettings {
    /// Supervision + déploiement actifs (défaut : non).
    pub enabled: bool,
    /// Commande du runtime (résolue comme `resolve_program` du firmware :
    /// chemin relatif → cwd → racine du monorepo).
    pub runtime_cmd: String,
    /// Répertoire d'état (flows.json + runtime.json).
    pub state_dir: String,
    /// Backoff initial de relance après crash (exponentiel, borné).
    pub restart_backoff_secs: u64,
    /// Backoff maximal de relance.
    pub restart_backoff_max_secs: u64,
    /// Délai SIGTERM → SIGKILL à l'arrêt du runtime.
    pub terminate_secs: u64,
    /// Délai d'attente de l'acquittement de rechargement (runtime.json).
    pub reload_ack_secs: u64,
    /// Délai du pré-flight deploy (`--check` du runtime sur le candidat).
    pub check_timeout_secs: u64,
    /// Variables d'environnement autorisées à franchir la frontière vers le
    /// runtime (names only — e.g. `VALKEY_URL`; the runtime never gets
    /// `DATABASE_URL`).
    pub env_allowlist: Vec<String>,
    /// Outils de debug (panneau Debug) actifs — **mode dev/debug uniquement** :
    /// défaut `false`, activé par la config de dev ; en mode run l'endpoint
    /// répond 403 et l'éditeur masque le bouton.
    pub debug_tools: bool,
    /// Credentials OpenObserve résolus depuis `settings.openobserve` du yaml
    /// — **injectés** dans l'env enfant (OPENOBSERVE_URL/_ROOT_EMAIL/
    /// _ROOT_PASSWORD) au-delà de l'allowlist : le serveur tient ces creds du
    /// yaml, pas de son env process (qui ne les a jamais — retour e2e
    /// 2026-09-04 : le nœud device échouait au build et le moteur
    /// crash-loopait sans acquittement). Même domaine de confiance que
    /// `DATABASE_URL`. `None` = O2 non configuré (tests).
    pub o2: Option<OpenobserveSettings>,
    /// URL de la Valkey du cache live dernier-point (`settings.valkey.url`)
    /// — injectée dans l'env enfant (`VALKEY_URL`) au-delà de l'allowlist,
    /// miroir de l'injection O2 : le serveur tient l'URL du yaml, pas de son
    /// env process. `None` = cache désactivé (le nœud `device-read` retombe
    /// sur le chemin OpenObserve).
    pub valkey_url: Option<String>,
    /// Coordonnées de livraison notify (D50/D54) — `Some((deliver_url,
    /// token))` injectées dans l'env enfant (`PNEX_NOTIFY_DELIVER_URL` /
    /// `_TOKEN`) quand `settings.notifications.internal_token` est posé ;
    /// `None` = canal websocket indisponible pour les nœuds flow (env
    /// absente → `BadFlowsJson` au build du nœud, explicitement).
    pub notify_deliver: Option<(String, String)>,
    /// Coordonnées d'écriture device (`pnex-device-write`) — `Some((url,
    /// token))` injectées dans l'env enfant (`PNEX_FLOW_WRITE_URL` /
    /// `_TOKEN`) quand `settings.flow.runtime_token` est posé (env
    /// `PNEX_FLOW_RUNTIME_TOKEN` prioritaire) ; `None` = le nœud
    /// `device-write` échoue fort au build (BadFlowsJson, école notify).
    pub device_write: Option<(String, String)>,
    /// Fencing identity of the worker running this runtime (`<id>:<boot>`,
    /// D106) — injected as `PNEX_FLOW_WORKER_FENCE`; the runtime stamps it on
    /// every internal backend call so writes of a superseded worker are
    /// rejected. Set by the flow cluster, never by the yaml.
    pub worker_fence: Option<String>,
}

impl std::fmt::Debug for FlowSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowSettings")
            .field("enabled", &self.enabled)
            .field("runtime_cmd", &self.runtime_cmd)
            .field("state_dir", &self.state_dir)
            .field("restart_backoff_secs", &self.restart_backoff_secs)
            .field("restart_backoff_max_secs", &self.restart_backoff_max_secs)
            .field("terminate_secs", &self.terminate_secs)
            .field("reload_ack_secs", &self.reload_ack_secs)
            .field("check_timeout_secs", &self.check_timeout_secs)
            .field("env_allowlist", &self.env_allowlist)
            .field("debug_tools", &self.debug_tools)
            .field("o2_configured", &self.o2.is_some())
            .field("valkey_configured", &self.valkey_url.is_some())
            .finish()
    }
}

impl Default for FlowSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            runtime_cmd: "pnex-flow-runtime".into(),
            state_dir: "./flow-state".into(),
            restart_backoff_secs: 5,
            restart_backoff_max_secs: 60,
            terminate_secs: 10,
            reload_ack_secs: 10,
            check_timeout_secs: 10,
            env_allowlist: vec![
                "OPENOBSERVE_URL".into(),
                "OPENOBSERVE_ROOT_EMAIL".into(),
                "OPENOBSERVE_ROOT_PASSWORD".into(),
                "VALKEY_URL".into(),
            ],
            debug_tools: false,
            o2: None,
            valkey_url: None,
            notify_deliver: None,
            device_write: None,
            worker_fence: None,
        }
    }
}

/// Forme sérialisable partielle de `settings.flow` (tout optionnel).
#[derive(Default, Deserialize)]
struct FlowPartial {
    enabled: Option<bool>,
    runtime_cmd: Option<String>,
    state_dir: Option<String>,
    restart_backoff_secs: Option<u64>,
    restart_backoff_max_secs: Option<u64>,
    terminate_secs: Option<u64>,
    reload_ack_secs: Option<u64>,
    check_timeout_secs: Option<u64>,
    env_allowlist: Option<Vec<String>>,
    debug_tools: Option<bool>,
    /// Token interne des nœuds `device-write` (env `PNEX_FLOW_RUNTIME_TOKEN`
    /// prioritaire sur le yaml).
    runtime_token: Option<String>,
}

impl FlowSettings {
    /// `settings.flow` optionnelle — défauts champ par champ.
    pub fn from_config(config: &Config) -> Self {
        let partial: FlowPartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("flow"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let d = Self::default();
        Self {
            enabled: partial.enabled.unwrap_or(d.enabled),
            runtime_cmd: partial.runtime_cmd.unwrap_or(d.runtime_cmd),
            state_dir: partial.state_dir.unwrap_or(d.state_dir),
            restart_backoff_secs: partial
                .restart_backoff_secs
                .unwrap_or(d.restart_backoff_secs),
            restart_backoff_max_secs: partial
                .restart_backoff_max_secs
                .unwrap_or(d.restart_backoff_max_secs),
            terminate_secs: partial.terminate_secs.unwrap_or(d.terminate_secs),
            reload_ack_secs: partial.reload_ack_secs.unwrap_or(d.reload_ack_secs),
            check_timeout_secs: partial.check_timeout_secs.unwrap_or(d.check_timeout_secs),
            env_allowlist: partial.env_allowlist.unwrap_or(d.env_allowlist),
            debug_tools: partial.debug_tools.unwrap_or(d.debug_tools),
            o2: OpenobserveSettings::from_config(config),
            valkey_url: crate::services::last_cache::ValkeySettings::from_config(config)
                .and_then(|s| s.url),
            notify_deliver: {
                let n = crate::services::settings::NotifySettings::from_config(config);
                n.internal_token.map(|t| (n.deliver_url, t))
            },
            device_write: {
                let mut token = partial.runtime_token;
                if let Ok(env) = std::env::var("PNEX_FLOW_RUNTIME_TOKEN") {
                    let env = env.trim().to_string();
                    if !env.is_empty() {
                        token = Some(env);
                    }
                }
                // Loopback to THIS server (its real port: a pod listening
                // elsewhere than 5150 must not write through another one).
                token.map(|t| {
                    (
                        format!(
                            "http://127.0.0.1:{}/internal/flow/device-write",
                            config.server.port
                        ),
                        t,
                    )
                })
            },
            worker_fence: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defauts_et_partiel() {
        let d = FlowSettings::default();
        assert!(!d.enabled, "le moteur est coupé par défaut");
        assert_eq!(d.runtime_cmd, "pnex-flow-runtime");
        assert_eq!(
            d.env_allowlist,
            vec![
                "OPENOBSERVE_URL".to_string(),
                "OPENOBSERVE_ROOT_EMAIL".to_string(),
                "OPENOBSERVE_ROOT_PASSWORD".to_string(),
                "VALKEY_URL".to_string(),
            ]
        );

        // Config sans section `flow` → défauts (logger seul champ requis).
        let minimal = serde_json::json!({
            "logger": { "enable": false, "level": "info", "format": "compact" },
            "server": { "port": 5150, "host": "http://localhost" },
            "database": { "uri": "postgres://pnex:pnex@localhost:5432/pnex", "enable_logging": false, "auto_migrate": false, "connect_timeout": 500, "idle_timeout": 500, "min_connections": 1, "max_connections": 5 }
        });
        let config: Config =
            serde_json::from_value(minimal.clone()).expect("config minimale désérialisable");
        let s = FlowSettings::from_config(&config);
        assert_eq!(s.state_dir, d.state_dir);
        assert!(
            s.o2.is_none(),
            "sans section openobserve : pas de creds à injecter"
        );

        // Section partielle : seuls les champs fournis surchargent.
        let config: Config = serde_json::from_value(serde_json::json!({
            "logger": { "enable": false, "level": "info", "format": "compact" },
            "server": { "port": 5150, "host": "http://localhost" },
            "database": { "uri": "postgres://pnex:pnex@localhost:5432/pnex", "enable_logging": false, "auto_migrate": false, "connect_timeout": 500, "idle_timeout": 500, "min_connections": 1, "max_connections": 5 },
            "settings": { "flow": { "enabled": true, "state_dir": "/tmp/flow-etat" } }
        }))
        .expect("config partielle désérialisable");
        let s = FlowSettings::from_config(&config);
        assert!(s.enabled);
        assert_eq!(s.state_dir, "/tmp/flow-etat");
        assert_eq!(s.runtime_cmd, d.runtime_cmd, "champ absent → défaut");

        // Section `openobserve` présente : les creds sont résolues pour
        // injection dans l'env enfant (le Debug n'imprime jamais la valeur).
        let config: Config = serde_json::from_value(serde_json::json!({
            "logger": { "enable": false, "level": "info", "format": "compact" },
            "server": { "port": 5150, "host": "http://localhost" },
            "database": { "uri": "postgres://pnex:pnex@localhost:5432/pnex", "enable_logging": false, "auto_migrate": false, "connect_timeout": 500, "idle_timeout": 500, "min_connections": 1, "max_connections": 5 },
            "settings": {
                "openobserve": {
                    "base_url": "http://localhost:5080",
                    "root_email": "root@example.com",
                    "root_password": "pass"
                }
            }
        }))
        .expect("config avec openobserve désérialisable");
        let s = FlowSettings::from_config(&config);
        assert!(s.o2.is_some(), "creds O2 résolues depuis le yaml");
        assert!(
            !format!("{s:?}").contains("pass"),
            "le Debug ne fuite pas le secret"
        );

        // Section `valkey` présente : l'URL est résolue pour injection dans
        // l'env enfant (le Debug n'imprime que le booléen, jamais l'URL).
        let config: Config = serde_json::from_value(serde_json::json!({
            "logger": { "enable": false, "level": "info", "format": "compact" },
            "server": { "port": 5150, "host": "http://localhost" },
            "database": { "uri": "postgres://pnex:pnex@localhost:5432/pnex", "enable_logging": false, "auto_migrate": false, "connect_timeout": 500, "idle_timeout": 500, "min_connections": 1, "max_connections": 5 },
            "settings": {
                "valkey": { "url": "redis://127.0.0.1:6379" }
            }
        }))
        .expect("config avec valkey désérialisable");
        let s = FlowSettings::from_config(&config);
        assert_eq!(
            s.valkey_url.as_deref(),
            Some("redis://127.0.0.1:6379"),
            "URL valkey résolue depuis le yaml"
        );
        assert!(
            !format!("{s:?}").contains("redis://"),
            "le Debug ne fuite pas l'URL"
        );
    }
}

// ─────────── Écriture des flows (partagée contrôleur ↔ assistant IA) ───────────
//
// Point d'écriture **unique** des versions de flow : le contrôleur HTTP
// (`controllers/flows.rs`) et les outils de l'assistant IA
// (`services/ai/tools.rs`) passent tous deux par ici — jamais de self-call
// HTTP, un seul endroit où valider avant de persister. Le deploy
// (`flow_supervisor`) reste hors de ce module : l'IA n'y a pas accès.

use pnex_core::{FlowGraph, FlowViolation};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect, Set,
    TransactionTrait,
};

use crate::models::_entities::{device_registries, flow_versions, flows};

/// Erreurs d'écriture d'un flow — le contrôleur les mappe en HTTP
/// (400 champ/violations, 409 conflit, 500) ; l'assistant les rend
/// lisibles pour le modèle.
#[derive(Debug)]
pub enum FlowWriteError {
    /// `validate_graph` non satisfait (le client/correction attendu).
    Violations(Vec<FlowViolation>),
    /// Nom vide après trim.
    NameRequired,
    /// Nom > 200 caractères.
    NameTooLong,
    /// device_registry_id hors de l'org.
    DeviceUnknown,
    /// Concurrence optimiste perdue.
    Conflict { expected: i64, current: i64 },
    /// Échec base de données.
    Db,
    /// A secret field could not be stored (unknown secret, value typed
    /// without the right, vault failure).
    Secret(crate::services::secrets::store::StoreError),
    /// The flow is deployed and the writer may only change a stopped flow
    /// (the assistant, D143); checked under the flow's row lock.
    Deployed,
}

/// Validation commune (nom + graphe + device) avant toute écriture.
async fn validate_flow_write(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    graph: &FlowGraph,
    device_registry_id: Option<i64>,
) -> Result<(), FlowWriteError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(FlowWriteError::NameRequired);
    }
    if name.chars().count() > 200 {
        return Err(FlowWriteError::NameTooLong);
    }
    let violations = pnex_core::validate_graph(graph);
    if !violations.is_empty() {
        return Err(FlowWriteError::Violations(violations));
    }
    if let Some(device_id) = device_registry_id {
        let known = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org_id))
            .filter(device_registries::Column::Id.eq(device_id))
            .one(db)
            .await
            .map_err(|_| FlowWriteError::Db)?
            .is_some();
        if !known {
            return Err(FlowWriteError::DeviceUnknown);
        }
    }
    Ok(())
}

/// Crée un flow **et sa version 1** (une transaction). Aucun effet runtime :
/// le flow reste `draft` jusqu'au deploy (qui ne passe jamais par ici).
///
/// Secret fields are stored as vault references (lot S5): the returned
/// graph is the stored one, never the typed values.
#[allow(clippy::too_many_arguments)]
pub async fn create_flow(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    graph: &FlowGraph,
    device_registry_id: Option<i64>,
    author: Option<String>,
    note: Option<String>,
    by: &crate::services::secrets::flow::GraphWriter<'_>,
) -> Result<(flows::Model, i64, FlowGraph), FlowWriteError> {
    validate_flow_write(db, org_id, name, graph, device_registry_id).await?;
    let txn = db.begin().await.map_err(|_| FlowWriteError::Db)?;
    let flow = flows::ActiveModel {
        name: Set(name.trim().to_string()),
        status: Set(pnex_core::FLOW_STATUS_DRAFT.to_string()),
        org_id: Set(org_id),
        device_registry_id: Set(device_registry_id),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| FlowWriteError::Db)?;
    let mut graph = graph.clone();
    crate::services::secrets::flow::store_graph_secrets(&txn, org_id, flow.id, &mut graph, by)
        .await
        .map_err(FlowWriteError::Secret)?;
    flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(1),
        graph: Set(serde_json::to_value(&graph).map_err(|_| FlowWriteError::Db)?),
        author: Set(author),
        note: Set(note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| FlowWriteError::Db)?;
    crate::services::secrets::flow::sync_usages(&txn, flow.id)
        .await
        .map_err(FlowWriteError::Secret)?;
    txn.commit().await.map_err(|_| FlowWriteError::Db)?;
    tracing::info!(flow_id = flow.id, org_id, "flow créé (v1)");
    Ok((flow, 1, graph))
}

/// Numéro de la dernière version d'un flow (0 si aucune — ne doit pas
/// arriver : la création pose toujours v1).
async fn latest_version_number<C: sea_orm::ConnectionTrait>(
    db: &C,
    flow_id: i64,
) -> Result<i64, FlowWriteError> {
    Ok(flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .select_only()
        .column_as(flow_versions::Column::VersionNumber.max(), "latest")
        .into_tuple::<Option<i64>>()
        .one(db)
        .await
        .map_err(|_| FlowWriteError::Db)?
        .flatten()
        .unwrap_or(0))
}

/// Enregistre une **nouvelle version** (append-only) avec concurrence
/// optimiste : `expected_version_number` doit être la version courante.
/// Aucun rechargement runtime — l'exécution ne change qu'au deploy.
#[allow(clippy::too_many_arguments)]
pub async fn append_version(
    db: &DatabaseConnection,
    flow: &flows::Model,
    expected_version_number: i64,
    graph: &FlowGraph,
    new_name: Option<String>,
    author: Option<String>,
    note: Option<String>,
    by: &crate::services::secrets::flow::GraphWriter<'_>,
) -> Result<(flows::Model, i64, FlowGraph), FlowWriteError> {
    append_version_with(
        db,
        flow,
        expected_version_number,
        graph,
        new_name,
        author,
        note,
        by,
        false,
    )
    .await
}

/// [`append_version`] for a writer that may only change a **stopped** flow
/// (the assistant, D143): the deployed status is re-read inside the write
/// transaction, under the flow's row lock, so a deploy landing between the
/// caller's read and this write is refused ([`FlowWriteError::Deployed`]).
#[allow(clippy::too_many_arguments)]
pub async fn append_version_if_stopped(
    db: &DatabaseConnection,
    flow: &flows::Model,
    expected_version_number: i64,
    graph: &FlowGraph,
    new_name: Option<String>,
    author: Option<String>,
    note: Option<String>,
    by: &crate::services::secrets::flow::GraphWriter<'_>,
) -> Result<(flows::Model, i64, FlowGraph), FlowWriteError> {
    append_version_with(
        db,
        flow,
        expected_version_number,
        graph,
        new_name,
        author,
        note,
        by,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn append_version_with(
    db: &DatabaseConnection,
    flow: &flows::Model,
    expected_version_number: i64,
    graph: &FlowGraph,
    new_name: Option<String>,
    author: Option<String>,
    note: Option<String>,
    by: &crate::services::secrets::flow::GraphWriter<'_>,
    only_if_stopped: bool,
) -> Result<(flows::Model, i64, FlowGraph), FlowWriteError> {
    let name_for_validation = new_name.as_deref().unwrap_or(&flow.name);
    validate_flow_write(
        db,
        flow.org_id,
        name_for_validation,
        graph,
        flow.device_registry_id,
    )
    .await?;
    let txn = db.begin().await.map_err(|_| FlowWriteError::Db)?;
    // Serialize concurrent saves of this flow: a no-op UPDATE takes the
    // parent row lock (Postgres), so the latest version read below is the
    // committed one and two saves can never both pass the optimistic check
    // (the loser gets a 409, never a 500 on the version unique index).
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    flows::Entity::update_many()
        .col_expr(
            flows::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(flows::Column::Id.eq(flow.id))
        .exec(&txn)
        .await
        .map_err(|_| FlowWriteError::Db)?;
    if only_if_stopped {
        let status = flows::Entity::find_by_id(flow.id)
            .one(&txn)
            .await
            .map_err(|_| FlowWriteError::Db)?
            .map(|f| f.status);
        if status.as_deref() == Some(pnex_core::FLOW_STATUS_DEPLOYED) {
            return Err(FlowWriteError::Deployed);
        }
    }
    let latest = latest_version_number(&txn, flow.id).await?;
    if expected_version_number != latest {
        return Err(FlowWriteError::Conflict {
            expected: expected_version_number,
            current: latest,
        });
    }
    let mut active: flows::ActiveModel = flow.clone().into();
    if let Some(name) = new_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        active.name = Set(name.to_string());
    }
    let flow = active.update(&txn).await.map_err(|_| FlowWriteError::Db)?;
    let mut graph = graph.clone();
    crate::services::secrets::flow::store_graph_secrets(&txn, flow.org_id, flow.id, &mut graph, by)
        .await
        .map_err(FlowWriteError::Secret)?;
    let new_version = flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(latest + 1),
        graph: Set(serde_json::to_value(&graph).map_err(|_| FlowWriteError::Db)?),
        author: Set(author),
        note: Set(note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|e| {
        if crate::services::db_lock::is_unique_violation(&e) {
            FlowWriteError::Conflict {
                expected: expected_version_number,
                current: latest + 1,
            }
        } else {
            FlowWriteError::Db
        }
    })?;
    crate::services::secrets::flow::sync_usages(&txn, flow.id)
        .await
        .map_err(FlowWriteError::Secret)?;
    txn.commit().await.map_err(|_| FlowWriteError::Db)?;
    tracing::info!(
        flow_id = flow.id,
        version = new_version.version_number,
        "flow enregistré (nouvelle version)"
    );
    Ok((flow, new_version.version_number, graph))
}
