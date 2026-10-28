use super::*;

/// Version number of a `flow_versions` row (→ `deployed_version_id`).
pub(super) async fn deployed_number_of(
    db: &DatabaseConnection,
    deployed_version_id: Option<i64>,
) -> Result<Option<i64>> {
    let Some(id) = deployed_version_id else {
        return Ok(None);
    };
    Ok(flow_versions::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|v| v.version_number))
}

/// The projected flows of `org_id` changed in the database (O2 org
/// provisioned, …): its owner worker reprojects them at its next tick
/// (D106 — asynchronous, never blocks the caller).
pub(crate) async fn reproject_and_signal(db: &DatabaseConnection, org_id: i64) -> Result<()> {
    crate::services::flow_cluster::org_changed(db, org_id).await;
    Ok(())
}

/// Reprojects the flows of **one org** from the database and hands them
/// to its owner worker, waiting for the acknowledgement keyed on
/// `ack_meta` (removal paths: delete, pin→mode — the runtime emits
/// `flow_stopped` on the removed flow, so the ack must be keyed on IT:
/// without override the meta would be empty and the ack would time out).
/// The database is already marked by these callers: the revision bump
/// makes the owner's next reconcile converge on the same fragment.
pub(crate) async fn reproject_and_signal_with_ack(
    ctx: &AppContext,
    org_id: i64,
    ack_meta: Option<FlowArtifactMeta>,
) -> Result<()> {
    let (artifact, _, _, fn_violations, skipped_tabs) =
        reproject_candidate(&ctx.db, org_id, None, None).await?;
    // Unresolved function refs (function/version deleted despite the
    // delete-guard): the affected nodes become no-op `comment`
    // entries — we warn and go on (self-healing/removal path: never
    // block a stop because of an object of ANOTHER flow).
    if !fn_violations.is_empty() {
        tracing::warn!(
            count = fn_violations.len(),
            "reprojection: unresolved function references (no-op degraded nodes)"
        );
    }
    if !skipped_tabs.is_empty() {
        tracing::warn!(
            tabs = skipped_tabs.len(),
            "reprojection: tabs with an unreadable graph isolated (non-blocking)"
        );
    }
    let res = crate::services::flow_cluster::apply_org(
        &ctx.db,
        &ctx.config,
        crate::services::flow_cluster::ApplyOrg {
            org_id,
            fragment: artifact,
            ack: ack_meta,
            enforce_flow: None,
        },
    )
    .await;
    crate::services::flow_cluster::org_changed(&ctx.db, org_id).await;
    res.map_err(flow_runtime_503)
}

/// Deployed flows of `org_id` paired with their deployed version, ordered
/// by flow id. Two queries whatever the number of flows (flows, then
/// versions `id IN (...)`) — this runs on every deploy gate and projection.
/// Flows without a (still existing) deployed version are left out.
pub(crate) async fn deployed_flows_with_versions(
    db: &impl sea_orm::ConnectionTrait,
    org_id: i64,
) -> Result<Vec<(flows::Model, flow_versions::Model)>> {
    let deployed = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .order_by_asc(flows::Column::Id)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let version_ids: Vec<i64> = deployed
        .iter()
        .filter_map(|f| f.deployed_version_id)
        .collect();
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut versions: std::collections::HashMap<i64, flow_versions::Model> =
        flow_versions::Entity::find()
            .filter(flow_versions::Column::Id.is_in(version_ids))
            .all(db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|v| (v.id, v))
            .collect();
    Ok(deployed
        .into_iter()
        .filter_map(|f| {
            let v = versions.remove(&f.deployed_version_id?)?;
            Some((f, v))
        })
        .collect())
}

/// Projected entries of every deployed flow of `org_id` (the fragment a
/// worker runs for that org). Unreadable tabs and stale references are
/// logged and isolated, never fatal.
pub(crate) async fn project_org(db: &DatabaseConnection, org_id: i64) -> Result<serde_json::Value> {
    let (artifact, _, _, fn_violations, skipped_tabs) =
        reproject_candidate(db, org_id, None, None).await?;
    if !fn_violations.is_empty() || !skipped_tabs.is_empty() {
        tracing::warn!(
            org_id,
            unresolved = fn_violations.len(),
            skipped = skipped_tabs.len(),
            "org projection: degraded entries isolated"
        );
    }
    Ok(artifact)
}

/// Artifact meta of the flow of interest for a **removal** (stop, delete,
/// pin→mode): the `flow_stopped` ack key must be the removed flow —
/// built here from its deployed version BEFORE any deletion (the
/// cascade erases `flow_versions` and the sync can no longer guess it).
/// `None` = no deployed version (no reload to acknowledge).
pub(super) async fn deployed_meta(
    db: &DatabaseConnection,
    flow: &flows::Model,
) -> Result<Option<FlowArtifactMeta>> {
    let Some(version_number) = deployed_number_of(db, flow.deployed_version_id).await? else {
        return Ok(None);
    };
    let o2_org = openobserve::provisioned_credentials(db, flow.org_id)
        .await
        .unwrap_or(None)
        .map(|c| c.o2_org)
        .unwrap_or_default();
    Ok(Some(FlowArtifactMeta {
        flow_id: flow.id,
        version_number,
        org_id: flow.org_id,
        o2_org,
    }))
}

/// Reprojection of the full artifact with an **optional override**: the
/// candidate version of a flow (possibly still `draft` **or
/// `stopped`** — Start) replaces/adds its line in the artifact, and with an
/// **optional exclusion**: a flow removed from the batch (Stop) — the DB is
/// not written (the marking only happens after an acknowledged deploy).
/// Returns (artifact, meta of the flow of interest: the override, otherwise the
/// first deployed flow).
///
/// The O2 org is resolved read-only (`provisioned_credentials`), with a
/// per-org cache over the batch.
///
/// **Notify snapshot (D50)**: `pnex-notify` nodes only carry
/// references (`channel_ids`/`template_id`) — each projection is completed
/// here with the resolved snapshot (`pnex_notify_channels` **enabled
/// only** / `pnex_notify_template`), making the node self-contained at send.
/// Stale references (missing/disabled channel, missing template) are
/// returned as `StaleNotifyNode` (deploy toast, **never blocking**:
/// the degraded node warns + passes through).
pub(crate) async fn reproject_candidate(
    db: &DatabaseConnection,
    org_id: i64,
    override_flow: Option<(flows::Model, flow_versions::Model)>,
    exclude: Option<i64>,
) -> Result<(
    serde_json::Value,
    FlowArtifactMeta,
    Vec<pnex_core::StaleNotifyNode>,
    Vec<pnex_core::FlowViolation>,
    // Already deployed tabs with an unreadable graph (removed node kind,
    // outdated schema…): isolated out of the artifact, deploy **not blocked** —
    // never a 500 that refuses every deploy while the tab rots in the DB.
    Vec<SkippedTab>,
)> {
    // One org only (D106): the unit a worker runs — never the instance.
    let deployed_flows = deployed_flows_with_versions(db, org_id).await?;

    let override_ids: Vec<i64> = override_flow
        .as_ref()
        .map(|(f, _)| vec![f.id])
        .unwrap_or_default();

    let mut pairs: Vec<(flows::Model, flow_versions::Model)> = Vec::new();
    for (f, version) in deployed_flows {
        if Some(f.id) == exclude {
            continue; // Stop : le tab quitte l'artefact (ack flow_stopped)
        }
        if override_ids.contains(&f.id) {
            continue; // replaced by the candidate version (added below)
        }
        pairs.push((f, version));
    }
    if let Some(pair) = override_flow {
        pairs.push(pair);
    }

    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut meta: Option<FlowArtifactMeta> = None;
    let mut skipped_tabs: Vec<SkippedTab> = Vec::new();
    // ── Batch resolution of functions (PnexFunction nodes) ─────────────
    let (fn_resolver, mut fn_violations) =
        crate::services::functions::resolve_for_projection(db, &pairs).await?;
    // The candidate flow (override): an unreadable graph on IT is
    // blocking (the deploy would go out without it, wrongly acknowledged).
    // (`override_ids` = its id, computed before `override_flow` is moved.)
    let mut o2_org_cache: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for (f, version) in &pairs {
        let graph: FlowGraph = match serde_json::from_value(version.graph.clone()) {
            Ok(g) => g,
            Err(e) => {
                let message = format!(
                    "Flow #{} v{}: stored graph is unreadable ({e}) — rebuild the flow with the current node types.",
                    f.id, version.version_number
                );
                if override_ids.first() == Some(&f.id) {
                    // Deploy candidate: 400 violations (same shape as
                    // engine_load), the editor's toast locates it.
                    fn_violations.push(pnex_core::FlowViolation::with_args(
                        None,
                        "graph_unreadable",
                        message.clone(),
                        serde_json::json!({
                            "flow_id": f.id.to_string(),
                            "version": version.version_number.to_string(),
                            "error": e.to_string(),
                        }),
                    ));
                } else {
                    // Tab of ANOTHER flow: isolated, the deploy goes on.
                    tracing::error!("reprojection : {message}");
                    skipped_tabs.push(SkippedTab {
                        flow_id: f.id,
                        code: "graph_unreadable".to_string(),
                        message,
                    });
                }
                continue;
            }
        };
        // Deploy gate (D101): the candidate flow may not reference a model
        // that is missing or failed its check.
        if override_ids.first() == Some(&f.id) {
            fn_violations.extend(vision_model_violations(db, f.org_id, &graph).await?);
        }
        // Per-org resolution (a single query per org of the batch).
        let o2_org = match o2_org_cache.get(&f.org_id) {
            Some(cached) => cached.clone(),
            None => {
                let resolved = openobserve::provisioned_credentials(db, f.org_id)
                    .await
                    .unwrap_or(None)
                    .map(|c| c.o2_org)
                    .unwrap_or_default();
                o2_org_cache.insert(f.org_id, resolved.clone());
                resolved
            }
        };
        let m = FlowArtifactMeta {
            flow_id: f.id,
            version_number: version.version_number,
            org_id: f.org_id,
            o2_org,
        };
        if meta.is_none() {
            meta = Some(m.clone());
        }
        if let serde_json::Value::Array(nodes) =
            pnex_core::to_red_flows_json_with(&graph, &m, &fn_resolver)
        {
            entries.extend(nodes);
        }
    }

    // ── Notify snapshot (D50): batch resolution over the whole batch ────────
    let stale = resolve_notify_snapshots(db, &mut entries).await?;
    stamp_taxonomies(db, &mut entries).await?;
    stamp_pack_settings(db, &mut entries).await?;

    Ok((
        serde_json::Value::Array(entries),
        meta.unwrap_or_else(FlowArtifactMeta::empty),
        stale,
        fn_violations,
        skipped_tabs,
    ))
}

/// Stamps the topics of the pinned taxonomy version into each
/// `pnex-topic-classify` entry (D168), plus `taxonomy_version` =
/// `<name>@<version>`. A version that is not one of the entry's org (deleted
/// since the deploy) stamps no topic: the node then matches nothing.
async fn stamp_taxonomies(
    db: &DatabaseConnection,
    entries: &mut [serde_json::Value],
) -> Result<()> {
    use crate::services::media_ingest::taxonomies;
    for entry in entries.iter_mut() {
        if entry.get("type").and_then(|t| t.as_str()) != Some("pnex-topic-classify") {
            continue;
        }
        let org_id = entry
            .get("pnex_org_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let version = entry.get("version").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let id = entry
            .get("taxonomy_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s.trim()).ok());
        let mut topics = serde_json::json!([]);
        let mut label = String::new();
        if let Some(id) = id {
            let v = taxonomies::find_version(db, org_id, id, version)
                .await
                .map_err(|_| Error::InternalServerError)?;
            let t = taxonomies::find(db, org_id, id).await.ok();
            if let (Some(v), Some(t)) = (v, t) {
                topics = serde_json::to_value(taxonomies::topics_of(&v)).unwrap_or_default();
                label = pnex_core::taxonomy::version_label(&t.name, version);
            }
        }
        entry["topics"] = topics;
        entry["taxonomy_version"] = label.into();
    }
    Ok(())
}

/// Stamps the org's pack settings in force (ontology.md D368) into each
/// Starlark entry: what `pack_setting` reads. A change applies at the next
/// deploy.
async fn stamp_pack_settings(
    db: &DatabaseConnection,
    entries: &mut [serde_json::Value],
) -> Result<()> {
    let mut by_org: std::collections::HashMap<i64, serde_json::Value> = Default::default();
    for entry in entries.iter_mut() {
        if entry.get("type").and_then(|t| t.as_str()) != Some("pnex-starlark") {
            continue;
        }
        let org_id = entry
            .get("pnex_org_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if let std::collections::hash_map::Entry::Vacant(e) = by_org.entry(org_id) {
            let settings = crate::services::ontology::packs::org_settings(db, org_id)
                .await
                .map_err(|_| Error::InternalServerError)?;
            e.insert(serde_json::Value::Object(settings));
        }
        entry["settings"] = by_org[&org_id].clone();
    }
    Ok(())
}

/// `vision-detect` nodes whose model is unknown to the org or did not pass
/// its D100 check (only a `valid` model deploys).
async fn vision_model_violations(
    db: &DatabaseConnection,
    org_id: i64,
    graph: &FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>> {
    use crate::models::_entities::ml_models;
    let mut out = Vec::new();
    for node in &graph.nodes {
        let pnex_core::FlowNodeKind::VisionDetect { config } = &node.kind else {
            continue;
        };
        let Ok(id) = uuid::Uuid::parse_str(config.model_id.trim()) else {
            continue; // empty/garbled id: the node check reports it
        };
        let model = ml_models::Entity::find_by_id(id)
            .filter(ml_models::Column::OrgId.eq(org_id))
            .filter(ml_models::Column::Task.eq(crate::services::vision::VISION_TASK))
            .one(db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        match model {
            None => out.push(pnex_core::FlowViolation::with_args(
                Some(&node.id),
                "vision_model_unknown",
                "The selected vision model no longer exists.",
                serde_json::json!({}),
            )),
            Some(m) if m.check_status != pnex_core::vision::ModelCheckStatus::Valid.wire() => {
                let detail = m.check_error.clone().unwrap_or_default();
                out.push(pnex_core::FlowViolation::with_args(
                    Some(&node.id),
                    "vision_model_invalid",
                    format!("The vision model {} failed its check: {detail}", m.name),
                    serde_json::json!({ "model": m.name, "detail": detail }),
                ));
            }
            Some(_) => {}
        }
    }
    Ok(out)
}

/// Resolves the references of the artifact's `pnex-notify` nodes and stamps
/// the snapshot into each entry. Two batch queries (channels, templates),
/// org filtering **in code** (O2 cache school). Returns the stale references
/// for the deploy toast.
async fn resolve_notify_snapshots(
    db: &DatabaseConnection,
    entries: &mut [serde_json::Value],
) -> Result<Vec<pnex_core::StaleNotifyNode>> {
    use crate::models::_entities::{notify_channels, notify_templates};

    struct NotifyRefs {
        org_id: i64,
        node_id: String,
        channel_ids: Vec<Uuid>,
        template_id: Uuid,
    }
    let mut notify_nodes: Vec<(usize, NotifyRefs)> = Vec::new();
    for (idx, entry) in entries.iter().enumerate() {
        if entry.get("type").and_then(|t| t.as_str()) != Some("pnex-notify") {
            continue;
        }
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let channel_ids: Vec<Uuid> = obj
            .get("channel_ids")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let template_id: Uuid = obj
            .get("template_id")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(Uuid::nil());
        notify_nodes.push((
            idx,
            NotifyRefs {
                org_id: obj.get("pnex_org_id").and_then(|v| v.as_i64()).unwrap_or(0),
                node_id: obj
                    .get("pnex_node_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                channel_ids,
                template_id,
            },
        ));
    }
    if notify_nodes.is_empty() {
        return Ok(vec![]);
    }

    // Union of the references of the whole batch → 2 batch queries.
    let all_channels: Vec<Uuid> = notify_nodes
        .iter()
        .flat_map(|(_, r)| r.channel_ids.iter().copied())
        .collect();
    let all_templates: Vec<Uuid> = notify_nodes.iter().map(|(_, r)| r.template_id).collect();
    let channel_rows = notify_channels::Entity::find()
        .filter(notify_channels::Column::Id.is_in(all_channels))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let template_rows = notify_templates::Entity::find()
        .filter(notify_templates::Column::Id.is_in(all_templates))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    let mut stale = Vec::new();
    for (idx, r) in &notify_nodes {
        // Channels of THIS org, found **and** enabled only — a
        // disabled/deleted channel is simply absent from the snapshot (the node
        // degrades, the toast explains).
        let mut stamped = Vec::new();
        for cid in &r.channel_ids {
            match channel_rows
                .iter()
                .find(|c| c.id == *cid && c.org_id == r.org_id)
            {
                Some(c) if c.enabled => stamped.push(serde_json::json!({
                    "id": c.id, "kind": c.kind, "name": c.name, "config": c.config,
                })),
                Some(_) => stale.push(pnex_core::StaleNotifyNode {
                    flow_id: entries[*idx]
                        .get("pnex_flow_id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    node_id: r.node_id.clone(),
                    reason: "channel_disabled".into(),
                }),
                None => stale.push(pnex_core::StaleNotifyNode {
                    flow_id: entries[*idx]
                        .get("pnex_flow_id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    node_id: r.node_id.clone(),
                    reason: "channel_missing".into(),
                }),
            }
        }
        let template = template_rows
            .iter()
            .find(|t| t.id == r.template_id && t.org_id == r.org_id);
        if template.is_none() {
            stale.push(pnex_core::StaleNotifyNode {
                flow_id: entries[*idx]
                    .get("pnex_flow_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                node_id: r.node_id.clone(),
                reason: "template_missing".into(),
            });
        }
        // Template vars (merged "detected + declared" column):
        // stamped into the snapshot — the runtime reads its accumulation mode
        // there (one var per named anchor). Drift vs the pick's stamp
        // (the node's `template_vars`) → non-blocking staleness
        // "template_changed" (channel_disabled/template_missing school).
        let row_vars: Vec<String> = template
            .and_then(|t| {
                serde_json::from_value::<Vec<pnex_core::TemplateVar>>(t.vars.clone()).ok()
            })
            .map(|vars| vars.into_iter().map(|v| v.name).collect())
            .unwrap_or_default();
        if template.is_some() {
            let stamped_vars: Vec<String> = entries[*idx]
                .get("template_vars")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let same = stamped_vars.len() == row_vars.len()
                && row_vars.iter().zip(&stamped_vars).all(|(a, b)| a == b);
            if !same {
                stale.push(pnex_core::StaleNotifyNode {
                    flow_id: entries[*idx]
                        .get("pnex_flow_id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    node_id: r.node_id.clone(),
                    reason: "template_changed".into(),
                });
            }
        }
        let t = template
            .map(|t| serde_json::json!({ "subject": t.subject, "body": t.body, "vars": row_vars }));
        let obj = entries[*idx]
            .as_object_mut()
            .expect("flows.json entry is an object");
        obj.insert("pnex_notify_channels".into(), serde_json::json!(stamped));
        obj.insert(
            "pnex_notify_template".into(),
            t.unwrap_or_else(|| serde_json::json!({ "subject": null, "body": "", "vars": [] })),
        );
    }
    Ok(stale)
}

/// `DeployError` to HTTP: `Check` = 400 `flow-invalid` carrying the real
/// engine error as `FlowViolation`s, `Runtime` = 503 `flow-runtime`.
pub(super) fn deploy_error_response(e: flow_supervisor::DeployError) -> Response {
    match e {
        flow_supervisor::DeployError::Check(checks) => {
            let violations: Vec<pnex_core::FlowViolation> = checks
                .iter()
                .map(|c| {
                    pnex_core::FlowViolation::new(
                        None,
                        "engine_load",
                        format!("Flow #{} : {}", c.flow_id, c.message),
                    )
                })
                .collect();
            crate::controllers::violations_error(pnex_core::err_codes::FLOW_INVALID, violations)
        }
        other => flow_runtime_503(other).into_response(),
    }
}

/// 503 `flow-runtime` (infrastructure failure: supervisor down, missing
/// acknowledgement...); `args.detail` = the real error, verbatim.
pub(super) fn flow_runtime_503(e: flow_supervisor::DeployError) -> loco_rs::Error {
    let description = match e {
        flow_supervisor::DeployError::Check(checks) => checks
            .iter()
            .map(|c| format!("flow #{} : {}", c.flow_id, c.message))
            .collect::<Vec<_>>()
            .join(" ; "),
        flow_supervisor::DeployError::Runtime(s) => s,
    };
    crate::controllers::coded_error(
        StatusCode::SERVICE_UNAVAILABLE,
        pnex_core::err_codes::FLOW_RUNTIME,
        "The flow runtime is unavailable.",
        Some(serde_json::json!({ "detail": description })),
    )
}

/// Single entry point of every change of the deployed set (deploy,
/// delete, stop_flows_reading_pin): **first** the sync/cast of the
/// regulation cards (`regulator_configs` materialization + `ControlConfig` push,
/// independent of the runtime — never a 503), **then** the reprojection of
/// the artifact (503 kept for the ETL runtime only). Guaranteed order:
/// a stopped engine leaves the database and the devices consistent.
/// Variant with **additional** regulated devices to cast and the removal's
/// ack key (delete, pin→mode — see [`reproject_and_signal_with_ack`]): the
/// delete cascades the projection rows before the sync, which therefore can
/// no longer guess which devices expect a clear.
pub(super) async fn reproject_and_cast_extra(
    ctx: &AppContext,
    org_id: i64,
    extra_devices: &[i64],
    ack_meta: Option<FlowArtifactMeta>,
) -> Result<()> {
    if let Err(e) =
        crate::services::regulator::sync_and_cast_with(&ctx.db, org_id, extra_devices).await
    {
        // Best-effort: a cast failure never blocks the ETL projection.
        tracing::error!("regulation sync failed (projection continued): {e}");
    }
    reproject_and_signal_with_ack(ctx, org_id, ack_meta).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::media_ingest::taxonomies;
    use sea_orm::ConnectionTrait;

    /// The pinned version of the entry's org is stamped; the same id seen
    /// from another org stamps nothing (R1).
    #[tokio::test]
    async fn taxonomy_topics_are_stamped_for_the_entry_org_only() {
        let db = crate::services::db_lock::migrated_test_db().await;
        let (own, other) = (987_001_i64, 987_002_i64);
        db.execute_unprepared(&format!(
            "DELETE FROM organizations WHERE id IN ({own}, {other}); \
             INSERT INTO organizations (id, name) VALUES ({own}, 'tax-own'), ({other}, 'tax-other')"
        ))
        .await
        .expect("orgs");
        let input = pnex_core::taxonomy::TaxonomyInput {
            name: Some("News".into()),
            description: None,
        };
        let t = taxonomies::create(&db, own, None, &input)
            .await
            .expect("taxonomy");
        let version = pnex_core::taxonomy::TaxonomyVersionInput {
            topics: vec![pnex_core::taxonomy::Topic {
                id: "economy".into(),
                label: "Economy".into(),
                definition: String::new(),
                keywords: vec!["inflation".into()],
            }],
            note: String::new(),
            expected_version: 0,
        };
        taxonomies::add_version(&db, own, t.id, None, &version)
            .await
            .expect("version");
        let entry = |org: i64, v: i32| {
            serde_json::json!({
                "type": "pnex-topic-classify", "taxonomy_id": t.id.to_string(),
                "version": v, "pnex_org_id": org,
            })
        };
        let mut entries = vec![entry(own, 1), entry(other, 1), entry(own, 2)];
        stamp_taxonomies(&db, &mut entries).await.expect("stamp");
        assert_eq!(entries[0]["topics"][0]["id"], "economy");
        assert_eq!(entries[0]["taxonomy_version"], "News@1");
        for e in &entries[1..] {
            assert_eq!(e["topics"], serde_json::json!([]), "{e}");
            assert_eq!(e["taxonomy_version"], "", "{e}");
        }
        db.execute_unprepared(&format!(
            "DELETE FROM organizations WHERE id IN ({own}, {other})"
        ))
        .await
        .expect("cleanup");
    }

    /// Starlark entries get the settings of their own org's installed packs
    /// (the org's value, else the pack's default); other entries get none.
    #[tokio::test]
    async fn pack_settings_are_stamped_for_the_entry_org_only() {
        let db = crate::services::db_lock::migrated_test_db().await;
        let (own, other) = (987_011_i64, 987_012_i64);
        db.execute_unprepared(&format!(
            "DELETE FROM organizations WHERE id IN ({own}, {other}); \
             INSERT INTO organizations (id, name) VALUES ({own}, 'set-own'), ({other}, 'set-other'); \
             INSERT INTO packs (org_id, key, version, settings) \
             VALUES ({own}, 'web_presence', '1', '{{\"confirm_threshold\": 90}}')"
        ))
        .await
        .expect("orgs");
        let entry = |kind: &str, org: i64| serde_json::json!({"type": kind, "pnex_org_id": org});
        let mut entries = vec![
            entry("pnex-starlark", own),
            entry("pnex-starlark", other),
            entry("pnex-js", own),
        ];
        stamp_pack_settings(&db, &mut entries).await.expect("stamp");
        assert_eq!(entries[0]["settings"]["web_presence.confirm_threshold"], 90);
        assert!(entries[0]["settings"]["web_presence.propose_threshold"].is_number());
        assert_eq!(entries[1]["settings"], serde_json::json!({}));
        assert!(entries[2].get("settings").is_none());
        db.execute_unprepared(&format!(
            "DELETE FROM organizations WHERE id IN ({own}, {other})"
        ))
        .await
        .expect("cleanup");
    }
}
