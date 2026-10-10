use super::*;

use crate::services::db_lock::{self, TenantLock};

/// Upper bound on the wait for the per-org deploy lock: above the runtime
/// acknowledgement budget of the holder (request timeout + retries).
const ORG_DEPLOY_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Serializes every change of the deployed set of one org across pods
/// (deploy, rollback, start, stop, restart, delete, pin-mode auto-stop):
/// the exclusivity gates (pins, cameras) read the deployed set, the runtime
/// applies the candidate, then the database is marked — without this lock
/// two pods could both pass the gate and deploy two writers of one pin.
/// The lock is a transaction-scoped Postgres advisory lock carried by a
/// dedicated connection; the holder releases it when
/// done, a crashed pod releases it with its connection.
pub(crate) async fn lock_org_deploys(ctx: &AppContext, org_id: i64) -> Result<TenantLock> {
    TenantLock::acquire(
        &ctx.db,
        db_lock::ns::FLOW_DEPLOY,
        org_id,
        ORG_DEPLOY_LOCK_TIMEOUT,
    )
    .await
    .map_err(|e| {
        if db_lock::is_lock_timeout(&e) {
            flow_runtime_503(flow_supervisor::DeployError::Runtime(
                "another deployment of this organization is still in progress".into(),
            ))
        } else {
            tracing::error!(org_id, "org deploy lock failed: {e}");
            Error::InternalServerError
        }
    })
}

// ─────────────────────────── Stop automatique (dépendance pin ↔ flows) ───────────────────────────

/// Dépendances pin ↔ flows (Phase 6 + device read/write 2026-09-19) : un
/// changement de mode d'un pin (in↔out) invalide les usages device des flows
/// déployés — la série O2 n'est plus alimentée, un nœud device-write pousse
/// sur un pin qui n'est plus une sortie. Pour chaque flow **déployé** de
/// l'org dont un nœud `device-read`/`device-write` ou une carte régulation
/// (pin capteur) touche ce (device_id, pin) : dé-déploiement immédiat
/// (status → draft, `deployed_version_id` → NULL — la version publiée reste
/// enregistrée, un redéploiement manuel est possible une fois la
/// configuration cohérente) puis reprojection unique de l'artefact.
///
/// Retourne les impacts `(flow_id, nom)` pour l'UI ; appelée par le
/// contrôleur pins (`set_mode`) **avant** le push device (la base est la
/// source de vérité, le stop doit refléter la base même si le device est
/// hors ligne).
pub(crate) async fn stop_flows_reading_pin(
    ctx: &AppContext,
    org_id: i64,
    device_id: &str,
    pin_label: &str,
) -> Result<Vec<(i64, String)>> {
    let lock = lock_org_deploys(ctx, org_id).await?;
    let res = stop_flows_reading_pin_locked(ctx, org_id, device_id, pin_label).await;
    lock.release().await;
    res
}

async fn stop_flows_reading_pin_locked(
    ctx: &AppContext,
    org_id: i64,
    device_id: &str,
    pin_label: &str,
) -> Result<Vec<(i64, String)>> {
    let deployed = deployed_flows_with_versions(&ctx.db, org_id).await?;

    let mut impacts: Vec<(i64, String)> = Vec::new();
    // Clé d'ack du rechargement : méta du **premier** flow dé-déployé (le
    // runtime émettra `flow_stopped` sur lui — piège de la meta vide, même
    // école que stop/delete).
    let mut ack_meta: Option<FlowArtifactMeta> = None;
    for (flow, version) in deployed {
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        // Read (device-read + pins capteur des cartes régulation) ET write
        // (device-write) : un flip de mode casse l'un comme l'autre.
        let touches = pnex_core::device_pin_refs_of(&graph, device_id)
            .iter()
            .any(|r| {
                pnex_core::normalize_measurement_name(&r.pin)
                    == pnex_core::normalize_measurement_name(pin_label)
            });
        if !touches {
            continue;
        }
        if ack_meta.is_none() {
            ack_meta = deployed_meta(&ctx.db, &flow).await?;
        }
        let mut active: flows::ActiveModel = flow.into();
        active.status = Set(pnex_core::FLOW_STATUS_DRAFT.to_string());
        active.deployed_version_id = Set(None);
        let stopped = active
            .update(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        tracing::warn!(
            flow_id = stopped.id,
            device_id,
            pin = pin_label,
            "flow dé-déployé automatiquement : le pin vient de changer de mode (in↔out)"
        );
        impacts.push((stopped.id, stopped.name));
    }

    // Un seul rechargement du runtime même si plusieurs flows ont été
    // arrêtés — l'artefact est reprojeté sans eux (et les cartes de
    // régulation qui lisaient ce pin sont castées `[]` : les ESP remettent
    // leurs sorties en safe — on arrête de réguler, on n'écrit jamais un pin).
    if !impacts.is_empty() {
        reproject_and_cast_extra(ctx, org_id, &[], ack_meta).await?;
    }
    Ok(impacts)
}

/// Dry-run du scan de `stop_flows_reading_pin` : les flows déployés de l'org
/// dont le graphe touche (device_id, pin) — sans mutation. Alimente la garde
/// 409 « arrêtez les flows avant de changer la config » (le front liste les
/// flows et propose l'arrêt confirmé).
pub(crate) async fn flows_impacted_by_pin(
    ctx: &AppContext,
    org_id: i64,
    device_id: &str,
    pin_label: &str,
) -> Result<Vec<(i64, String)>> {
    let deployed = deployed_flows_with_versions(&ctx.db, org_id).await?;
    let mut impacted = Vec::new();
    for (flow, version) in deployed {
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        let touches = pnex_core::device_pin_refs_of(&graph, device_id)
            .iter()
            .any(|r| {
                pnex_core::normalize_measurement_name(&r.pin)
                    == pnex_core::normalize_measurement_name(pin_label)
            });
        if touches {
            impacted.push((flow.id, flow.name));
        }
    }
    Ok(impacted)
}

/// Cross-flow output-pin exclusivity (deploy gate): one violation per
/// (device, pin) the candidate writes that a DIFFERENT deployed flow of the
/// org writes too. The candidate's own claims are excluded, so redeploy /
/// rollback / start of the owner always pass. Runs only from `deploy_version`
/// — stop/delete reprojections can never be blocked by this rule, and
/// grandfathered conflicts between already-deployed flows keep running until
/// one of them is redeployed (self-healing).
pub(crate) async fn pin_write_conflicts_for_deploy(
    ctx: &AppContext,
    org_id: i64,
    candidate_flow_id: i64,
    candidate_graph: &FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>> {
    let claims = pnex_core::device_write_pin_refs_of(candidate_graph);
    if claims.is_empty() {
        return Ok(Vec::new());
    }
    let deployed = deployed_flows_with_versions(&ctx.db, org_id).await?;
    // (device, pin) -> owning flow name (first seen wins; multi-owners are
    // rare grandfathered states, every conflicting owner is reported).
    let mut owners: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();
    for (flow, version) in deployed {
        if flow.id == candidate_flow_id {
            continue;
        }
        // A third party's unreadable graph must not block this deploy: that
        // tab is already isolated as a skipped_tab by the projection.
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        for claim in pnex_core::device_write_pin_refs_of(&graph) {
            owners
                .entry((claim.device_id, claim.pin))
                .or_insert_with(|| flow.name.clone());
        }
    }
    let mut violations = Vec::new();
    for claim in claims {
        if let Some(owner) = owners.get(&(claim.device_id.clone(), claim.pin.clone())) {
            violations.push(pnex_core::FlowViolation::with_args(
                Some(claim.node_id.as_str()),
                pnex_core::err_codes::PIN_ALREADY_ASSIGNED,
                "output pin already driven by another deployed flow (one write source per output)",
                serde_json::json!({
                    "pin": claim.pin,
                    "device": claim.device_id,
                    "flow": owner,
                }),
            ));
        }
    }
    Ok(violations)
}

/// Cross-flow recorder exclusivity (deploy gate, same school as the pins):
/// one `video-record` per camera across the org's deployed flows. The
/// candidate's own claims are excluded, so a redeploy of the owner passes.
pub(crate) async fn video_record_conflicts_for_deploy(
    ctx: &AppContext,
    org_id: i64,
    candidate_flow_id: i64,
    candidate_graph: &FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>> {
    let claims = pnex_core::video_record_claims_of(candidate_graph);
    if claims.is_empty() {
        return Ok(Vec::new());
    }
    let deployed = deployed_flows_with_versions(&ctx.db, org_id).await?;
    // camera -> recording flow name (first seen wins).
    let mut owners: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (flow, version) in deployed {
        if flow.id == candidate_flow_id {
            continue;
        }
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        for claim in pnex_core::video_record_claims_of(&graph) {
            owners
                .entry(claim.device_id)
                .or_insert_with(|| flow.name.clone());
        }
    }
    Ok(claims
        .into_iter()
        .filter_map(|claim| {
            owners.get(&claim.device_id).map(|owner| {
                pnex_core::FlowViolation::with_args(
                    Some(claim.node_id.as_str()),
                    pnex_core::err_codes::CAMERA_ALREADY_RECORDED,
                    "camera already recorded by another deployed flow (one video-record per camera)",
                    serde_json::json!({ "camera": claim.device_id, "flow": owner }),
                )
            })
        })
        .collect())
}

/// Org controls gate (D127, D133): a `control-source` listens to at least
/// one control (an empty node saves as a draft, never deploys), and every
/// listed control must exist in the org, otherwise the node would wait
/// forever on a value nobody can write. One violation per empty node and
/// per (node, missing control).
pub(crate) async fn unknown_controls_for_deploy(
    ctx: &AppContext,
    org_id: i64,
    candidate_graph: &FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>> {
    let mut violations = Vec::new();
    for n in &candidate_graph.nodes {
        if let pnex_core::FlowNodeKind::ControlSource { config } = &n.kind {
            if let Some((code, message)) = config.check_deployable() {
                violations.push(pnex_core::FlowViolation::new(
                    Some(n.id.as_str()),
                    code,
                    message,
                ));
            }
        }
    }
    let wanted = pnex_core::control_refs_of(candidate_graph);
    if wanted.is_empty() {
        return Ok(violations);
    }
    let existing: std::collections::HashSet<uuid::Uuid> = crate::models::controls::Controls::find()
        .filter(crate::models::_entities::controls::Column::OrgId.eq(org_id))
        .filter(crate::models::_entities::controls::Column::Id.is_in(wanted))
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|m| m.id)
        .collect();
    for n in &candidate_graph.nodes {
        let pnex_core::FlowNodeKind::ControlSource { config } = &n.kind else {
            continue;
        };
        for id in config.controls.iter().filter(|id| !existing.contains(id)) {
            violations.push(pnex_core::FlowViolation::with_args(
                Some(n.id.as_str()),
                pnex_core::err_codes::CONTROL_UNKNOWN,
                "control-source lists a control that does not exist in the organization",
                serde_json::json!({ "control": id.to_string() }),
            ));
        }
    }
    Ok(violations)
}

/// Media streams gate (D163, R3): every slug listed by a `media-source`
/// must be a live stream of the org, otherwise the node would listen to a
/// channel nobody publishes on. One violation per (node, unknown slug).
pub(crate) async fn unknown_media_streams_for_deploy(
    ctx: &AppContext,
    org_id: i64,
    candidate_graph: &FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>> {
    let mut violations = Vec::new();
    for n in &candidate_graph.nodes {
        let pnex_core::FlowNodeKind::MediaSource { config } = &n.kind else {
            continue;
        };
        for slug in &config.streams {
            let found = crate::services::media_ingest::streams::find_by_slug(&ctx.db, org_id, slug)
                .await
                .map_err(|_| Error::InternalServerError)?;
            if found.is_none() {
                violations.push(pnex_core::FlowViolation::with_args(
                    Some(n.id.as_str()),
                    pnex_core::err_codes::MEDIA_STREAM_UNKNOWN,
                    "media-source lists a stream that does not exist in the organization",
                    serde_json::json!({ "stream": slug }),
                ));
            }
        }
    }
    Ok(violations)
}

/// Taxonomy gate (D168, R1): every `topic_classify` must pin an existing
/// version of a taxonomy of the org — another org's id reads as unknown.
pub(crate) async fn unknown_taxonomies_for_deploy(
    ctx: &AppContext,
    org_id: i64,
    candidate_graph: &FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>> {
    let mut violations = Vec::new();
    for n in &candidate_graph.nodes {
        let pnex_core::FlowNodeKind::TopicClassify { config } = &n.kind else {
            continue;
        };
        let Ok(id) = uuid::Uuid::parse_str(config.taxonomy_id.trim()) else {
            continue; // the node check reports it
        };
        let found = crate::services::media_ingest::taxonomies::find_version(
            &ctx.db,
            org_id,
            id,
            config.version,
        )
        .await
        .map_err(|_| Error::InternalServerError)?;
        if found.is_none() {
            violations.push(pnex_core::FlowViolation::with_args(
                Some(n.id.as_str()),
                pnex_core::err_codes::TAXONOMY_UNKNOWN,
                "topic-classify pins a taxonomy version that does not exist in the organization",
                serde_json::json!({ "version": config.version.to_string() }),
            ));
        }
    }
    Ok(violations)
}

/// Dry-run, WRITE usage only: deployed flows of the org whose graph WRITES
/// this (device slug, pin label). Reads never count — a flow reading a pin
/// leaves it manually writable. Feeds the manual-write 409 guard.
pub(crate) async fn flows_writing_pin(
    ctx: &AppContext,
    org_id: i64,
    device_id: &str,
    pin_label: &str,
) -> Result<Vec<(i64, String)>> {
    let deployed = deployed_flows_with_versions(&ctx.db, org_id).await?;
    let mut writers = Vec::new();
    for (flow, version) in deployed {
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        let writes = pnex_core::device_pin_refs_of(&graph, device_id)
            .iter()
            .any(|r| {
                r.usage == pnex_core::DevicePinUsage::Write
                    && pnex_core::normalize_measurement_name(&r.pin)
                        == pnex_core::normalize_measurement_name(pin_label)
            });
        if writes {
            writers.push((flow.id, flow.name));
        }
    }
    Ok(writers)
}

/// Batched variant for the device page / pinout: ONE scan of the deployed
/// flows for the whole device. Key = normalized pin label, value = owning
/// flow (smallest flow_id wins ties → deterministic display of grandfathered
/// double-claims).
pub(crate) async fn pin_write_owners_by_device(
    ctx: &AppContext,
    org_id: i64,
    device_id: &str,
) -> Result<std::collections::HashMap<String, Vec<(i64, String)>>> {
    let deployed = deployed_flows_with_versions(&ctx.db, org_id).await?;
    // Order by flow_id so multi-owner display (grandfathered double-claims)
    // is deterministic: oldest flow first.
    let mut ordered: Vec<(i64, String, Vec<pnex_core::DevicePinRef>)> = Vec::new();
    for (flow, version) in deployed {
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        let write_refs: Vec<pnex_core::DevicePinRef> =
            pnex_core::device_pin_refs_of(&graph, device_id)
                .into_iter()
                .filter(|r| r.usage == pnex_core::DevicePinUsage::Write)
                .collect();
        ordered.push((flow.id, flow.name, write_refs));
    }
    ordered.sort_by_key(|(id, _, _)| *id);
    // ALL owners are listed: hiding newer claims behind the oldest one made
    // the reservation hint lie about who actually drives the pin.
    let mut owners: std::collections::HashMap<String, Vec<(i64, String)>> =
        std::collections::HashMap::new();
    for (flow_id, flow_name, write_refs) in ordered {
        for r in write_refs {
            owners
                .entry(pnex_core::normalize_measurement_name(&r.pin))
                .or_default()
                .push((flow_id, flow_name.clone()));
        }
    }
    Ok(owners)
}

// ─────────────────────────── POST /flows/{id}/deploy | /rollback ───────────────────────────

/// `POST /flows/{id}/deploy` — publie une version (`version_number` absent =
/// dernière) : reprojection de l'ensemble des flows déployés → SIGUSR1 →
/// `deployed_version_id` mis à jour.
pub(super) async fn deploy(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: Option<Json<pnex_core::DeployFlow>>,
) -> Result<Response> {
    let lock = lock_org_deploys(&ctx, org.org.id).await?;
    let res = deploy_version(ctx, org, id, body).await;
    lock.release().await;
    res
}

/// `POST /flows/{id}/rollback` — alias explicite du deploy d'une version
/// antérieure (même mécanique, intention produit distincte).
pub(super) async fn rollback(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: Option<Json<pnex_core::DeployFlow>>,
) -> Result<Response> {
    let lock = lock_org_deploys(&ctx, org.org.id).await?;
    let res = deploy_version(ctx, org, id, body).await;
    lock.release().await;
    res
}

/// Deploys one version. The caller holds [`lock_org_deploys`] for the org
/// (gate → apply → mark deployed is atomic per org across pods).
async fn deploy_version(
    ctx: AppContext,
    org: OrgContext,
    id: i64,
    body: Option<Json<pnex_core::DeployFlow>>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "flow-deploy-forbidden",
            "Owner, admin or member role required to deploy flows.",
        ));
    }
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let version_number = match body.and_then(|Json(p)| p.version_number) {
        Some(n) => n,
        // Version absente → dernière version déployée par défaut.
        None => {
            let Some(latest) = latest_version(&ctx.db, flow.id).await? else {
                return Err(Error::NotFound);
            };
            latest.version_number
        }
    };
    let Some(version) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .filter(flow_versions::Column::VersionNumber.eq(version_number))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };

    // Cross-flow output-pin exclusivity (one write source per output): the
    // candidate must not write a (device, pin) already written by another
    // deployed flow of the org. 400 violations naming the owning flow — same
    // shape as the save/engine violations the editor already renders. Runs
    // before the projection so a rejected deploy never touches the runtime;
    // engine-off still answers 400 (gate) not 503. An unreadable candidate
    // graph is ignored here (graph_unreadable answers for it below).
    if let Ok(candidate_graph) = serde_json::from_value::<FlowGraph>(version.graph.clone()) {
        let mut conflicts =
            pin_write_conflicts_for_deploy(&ctx, org.org.id, flow.id, &candidate_graph).await?;
        conflicts.extend(
            video_record_conflicts_for_deploy(&ctx, org.org.id, flow.id, &candidate_graph).await?,
        );
        conflicts.extend(unknown_controls_for_deploy(&ctx, org.org.id, &candidate_graph).await?);
        conflicts
            .extend(unknown_media_streams_for_deploy(&ctx, org.org.id, &candidate_graph).await?);
        conflicts.extend(unknown_taxonomies_for_deploy(&ctx, org.org.id, &candidate_graph).await?);
        if !conflicts.is_empty() {
            return Ok((
                StatusCode::BAD_REQUEST,
                format::json(serde_json::json!({ "violations": conflicts })),
            )
                .into_response());
        }
    }

    // Candidat projeté **sans écrire la DB** : la version candidate remplace
    // celle du flow dans l'artefact complet. Pré-flight (`--check`) puis
    // rechargement acquitté — un tab invalide ne peut plus ni casser le
    // runtime ni rester marqué `deployed` (l'ancien ordre DB-d'abord laissait
    // une version fautive reprojetée à chaque boot).
    let (artifact, meta, stale_notify, fn_violations, skipped_tabs) = reproject_candidate(
        &ctx.db,
        flow.org_id,
        Some((flow.clone(), version.clone())),
        None,
    )
    .await?;
    // Refs de fonctions non résolues dans le candidat : bloquant au deploy
    // (même forme de 400 que engine_load, l'éditeur localise le nœud).
    if !fn_violations.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "violations": fn_violations })),
        )
            .into_response());
    }
    let apply = crate::services::flow_cluster::ApplyOrg {
        org_id: flow.org_id,
        fragment: artifact,
        ack: Some(meta),
        enforce_flow: Some(flow.id),
    };
    if let Err(e) = crate::services::flow_cluster::apply_org(&ctx.db, &ctx.config, apply).await {
        return Ok(deploy_error_response(e));
    }

    // Succès : la DB est marquée **après** acquittement (la version ne peut
    // plus être « déployée » sans tourner).
    let mut active: flows::ActiveModel = flow.clone().into();
    active.deployed_version_id = Set(Some(version.id));
    active.status = Set(pnex_core::FLOW_STATUS_DEPLOYED.to_string());
    let flow = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // The deployed version's secrets stay in use while it runs (lot S5).
    if let Err(e) = crate::services::secrets::flow::sync_usages(&ctx.db, flow.id).await {
        tracing::error!(flow_id = flow.id, error = %e, "flow secret usages sync failed");
    }
    // The owner reprojects from the database it now agrees with (no-op
    // for the runtime: same fragment as the acknowledged candidate).
    crate::services::flow_cluster::org_changed(&ctx.db, flow.org_id).await;

    // Sync/cast des régulations APRÈS le marquage DB (il lit l'état déployé).
    // Jamais de 503 cast — même doctrine que reproject_and_cast_extra.
    // Pas de reprojection ici : le candidat acquitté est déjà l'artefact
    // exact que reproject_and_signal produirait.
    if let Err(e) = crate::services::regulator::sync_and_cast_with(&ctx.db, flow.org_id, &[]).await
    {
        tracing::error!("sync des régulations échoué (deploy poursuivi) : {e}");
    }

    let latest_number = latest_version(&ctx.db, flow.id)
        .await?
        .map(|v| v.version_number)
        .unwrap_or(0);
    let graph: FlowGraph =
        serde_json::from_value(version.graph).map_err(|_| Error::InternalServerError)?;
    tracing::info!(flow_id = flow.id, version = version_number, "flow déployé");
    // Champ additif : références notify périmées (toast éditeur, non
    // bloquant) — les consommateurs existants l'ignorent (CONTRACT inchangé).
    let mut dto = serde_json::to_value(flow_dto(flow, graph, latest_number, Some(version_number)))
        .map_err(|_| Error::InternalServerError)?;
    if let Some(obj) = dto.as_object_mut() {
        obj.insert(
            "stale_notify_nodes".into(),
            serde_json::json!(stale_notify
                .iter()
                .filter(|s| s.flow_id == id)
                .cloned()
                .collect::<Vec<_>>()),
        );
        // Champ additif : onglets d'AUTRES flows isolés à la projection
        // (graphe illisible — kind supprimé, schéma daté). Le deploy de CE
        // flow passe ; le toast de l'éditeur signale les tabs morts.
        obj.insert("skipped_tabs".into(), serde_json::json!(skipped_tabs));
    }
    Ok(format::json(dto).into_response())
}

// ─────────────────────────── POST /flows/{id}/stop | start | restart ───────────────────────────

/// `POST /flows/{id}/stop` — retire le tab du flow de l'artefact
/// (reprojection sans lui) et attend l'ack `flow_stopped` avant de marquer
/// la DB `stopped`. `deployed_version_id` est **conservé** (reprise sans
/// rebuild via Start/Deploy). Garde-fous : 403 `can_write`, 404 hors-org,
/// 409 si pas `deployed` ; 503 `flow_runtime` si le moteur n'acquitte pas
/// (DB intacte — le flow tourne encore).
pub(super) async fn stop(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "flow-exec-forbidden",
            "Owner, admin or member role required to manage flow execution.",
        ));
    }
    if find_flow(&ctx.db, &org, id).await?.is_none() {
        return Err(Error::NotFound);
    }
    let lock = lock_org_deploys(&ctx, org.org.id).await?;
    // Status re-read under the lock: a concurrent stop/delete may have won.
    let res = async {
        let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
            return Err(Error::NotFound);
        };
        if flow.status != pnex_core::FLOW_STATUS_DEPLOYED {
            return Err(conflict(
                "flow-stop-not-deployed",
                "Only a deployed flow can be stopped.",
            ));
        }
        stop_deployed(&ctx, flow).await
    }
    .await;
    lock.release().await;
    res?;
    Ok(
        format::json(serde_json::json!({ "status": pnex_core::FLOW_STATUS_STOPPED }))
            .into_response(),
    )
}

/// Chemin d'arrêt partagé (stop + restart) : reprojection **excluant** le
/// flow, meta du flow stoppé comme clé d'ack (piège de la meta vide —
/// l'ack `flow_stopped` est keyé sur CE flow), puis marquage DB après
/// acquittement seulement.
async fn stop_deployed(ctx: &AppContext, flow: flows::Model) -> Result<()> {
    if flow.deployed_version_id.is_none() {
        return Err(conflict(
            "flow-no-deployed-version",
            "The flow has no deployed version.",
        ));
    }
    let Some(meta) = deployed_meta(&ctx.db, &flow).await? else {
        return Err(Error::NotFound);
    };
    let (artifact, _, _, fn_violations, skipped_tabs) =
        reproject_candidate(&ctx.db, flow.org_id, None, Some(flow.id)).await?;
    if !fn_violations.is_empty() {
        tracing::warn!(
            count = fn_violations.len(),
            "stop flow #{} : références de fonctions non résolues (nœuds dégradés no-op, retrait poursuivi)",
            flow.id
        );
    }
    if !skipped_tabs.is_empty() {
        tracing::warn!(
            tabs = skipped_tabs.len(),
            "stop flow #{} : onglets au graphe illisible isolés (retrait poursuivi)",
            flow.id
        );
    }
    let org_id = flow.org_id;
    crate::services::flow_cluster::apply_org(
        &ctx.db,
        &ctx.config,
        crate::services::flow_cluster::ApplyOrg {
            org_id,
            fragment: artifact,
            ack: Some(meta),
            enforce_flow: None,
        },
    )
    .await
    .map_err(flow_runtime_503)?;

    // Succès : DB marquée **après** ack — un stop non acquitté laisse le
    // flow déployé et en marche.
    let mut active: flows::ActiveModel = flow.into();
    active.status = Set(pnex_core::FLOW_STATUS_STOPPED.to_string());
    active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    crate::services::flow_cluster::org_changed(&ctx.db, org_id).await;
    Ok(())
}

/// `POST /flows/{id}/start` — reprend l'exécution d'un flow `stopped` en
/// re-déployant **sa version déployée** (pas la dernière sauvegardée :
/// Deploy, lui, monte la version fraîche). Garde-fous : 403 `can_write`,
/// 404 hors-org, 409 si pas `stopped` ou jamais déployé.
pub(super) async fn start(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "flow-exec-forbidden",
            "Owner, admin or member role required to manage flow execution.",
        ));
    }
    if find_flow(&ctx.db, &org, id).await?.is_none() {
        return Err(Error::NotFound);
    }
    let lock = lock_org_deploys(&ctx, org.org.id).await?;
    // Status re-read under the lock (see `stop`).
    let res = async {
        let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
            return Err(Error::NotFound);
        };
        if flow.status != pnex_core::FLOW_STATUS_STOPPED {
            return Err(conflict(
                "flow-start-not-stopped",
                "Only a stopped flow can be started.",
            ));
        }
        start_stopped(ctx.clone(), org, flow).await
    }
    .await;
    lock.release().await;
    res
}

/// Chemin de reprise partagé (start + restart) : délègue à `deploy_version`
/// sur la version **déployée** (l'override de `reproject_candidate` inclut
/// le tab même hors statut `deployed` ; la DB repasse `deployed` après ack).
async fn start_stopped(ctx: AppContext, org: OrgContext, flow: flows::Model) -> Result<Response> {
    let Some(deployed_id) = flow.deployed_version_id else {
        return Err(conflict(
            "flow-no-deployed-version",
            "The flow has no deployed version.",
        ));
    };
    let Some(version) = flow_versions::Entity::find_by_id(deployed_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    deploy_version(
        ctx,
        org,
        flow.id,
        Some(Json(pnex_core::DeployFlow {
            version_number: Some(version.version_number),
        })),
    )
    .await
}

/// `POST /flows/{id}/restart` — stop puis start chaînés (deux cycles de
/// rechargement : le reload est un diff par hash, un tab identique ne
/// swappe pas). Depuis `stopped`, Restart ≡ Start. Garde-fous : 403
/// `can_write`, 404 hors-org, 409 si jamais déployé.
pub(super) async fn restart(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "flow-exec-forbidden",
            "Owner, admin or member role required to manage flow execution.",
        ));
    }
    if find_flow(&ctx.db, &org, id).await?.is_none() {
        return Err(Error::NotFound);
    }
    // One lock for both halves: no other deploy can slip between them.
    let lock = lock_org_deploys(&ctx, org.org.id).await?;
    let res = async {
        let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
            return Err(Error::NotFound);
        };
        if flow.deployed_version_id.is_none() {
            return Err(conflict(
                "flow-no-deployed-version",
                "The flow has no deployed version.",
            ));
        }
        if flow.status == pnex_core::FLOW_STATUS_DEPLOYED {
            stop_deployed(&ctx, flow.clone()).await?;
        }
        let flow = find_flow(&ctx.db, &org, id).await?.ok_or(Error::NotFound)?;
        start_stopped(ctx.clone(), org, flow).await
    }
    .await;
    lock.release().await;
    res
}

// ─────────────────────────── GET /flows/{id}/runtime ───────────────────────────

/// `GET /flows/{id}/runtime` — santé du moteur (pid, vivant, rechargements)
/// croisée avec **la version déployée de CE flow, lue en DB**
/// (`flows.deployed_version_id`, posée uniquement quand le moteur a
/// acquitté un deploy). 404 si le flow n'est pas de l'org.
pub(super) async fn runtime(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let settings = FlowSettings::from_config(&ctx.config);
    // Santé process × santé de l'engine de CE flow (événements `flow_*` du
    // runtime), au prisme du flow demandé.
    let mut status =
        crate::services::flow_cluster::runtime_status(&ctx.db, &ctx.config, flow.org_id, id).await;
    // Point de vue du flow : sa version déployée (DB) — jamais une version
    // « globale » du moteur qui appartiendrait à un autre flow.
    status.deployed_flow_id = Some(id);
    status.deployed_version_number = deployed_number_of(&ctx.db, flow.deployed_version_id).await?;
    // Porte l'activation des outils de debug (mode dev/debug uniquement) :
    // l'éditeur masque le panneau sans second endpoint.
    status.debug_tools = settings.debug_tools;
    // Multi-user awareness: the editor polls this endpoint and compares
    // these with what it loaded (version saved / status changed elsewhere).
    status.latest_version_number = latest_version(&ctx.db, flow.id)
        .await?
        .map(|v| v.version_number);
    status.flow_status = Some(flow.status.clone());
    Ok(format::json(status).into_response())
}

// ─────────────────────────── Debug (panneau) ───────────────────────────

/// `GET /flows/{id}/debug` — feed du panneau (100 dernières entrées du flow).
/// Lecture seule (même niveau que `runtime`), 200 avec feed vide si le
/// moteur est arrêté (un 503 rendrait le drawer inutilisable). Garde-fou :
/// 403 hors mode dev/debug (`settings.flow.debug_tools`).
/// Latest camera/vision node statuses of a flow (D103) plus the last
/// message of each debug/display node — run mode too: statuses are health,
/// and a flow designer must always see what a debug node receives (the
/// full history of the side panel stays behind `debug_tools`).
pub(super) async fn node_status(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let entries =
        crate::services::flow_cluster::debug_entries(&ctx.db, &ctx.config, flow.org_id, id, None)
            .await;
    Ok(format::json(pnex_core::FlowDebugFeed {
        flow_id: id,
        entries,
    })
    .into_response())
}

pub(super) async fn debug(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.debug_tools {
        return Err(forbidden(
            "flow-debug-disabled",
            "Debug tools are disabled (run mode).",
        ));
    }
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let entries = crate::services::flow_cluster::debug_entries(
        &ctx.db,
        &ctx.config,
        flow.org_id,
        id,
        Some(100),
    )
    .await;
    Ok(format::json(pnex_core::FlowDebugFeed {
        flow_id: id,
        entries,
    })
    .into_response())
}
