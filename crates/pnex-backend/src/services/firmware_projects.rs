//! Custom firmware projects (custom-firmware.md D89) — append-only
//! revisions of one `main.cpp` + pinned catalog libraries, org-scoped.
//! School `services/functions.rs` (transaction, circular current pointer).
//!
//! Also hosts the « Verify » checks (D90): rows in `firmware_checks`, run
//! by the queue worker (`workers::firmware_check` — the process that has
//! PlatformIO, `pnex-builder` in containers), polled by the IDE.

use pnex_core::caps::Soc;
use pnex_core::firmware::{
    check_main_cpp, resolve_lib_deps, CompileDiagnostic, CreateFirmwareProject,
    SaveFirmwareRevision, SourceViolation, STARTER_SKETCH,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use sha2::Digest;
use uuid::Uuid;

use crate::models::_entities::{
    device_registries, firmware_checks, firmware_projects, firmware_revisions,
};

/// Write failure of a project/revision.
#[derive(Debug)]
pub enum FirmwareWriteError {
    /// Field-level machine token (`required`, `max_length:200`, `invalid`).
    Field(&'static str, String),
    /// Sketch refused by the lexical guard (D91).
    Source(Vec<SourceViolation>),
    /// Library outside the catalog / incompatible with the chip (code, id).
    Lib(&'static str, String),
    Db(String),
}

fn db_err(ctx: &str) -> impl Fn(sea_orm::DbErr) -> FirmwareWriteError + '_ {
    move |e| FirmwareWriteError::Db(format!("{ctx}: {e}"))
}

fn validate_name(name: &str) -> Result<String, FirmwareWriteError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(FirmwareWriteError::Field("name", "required".into()));
    }
    if name.chars().count() > 200 {
        return Err(FirmwareWriteError::Field("name", "max_length:200".into()));
    }
    Ok(name.to_string())
}

fn clean_optional(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Chip family wire id → SoC (400 `chip_family: invalid` otherwise).
pub fn parse_chip_family(s: &str) -> Result<Soc, FirmwareWriteError> {
    Soc::from_board_soc(s).ok_or_else(|| FirmwareWriteError::Field("chip_family", "invalid".into()))
}

/// Validates the sketch and the libraries for the chip.
fn validate_content(
    main_cpp: &str,
    lib_deps: &[String],
    soc: Soc,
) -> Result<(), FirmwareWriteError> {
    check_main_cpp(main_cpp).map_err(FirmwareWriteError::Source)?;
    resolve_lib_deps(lib_deps, soc).map_err(|(code, id)| FirmwareWriteError::Lib(code, id))?;
    Ok(())
}

/// sha256 hex of the revision content (dedup of identical saves).
pub fn content_hash(main_cpp: &str, lib_deps: &[String]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(main_cpp.as_bytes());
    h.update([0u8]);
    h.update(lib_deps.join("\n").as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Catalog ids of a stored revision.
pub fn revision_libs(rev: &firmware_revisions::Model) -> Vec<String> {
    serde_json::from_value(rev.lib_deps.clone()).unwrap_or_default()
}

/// Project + revision 1 (starter sketch when `main_cpp` is absent).
pub async fn create_project(
    db: &DatabaseConnection,
    org_id: i64,
    input: &CreateFirmwareProject,
) -> Result<(firmware_projects::Model, firmware_revisions::Model), FirmwareWriteError> {
    let name = validate_name(&input.name)?;
    let soc = parse_chip_family(&input.chip_family)?;
    let main_cpp = input
        .main_cpp
        .clone()
        .unwrap_or_else(|| STARTER_SKETCH.to_string());
    validate_content(&main_cpp, &input.lib_deps, soc)?;

    let txn = db.begin().await.map_err(db_err("transaction"))?;
    let created = firmware_projects::ActiveModel {
        org_id: Set(org_id),
        name: Set(name),
        description: Set(clean_optional(&input.description)),
        chip_family: Set(soc.name().to_string()),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(db_err("insert project"))?;
    let revision = firmware_revisions::ActiveModel {
        firmware_project_id: Set(created.id),
        org_id: Set(org_id),
        revision_number: Set(1),
        content_hash: Set(content_hash(&main_cpp, &input.lib_deps)),
        main_cpp: Set(main_cpp),
        lib_deps: Set(serde_json::json!(input.lib_deps)),
        note: Set(None),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(db_err("insert revision"))?;
    // Circular current pointer set once both rows exist.
    let mut active: firmware_projects::ActiveModel = created.into();
    active.current_revision_id = Set(Some(revision.id));
    let created = active
        .update(&txn)
        .await
        .map_err(db_err("current revision"))?;
    txn.commit().await.map_err(db_err("commit"))?;
    Ok((created, revision))
}

/// Current revision = highest `revision_number` (append-only).
pub async fn latest_revision<C: sea_orm::ConnectionTrait>(
    db: &C,
    project_id: i64,
) -> Result<Option<firmware_revisions::Model>, sea_orm::DbErr> {
    firmware_revisions::Entity::find()
        .filter(firmware_revisions::Column::FirmwareProjectId.eq(project_id))
        .order_by_desc(firmware_revisions::Column::RevisionNumber)
        .one(db)
        .await
}

/// Metadata update and/or new revision. The caller checked the optimistic
/// concurrency; an unchanged content (same hash) creates no revision.
pub async fn save_revision(
    db: &DatabaseConnection,
    project: firmware_projects::Model,
    current: firmware_revisions::Model,
    input: &SaveFirmwareRevision,
) -> Result<(firmware_projects::Model, firmware_revisions::Model), FirmwareWriteError> {
    let soc = parse_chip_family(&project.chip_family)?;
    let txn = db.begin().await.map_err(db_err("transaction"))?;
    let mut project_am: firmware_projects::ActiveModel = project.clone().into();
    let mut touched = false;
    if let Some(name) = &input.name {
        project_am.name = Set(validate_name(name)?);
        touched = true;
    }
    if input.description.is_some() {
        project_am.description = Set(clean_optional(&input.description));
        touched = true;
    }
    let main_cpp = input
        .main_cpp
        .clone()
        .unwrap_or_else(|| current.main_cpp.clone());
    let lib_deps = input
        .lib_deps
        .clone()
        .unwrap_or_else(|| revision_libs(&current));
    let hash = content_hash(&main_cpp, &lib_deps);
    let mut revision = current.clone();
    if hash != current.content_hash {
        validate_content(&main_cpp, &lib_deps, soc)?;
        revision = firmware_revisions::ActiveModel {
            firmware_project_id: Set(project.id),
            org_id: Set(project.org_id),
            revision_number: Set(current.revision_number + 1),
            content_hash: Set(hash),
            main_cpp: Set(main_cpp),
            lib_deps: Set(serde_json::json!(lib_deps)),
            note: Set(clean_optional(&input.note)),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(db_err("insert revision"))?;
        project_am.current_revision_id = Set(Some(revision.id));
        touched = true;
    }
    let project = if touched {
        project_am
            .update(&txn)
            .await
            .map_err(db_err("update project"))?
    } else {
        project
    };
    txn.commit().await.map_err(db_err("commit"))?;
    Ok((project, revision))
}

/// Count of devices running a project (one query per project — lists stay
/// small, school functions list).
pub async fn device_count(db: &DatabaseConnection, project_id: i64) -> Result<i64, sea_orm::DbErr> {
    device_registries::Entity::find()
        .filter(device_registries::Column::FirmwareProjectId.eq(project_id))
        .count(db)
        .await
        .map(|n| n as i64)
}

// ───────────────────────────── Verify checks (D90) ─────────────────────────────

/// Default PlatformIO board of a chip family for a device-less check.
pub fn default_check_board(soc: Soc) -> &'static str {
    match soc {
        Soc::Esp8266 => "nodemcuv2",
        Soc::Esp32 => "esp32dev",
        Soc::Esp32C3 => "esp32-c3-devkitm-1",
        Soc::Esp32C6 => "esp32-c6-devkitc-1",
        Soc::Esp32S3 => "esp32-s3-devkitc-1",
    }
}

pub const CHECK_QUEUED: &str = "queued";
pub const CHECK_RUNNING: &str = "running";

/// Compile inputs of a device-less check: generic project of the chip,
/// default board, the revision sketch + resolved catalog libraries.
pub fn check_inputs(
    project: &firmware_projects::Model,
    revision: &firmware_revisions::Model,
    sandbox: Option<pnex_firmware_builder::Sandbox>,
) -> Result<
    (
        pnex_firmware_builder::DeviceSpec,
        pnex_firmware_builder::BuildOptions,
    ),
    String,
> {
    let soc = Soc::from_board_soc(&project.chip_family).ok_or("unknown chip family")?;
    let lib_specs = resolve_lib_deps(&revision_libs(revision), soc)
        .map_err(|(code, id)| format!("{code}: {id}"))?
        .into_iter()
        .map(String::from)
        .collect();
    let device = pnex_firmware_builder::DeviceSpec {
        org_id: project.org_id,
        device_id: "check".into(),
        project: pnex_core::firmware::generic_project(soc).into(),
        soc: soc.name().into(),
        pio_board: Some(default_check_board(soc).into()),
        board_name: Some("check".into()),
        screen: None,
        fw_version: "0".into(),
    };
    let opts = pnex_firmware_builder::BuildOptions {
        custom: Some(pnex_firmware_builder::CustomSource {
            main_cpp: revision.main_cpp.clone(),
            lib_specs,
        }),
        sandbox,
    };
    Ok((device, opts))
}

/// Placeholder credentials: a check never compiles real secrets.
pub fn check_secrets() -> pnex_firmware_builder::BuildSecrets {
    pnex_firmware_builder::BuildSecrets {
        wifi_ssid: "check".into(),
        wifi_password: "check".into(),
        host: "localhost".into(),
        token: "check".into(),
        device_id: "check".into(),
        encryption_key: String::new(),
        ca_cert_pem: None,
        ota_pubkey: String::new(),
        client_cert: (String::new(), String::new()),
    }
}

/// Records a queued check of the current revision (the caller enqueues the
/// worker job).
pub async fn create_check(
    db: &DatabaseConnection,
    project: &firmware_projects::Model,
    revision_number: i64,
) -> Result<firmware_checks::Model, sea_orm::DbErr> {
    firmware_checks::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(project.org_id),
        firmware_project_id: Set(project.id),
        revision_number: Set(revision_number),
        status: Set(CHECK_QUEUED.into()),
        diagnostics: Set(None),
        log_tail: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        updated_at: Set(chrono::Utc::now().into()),
    }
    .insert(db)
    .await
}

/// Check of this org/project.
pub async fn get_check(
    db: &DatabaseConnection,
    org_id: i64,
    project_id: i64,
    id: Uuid,
) -> Result<Option<firmware_checks::Model>, sea_orm::DbErr> {
    firmware_checks::Entity::find_by_id(id)
        .filter(firmware_checks::Column::OrgId.eq(org_id))
        .filter(firmware_checks::Column::FirmwareProjectId.eq(project_id))
        .one(db)
        .await
}

/// Terminal (or running) state of a check — the worker is the only writer.
pub async fn set_check_state(
    db: &DatabaseConnection,
    check: firmware_checks::Model,
    status: &str,
    diagnostics: Option<Vec<CompileDiagnostic>>,
    log_tail: Option<String>,
) -> Result<(), sea_orm::DbErr> {
    let mut am: firmware_checks::ActiveModel = check.into();
    am.status = Set(status.into());
    am.diagnostics = Set(diagnostics.map(|d| serde_json::json!(d)));
    am.log_tail = Set(log_tail);
    am.updated_at = Set(chrono::Utc::now().into());
    am.update(db).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_covers_libs() {
        let a = content_hash("x", &[]);
        assert_eq!(a, content_hash("x", &[]));
        assert_ne!(a, content_hash("x", &["dht".into()]));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn every_family_has_a_check_board() {
        for soc in pnex_core::firmware::CHIP_FAMILIES {
            assert!(!default_check_board(soc).is_empty());
        }
    }
}
