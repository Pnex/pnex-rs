use super::*;

/// Mode de capacité tel qu'exposé dans l'API (l'enum SeaORM générée
/// sérialise en Capitalized, on mappe — cf. `role_str` dans orgs).
pub fn capability_mode_str(mode: CapabilityMode) -> &'static str {
    match mode {
        CapabilityMode::Input => "input",
        CapabilityMode::Output => "output",
        CapabilityMode::InputOutput => "input_output",
    }
}

/// Noms des mesures découvertes (clés du JSONB) si le dynamic est autorisé.
pub(crate) fn discovered_names(value: &Option<serde_json::Value>) -> Vec<String> {
    value
        .as_ref()
        .and_then(|j| j.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Capacités par predefined device (join table), pour les ids donnés.
pub(crate) async fn capabilities_of(
    db: &DatabaseConnection,
    pd_ids: &[i64],
) -> Result<HashMap<i64, Vec<device_capabilities::Model>>> {
    if pd_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = predefined_device_capabilities::Entity::find()
        .find_also_related(device_capabilities::Entity)
        .filter(predefined_device_capabilities::Column::PredefinedDeviceId.is_in(pd_ids.to_vec()))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mut map: HashMap<i64, Vec<device_capabilities::Model>> = HashMap::new();
    for (link, cap) in rows {
        if let Some(cap) = cap {
            map.entry(link.predefined_device_id).or_default().push(cap);
        }
    }
    Ok(map)
}

/// Latest build of each device for the DTO, from records sorted by id
/// DESCENDING (several records per device: every build is a new row). The
/// newest record gives the phase; the newest successful OTA-stamped one
/// gives `deployable_version`.
pub(super) fn latest_builds_by_device(
    records: Vec<build_records::Model>,
) -> HashMap<String, pnex_core::LatestBuild> {
    let mut out: HashMap<String, pnex_core::LatestBuild> = HashMap::new();
    for record in records {
        let Some(device_id) = record.device_id.clone() else {
            continue;
        };
        let deployable = (record.success && record.ota_sha256.is_some())
            .then(|| record.fw_version.clone())
            .flatten();
        match out.get_mut(&device_id) {
            Some(latest) => {
                if latest.deployable_version.is_none() {
                    latest.deployable_version = deployable;
                }
            }
            None => {
                let mut latest = latest_build_dto(record);
                latest.deployable_version = deployable;
                out.insert(device_id, latest);
            }
        }
    }
    out
}

/// Newest build record → DTO (phase + stamps); `deployable_version` is
/// filled by [`latest_builds_by_device`].
pub(super) fn latest_build_dto(record: build_records::Model) -> pnex_core::LatestBuild {
    pnex_core::LatestBuild {
        deployable_version: None,
        success: record.success,
        build_phase: record.build_phase,
        fw_version: record.fw_version,
        // Staleness tripwire: the build's stamped fingerprint vs the one of
        // the tree embedded in THIS server binary. Null stamp (legacy
        // record) → unknown, no warning.
        sources_stale: record
            .sources_fingerprint
            .map(|fp| fp != pnex_firmware_builder::source_fingerprint()),
        updated_at: record.updated_at.to_rfc3339(),
        failure_code: record.failure_code,
        failure_detail: record.failure_detail,
    }
}

/// Active assignment → hydrated OTA status for the Device DTO.
pub(super) fn ota_state_dto(row: ota_assignments::Model) -> pnex_core::OtaAssignmentState {
    pnex_core::OtaAssignmentState {
        state: row.state,
        target_version: row.target_version,
        progress: row.progress.map(|p| p as u8),
        error: row.error,
        updated_at: row.updated_at.to_rfc3339(),
    }
}

/// Newest active assignment for one device (detail enrichment).
async fn active_ota_for(
    db: &DatabaseConnection,
    device_pk: i64,
) -> Result<Option<pnex_core::OtaAssignmentState>> {
    Ok(crate::services::ota::newest_active(db, device_pk)
        .await?
        .map(ota_state_dto))
}

/// Assemble le DTO `Device` (pnex-core) depuis le registre + son contexte.
#[allow(clippy::too_many_arguments)] // même école que handle_announce
pub(super) fn device_dto(
    device: device_registries::Model,
    predefined: &predefined_devices::Model,
    type_name: &str,
    capabilities: &[device_capabilities::Model],
    token: Option<&device_tokens::Model>,
    last_seen: Option<String>,
    connected: bool,
    latest_build: Option<pnex_core::LatestBuild>,
    ota_state: Option<pnex_core::OtaAssignmentState>,
) -> pnex_core::Device {
    pnex_core::Device {
        id: device.id,
        org_id: device.org_id,
        device_id: device.device_id,
        metadata: device.metadata,
        predefined_device_name: predefined.name.clone(),
        device_type: type_name.to_string(),
        capabilities: capabilities
            .iter()
            .map(|c| pnex_core::DeviceCapability {
                id: c.id,
                name: c.name.clone(),
                mode: capability_mode_str(c.mode).to_string(),
            })
            .collect(),
        active: device.active,
        last_seen,
        connected,
        device_token: token.map(|t| pnex_core::DeviceTokenInfo {
            token: t.token.clone(),
            encryption_key: t.encryption_key.clone(),
            is_active: t.is_active,
            created: Some(t.created_at.to_rfc3339()),
        }),
        latest_build,
        fw_version: device.fw_version,
        ota_ready: device.ota_ready.unwrap_or(false),
        ota: ota_state,
        allow_dynamic_measurements: device.allow_dynamic_measurements,
        discovered_measurements: if device.allow_dynamic_measurements {
            discovered_names(&device.discovered_measurements)
        } else {
            Vec::new()
        },
        max_unique_measurements: device.max_unique_measurements,
        firmware_project_id: device.firmware_project_id,
    }
}

/// DTO complet d'un device isolé (détail / création / update).
pub(super) async fn device_full(
    db: &DatabaseConnection,
    device: device_registries::Model,
) -> Result<pnex_core::Device> {
    let predefined = predefined_devices::Entity::find_by_id(device.predefined_device_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::InternalServerError)?;
    let type_name = device_types::Entity::find_by_id(predefined.device_type_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|t| t.name)
        .unwrap_or_default();
    let capabilities = capabilities_of(db, &[predefined.id])
        .await?
        .remove(&predefined.id)
        .unwrap_or_default();
    let token = device_tokens::Entity::find()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let seen = crate::services::device_liveness::seen_of(db, device.id).await?;
    let last_seen = seen.last_seen.map(|t| t.to_rfc3339());
    let connected = seen.connected;
    let latest_build = latest_builds_by_device(
        build_records::Entity::find()
            .filter(build_records::Column::OrgId.eq(device.org_id))
            .filter(build_records::Column::DeviceId.eq(&device.device_id))
            .order_by_desc(build_records::Column::Id)
            .all(db)
            .await
            .map_err(|_| Error::InternalServerError)?,
    )
    .remove(&device.device_id);
    let ota_state = active_ota_for(db, device.id).await?;
    Ok(device_dto(
        device,
        &predefined,
        &type_name,
        &capabilities,
        token.as_ref(),
        last_seen,
        connected,
        latest_build,
        ota_state,
    ))
}

pub(super) async fn find_device(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<Option<device_registries::Model>> {
    device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}
