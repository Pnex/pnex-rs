//! Compile-only check of a custom firmware revision (custom-firmware.md
//! D90) — queue worker, same school as [`super::stitch_panorama`]: tiny
//! args, the row `firmware_checks` carries the state (`queued → running →
//! succeeded|failed|error`). Runs where PlatformIO lives (the `pnex-builder`
//! container, or `--server-and-worker` in dev); no secrets, no artifact.
//!
//! A compile failure is a normal outcome (`failed` + diagnostics); tool or
//! infra failures (spawn, timeout, disabled on this worker) are `error`.
//! `perform` returns `Ok(())` in every settled case: no replay.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::_entities::{firmware_checks, firmware_projects, firmware_revisions};
use crate::services::firmware::FirmwareSettings;
use crate::services::firmware_projects::{
    check_inputs, check_secrets, set_check_state, CHECK_RUNNING,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct FirmwareCheckArgs {
    pub check_id: Uuid,
}

pub struct FirmwareCheckWorker {
    ctx: AppContext,
}

impl FirmwareCheckWorker {
    /// Loads the project + revision of a check; `Err` = short message.
    async fn inputs(
        &self,
        check: &firmware_checks::Model,
    ) -> std::result::Result<(firmware_projects::Model, firmware_revisions::Model), String> {
        let project = firmware_projects::Entity::find_by_id(check.firmware_project_id)
            .filter(firmware_projects::Column::OrgId.eq(check.org_id))
            .one(&self.ctx.db)
            .await
            .map_err(|e| format!("db: {e}"))?
            .ok_or("project not found")?;
        let revision = firmware_revisions::Entity::find()
            .filter(firmware_revisions::Column::FirmwareProjectId.eq(project.id))
            .filter(firmware_revisions::Column::RevisionNumber.eq(check.revision_number))
            .one(&self.ctx.db)
            .await
            .map_err(|e| format!("db: {e}"))?
            .ok_or("revision not found")?;
        Ok((project, revision))
    }
}

#[async_trait]
impl BackgroundWorker<FirmwareCheckArgs> for FirmwareCheckWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: FirmwareCheckArgs) -> Result<()> {
        let Some(check) = firmware_checks::Entity::find_by_id(args.check_id)
            .one(&self.ctx.db)
            .await?
        else {
            return Ok(()); // project deleted meanwhile (cascade)
        };
        let settings = FirmwareSettings::from_config(&self.ctx.config);
        let db = &self.ctx.db;
        if !settings.custom.enabled {
            // The API accepted the check but this worker has the feature
            // off (env differs between containers) — say so explicitly.
            set_check_state(
                db,
                check,
                "error",
                None,
                Some("Custom firmware builds are disabled on the build worker.".into()),
            )
            .await?;
            return Ok(());
        }
        if settings.custom.sandbox.is_none() {
            tracing::warn!(check = %args.check_id, "custom firmware compiled WITHOUT sandbox");
        }
        let (project, revision) = match self.inputs(&check).await {
            Ok(pair) => pair,
            Err(msg) => {
                set_check_state(db, check, "error", None, Some(msg)).await?;
                return Ok(());
            }
        };
        let (device, opts) =
            match check_inputs(&project, &revision, settings.custom.sandbox.clone()) {
                Ok(pair) => pair,
                Err(msg) => {
                    set_check_state(db, check, "error", None, Some(msg)).await?;
                    return Ok(());
                }
            };
        let running = {
            let mut am: firmware_checks::ActiveModel = check.clone().into();
            am.status = sea_orm::ActiveValue::Set(CHECK_RUNNING.into());
            am.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            sea_orm::ActiveModelTrait::update(am, db).await?
        };
        let outcome = pnex_firmware_builder::compile_check(
            &settings.pio_cmd,
            settings.timeout_secs,
            &check_secrets(),
            &device,
            &opts,
        )
        .await;
        match outcome {
            Ok(o) => {
                let status = if o.ok { "succeeded" } else { "failed" };
                tracing::info!(check = %args.check_id, status, "firmware check settled");
                set_check_state(db, running, status, Some(o.diagnostics), Some(o.log_tail)).await?;
            }
            Err(e) => {
                tracing::error!(check = %args.check_id, "firmware check error: {e}");
                set_check_state(db, running, "error", None, Some(e.to_string())).await?;
            }
        }
        Ok(())
    }
}
