//! The `site_crawl` flow node on the server (network-check.md D407). The
//! node's options come from the deployed version of the flow; the site and
//! its relevance rules from the message. One `crawl_site` mission per site
//! is kept for the flows (disabled: the hourly sweep never plays it), so
//! the registry, gone pages and proofs of D307–D310 work unchanged.

use loco_rs::app::AppContext;
use pnex_core::browser_agent::{self as ba, CrawlExecutor, CrawlSiteParams, MissionSpec};
use pnex_core::err_codes;
use pnex_core::flow::FlowNodeKind;
use pnex_core::site_crawl::{
    SiteCrawlNodeReply, SiteCrawlNodeRequest, SiteCrawlSummary, NODE_PORT_FAILED, NODE_PORT_PAGE,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};
use uuid::Uuid;

use super::ontology::flow_nodes::{deployed, NodeError};

/// Name prefix of the missions kept for flows.
const MISSION_PREFIX: &str = "Flow · ";

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn failed(code: &str, args: Value) -> SiteCrawlNodeReply {
    SiteCrawlNodeReply {
        port: NODE_PORT_FAILED,
        code: Some(code.into()),
        args,
        ..Default::default()
    }
}

/// The flows' mission of the site `target`, created disabled the first
/// time; its spec takes the message's parameters.
async fn mission_for(
    ctx: &AppContext,
    org_id: i64,
    target: &str,
    spec: &MissionSpec,
) -> Result<Uuid, sea_orm::DbErr> {
    let spec = serde_json::to_value(spec).unwrap_or_default();
    let found = ctx
        .db
        .query_one_raw(stmt(
            "UPDATE extension_missions SET spec = $3 \
             WHERE org_id = $1 AND kind = 'crawl_site' AND target = $2 AND NOT enabled \
               AND name LIKE $4 RETURNING id",
            vec![
                org_id.into(),
                target.to_string().into(),
                spec.clone().into(),
                format!("{MISSION_PREFIX}%").into(),
            ],
        ))
        .await?
        .and_then(|r| r.try_get::<Uuid>("", "id").ok());
    if let Some(id) = found {
        return Ok(id);
    }
    let id = Uuid::new_v4();
    let name: String = format!("{MISSION_PREFIX}{}", target.trim_start_matches("web:site:"))
        .chars()
        .take(ba::NAME_MAX)
        .collect();
    ctx.db
        .execute_raw(stmt(
            "INSERT INTO extension_missions (id, org_id, name, kind, spec, target, interval_secs, \
                enabled) VALUES ($1, $2, $3, 'crawl_site', $4, $5, $6, false)",
            vec![
                id.into(),
                org_id.into(),
                name.into(),
                spec.into(),
                target.to_string().into(),
                ba::MAX_INTERVAL_SECS.into(),
            ],
        ))
        .await?;
    Ok(id)
}

/// One site of the deployed node `req.node`.
pub async fn run(
    ctx: &AppContext,
    req: &SiteCrawlNodeRequest,
) -> Result<SiteCrawlNodeReply, NodeError> {
    let FlowNodeKind::SiteCrawl { config } = deployed(&ctx.db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org_id = req.node.org_id;
    let Some(origin) = ba::site_origin(&req.url) else {
        return Ok(failed(err_codes::SITE_CRAWL_URL_INVALID, Value::Null));
    };
    let record = config.record.clone().map(|mut r| {
        r.link_to = req.link_to.clone().filter(|t| Uuid::parse_str(t).is_ok());
        if r.link_to.is_none() {
            r.link_type = None;
        }
        r
    });
    let asked = MissionSpec::CrawlSite(CrawlSiteParams {
        url: origin,
        executor: CrawlExecutor::Server,
        max_pages: config.max_pages,
        max_depth: ba::CRAWL_DEPTH_MAX,
        exclude: config.exclude.clone(),
        matchers: req.matchers.clone(),
        proofs: config.proofs,
        record,
    });
    let (spec, target) = match ba::check_spec(&asked) {
        Ok(ok) => ok,
        Err((field, _)) => {
            return Ok(failed(
                err_codes::SITE_CRAWL_INVALID,
                json!({ "field": field.trim_start_matches("spec.params.") }),
            ))
        }
    };
    let MissionSpec::CrawlSite(params) = &spec else {
        return Err(NodeError::NotDeployed);
    };
    let mission = mission_for(ctx, org_id, &target, &spec).await?;
    ctx.db
        .execute_raw(stmt(
            "UPDATE extension_missions SET state = 'running', state_code = NULL, \
               cursor = coalesce(cursor, '{}'::jsonb) || jsonb_build_object('last_crawl_at', now()) \
             WHERE id = $1",
            vec![mission.into()],
        ))
        .await?;
    let out = super::crawl::crawl(ctx, org_id, mission, params).await;
    ctx.db
        .execute_raw(stmt(
            "UPDATE extension_missions SET state = $2, state_code = $3, \
               cursor = coalesce(cursor, '{}'::jsonb) || $4::jsonb WHERE id = $1",
            vec![
                mission.into(),
                if out.blocked.is_some() {
                    "blocked"
                } else {
                    "idle"
                }
                .into(),
                out.blocked.map(str::to_string).into(),
                json!({ "pages": out.pages, "relevant": out.relevant, "truncated": out.truncated })
                    .into(),
            ],
        ))
        .await?;
    tracing::info!(org_id, mission = %mission, pages = out.pages, relevant = out.relevant, blocked = ?out.blocked, "flow site crawled");
    let pages = out
        .found
        .into_iter()
        .filter(|p| p.changed || !config.only_changed)
        .collect();
    Ok(SiteCrawlNodeReply {
        port: NODE_PORT_PAGE,
        pages,
        summary: Some(SiteCrawlSummary {
            url: params.url.clone(),
            pages: out.pages,
            relevant: out.relevant,
            truncated: out.truncated,
            blocked: out.blocked.map(str::to_string),
            mission_id: mission.to_string(),
        }),
        ..Default::default()
    })
}
