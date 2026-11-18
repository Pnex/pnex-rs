//! `pipeline_ask` flow nodes on the server side (pipeline-engine.md D999).
//!
//! The flow runtime runs user code and is not trusted: the pipeline and the
//! daily ceiling come from the *deployed* node, the request gives rows only.
//! The rows run in the browser agent of the person who deployed the flow,
//! with that person's rights re-read now (R2); a row naming an object whose
//! subject objected is refused (D341).

use std::time::Duration;

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::pipeline::ask::{
    PipelineAskReply, PipelineAskRequest, NODE_PORT_ASKED, NODE_PORT_REFUSED, ROWS_MAX,
};
use pnex_core::pipeline::Step;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::services::ontology::flow_nodes::{deployed, spend, NodeError};

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

fn refused(code: &str, args: Value) -> PipelineAskReply {
    PipelineAskReply {
        port: NODE_PORT_REFUSED,
        code: Some(code.to_string()),
        args,
        ..Default::default()
    }
}

/// The columns a row must hold: the asked axis and its other columns.
fn asked_columns(doc: &pnex_core::pipeline::Document) -> Option<Vec<String>> {
    let Some(Step::Enumerate(e)) = doc.steps.first() else {
        return None;
    };
    if !e.track {
        return None;
    }
    e.cross.iter().find_map(|a| {
        let cols = a.asked.as_ref()?;
        Some(
            std::iter::once(a.as_.clone())
                .chain(cols.iter().cloned())
                .collect(),
        )
    })
}

/// Rows keeping only the asked columns, every value as text; `None` when
/// one misses a column.
fn shaped(rows: &[Map<String, Value>], columns: &[String]) -> Option<Vec<Map<String, Value>>> {
    rows.iter()
        .map(|r| {
            columns
                .iter()
                .map(|c| {
                    let v = match r.get(c)? {
                        Value::String(s) => s.clone(),
                        Value::Null => return None,
                        other => other.to_string(),
                    };
                    Some((c.clone(), Value::String(v)))
                })
                .collect::<Option<Map<String, Value>>>()
        })
        .collect()
}

/// `type:id` of the org's objects the rows name, for the objection check.
async fn named_objects(
    db: &impl ConnectionTrait,
    org_id: i64,
    rows: &[Map<String, Value>],
) -> Result<Vec<String>, sea_orm::DbErr> {
    let ids: Vec<Uuid> = rows
        .iter()
        .flat_map(|r| r.values())
        .filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(db
        .query_all_raw(stmt(
            "SELECT type_key, id::text AS id FROM objects WHERE org_id = $1 AND id = ANY($2)",
            vec![org_id.into(), ids.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| {
            let t: String = r.try_get("", "type_key").ok()?;
            let id: String = r.try_get("", "id").ok()?;
            Some(format!("{t}:{id}"))
        })
        .collect())
}

/// Spends `n` of the node's rows of the day in Valkey; `false` when over
/// `max` (nothing spent) or when Valkey is out of reach (closed).
async fn spend_day(ctx: &AppContext, req: &PipelineAskRequest, n: usize, max: u32) -> bool {
    let Some(mut conn) = crate::services::shared_valkey::conn(&ctx.config).await else {
        return false;
    };
    let day = chrono::Utc::now().format("%Y%m%d");
    let key = format!(
        "pnex:pask:v1:{}:{}:{}:{day}",
        req.node.org_id, req.node.flow_id, req.node.node_id
    );
    let op = async {
        let total: i64 = redis::AsyncCommands::incr(&mut conn, &key, n as i64).await?;
        let _: bool = redis::AsyncCommands::expire(&mut conn, &key, 2 * 24 * 3600).await?;
        if total > max as i64 {
            let _: i64 = redis::AsyncCommands::decr(&mut conn, &key, n as i64).await?;
            return Ok::<bool, redis::RedisError>(false);
        }
        Ok(true)
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(5), op).await,
        Ok(Ok(true))
    )
}

/// One message of a deployed `pipeline_ask` node.
pub async fn ask_node(
    ctx: &AppContext,
    req: &PipelineAskRequest,
) -> Result<PipelineAskReply, NodeError> {
    let db = &ctx.db;
    let pnex_core::FlowNodeKind::PipelineAsk { config } = deployed(db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org_id = req.node.org_id;
    let deployer: Option<i64> = db
        .query_one_raw(stmt(
            "SELECT deployed_by FROM flows WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), req.node.flow_id.into()],
        ))
        .await?
        .map(|r| r.try_get("", "deployed_by"))
        .transpose()?
        .flatten();
    let grants = match deployer {
        Some(user) => crate::services::iam::store::member_grants(db, org_id, user).await?,
        None => None,
    };
    let (Some(user), Some(grants)) = (deployer, grants) else {
        return Ok(refused(err_codes::ACCESS_DENIED, json!({})));
    };
    if !grants.can("pipelines:run") {
        return Ok(refused(
            err_codes::ACCESS_DENIED,
            json!({ "permission": "pipelines:run" }),
        ));
    }
    let not_found = || refused(err_codes::PIPELINE_ASK_NOT_FOUND, json!({}));
    let Some(sp) = super::store::shipped()
        .iter()
        .find(|s| s.id() == config.pipeline)
    else {
        return Ok(not_found());
    };
    let installed = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM packs WHERE org_id = $1 AND key = $2",
            vec![org_id.into(), sp.pack.clone().into()],
        ))
        .await?
        .is_some();
    let Some(columns) = asked_columns(&sp.doc).filter(|_| installed) else {
        return Ok(not_found());
    };
    if req.rows.is_empty() {
        return Ok(PipelineAskReply::default());
    }
    let Some(rows) = shaped(&req.rows[..req.rows.len().min(ROWS_MAX)], &columns) else {
        return Ok(refused(
            err_codes::PIPELINE_ASK_ROW_INVALID,
            json!({ "columns": columns.join(", ") }),
        ));
    };
    let keys = named_objects(db, org_id, &rows).await?;
    let opposed = crate::services::profiles::opposed_any(db, org_id, &keys)
        .await
        .map_err(|e| NodeError::from(sea_orm::DbErr::Custom(format!("{e:?}"))))?;
    if opposed {
        return Ok(refused(err_codes::PIPELINE_ASK_OPPOSED, json!({})));
    }
    spend(org_id)?;
    let max = config.per_day();
    if !spend_day(ctx, req, rows.len(), max).await {
        return Ok(refused(
            err_codes::PIPELINE_ASK_DAILY_CAP,
            json!({ "max": max }),
        ));
    }
    let n = rows.len();
    let out = super::store::run_asked(ctx, org_id, Some(user), &config.pipeline, rows)
        .await
        .map_err(|e| NodeError::from(sea_orm::DbErr::Custom(format!("pipeline ask: {e:?}"))))?;
    let failure = out
        .failure
        .as_ref()
        .or_else(|| out.items.first().and_then(|(_, f, _)| f.as_ref()))
        .filter(|f| f.code != super::WAITING);
    Ok(match failure.map(|f| (f.code, f.args.clone())) {
        None => PipelineAskReply {
            port: NODE_PORT_ASKED,
            asked: n,
            ..Default::default()
        },
        Some(("no_agent", _)) => refused(err_codes::EXT_NO_AGENT, json!({})),
        Some((code, args)) => refused(
            err_codes::FLOW_RUNTIME,
            json!({ "reason": code, "args": args }),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_keep_the_asked_columns_as_text() {
        let cols = vec!["person_id".to_string(), "step".to_string()];
        let rows = vec![json!({"person_id": "p", "step": 0, "extra": 1})
            .as_object()
            .unwrap()
            .clone()];
        let out = shaped(&rows, &cols).unwrap();
        assert_eq!(out[0]["step"], "0");
        assert!(!out[0].contains_key("extra"));
        let missing = vec![json!({"person_id": "p"}).as_object().unwrap().clone()];
        assert!(shaped(&missing, &cols).is_none());
    }

    #[test]
    fn the_shipped_person_match_declares_its_asked_columns() {
        let sp = crate::services::pipeline_engine::store::shipped()
            .iter()
            .find(|s| s.id() == "prospecting_b2b/person_match_linkedin")
            .expect("shipped");
        let cols = asked_columns(&sp.doc).unwrap();
        assert_eq!(cols[0], "person_id");
    }
}
