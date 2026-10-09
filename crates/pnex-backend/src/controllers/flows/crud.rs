use super::*;

// ─────────────────────────── POST /flows ───────────────────────────

/// `POST /api/v1/flows` — crée le flow **et sa version 1** (une transaction).
/// Aucun effet runtime : le flow reste `draft` jusqu'au deploy.
pub(super) async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateFlow>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "flow-write-forbidden",
            "Owner, admin or member role required to manage flows.",
        ));
    }
    let ring = crate::services::secrets::Keyring::from_config(&ctx.config).ok();
    let (flow, version, graph) = match crate::services::flow::create_flow(
        &ctx.db,
        org.org.id,
        &params.name,
        &params.graph,
        params.device_id,
        params.author,
        params.note,
        &graph_writer(&org, ring.as_ref()),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return flow_write_error_result(e),
    };
    Ok((
        StatusCode::CREATED,
        format::json(flow_dto(flow, graph, version, None)),
    )
        .into_response())
}

/// Vault rights of the acting user on a graph save (D117).
fn graph_writer<'a>(
    org: &OrgContext,
    ring: Option<&'a crate::services::secrets::Keyring>,
) -> crate::services::secrets::flow::GraphWriter<'a> {
    crate::services::secrets::flow::GraphWriter {
        ring,
        user_id: Some(org.auth.user.id),
        can_write_secrets: org.can_manage_secrets(),
    }
}

/// Write errors: vault ones answer like `/secrets` (404 / 403 codes).
fn flow_write_error_result(e: crate::services::flow::FlowWriteError) -> Result<Response> {
    match e {
        crate::services::flow::FlowWriteError::Secret(e) => {
            crate::controllers::secrets::store_error(e)
        }
        e => Ok(flow_write_error_response(e)),
    }
}

// ─────────────────────────── GET /flows ───────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(super) struct ListFlowsQuery {
    search: Option<String>,
    status: Option<String>,
    /// D42: effective label (`name` or `name:value`, inherited included).
    label: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/flows` — flows de l'org, paginés (D14), filtres
/// `search` (nom) et `status`.
pub(super) async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListFlowsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org.org.id))
        .order_by_desc(flows::Column::Id);
    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(flows::Column::Status.eq(status));
    }
    // Search, COUNT and LIMIT/OFFSET in SQL (org-scoped).
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        query = query.filter(pagination::sql_contains(
            (flows::Entity, flows::Column::Name),
            &pat,
        ));
    }
    // D42: effective label filter.
    match crate::services::resources::labels::list_filter(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_FLOW,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            let ids: Vec<i64> = ids.iter().filter_map(|id| id.parse().ok()).collect();
            query = query.filter(flows::Column::Id.is_in(ids));
        }
        Err(crate::services::resources::labels::ListLabelFilterError::Invalid(reason)) => {
            return Ok(field_status("label", &reason));
        }
        Err(crate::services::resources::labels::ListLabelFilterError::Db(_)) => {
            return Err(Error::InternalServerError);
        }
    }
    let (count, page_rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Hydratation en vrac (pas de N+1) : dernier + numéro déployé par flow.
    let ids: Vec<i64> = page_rows.iter().map(|f| f.id).collect();
    let latest: std::collections::HashMap<i64, i64> = flow_versions::Entity::find()
        .select_only()
        .column(flow_versions::Column::FlowId)
        .column_as(flow_versions::Column::VersionNumber.max(), "latest")
        .filter(flow_versions::Column::FlowId.is_in(ids.clone()))
        .group_by(flow_versions::Column::FlowId)
        .into_tuple::<(i64, i64)>()
        .all(&ctx.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let deployed_ids: Vec<i64> = page_rows
        .iter()
        .filter_map(|f| f.deployed_version_id)
        .collect();
    let deployed_numbers: std::collections::HashMap<i64, i64> = if deployed_ids.is_empty() {
        Default::default()
    } else {
        flow_versions::Entity::find()
            .filter(flow_versions::Column::Id.is_in(deployed_ids))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|v| (v.id, v.version_number))
            .collect()
    };
    let results: Vec<pnex_core::FlowSummary> = page_rows
        .into_iter()
        .map(|f| {
            let latest_n = latest.get(&f.id).copied().unwrap_or(0);
            let deployed_n = f
                .deployed_version_id
                .and_then(|id| deployed_numbers.get(&id).copied());
            summary_dto(f, latest_n, deployed_n)
        })
        .collect();

    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    if let Some(s) = q.status.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("status".to_string(), s.to_string()));
    }
    if let Some(l) = q.label.as_deref().filter(|l| !l.is_empty()) {
        filters.push(("label".to_string(), l.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/flows",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────────── GET /flows/{id} ───────────────────────────

/// `GET /api/v1/flows/{id}` — détail + graphe de la dernière version.
pub(super) async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let latest = latest_version(&ctx.db, flow.id).await?;
    let latest_number = latest.as_ref().map(|v| v.version_number).unwrap_or(0);
    let graph: FlowGraph = latest
        .map(|v| serde_json::from_value(v.graph).map_err(|_| Error::InternalServerError))
        .transpose()?
        .unwrap_or_default();
    let deployed_number = deployed_number_of(&ctx.db, flow.deployed_version_id).await?;
    Ok(format::json(flow_dto(flow, graph, latest_number, deployed_number)).into_response())
}

// ─────────────────────────── PATCH /flows/{id} ───────────────────────────

/// `PATCH /api/v1/flows/{id}` — enregistre une **nouvelle version**
/// (append-only). 409 si `expected_version_number` ≠ version courante.
/// Aucun rechargement runtime — l'exécution ne change qu'au deploy.
pub(super) async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(params): Json<pnex_core::UpdateFlow>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "flow-write-forbidden",
            "Owner, admin or member role required to manage flows.",
        ));
    }
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let ring = crate::services::secrets::Keyring::from_config(&ctx.config).ok();
    let (flow, version, graph) = match crate::services::flow::append_version(
        &ctx.db,
        &flow,
        params.expected_version_number,
        &params.graph,
        params.name,
        params.author,
        params.note,
        &graph_writer(&org, ring.as_ref()),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return flow_write_error_result(e),
    };
    Ok(format::json(flow_dto(flow, graph, version, None)).into_response())
}

// ─────────────────────────── DELETE /flows/{id} ───────────────────────────

/// `DELETE /api/v1/flows/{id}` — 204 ; versions supprimées en cascade. Si le
/// flow était déployé, l'artefact est reprojeté sans lui (runtime rechargé)
/// — **best-effort** : la ligne est déjà supprimée, un échec de rechargement
/// ne peut plus défaire le delete, l'API répond 204 et le problème est
/// loggué (jamais de 503 « le flow est parti mais la réponse dit l'inverse »).
pub(super) async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "flow-write-forbidden",
            "Owner, admin or member role required to manage flows.",
        ));
    }
    if find_flow(&ctx.db, &org, id).await?.is_none() {
        return Err(Error::NotFound);
    }
    // Same per-org lock as deploy/stop: a delete racing a deploy of the
    // same org must not interleave its reprojection with the gate.
    let lock = super::lifecycle::lock_org_deploys(&ctx, org.org.id).await?;
    let res = delete_locked(&ctx, &org, id).await;
    lock.release().await;
    res
}

async fn delete_locked(ctx: &AppContext, org: &OrgContext, id: i64) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, org, id).await? else {
        return Err(Error::NotFound);
    };
    let was_deployed = flow.deployed_version_id.is_some();
    // Devices régulés par CE flow : capturés AVANT le delete — la cascade
    // FK efface les lignes de projection et le sync ne peut plus les
    // deviner ; ce sont eux qui recevront le clear (`[]`).
    let cast_targets: Vec<i64> = crate::models::_entities::regulator_configs::Entity::find()
        .filter(crate::models::_entities::regulator_configs::Column::FlowId.eq(flow.id))
        .all(&ctx.db)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.device_registry_id)
        .collect();
    // Clé d'ack du retrait : méta du flow supprimé (le runtime émettra
    // `flow_stopped` sur lui) — capturée AVANT la cascade qui efface sa
    // version déployée (piège de la meta vide, même école que stop).
    // A stopped flow keeps its deployed version but is not running: the
    // runtime never emits `flow_stopped` for it, so waiting for that ack
    // only burned the reload timeout on every delete.
    let running = flow.status == pnex_core::FLOW_STATUS_DEPLOYED;
    let ack_meta = if was_deployed && running {
        deployed_meta(&ctx.db, &flow).await?
    } else {
        None
    };
    // D42 : purge symétrique de la couche d'organisation avant le delete.
    crate::services::resources::purge_for(
        &ctx.db,
        org.org.id,
        pnex_core::resources::KIND_FLOW,
        &flow.id.to_string(),
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    // Usages + dedicated secrets of the flow (lot S5).
    if let Err(e) = crate::services::secrets::flow::release(&ctx.db, org.org.id, flow.id).await {
        tracing::error!(flow_id = flow.id, error = %e, "flow secrets release failed");
        return Err(Error::InternalServerError);
    }
    flows::Entity::delete_by_id(flow.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if was_deployed {
        if let Err(e) = reproject_and_cast_extra(ctx, org.org.id, &cast_targets, ack_meta).await {
            tracing::error!(
                flow_id = flow.id,
                "flow supprimé mais rechargement du runtime en échec : {e}"
            );
        }
    }
    tracing::info!(flow_id = flow.id, "flow supprimé");
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── GET /flows/{id}/versions ───────────────────────────

/// `GET /api/v1/flows/{id}/versions` — historique append-only, paginé (D14),
/// du plus récent au plus ancien.
pub(super) async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Query(q): Query<VersionsQuery>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    // COUNT + one page in SQL, without the (large) graph column.
    let base = flow_versions::Entity::find().filter(flow_versions::Column::FlowId.eq(flow.id));
    let count = sea_orm::PaginatorTrait::count(base.clone(), &ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)? as i64;
    type Row = (
        i64,
        i64,
        Option<String>,
        Option<String>,
        sea_orm::prelude::DateTimeWithTimeZone,
    );
    let rows: Vec<Row> = base
        .select_only()
        .column(flow_versions::Column::Id)
        .column(flow_versions::Column::VersionNumber)
        .column(flow_versions::Column::Author)
        .column(flow_versions::Column::Note)
        .column(flow_versions::Column::CreatedAt)
        .order_by_desc(flow_versions::Column::VersionNumber)
        .offset(page.offset as u64)
        .limit(page.limit as u64)
        .into_tuple()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<pnex_core::FlowVersionSummary> = rows
        .into_iter()
        .map(
            |(vid, version_number, author, note, created_at)| pnex_core::FlowVersionSummary {
                id: vid,
                version_number,
                author,
                note,
                deployed: flow.deployed_version_id == Some(vid),
                created_at: created_at.to_rfc3339(),
            },
        )
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/flows/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct VersionsQuery {
    limit: Option<String>,
    offset: Option<String>,
}

// ─────────────────────────── GET /flows/{id}/versions/{n} ───────────────────────────

/// `GET /api/v1/flows/{id}/versions/{n}` — graphe d'une version précise
/// (rollback ou audit).
pub(super) async fn version_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, n)): Path<(i64, i64)>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .filter(flow_versions::Column::VersionNumber.eq(n))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let graph: FlowGraph =
        serde_json::from_value(version.graph.clone()).map_err(|_| Error::InternalServerError)?;
    Ok(format::json(pnex_core::FlowVersionDetail {
        id: version.id,
        version_number: version.version_number,
        author: version.author,
        note: version.note,
        deployed: flow.deployed_version_id == Some(version.id),
        created_at: version.created_at.to_rfc3339(),
        graph,
    })
    .into_response())
}
