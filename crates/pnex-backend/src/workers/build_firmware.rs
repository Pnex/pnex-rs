//! Worker de build firmware (Phase 6) — consomme la queue PostgreSQL.
//!
//! Parité du poll Celery Django (`update_build_record`) transposée : le
//! worker exécute le pipeline ([`pnex_firmware_builder::run_build`]) et
//! écrit directement les transitions `running → succeeded|failed` dans
//! `build_records` (plus de poll 30 s : le worker EST l'exécutant).
//!
//! Secrets : WiFi/hôte viennent des args de la queue (limite documentée —
//! visibles de l'admin DB dans `pg_loco_queue.task_data`, parité spec k8s
//! Django ; purge via `cargo loco jobs clear-jobs`) ; le **token et la clé
//! de chiffrement sont relus en base** au moment du `perform`, ils ne
//! transitent jamais par la queue.
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

use crate::models::_entities::{build_records, device_registries, device_tokens};
use crate::services::firmware::{FirmwareSettings, PHASE_FAILED, PHASE_RUNNING, PHASE_SUCCEEDED};

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
    // Secrets d'exécution (cf. limite documentée en tête de module).
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub pnex_host: String,
}

pub struct BuildFirmwareWorker {
    db: sea_orm::DatabaseConnection,
    settings: FirmwareSettings,
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
        }
    }

    async fn perform(&self, args: BuildFirmwareArgs) -> Result<()> {
        set_phase(&self.db, args.build_record_id, PHASE_RUNNING, false, None).await?;
        match self.run(&args).await {
            Ok(artifact) => {
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
            }
            // Message d'erreur dans les logs serveur uniquement — jamais
            // renvoyé au client (peut contenir des chemins/fragments).
            Err(msg) => {
                tracing::error!(build = args.build_record_id, erreur = %msg, "build firmware échoué");
                set_phase(&self.db, args.build_record_id, PHASE_FAILED, false, None).await?;
            }
        }
        Ok(())
    }
}

impl BuildFirmwareWorker {
    /// Exécute le pipeline complet pour les args du job.
    async fn run(&self, args: &BuildFirmwareArgs) -> std::result::Result<BuildArtifact, String> {
        // Token + clé relus en base (jamais via la queue).
        let (token, encryption_key) = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(args.org_id))
            .filter(device_registries::Column::DeviceId.eq(&args.device_id))
            .find_also_related(device_tokens::Entity)
            .one(&self.db)
            .await
            .map_err(|e| format!("db : {e}"))?
            .and_then(|(_, token)| token)
            .map(|token| (token.token, token.encryption_key))
            .ok_or_else(|| format!("token du device {} introuvable", args.device_id))?;

        let config = BuildConfig {
            source: self.settings.source(),
            pio_cmd: self.settings.pio_cmd.clone(),
            esptool_cmd: self.settings.esptool_cmd.clone(),
            timeout_secs: self.settings.timeout_secs,
            store: self.settings.store()?,
        };
        let secrets = BuildSecrets {
            wifi_ssid: args.wifi_ssid.clone(),
            wifi_password: args.wifi_password.clone(),
            host: args.pnex_host.clone(),
            token,
            device_id: args.device_id.clone(),
            encryption_key,
        };
        let device = DeviceSpec {
            org_id: args.org_id,
            device_id: args.device_id.clone(),
            project: args.predefined_device_name.clone(),
            soc: args.soc.clone(),
        };
        pnex_firmware_builder::run_build(&config, &secrets, &device)
            .await
            .map_err(|e| e.to_string())
    }
}
