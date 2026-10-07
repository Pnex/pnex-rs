//! Worker de build firmware (Phase 6) — consomme la queue PostgreSQL.
//!
//! Adapted from the legacy `update_build_record` poll job: the
//! worker runs the pipeline ([`pnex_firmware_builder::run_build`]) and
//! writes the `running → succeeded|failed` transitions directly to
//! `build_records` (no more 30 s poll: the worker IS the executor).
//!
//! Secrets: the queue args carry the WiFi SSID and the **vault id** of its
//! password (secrets.md lot S6), decrypted here at `perform` time; the
//! **token and the encryption key are re-read from the database**. No
//! secret value travels through `pg_loco_queue.task_data`.
//!
//! Échec de build → record `failed` + `perform` retourne `Ok(())` : les
//! échecs de compilation sont déterministes, pas de rejeu (retries bornés
//! — conception §1).

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};

use pnex_firmware_builder::{BuildArtifact, BuildConfig, BuildSecrets, DeviceSpec};

use crate::models::_entities::{
    build_records, device_registries, device_tokens, firmware_projects,
};
use crate::services::firmware::{FirmwareSettings, PHASE_FAILED, PHASE_RUNNING, PHASE_SUCCEEDED};
use crate::services::firmware_projects::{check_inputs, latest_revision};

/// Charge utile du job. `build_record_id` est la seule clé nécessaire — le
/// reste est figé au moment de la demande (le device peut avoir changé
/// d'org entre-temps ; on compile pour l'org demandeur).
#[derive(Debug, Serialize, Deserialize)]
pub struct BuildFirmwareArgs {
    pub build_record_id: i64,
    pub org_id: i64,
    pub device_id: String,
    /// Sous-répertoire du workspace firmware (= predefined_device_name).
    pub predefined_device_name: String,
    /// SoC du board (offsets merge-bin).
    pub soc: String,
    /// PlatformIO board id de la variante — `None` = ini hardcodé inchangé.
    pub pio_board: Option<String>,
    /// Id fil de la board (annonce firmware `PNEX_BOARD_NAME`).
    pub board_name: Option<String>,
    /// Driver écran à compiler quand l'utilisateur l'a activé (define
    /// `PNEX_SCREEN_{KIND}`=1) — `None` = pas d'écran.
    #[serde(default)]
    pub screen: Option<pnex_firmware_builder::ScreenSpec>,
    pub wifi_ssid: String,
    /// Vault secret of the WiFi password (decrypted by the worker).
    pub wifi_secret_id: uuid::Uuid,
    pub pnex_host: String,
    /// WebSocket `wss://` (TLS) ou `ws://` (local) — figé à la demande.
    pub ws_ssl: bool,
}

pub struct BuildFirmwareWorker {
    db: sea_orm::DatabaseConnection,
    settings: FirmwareSettings,
    /// Vault keyring (WiFi password); `None` = misconfigured server, the
    /// build fails with an explicit error.
    keyring: Option<crate::services::secrets::Keyring>,
}

/// Why a build failed (O4): a code of `pnex_core::BUILD_FAILURE_CODES`,
/// the server log message (never shown: may hold paths) and, for a tool
/// failure, its output tail with the credentials masked (shown).
#[derive(Debug)]
struct Failure {
    code: &'static str,
    log: String,
    detail: Option<String>,
}

impl Failure {
    fn new(code: &'static str, log: impl Into<String>) -> Self {
        Self {
            code,
            log: log.into(),
            detail: None,
        }
    }

    /// Classifies a builder error; tool output is scrubbed of `secrets`.
    fn from_build(e: pnex_firmware_builder::BuildError, secrets: &BuildSecrets) -> Self {
        use pnex_firmware_builder::BuildError as E;
        let log = e.to_string();
        let (code, detail) = match e {
            E::Timeout => ("build_timeout", None),
            E::Tool(out) => {
                let code = if out.starts_with("pio run") {
                    "build_compile"
                } else if out.starts_with("esptool") {
                    "build_merge"
                } else {
                    "build_tool"
                };
                // Drop the "label : status" header line, keep the output.
                let body = out
                    .split_once('\n')
                    .map(|(_, rest)| rest)
                    .unwrap_or_default();
                (
                    code,
                    Some(bounded_tail(&pnex_firmware_builder::scrub_secrets(
                        body, secrets,
                    ))),
                )
            }
            E::Source(_) => ("build_source", None),
            E::NotFound(_) => ("build_artifact", None),
            E::Store(_) => ("build_store", None),
        };
        Self {
            code,
            log,
            detail: detail.filter(|d| !d.trim().is_empty()),
        }
    }
}

/// Last characters of a detail, within `BUILD_FAILURE_DETAIL_MAX`.
fn bounded_tail(text: &str) -> String {
    let n = text.chars().count();
    let skip = n.saturating_sub(pnex_core::BUILD_FAILURE_DETAIL_MAX);
    text.chars().skip(skip).collect()
}

/// Records the failure reason of a build (best effort: the phase is
/// already `failed`).
async fn set_failure(db: &sea_orm::DatabaseConnection, id: i64, failure: &Failure) {
    let reason = build_records::ActiveModel {
        id: Set(id),
        failure_code: Set(Some(failure.code.to_string())),
        failure_detail: Set(failure.detail.clone()),
        ..Default::default()
    };
    if let Err(e) = reason.update(db).await {
        tracing::warn!(build = id, "build failure reason not recorded: {e}");
    }
}

/// Pose une transition de phase (le worker est l'unique écrivain des
/// phases running/succeeded/failed ; `queued` est posé par le contrôleur).
async fn set_phase(
    db: &sea_orm::DatabaseConnection,
    id: i64,
    phase: &str,
    success: bool,
    artifact_key: Option<String>,
) -> Result<()> {
    let model = build_records::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| Error::Message(format!("build record {id} introuvable")))?;
    let mut record: build_records::ActiveModel = model.into();
    record.build_phase = Set(Some(phase.to_string()));
    record.success = Set(success);
    record.firmware_bin_s3_key = Set(artifact_key);
    record.update(db).await?;
    Ok(())
}

#[async_trait]
impl BackgroundWorker<BuildFirmwareArgs> for BuildFirmwareWorker {
    fn build(ctx: &AppContext) -> Self {
        Self {
            db: ctx.db.clone(),
            settings: FirmwareSettings::from_config(&ctx.config),
            keyring: crate::services::secrets::Keyring::from_config(&ctx.config).ok(),
        }
    }

    async fn perform(&self, args: BuildFirmwareArgs) -> Result<()> {
        set_phase(&self.db, args.build_record_id, PHASE_RUNNING, false, None).await?;
        match self.run(&args).await {
            Ok((artifact, revision_id)) => {
                tracing::info!(
                    build = args.build_record_id,
                    key = %artifact.key,
                    "build firmware réussi"
                );
                set_phase(
                    &self.db,
                    args.build_record_id,
                    PHASE_SUCCEEDED,
                    true,
                    Some(artifact.key),
                )
                .await?;
                // Stamp the firmware version (the record id — same value
                // baked into the binary as PNEX_FW_VERSION) + the raw app
                // image metadata consumed by OTA deployments + the embedded
                // tree fingerprint (staleness tripwire).
                let stamp: build_records::ActiveModel = build_records::ActiveModel {
                    id: Set(args.build_record_id),
                    fw_version: Set(Some(args.build_record_id.to_string())),
                    ota_sha256: Set(Some(artifact.ota_sha256)),
                    ota_size_bytes: Set(Some(artifact.ota_size_bytes as i64)),
                    sources_fingerprint: Set(Some(
                        pnex_firmware_builder::source_fingerprint().to_string(),
                    )),
                    // Custom firmware revision compiled (D89) — "which code
                    // runs where"; None for the generic firmware.
                    firmware_revision_id: Set(revision_id),
                    ..Default::default()
                };
                stamp.update(&self.db).await?;
                // Retention: keep the newest records of the device (each
                // build is a new row). Best effort, never fails the build.
                match self.settings.store(&self.db) {
                    Ok(store) => {
                        if let Err(e) = crate::services::build_retention::prune_device_builds(
                            &self.db,
                            store.as_ref(),
                            args.org_id,
                            &args.device_id,
                        )
                        .await
                        {
                            tracing::warn!(
                                build = args.build_record_id,
                                "build retention failed: {e}"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(build = args.build_record_id, "build retention skipped: {e}")
                    }
                }
            }
            // The full message stays in the server logs (paths, fragments);
            // the record keeps a code and the scrubbed tool output (O4).
            Err(failure) => {
                tracing::error!(
                    build = args.build_record_id,
                    code = failure.code,
                    erreur = %failure.log,
                    "build firmware échoué"
                );
                set_phase(&self.db, args.build_record_id, PHASE_FAILED, false, None).await?;
                set_failure(&self.db, args.build_record_id, &failure).await;
            }
        }
        Ok(())
    }
}

impl BuildFirmwareWorker {
    /// Exécute le pipeline complet pour les args du job.
    /// Returns the artifact and, for a custom firmware device, the id of
    /// the revision compiled.
    async fn run(
        &self,
        args: &BuildFirmwareArgs,
    ) -> std::result::Result<(BuildArtifact, Option<i64>), Failure> {
        // Token + clé relus en base (jamais via la queue).
        let (registry, token) = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(args.org_id))
            .filter(device_registries::Column::DeviceId.eq(&args.device_id))
            .find_also_related(device_tokens::Entity)
            .one(&self.db)
            .await
            .map_err(|e| Failure::new("build_internal", format!("db : {e}")))?
            .ok_or_else(|| {
                Failure::new(
                    "build_device",
                    format!("device {} introuvable", args.device_id),
                )
            })?;
        let (token, encryption_key) = token
            .map(|token| (token.token, token.encryption_key))
            .ok_or_else(|| {
                Failure::new(
                    "build_device",
                    format!("token du device {} introuvable", args.device_id),
                )
            })?;

        let config = BuildConfig {
            pio_cmd: self.settings.pio_cmd.clone(),
            esptool_cmd: self.settings.esptool_cmd.clone(),
            timeout_secs: self.settings.timeout_secs,
            store: self
                .settings
                .store(&self.db)
                .map_err(|e| Failure::new("build_store", e))?,
        };
        let ring = self
            .keyring
            .as_ref()
            .ok_or_else(|| Failure::new("build_wifi", "secrets keyring unavailable"))?;
        let wifi_password = crate::services::secrets::wifi::reveal(
            &self.db,
            ring,
            args.org_id,
            args.wifi_secret_id,
        )
        .await
        .map_err(|e| {
            Failure::new(
                "build_wifi",
                format!("WiFi password unavailable from the vault: {e}"),
            )
        })?;
        let ca_cert_pem = device_ca_pem();
        if args.ws_ssl && ca_cert_pem.is_none() && self.settings.require_device_ca {
            return Err(Failure::new(
                "build_no_ca",
                "no device CA to pin (PNEX_CA_CERT_FILE): wss build refused",
            ));
        }
        if let Some(pem) = ca_cert_pem.as_deref() {
            check_device_ca_size(pem, &args.soc)?;
        }
        let secrets = BuildSecrets {
            wifi_ssid: args.wifi_ssid.clone(),
            wifi_password,
            host: args.pnex_host.clone(),
            ws_ssl: args.ws_ssl,
            token,
            device_id: args.device_id.clone(),
            encryption_key,
            ca_cert_pem,
        };
        let mut device = DeviceSpec {
            org_id: args.org_id,
            device_id: args.device_id.clone(),
            project: args.predefined_device_name.clone(),
            soc: args.soc.clone(),
            pio_board: args.pio_board.clone(),
            board_name: args.board_name.clone(),
            screen: args.screen.clone(),
            fw_version: args.build_record_id.to_string(),
        };
        let Some(project_id) = registry.firmware_project_id else {
            let artifact = pnex_firmware_builder::run_build(&config, &secrets, &device)
                .await
                .map_err(|e| Failure::from_build(e, &secrets))?;
            return Ok((artifact, None));
        };

        // Custom firmware (D89/D94): the latest revision of the device's
        // project, compiled in the generic project of its chip.
        if !self.settings.custom.enabled {
            return Err(Failure::new(
                "build_custom_disabled",
                "custom firmware builds are disabled on this worker",
            ));
        }
        let project = firmware_projects::Entity::find_by_id(project_id)
            .filter(firmware_projects::Column::OrgId.eq(args.org_id))
            .one(&self.db)
            .await
            .map_err(|e| Failure::new("build_internal", format!("db : {e}")))?
            .ok_or_else(|| Failure::new("build_custom_project", "firmware project not found"))?;
        let revision = latest_revision(&self.db, project.id)
            .await
            .map_err(|e| Failure::new("build_internal", format!("db : {e}")))?
            .ok_or_else(|| {
                Failure::new("build_custom_project", "firmware project without revision")
            })?;
        let (check_device, opts) =
            check_inputs(&project, &revision, self.settings.custom.sandbox.clone())
                .map_err(|e| Failure::new("build_custom_project", e))?;
        device.project = check_device.project;
        // A custom firmware owns every pin: no debug screen driver compiled
        // in (it would drive pins the sketch may use).
        device.screen = None;
        if device.pio_board.is_none() {
            device.pio_board = check_device.pio_board;
        }
        if self.settings.custom.sandbox.is_none() {
            tracing::warn!(
                build = args.build_record_id,
                "custom firmware compiled WITHOUT sandbox"
            );
        }
        let artifact = pnex_firmware_builder::run_build_with(&config, &secrets, &device, &opts)
            .await
            .map_err(|e| Failure::from_build(e, &secrets))?;
        Ok((artifact, Some(revision.id)))
    }
}

/// Root CA the firmware pins over wss (D70): the file named by
/// `PNEX_CA_CERT_FILE`, written by the TLS edge's pki-init (local CA, or
/// ISRG Root X1 in cloud mode). Read at every build so a CA change needs
/// no restart; missing/unreadable → no pin (logged — ESP32 wss then fails).
fn device_ca_pem() -> Option<String> {
    let path = std::env::var("PNEX_CA_CERT_FILE").ok()?;
    match std::fs::read_to_string(&path) {
        Ok(pem) if pem.contains("BEGIN CERTIFICATE") => Some(pem),
        Ok(_) => {
            tracing::warn!(%path, "PNEX_CA_CERT_FILE holds no PEM certificate — build without CA");
            None
        }
        Err(e) => {
            tracing::warn!(%path, "PNEX_CA_CERT_FILE unreadable ({e}) — build without CA");
            None
        }
    }
}

/// Refuses a device CA the target firmware cannot pin (SEC-16): the
/// firmware would otherwise boot unpinned, or with a pre-fix library
/// overflow its buffer.
fn check_device_ca_size(pem: &str, soc: &str) -> Result<(), Failure> {
    let max = pnex_core::builds::device_ca_max_pem_bytes(soc);
    if pem.len() > max {
        return Err(Failure::new(
            "build_ca_too_large",
            format!(
                "device CA is {} bytes of PEM, the {soc} firmware pins at most {max}: keep only the root that signs the server certificate",
                pem.len()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SEC-16: a CA bundle too large for the chip fails the build with
    /// its own code; one root fits everywhere, two roots only on ESP32.
    #[test]
    fn oversized_device_ca_is_refused_per_soc() {
        let one_root = "x".repeat(765);
        let two_roots = "x".repeat(2676);
        assert!(check_device_ca_size(&one_root, "esp8266").is_ok());
        assert!(check_device_ca_size(&two_roots, "esp32-c6").is_ok());
        let err = check_device_ca_size(&two_roots, "esp8266").unwrap_err();
        assert_eq!(err.code, "build_ca_too_large");
        let three_roots = "x".repeat(4615);
        assert_eq!(
            check_device_ca_size(&three_roots, "esp32")
                .unwrap_err()
                .code,
            "build_ca_too_large"
        );
    }

    /// No WiFi password in the queued job: only the vault id.
    #[test]
    fn queued_args_never_carry_the_wifi_password() {
        let args = BuildFirmwareArgs {
            build_record_id: 1,
            org_id: 2,
            device_id: "d".into(),
            predefined_device_name: "p".into(),
            soc: "esp32".into(),
            pio_board: None,
            board_name: None,
            screen: None,
            wifi_ssid: "ssid".into(),
            wifi_secret_id: uuid::Uuid::from_u128(3),
            pnex_host: "h".into(),
            ws_ssl: true,
        };
        let json = serde_json::to_value(&args).unwrap();
        assert!(json.get("wifi_password").is_none(), "{json}");
        assert_eq!(json["wifi_secret_id"], uuid::Uuid::from_u128(3).to_string());
    }
}
