//! Nœud custom EdgeLinkd `pnex-sql` (décision D18) — requête SQL Postgres en
//! **lecture seule**, premier nœud natif du moteur de flow ETL PNEX.
//!
//! Garde-fous (PRD §3) :
//! - la requête est validée `SELECT`/`WITH` seul **au build du nœud** (un
//!   graphe invalide est rejeté au déploiement, côté backend et runtime) ;
//! - le `msg` entrant est validé **à la frontière** du nœud via le contrat
//!   typé `pnex_core::SqlQueryRequest` (payload objet ou absent, sinon rejet
//!   sans panic) ;
//! - la connexion vient de l'environnement du runtime (`DATABASE_URL`),
//!   **jamais** du graphe — aucun secret dans `flows.json` ;
//! - toute l'I/O SQL vit ici (nœud natif compilé), jamais dans un script user.
//!
//! Le rôle Postgres côté production doit de toute façon être en lecture seule
//! (voir `docs/architecture/flow-engine.md`) — l'analyse syntaxique est une
//! défense complémentaire, pas la seule.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

// Imports en wildcards comme le plugin de référence edgelink-nodes-dummy :
// la macro `#[flow_node]` développe du code (MetaNode, NodeFactory,
// FlowsElement, ElementId, Context…) résolu dans le scope du module.
use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use sqlx::{Column, Row};

/// Point d'ancrage référencé par le binaire `pnex-flow-runtime` : garantit que
/// l'édition de liens conserve les soumissions `inventory` de ce crate.
pub fn registered() {}

fn default_timeout_secs() -> f64 {
    30.0
}

#[derive(Debug, Deserialize)]
struct PnexSqlNodeConfig {
    query: String,

    /// Clés que `msg.payload` doit contenir (contrat d'entrée typé).
    #[serde(default)]
    params: Vec<String>,

    /// Délai d'exécution de la requête (et d'acquisition de connexion).
    #[serde(default = "default_timeout_secs")]
    timeout_secs: f64,
}

#[derive(Debug)]
#[flow_node("pnex-sql", red_name = "pnex-sql")]
struct PnexSqlNode {
    base: BaseFlowNodeState,
    config: PnexSqlNodeConfig,
    pool: sqlx::PgPool,
}

impl PnexSqlNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = PnexSqlNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-sql : config invalide : {e}"))
        })?;

        // Contrat typé au build : lecture seule uniquement.
        if let Err(v) = pnex_core::validate_sql_readonly(&cfg.query) {
            return Err(EdgelinkError::BadFlowsJson(format!("pnex-sql : {}", v.message)).into());
        }

        // Secret par environnement du process, jamais par flows.json.
        let url = std::env::var("DATABASE_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(
                "pnex-sql : DATABASE_URL absente de l'environnement du runtime".into(),
            )
        })?;

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs_f64(cfg.timeout_secs))
            .connect_lazy(&url)
            .map_err(|e| EdgelinkError::InvalidOperation(format!("pnex-sql : pool invalide : {e}")))?;

        Ok(Box::new(PnexSqlNode { base: base_node, config: cfg, pool }))
    }

    /// Un message entrant → une requête SELECT → un msg de sortie dont
    /// `payload` est le tableau de lignes (contrat `SqlQueryResult`).
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        log::debug!("pnex-sql [{}] : réception d'un message", self.name());
        // 1) Frontière d'entrée : payload conforme au contrat typé (objet avec
        //    les clés requises, ou déclencheur pur quand aucune n'est déclarée).
        {
            let m = msg.read().await;
            let payload_json = match m.get("payload").cloned() {
                Some(v) => Some(serde_json::to_value(&v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!("pnex-sql : payload non sérialisable : {e}"))
                })?),
                None => None,
            };
            let request = pnex_core::SqlQueryRequest::validate_payload(payload_json.as_ref(), &self.config.params)
                .map_err(|v| {
                    EdgelinkError::InvalidOperation(format!("pnex-sql [{}] : {}", self.name(), v.message))
                })?;
            // Phase 1 : les paramètres nommés ne sont pas encore substitués —
            // le contrat vérifie le type, la substitution arrive avec la Phase 2.
            let _ = request.params;
        }

        // 2) Exécution bornée (timeout) et annulable (arrêt du flow).
        let deadline = Duration::from_secs_f64(self.config.timeout_secs);
        log::debug!("pnex-sql [{}] : exécution de la requête (délai {} s)", self.name(), self.config.timeout_secs);
        let rows = tokio::select! {
            res = tokio::time::timeout(deadline, self.query_rows()) => match res {
                Ok(inner) => inner?,
                Err(_) => {
                    return Err(EdgelinkError::InvalidOperation(format!(
                        "pnex-sql [{}] : requête interrompue (délai {} s dépassé)",
                        self.name(),
                        self.config.timeout_secs
                    ))
                    .into())
                }
            },
            _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
        };

        // 3) Frontière de sortie : toujours un tableau de lignes.
        log::debug!("pnex-sql [{}] : {} lignes récupérées", self.name(), rows.len());
        let result = pnex_core::SqlQueryResult { rows };
        let payload: Variant = serde_json::from_value(result.to_value()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-sql : résultat non convertible : {e}"))
        })?;
        {
            let mut m = msg.write().await;
            m.set("payload".to_string(), payload);
        }

        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }

    /// Exécute la requête et convertit chaque ligne en objet JSON
    /// (colonne → valeur). Les types non couverts sont une erreur explicite,
    /// jamais une coercion silencieuse.
    async fn query_rows(&self) -> anyhow::Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let rows = sqlx::query(&self.config.query).fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut obj = serde_json::Map::new();
            for (i, col) in row.columns().iter().enumerate() {
                // `Column::name` explicite : PgColumn a aussi un `name` privé
                // inhérent qui prendrait la résolution sinon.
                obj.insert(Column::name(col).to_string(), cell_to_json(row, i)?);
            }
            out.push(obj);
        }
        Ok(out)
    }
}

fn cell_to_json(row: &sqlx::postgres::PgRow, i: usize) -> anyhow::Result<serde_json::Value> {
    use serde_json::Value;
    // UFCS : PgTypeInfo/PgColumn ont des inherent `name` privés qui bloquent
    // la résolution des méthodes de trait `sqlx::TypeInfo`/`sqlx::Column`.
    let ty = sqlx::TypeInfo::name(row.columns()[i].type_info()).to_string();
    let v = match ty.as_str() {
        "BOOL" => Value::Bool(row.try_get::<bool, _>(i)?),
        "INT2" => serde_json::json!(row.try_get::<i16, _>(i)?),
        "INT4" | "OID" => serde_json::json!(row.try_get::<i32, _>(i)?),
        "INT8" => serde_json::json!(row.try_get::<i64, _>(i)?),
        "FLOAT4" => serde_json::json!(row.try_get::<f32, _>(i)?),
        "FLOAT8" => serde_json::json!(row.try_get::<f64, _>(i)?),
        "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" | "CITEXT" | "UNKNOWN" => {
            serde_json::json!(row.try_get::<String, _>(i)?)
        }
        "JSON" | "JSONB" => row.try_get::<serde_json::Value, _>(i)?,
        "TIMESTAMPTZ" => serde_json::json!(row.try_get::<chrono::DateTime<chrono::Utc>, _>(i)?.to_rfc3339()),
        "TIMESTAMP" => {
            serde_json::json!(row.try_get::<chrono::NaiveDateTime, _>(i)?.and_utc().to_rfc3339())
        }
        "DATE" => serde_json::json!(row.try_get::<chrono::NaiveDate, _>(i)?.to_string()),
        "TIME" => serde_json::json!(row.try_get::<chrono::NaiveTime, _>(i)?.to_string()),
        "UUID" => serde_json::json!(row.try_get::<uuid::Uuid, _>(i)?.to_string()),
        other => {
            return Err(anyhow::anyhow!(
                "pnex-sql : type de colonne « {other} » non converti en JSON (Phase 1) — cast explicite requis, ex. ::text"
            ))
        }
    };
    Ok(v)
}

#[async_trait]
impl FlowNodeBehavior for PnexSqlNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(self.as_ref(), cancel.child_token(), |node: &PnexSqlNode, msg: MsgHandle| async move {
                // `with_uow` route les erreurs vers `flow.handle_error` sans
                // les logger : on journalise ici pour l'exploitation.
                match node.execute(msg.clone(), cancel.child_token()).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        log::warn!("pnex-sql [{}] : message rejeté : {e}", node.name());
                        Err(e)
                    }
                }
            })
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgelink_core::runtime::registry::RegistryBuilder;

    #[test]
    fn node_enregistre_dans_le_registre() {
        // Les soumissions inventory de CE crate sont visibles dans son propre
        // binaire de test : le nœud doit apparaître aux côtés des builtins.
        let reg = RegistryBuilder::default().build().expect("registre");
        let meta = reg.get("pnex-sql").expect("nœud pnex-sql absent du registre");
        assert_eq!(meta.type_, "pnex-sql");
    }

    #[test]
    fn registered_ne_panique_pas() {
        registered();
    }

    #[test]
    fn config_rejete_requete_ecriture_au_build() {
        // Vérifie le contrat de build via la même validation que `build()`.
        let err = pnex_core::validate_sql_readonly("DELETE FROM t").unwrap_err();
        assert_eq!(err.code, "readonly_sql");
    }
}
