//! `POST /flows/{id}/nodes/{node}/run` — fires one Inject trigger of the
//! deployed version now (pnex-core `flow::run_now`), instead of waiting for
//! its interval or cron.

use super::*;

use pnex_core::err_codes;
use pnex_core::flow::{run_now_channel, FlowNodeKind, RunNowEvent};

use crate::controllers::coded_error;

const VALKEY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

#[utoipa::path(
    post,
    path = "/api/v1/flows/{id}/nodes/{node}/run",
    tag = "flows",
    operation_id = "flows_nodes_run",
    summary = "Run a flow now from one of its Inject triggers",
    description = "Fires the Inject node `node` of the deployed version once, as its interval or cron would: its payload and topic go down its wires. Accepted once the runtime received it; what the flow then does is seen in its debug feed and node statuses. Requires the flow deploy permission (owner, admin or member).",
    params(
        ("id" = i64, Path, description = "Flow id"),
        ("node" = String, Path, description = "Canvas id of an Inject node of the deployed version")
    ),
    responses(
        (status = 202, body = FlowRunNowResult),
        (status = 400, description = "The node is not an Inject of the deployed version (`flow-inject-node`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Flow not found in this organisation", body = crate::openapi::ErrorDetail),
        (status = 409, description = "The flow is not deployed (`flow-inject-not-deployed`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "No runtime received it (`flow-inject-unavailable`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn run_now(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, node)): Path<(i64, String)>,
) -> Result<Response> {
    // `flows:deploy` is checked by the extractor.
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let not_deployed = || {
        coded_error(
            StatusCode::CONFLICT,
            err_codes::FLOW_INJECT_NOT_DEPLOYED,
            "Deploy the flow before running it.",
            None,
        )
    };
    if flow.status != pnex_core::FLOW_STATUS_DEPLOYED {
        return Err(not_deployed());
    }
    let Some(version_id) = flow.deployed_version_id else {
        return Err(not_deployed());
    };
    let version = flow_versions::Entity::find_by_id(version_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .filter(|v| v.flow_id == flow.id)
        .ok_or_else(not_deployed)?;
    let graph: FlowGraph =
        serde_json::from_value(version.graph).map_err(|_| Error::InternalServerError)?;
    let is_inject = graph
        .nodes
        .iter()
        .any(|n| n.id == node && matches!(n.kind, FlowNodeKind::Inject { .. }));
    if !is_inject {
        return Err(coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::FLOW_INJECT_NODE,
            "This node is not an Inject trigger of the deployed version.",
            None,
        ));
    }
    let event = RunNowEvent {
        flow_id: flow.id,
        node_id: node.clone(),
        by: Some(org.auth.user.email.clone()),
        ts_ms: chrono::Utc::now().timestamp_millis(),
    };
    let unavailable = || {
        coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::FLOW_INJECT_UNAVAILABLE,
            "The flow runtime is not answering: the flow was not run.",
            None,
        )
    };
    let Some(mut conn) = crate::services::shared_valkey::conn(&ctx.config).await else {
        return Err(unavailable());
    };
    let frame = serde_json::to_string(&event).map_err(|_| Error::InternalServerError)?;
    let op =
        redis::AsyncCommands::publish::<_, _, i64>(&mut conn, run_now_channel(org.org.id), frame);
    // No subscriber = no runtime holds the flow's companion node: say so
    // rather than accept a click nothing will act on.
    match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        Ok(Ok(n)) if n > 0 => {}
        Ok(Ok(_)) => return Err(unavailable()),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "run now: valkey error");
            return Err(unavailable());
        }
        Err(_) => {
            tracing::warn!("run now: valkey timeout");
            return Err(unavailable());
        }
    }
    Ok((
        StatusCode::ACCEPTED,
        format::json(FlowRunNowResult {
            flow_id: flow.id,
            node_id: node,
        }),
    )
        .into_response())
}
