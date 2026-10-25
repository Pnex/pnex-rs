use super::*;

/// Flow of the current org, else None (404).
pub(super) async fn find_flow(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<Option<flows::Model>> {
    flows::Entity::find_by_id(id)
        .filter(flows::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Latest version of a flow (highest number).
pub(super) async fn latest_version(
    db: &DatabaseConnection,
    flow_id: i64,
) -> Result<Option<flow_versions::Model>> {
    flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

pub(super) fn summary_dto(
    f: flows::Model,
    latest: i64,
    deployed_number: Option<i64>,
) -> pnex_core::FlowSummary {
    pnex_core::FlowSummary {
        id: f.id,
        org_id: f.org_id,
        device_id: f.device_registry_id,
        name: f.name,
        status: pnex_core::FlowStatus::from_db(&f.status),
        deployed_version_number: deployed_number,
        latest_version_number: latest,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

pub(super) fn flow_dto(
    f: flows::Model,
    (graph, unreadable_nodes): (FlowGraph, Vec<pnex_core::FlowViolation>),
    latest: i64,
    deployed_number: Option<i64>,
) -> pnex_core::Flow {
    pnex_core::Flow {
        id: f.id,
        org_id: f.org_id,
        device_id: f.device_registry_id,
        name: f.name,
        status: pnex_core::FlowStatus::from_db(&f.status),
        deployed_version_number: deployed_number,
        latest_version_number: latest,
        graph,
        unreadable_nodes,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

/// Tab of ANOTHER deployed flow left out of the projection (unreadable
/// stored graph); the deploy itself goes on.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub(crate) struct SkippedTab {
    pub flow_id: i64,
    pub code: String,
    pub message: String,
}

/// `POST /flows/{id}/deploy` answer: the flow plus non-blocking warnings.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub(super) struct FlowDeployResult {
    #[serde(flatten)]
    pub flow: pnex_core::Flow,
    /// Notify references of this flow that are stale (channel missing or
    /// disabled, template missing or changed).
    pub stale_notify_nodes: Vec<pnex_core::StaleNotifyNode>,
    /// Tabs of other flows isolated from the artifact.
    pub skipped_tabs: Vec<SkippedTab>,
}

/// `POST /flows/{id}/stop` answer.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub(super) struct FlowStopResult {
    /// Always `stopped`.
    pub status: pnex_core::FlowStatus,
}
