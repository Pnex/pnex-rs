use super::*;

/// Flow de l'org courante, sinon None (→ 404).
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

/// Dernière version d'un flow (numéro le plus élevé).
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
        status: f.status,
        deployed_version_number: deployed_number,
        latest_version_number: latest,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

pub(super) fn flow_dto(
    f: flows::Model,
    graph: FlowGraph,
    latest: i64,
    deployed_number: Option<i64>,
) -> pnex_core::Flow {
    pnex_core::Flow {
        id: f.id,
        org_id: f.org_id,
        device_id: f.device_registry_id,
        name: f.name,
        status: f.status,
        deployed_version_number: deployed_number,
        latest_version_number: latest,
        graph,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}
