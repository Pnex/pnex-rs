use super::*;

/// Numéro de version d'une ligne `flow_versions` (→ `deployed_version_id`).
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
    // Refs de fonctions non résolues (fonction/version supprimée malgré le
    // delete-guard) : les nœuds concernés partent en entrées `comment`
    // no-op — on warn et on poursuit (chemin self-healing/retrait : jamais
    // de blocage d'un stop par un objet d'un AUTRE flow).
    if !fn_violations.is_empty() {
        tracing::warn!(
            count = fn_violations.len(),
            "reprojection : références de fonctions non résolues (nœuds dégradés no-op)"
        );
    }
    if !skipped_tabs.is_empty() {
        tracing::warn!(
            tabs = skipped_tabs.len(),
            "reprojection : onglets au graphe illisible isolés (non bloquant)"
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

/// Méta d'artefact du flow d'intérêt pour un **retrait** (stop, delete,
/// pin→mode) : la clé d'ack `flow_stopped` doit être le flow retiré —
/// construite ici depuis sa version déployée AVANT toute suppression (la
/// cascade efface `flow_versions` et le sync ne peut plus la deviner).
/// `None` = pas de version déployée (pas de rechargement à acquitter).
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

/// Reprojection de l'artefact complet avec **override optionnel** : la
/// version candidate d'un flow (éventuellement encore `draft` **ou
/// `stopped`** — Start) remplace/ajoute sa ligne dans l'artefact, et avec
/// **exclusion optionnelle** : un flow retiré du lot (Stop) — la DB n'est
/// pas écrite (le marquage n'intervient qu'après un deploy acquitté).
/// Retourne (artefact, méta du flow d'intérêt : l'override, sinon le
/// premier flow déployé).
///
/// L'org O2 est résolue en lecture seule (`provisioned_credentials`), avec
/// cache par org du lot.
///
/// **Snapshot notify (D50)** : les nœuds `pnex-notify` ne portent que des
/// références (`channel_ids`/`template_id`) — chaque projection est ici
/// complétée par le snapshot résolu (`pnex_notify_channels` **enabled
/// only** / `pnex_notify_template`), rendant le nœud autonome au send.
/// Les références périmées (canal absent/désactivé, template absent) sont
/// retournées en `StaleNotifyNode` (toast de deploy, **jamais bloquant** :
/// le nœud dégradé warn + passthrough).
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
    // Onglets déjà déployés au graphe illisible (kind de nœud supprimé,
    // schéma daté…) : isolés hors de l'artefact, deploy **non bloqué** —
    // jamais un 500 qui refuse tout deploy tant que le tab pourrit en DB.
    Vec<serde_json::Value>,
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
            continue; // remplacé par la version candidate (ajoutée plus bas)
        }
        pairs.push((f, version));
    }
    if let Some(pair) = override_flow {
        pairs.push(pair);
    }

    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut meta: Option<FlowArtifactMeta> = None;
    let mut skipped_tabs: Vec<serde_json::Value> = Vec::new();
    // ── Résolution batch des fonctions (nœuds PnexFunction) ─────────────
    let (fn_resolver, mut fn_violations) =
        crate::services::functions::resolve_for_projection(db, &pairs).await?;
    // Le flow candidat (override) : un graphe illisible sur LUI est
    // bloquant (le deploy partirait sans lui, acquitté à tort).
    // (`override_ids` = son id, calculé avant le move de `override_flow`.)
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
                    // Candidat du deploy : 400 violations (même forme que
                    // engine_load), le toast de l'éditeur le localise.
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
                    // Tab d'un AUTRE flow : isolé, le deploy poursuit.
                    tracing::error!("reprojection : {message}");
                    skipped_tabs.push(serde_json::json!({
                        "flow_id": f.id,
                        "code": "graph_unreadable",
                        "message": message,
                    }));
                }
                continue;
            }
        };
        // Deploy gate (D101): the candidate flow may not reference a model
        // that is missing or failed its check.
        if override_ids.first() == Some(&f.id) {
            fn_violations.extend(vision_model_violations(db, f.org_id, &graph).await?);
        }
        // Résolution par org (une seule requête par org du lot).
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

    // ── Snapshot notify (D50) : résolution batch sur tout le lot ────────
    let stale = resolve_notify_snapshots(db, &mut entries).await?;

    Ok((
        serde_json::Value::Array(entries),
        meta.unwrap_or_else(FlowArtifactMeta::empty),
        stale,
        fn_violations,
        skipped_tabs,
    ))
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

/// Résout les références des nœuds `pnex-notify` de l'artefact et estampe
/// le snapshot dans chaque entrée. Deux requêtes batch (canaux, templates),
/// filtrage par org **en code** (école du cache O2). Retourne les références
/// périmées pour le toast de deploy.
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

    // Union des références de tout le lot → 2 requêtes batch.
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
        // Canaux de CETTE org, trouvés **et** enabled seulement — un canal
        // désactivé/supprimé est simplement absent du snapshot (le nœud
        // dégrade, le toast explique).
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
        // Vars de template (colonne mergée « détectées + déclarées ») :
        // stampées dans le snapshot — le runtime y lit son mode accumulation
        // (une var par ancre nommée). Drift vs le stamp du pick
        // (`template_vars` du nœud) → staleness non bloquante
        // « template_changed » (école channel_disabled/template_missing).
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
            .expect("entrée flows.json objet");
        obj.insert("pnex_notify_channels".into(), serde_json::json!(stamped));
        obj.insert(
            "pnex_notify_template".into(),
            t.unwrap_or_else(|| serde_json::json!({ "subject": null, "body": "", "vars": [] })),
        );
    }
    Ok(stale)
}

/// Mapping `DeployError` → HTTP : `Check` = 400 `{"violations":[...]}` avec
/// l'erreur moteur **réelle** (forme `FlowViolation`, déjà rendue telle
/// quelle par l'éditeur), `Runtime` = 503 `flow_runtime`.
pub(super) fn deploy_error_response(e: flow_supervisor::DeployError) -> Response {
    match e {
        flow_supervisor::DeployError::Check(checks) => {
            let violations: Vec<serde_json::Value> = checks
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "node_id": null,
                        "code": "engine_load",
                        "message": format!("Flow #{} : {}", c.flow_id, c.message),
                    })
                })
                .collect();
            (
                StatusCode::BAD_REQUEST,
                format::json(serde_json::json!({ "violations": violations })),
            )
                .into_response()
        }
        other => flow_runtime_503(other).into_response(),
    }
}

/// 503 `flow_runtime` (pannes d'infra : superviseur coupé, acquittement
/// absent…) — description = erreur réelle.
pub(super) fn flow_runtime_503(e: flow_supervisor::DeployError) -> loco_rs::Error {
    let description = match e {
        flow_supervisor::DeployError::Check(checks) => checks
            .iter()
            .map(|c| format!("flow #{} : {}", c.flow_id, c.message))
            .collect::<Vec<_>>()
            .join(" ; "),
        flow_supervisor::DeployError::Runtime(s) => s,
    };
    Error::CustomError(
        StatusCode::SERVICE_UNAVAILABLE,
        loco_rs::controller::ErrorDetail::new("flow_runtime", description),
    )
}

/// Point d'entrée unique de tout changement du set déployé (deploy,
/// delete, stop_flows_reading_pin) : **d'abord** le sync/cast des cartes de
/// régulation (matérialisation `regulator_configs` + push `ControlConfig`,
/// indépendant du runtime — jamais de 503), **ensuite** la reprojection de
/// l'artefact (503 conservé pour le runtime ETL uniquement). Ordre garanti :
/// un moteur coupé laisse la base et les devices cohérents.
/// Variante avec devices régulés **additionnels** à caster et clé d'ack du
/// retrait (delete, pin→mode — cf. [`reproject_and_signal_with_ack`]) : le
/// delete cascade les lignes de projection avant le sync, qui ne peut donc
/// plus deviner quels devices attendent un clear.
pub(super) async fn reproject_and_cast_extra(
    ctx: &AppContext,
    org_id: i64,
    extra_devices: &[i64],
    ack_meta: Option<FlowArtifactMeta>,
) -> Result<()> {
    if let Err(e) =
        crate::services::regulator::sync_and_cast_with(&ctx.db, org_id, extra_devices).await
    {
        // Best-effort : un échec de cast ne bloque jamais la projection ETL.
        tracing::error!("sync des régulations échoué (projection poursuivie) : {e}");
    }
    reproject_and_signal_with_ack(ctx, org_id, ack_meta).await
}
