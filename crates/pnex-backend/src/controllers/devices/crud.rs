use super::*;

/// Quota du tier de l'org pour ce type de device (None = pas de plafond).
pub(crate) async fn tier_limit_for(
    db: &DatabaseConnection,
    org: &OrgContext,
    type_name: &str,
) -> Result<Option<i32>> {
    let Some(tier_id) = org.org.subscription_tier_id else {
        return Ok(None);
    };
    let Some(tier) = subscription_tiers::Entity::find_by_id(tier_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(None);
    };
    Ok(match type_name.to_ascii_lowercase().as_str() {
        "sensor" => Some(tier.max_sensor_devices),
        "actuator" => Some(tier.max_actuator_devices),
        "mixed" => Some(tier.max_mixed_devices),
        _ => None,
    })
}

// ───────────────────── Registre devices (org) ─────────────────────

#[derive(Debug, Default, Deserialize)]
pub(super) struct ListDevicesQuery {
    /// Type name; "all" = no-op (legacy parity).
    device_type: Option<String>,
    capability: Option<String>,
    /// Correspondance exacte sur l'identifiant firmware.
    device_id: Option<String>,
    /// « true » | « false » ; autre/absent = tous.
    active: Option<String>,
    /// Recherche OU sur device_id, modèle (nom/pretty/description), type et
    /// capacités.
    search: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/devices` — devices de l'org, paginés (D14).
///
/// Les filtres `capability`/`search` portent sur la M2M du modèle :
/// l'ensemble org est borné par les quotas tier, on filtre en Rust puis on
/// découpe — le count reflète bien le total filtré.
pub(super) async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListDevicesQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    // Filters, COUNT and LIMIT/OFFSET run in SQL (org-scoped); only the
    // page is hydrated (types, capabilities, tokens, states, builds, OTA).
    use sea_orm::sea_query::Query as SqlQuery;
    use sea_orm::{Condition, JoinType, RelationTrait};
    let mut select = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        // Inner join: a registry row without its model is never listed.
        .join(
            JoinType::InnerJoin,
            device_registries::Relation::PredefinedDevices.def(),
        )
        .join(
            JoinType::LeftJoin,
            predefined_devices::Relation::DeviceTypes.def(),
        );
    if let Some(t) = q.device_type.as_deref().filter(|t| *t != "all") {
        select = select.filter(device_types::Column::Name.eq(t));
    }
    // Models carrying a capability that satisfies `cond`.
    let models_with_cap = |cond: sea_orm::sea_query::SimpleExpr| {
        SqlQuery::select()
            .column((
                predefined_device_capabilities::Entity,
                predefined_device_capabilities::Column::PredefinedDeviceId,
            ))
            .from(predefined_device_capabilities::Entity)
            .inner_join(
                device_capabilities::Entity,
                Expr::col((device_capabilities::Entity, device_capabilities::Column::Id)).equals((
                    predefined_device_capabilities::Entity,
                    predefined_device_capabilities::Column::DeviceCapabilityId,
                )),
            )
            .and_where(cond)
            .to_owned()
    };
    if let Some(c) = q.capability.as_deref() {
        select = select.filter(
            device_registries::Column::PredefinedDeviceId.in_subquery(models_with_cap(
                Expr::col((
                    device_capabilities::Entity,
                    device_capabilities::Column::Name,
                ))
                .eq(c),
            )),
        );
    }
    if let Some(v) = q.device_id.as_deref() {
        select = select.filter(device_registries::Column::DeviceId.eq(v));
    }
    match q.active.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("true") => select = select.filter(device_registries::Column::Active.eq(true)),
        Some("false") => select = select.filter(device_registries::Column::Active.eq(false)),
        _ => {}
    }
    // Multi-field search: device_id, model (name/pretty/description), type,
    // capabilities — case-insensitive OR.
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        select =
            select.filter(
                Condition::any()
                    .add(pagination::sql_contains(
                        (
                            device_registries::Entity,
                            device_registries::Column::DeviceId,
                        ),
                        &pat,
                    ))
                    .add(pagination::sql_contains(
                        (predefined_devices::Entity, predefined_devices::Column::Name),
                        &pat,
                    ))
                    .add(pagination::sql_contains(
                        (
                            predefined_devices::Entity,
                            predefined_devices::Column::PrettyName,
                        ),
                        &pat,
                    ))
                    .add(pagination::sql_contains(
                        (
                            predefined_devices::Entity,
                            predefined_devices::Column::Description,
                        ),
                        &pat,
                    ))
                    .add(pagination::sql_contains(
                        (device_types::Entity, device_types::Column::Name),
                        &pat,
                    ))
                    .add(device_registries::Column::PredefinedDeviceId.in_subquery(
                        models_with_cap(pagination::sql_contains(
                            (
                                device_capabilities::Entity,
                                device_capabilities::Column::Name,
                            ),
                            &pat,
                        )),
                    )),
            );
    }
    // Newest first: a freshly registered device (wizard) shows up on
    // page 1 — deliberate divergence from the legacy behavior (no explicit sort).
    let select = select.order_by_desc(device_registries::Column::Id);
    let (count, rows) = pagination::sql_page(&ctx.db, select, page)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Page context, batched: models, types, capabilities, tokens, states.
    let pd_ids: Vec<i64> = rows.iter().map(|d| d.predefined_device_id).collect();
    let device_pks: Vec<i64> = rows.iter().map(|d| d.id).collect();
    let predefined: HashMap<i64, predefined_devices::Model> = if pd_ids.is_empty() {
        HashMap::new()
    } else {
        predefined_devices::Entity::find()
            .filter(predefined_devices::Column::Id.is_in(pd_ids.clone()))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|p| (p.id, p))
            .collect()
    };
    let type_names: HashMap<i64, String> = device_types::Entity::find()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let caps = capabilities_of(&ctx.db, &pd_ids).await?;
    let tokens: HashMap<i64, device_tokens::Model> = device_tokens::Entity::find()
        .filter(device_tokens::Column::DeviceRegistryId.is_in(device_pks.clone()))
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.device_registry_id, t))
        .collect();
    // Liveness leases (D108): last seen + connected per device, batched.
    let states = crate::services::device_liveness::seen_many(&ctx.db, &device_pks).await?;

    // Dernier build par device de la page (batché, clé = device_id string du
    // registre — même sémantique que build_records). `order_by_desc(Id)` :
    // le record le plus récent gagne dans la map.
    let mut page_devices: Vec<pnex_core::Device> = Vec::with_capacity(rows.len());
    for device in rows {
        let Some(predefined) = predefined.get(&device.predefined_device_id) else {
            continue;
        };
        let type_name = type_names
            .get(&predefined.device_type_id)
            .map(String::as_str)
            .unwrap_or_default();
        let token = tokens.get(&device.id);
        let (last_seen, connected) = states.get(&device.id).map_or((None, false), |st| {
            (st.last_seen.map(|t| t.to_rfc3339()), st.connected)
        });
        page_devices.push(device_dto(
            device,
            predefined,
            type_name,
            caps.get(&predefined.id).map(Vec::as_slice).unwrap_or(&[]),
            token,
            last_seen,
            connected,
            None,
            None, // ota hydrated in batch below
        ));
    }
    let page_device_ids: Vec<String> = page_devices.iter().map(|d| d.device_id.clone()).collect();
    if !page_device_ids.is_empty() {
        let mut builds: HashMap<String, pnex_core::LatestBuild> = latest_builds_by_device(
            build_records::Entity::find()
                .filter(build_records::Column::OrgId.eq(org.org.id))
                .filter(build_records::Column::DeviceId.is_in(page_device_ids))
                .order_by_desc(build_records::Column::Id)
                .all(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?,
        );
        for device in &mut page_devices {
            device.latest_build = builds.remove(&device.device_id);
        }
    }

    // Active OTA deployments for the page devices (batch — newest wins).
    let page_device_pks: Vec<i64> = page_devices.iter().map(|d| d.id).collect();
    if !page_device_pks.is_empty() {
        let mut ota_by_device: HashMap<i64, pnex_core::OtaAssignmentState> = HashMap::new();
        for row in ota_assignments::Entity::find()
            .filter(ota_assignments::Column::DeviceRegistryId.is_in(page_device_pks))
            .filter(ota_assignments::Column::State.is_in([
                crate::services::ota::ST_PENDING,
                crate::services::ota::ST_DOWNLOADING,
                crate::services::ota::ST_FLASHING,
            ]))
            .order_by_desc(ota_assignments::Column::Id)
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
        {
            ota_by_device
                .entry(row.device_registry_id)
                .or_insert_with(|| ota_state_dto(row));
        }
        for device in &mut page_devices {
            device.ota = ota_by_device.remove(&device.id);
        }
    }
    let mut filters = Vec::new();
    if let Some(t) = q.device_type.as_ref().filter(|t| t.as_str() != "all") {
        filters.push(("device_type".to_string(), t.clone()));
    }
    if let Some(c) = &q.capability {
        filters.push(("capability".to_string(), c.clone()));
    }
    if let Some(v) = &q.device_id {
        filters.push(("device_id".to_string(), v.clone()));
    }
    if let Some(v) = &q.active {
        filters.push(("active".to_string(), v.clone()));
    }
    if let Some(v) = &q.search {
        filters.push(("search".to_string(), v.clone()));
    }
    format::json(pagination::envelope(
        "/api/v1/devices",
        &filters,
        page,
        count,
        page_devices,
    ))
}

/// `POST /api/v1/devices` — création inactive + token, ou réactivation.
pub(super) async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateDevice>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "device-write-forbidden",
            "Owner, admin or member role required to manage devices.",
        ));
    }
    let device_id = params.device_id.trim().to_string();
    if device_id.is_empty() {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "device_id",
            err_codes::FIELD_REQUIRED,
        ));
    }

    let Some(predefined) = predefined_devices::Entity::find()
        .filter(predefined_devices::Column::Name.eq(&params.predefined_device_name))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "predefined_device_name",
            &format!(
                "PredefinedDevice with name {} does not exist.",
                params.predefined_device_name
            ),
        ));
    };

    // Board variante figée : validée puis gelée à
    // l'enregistrement — le device garde SA variante même si le modèle
    // évolue. Absent = board par défaut du modèle.
    let frozen_board_id = match params.board_id {
        Some(bid) => {
            let board_found = mcu_boards::Entity::find_by_id(bid)
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?;
            let Some(board) = board_found else {
                return Ok(field_status(
                    StatusCode::BAD_REQUEST,
                    "board_id",
                    &format!("board {bid} inconnu"),
                ));
            };
            let default_found = mcu_boards::Entity::find_by_id(predefined.board_id)
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?;
            let Some(default_board) = default_found else {
                return Ok(field_status(
                    StatusCode::BAD_REQUEST,
                    "board_id",
                    &format!("board par défaut du modèle {} introuvable", predefined.name),
                ));
            };
            // Compatible = same SoC known to the chip-caps, or else the same
            // raw value (boards with an unknown SoC).
            let compatible = match (
                pnex_core::Soc::from_board_soc(&board.soc),
                pnex_core::Soc::from_board_soc(&default_board.soc),
            ) {
                (Some(a), Some(b)) => a == b,
                _ => board.soc == default_board.soc,
            };
            if !compatible {
                return Ok(field_status(
                    StatusCode::BAD_REQUEST,
                    "board_id",
                    &format!(
                        "board {} : SoC {} incompatible avec le modèle {} (SoC {})",
                        board.name, board.soc, predefined.name, default_board.soc
                    ),
                ));
            }
            Some(bid)
        }
        None => Some(predefined.board_id),
    };

    // Custom firmware chosen at provisioning (D94): project of the org whose
    // chip family matches the frozen board SoC.
    if let Some(pid) = params.firmware_project_id {
        // Predefined boards and agents keep their firmware (edge-model.md §2 bis).
        if !pnex_core::DeviceFamily::of(&predefined.name).accepts_custom_firmware() {
            return Err(Error::CustomError(
                StatusCode::BAD_REQUEST,
                loco_rs::controller::ErrorDetail::new(
                    err_codes::FIRMWARE_FAMILY_LOCKED,
                    "This model runs a firmware maintained by PneX: only generic models accept a custom firmware.",
                ),
            ));
        }
        let project = crate::models::_entities::firmware_projects::Entity::find_by_id(pid)
            .filter(crate::models::_entities::firmware_projects::Column::OrgId.eq(org.org.id))
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        let Some(project) = project else {
            return Ok(field_status(
                StatusCode::BAD_REQUEST,
                "firmware_project_id",
                "invalid",
            ));
        };
        let board_soc = match frozen_board_id {
            Some(bid) => mcu_boards::Entity::find_by_id(bid)
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?
                .and_then(|b| pnex_core::Soc::from_board_soc(&b.soc)),
            None => None,
        };
        if board_soc.map(|s| s.name()) != Some(project.chip_family.as_str()) {
            return Err(Error::CustomError(
                StatusCode::BAD_REQUEST,
                loco_rs::controller::ErrorDetail::new(
                    "firmware-chip-mismatch",
                    "The device chip does not match the project chip family.",
                ),
            ));
        }
    }

    // Device connu de l'org : réactivation (200) ou refus (400).
    if let Some(existing) = device_registries::Entity::find()
        .filter(
            device_registries::Column::OrgId
                .eq(org.org.id)
                .and(device_registries::Column::DeviceId.eq(&device_id)),
        )
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        if existing.active {
            return Ok(detail_status(
                StatusCode::BAD_REQUEST,
                "This device is already registered and active.",
            ));
        }
        let txn = ctx
            .db
            .begin()
            .await
            .map_err(|_| Error::InternalServerError)?;
        let mut active: device_registries::ActiveModel = existing.into();
        active.active = Set(true);
        let device = active
            .update(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?;
        ensure_token(&txn, &device).await?;
        txn.commit().await.map_err(|_| Error::InternalServerError)?;
        return Ok(detail_status(
            StatusCode::OK,
            "Device reactivated successfully.",
        ));
    }

    // Tier quota: all states combined (legacy parity).
    let type_name = device_types::Entity::find_by_id(predefined.device_type_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|t| t.name)
        .unwrap_or_default();
    let tier_limit = tier_limit_for(&ctx.db, &org, &type_name).await?;

    // Création inactive + token (transaction : jamais de device sur token).
    let is_agent = predefined.name == pnex_core::EDGE_AGENT_PREDEF;
    // Only agents ingest free-form keys; boards announce their metrics.
    let allow_dynamic = is_agent;
    // Edge agent (D95): free-form keys, bounded by a larger distinct keys
    // quota (editable from the UI).
    let max_unique = if is_agent {
        crate::services::edge_agent::AGENT_DEFAULT_MAX_KEYS
    } else {
        100
    };
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    if let Some(limit) = tier_limit {
        // Count + insert atomic per org across pods: the advisory lock is
        // held by this transaction until the device row is committed, so
        // two concurrent creations can never both see `limit - 1`.
        crate::services::db_lock::xact_lock(
            &txn,
            crate::services::db_lock::ns::DEVICE_QUOTA,
            org.org.id,
        )
        .await
        .map_err(|_| Error::InternalServerError)?;
        let count = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org.org.id))
            .inner_join(predefined_devices::Entity)
            .filter(predefined_devices::Column::DeviceTypeId.eq(predefined.device_type_id))
            .count(&txn)
            .await
            .map_err(|_| Error::InternalServerError)? as i64;
        if count >= i64::from(limit) {
            return Ok(detail_status(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Device limit reached for {} devices in your subscription tier.",
                    type_name.to_ascii_lowercase()
                ),
            ));
        }
    }

    // Soldered screen (builtin board profile): seed the peripherals jsonb
    // at creation — pins reserved from day one, UI shows the locked state.
    let builtin_screen_kind: Option<String> = match frozen_board_id {
        Some(bid) => mcu_boards::Entity::find_by_id(bid)
            .one(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?
            .and_then(|b| b.details)
            .and_then(|d| serde_json::from_value::<pnex_core::BoardDetails>(d).ok())
            .and_then(|d| {
                d.v2().and_then(|p| {
                    p.peripherals
                        .screens
                        .iter()
                        .find(|s| s.builtin)
                        .map(|s| s.kind.clone())
                })
            }),
        None => None,
    };

    let device = device_registries::ActiveModel {
        device_id: Set(device_id),
        metadata: Set(params.metadata),
        active: Set(false),
        allow_dynamic_measurements: Set(allow_dynamic),
        discovered_measurements: Set(None),
        max_unique_measurements: Set(max_unique),
        org_id: Set(org.org.id),
        predefined_device_id: Set(predefined.id),
        board_id: Set(frozen_board_id),
        firmware_project_id: Set(params.firmware_project_id),
        // No screen seeded for a custom firmware: it owns every pin.
        peripherals: Set(builtin_screen_kind
            .filter(|_| params.firmware_project_id.is_none())
            .map(|k| serde_json::json!({ "screen": k }))),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| Error::InternalServerError)?;
    ensure_token(&txn, &device).await?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;

    let dto = device_full(&ctx.db, device).await?;
    Ok((StatusCode::CREATED, format::json(dto)).into_response())
}

/// `GET /api/v1/devices/{id}` — détail, membres de l'org.
pub(super) async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    format::json(device_full(&ctx.db, device).await?)
}

/// `PUT|PATCH /api/v1/devices/{id}` — metadata only (legacy contract: any
/// other key, or a missing `metadata`, → 400 "Only metadata updates
/// are allowed."). The payload is read as raw JSON to detect forbidden
/// keys before deserialization.
pub(super) async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "device-write-forbidden",
            "Owner, admin or member role required to manage devices.",
        ));
    }
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let only_metadata = body
        .as_object()
        .is_some_and(|obj| obj.len() == 1 && obj.contains_key("metadata"));
    if !only_metadata {
        return Ok(detail_status(
            StatusCode::BAD_REQUEST,
            "Only metadata updates are allowed.",
        ));
    }
    let metadata = body.get("metadata").cloned();
    let mut active: device_registries::ActiveModel = device.into();
    active.metadata = Set(metadata);
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(device_full(&ctx.db, updated).await?)
}

/// `DELETE /api/v1/devices/{id}` — device + token + build_records.
/// 204 sans body (le décompte des records nettoyés part dans les logs).
pub(super) async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "device-write-forbidden",
            "Owner, admin or member role required to manage devices.",
        ));
    }
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let cleaned = build_records::Entity::delete_many()
        .filter(
            build_records::Column::OrgId
                .eq(org.org.id)
                .and(build_records::Column::DeviceId.eq(&device.device_id)),
        )
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .rows_affected;
    device_tokens::Entity::delete_many()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // D42 : purge symétrique de la couche d'organisation (labels, tree,
    // edges des deux bouts) — zéro effet sur le firmware ni le contrat.
    crate::services::resources::purge_for(
        &ctx.db,
        org.org.id,
        pnex_core::resources::KIND_DEVICE,
        &device.id.to_string(),
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    device_registries::Entity::delete_by_id(device.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(device = %device.device_id, firmware_cleaned = cleaned, "device supprimé");
    Ok(StatusCode::NO_CONTENT.into_response())
}
