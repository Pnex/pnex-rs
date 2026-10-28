//! Server stitch worker (Take 360 V2, plan cheerful-booping-teacup B3)
//! — consumes the PostgreSQL queue, exact pattern of [`super::build_firmware`]
//! : `perform_later` with tiny args, `perform` that writes the
//! `running → succeeded|failed` transitions directly.
//!
//! Pipeline: read the frames from the MediaStore (same settings as media,
//! keys `stitch_jobs/{job_id}/frame_{k:03}.jpg`) → JPEG decoding →
//! [`pnex_stitcher::stitch`] (`Quality` pipeline, alignment + DP seams +
//! multi-band) → publish the JPEG (GPano included) as **version n+1**
//! of the asset via [`crate::controllers::media::write_version`] (reused,
//! not duplicated: `current_version_id` follows).
//!
//! CPU-bound (minutes at 4096): the stitch runs in
//! `tokio::task::spawn_blocking` — the worker is the only big RAM
//! consumer (~170 MB for 28 frames, plan A3: ~200 MB peak in blend), a
//! single loco worker (`PNEX_QUEUE_NUM_WORKERS=1`).
//!
//! Deterministic failure (coverage/decoding/…) → `failed` job + short
//! message in `error` (never a disk path — the detail goes to the server
//! logs), `perform` returns `Ok(())`: no replay, same pattern as
//! [`super::build_firmware`].

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::controllers::media::{write_version, IncomingVersion};
use crate::models::_entities::{media_assets, stitch_jobs};
use crate::services::media::MediaSettings;
use crate::services::stitch::{
    StitchSettings, STATE_FAILED, STATE_QUEUED, STATE_RUNNING, STATE_SUCCEEDED,
};

/// Global stitch concurrency semaphore — FIXED knob (decision
/// 2026-09-10: 2 concurrent jobs max on RPi 8 GB, ~300 MB peak per job).
/// Initialised on the first `perform` with the settings (the workers share
/// the same ones, the first initialisation wins).
static STITCH_PERMITS: OnceLock<Semaphore> = OnceLock::new();

/// Camera pose at trigger time (one entry of `poses_json`, one per frame).
/// Defined here so the controller validates the exact shape the worker
/// reads back.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct PoseDeg {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
}

/// Job payload — the rest (poses, hfov, asset, frames) is re-read from the
/// `stitch_jobs` row + the MediaStore at `perform` time (nothing heavy
/// goes through `pg_loco_queue`).
#[derive(Debug, Serialize, Deserialize)]
pub struct StitchPanoramaArgs {
    pub job_id: Uuid,
    pub org_id: i64,
}

pub struct StitchPanoramaWorker {
    /// Full context (not just the DB): publishing the version goes through
    /// `write_version`, which wants an `&AppContext` (flow_supervisor
    /// pattern: the ctx is cloned outside the HTTP request).
    ctx: AppContext,
    media: MediaSettings,
    stitch: StitchSettings,
}

impl StitchPanoramaWorker {
    /// Sets a state transition (`running`/`succeeded`/`failed` — `queued`
    /// is set by the controller); the worker is the only writer of the
    /// terminal states.
    async fn set_state(
        db: &sea_orm::DatabaseConnection,
        id: Uuid,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        let model = stitch_jobs::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or_else(|| Error::Message(format!("stitch job {id} not found")))?;
        let mut job: stitch_jobs::ActiveModel = model.into();
        job.state = Set(state.to_string());
        job.error = Set(error);
        job.updated_at = Set(chrono::Utc::now().into());
        job.update(db).await?;
        Ok(())
    }

    /// Conditional claim `queued → running` (or stale `running → running`,
    /// older than the job timeout). `false` = someone else handles or
    /// handled the job.
    async fn claim(db: &sea_orm::DatabaseConnection, id: Uuid, timeout_secs: u64) -> Result<bool> {
        let now = chrono::Utc::now();
        let stale: sea_orm::prelude::DateTimeWithTimeZone =
            (now - chrono::Duration::seconds(timeout_secs as i64)).into();
        let now: sea_orm::prelude::DateTimeWithTimeZone = now.into();
        let res = stitch_jobs::Entity::update_many()
            .col_expr(stitch_jobs::Column::State, Expr::value(STATE_RUNNING))
            .col_expr(
                stitch_jobs::Column::Error,
                Expr::value(Option::<String>::None),
            )
            .col_expr(stitch_jobs::Column::UpdatedAt, Expr::value(now))
            .filter(stitch_jobs::Column::Id.eq(id))
            .filter(
                sea_orm::Condition::any()
                    .add(stitch_jobs::Column::State.eq(STATE_QUEUED))
                    .add(
                        sea_orm::Condition::all()
                            .add(stitch_jobs::Column::State.eq(STATE_RUNNING))
                            .add(stitch_jobs::Column::UpdatedAt.lt(stale)),
                    ),
            )
            .exec(db)
            .await?;
        Ok(res.rows_affected == 1)
    }

    /// Runs the full pipeline for a job: frames → stitch → version
    /// n+1 of the asset. `Err(msg)` = short message meant for the `error`
    /// column (the full detail goes to the logs).
    async fn run(&self, job: &stitch_jobs::Model) -> std::result::Result<(), String> {
        // Org asset (scoping is carried by the job row: the asset
        // belongs to the job's org — defence in depth).
        let asset = media_assets::Entity::find()
            .filter(media_assets::Column::Id.eq(job.asset_id))
            .filter(media_assets::Column::OrgId.eq(job.org_id))
            .one(&self.ctx.db)
            .await
            .map_err(|e| format!("db : {e}"))?
            .ok_or_else(|| "asset not found".to_string())?;

        // Poses frozen at job creation — one entry per frame.
        let poses: Vec<PoseDeg> =
            serde_json::from_value(job.poses_json.clone()).map_err(|e| format!("poses : {e}"))?;
        if poses.len() != job.frames_total as usize {
            return Err(format!(
                "poses: {} entries for {} frames",
                poses.len(),
                job.frames_total
            ));
        }

        let store = self
            .media
            .store()
            .map_err(|e| format!("media store: {e}"))?;
        let mut frames: Vec<pnex_stitcher::Frame> = Vec::with_capacity(job.frames_total as usize);
        for k in 0..job.frames_total {
            let key = MediaSettings::stitch_key(job.id, k as u32);
            let bytes = store
                .get(&key)
                .await
                .map_err(|_| format!("frame {k} missing from the store"))?;
            let (rgb, width, height) = pnex_stitcher::decode_jpeg_rgb(&bytes)
                .map_err(|_| format!("frame {k}: cannot decode"))?;
            let pose = poses[k as usize];
            frames.push(pnex_stitcher::Frame {
                rgb,
                width,
                height,
                yaw_deg: pose.yaw_deg,
                pitch_deg: pose.pitch_deg,
                roll_deg: pose.roll_deg,
            });
        }

        // Pose model (SuperPoint+LightGlue): loaded per job (loading the
        // sessions, ~1 s, is negligible next to inference). Absent or
        // failing → None → NCC fallback in the stitcher (graceful degradation).
        // Resolution: configured dir (relative to the process CWD) then
        // repo path (the server often runs from crates/pnex-backend —
        // found 2026-09-10: deploy/models does not resolve there).
        let model_dirs = [
            self.stitch.models_dir.clone(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/models").to_string(),
        ];
        let model = model_dirs.iter().find_map(|dir| {
            match pnex_stitcher::pose_model::PoseModel::load(dir) {
                Ok(m) => {
                    tracing::info!(job = %job.id, dir = %dir, "pose model loaded");
                    Some(m)
                }
                Err(_) => None,
            }
        });
        if model.is_none() {
            tracing::warn!(
                job = %job.id,
                essayé = ?model_dirs,
                "pose model unavailable → NCC alignment"
            );
        }

        // Bounded concurrency: the semaphore carries the fixed knob (2 by
        // default) regardless of the number of queue workers.
        let semaphore = STITCH_PERMITS
            .get_or_init(|| Semaphore::new(self.stitch.max_concurrent.clamp(1, 8) as usize));
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|_| "stitch semaphore closed".to_string())?;

        // CPU-bound: spawn_blocking — the frames (cloned once, ~170 MB
        // for 28×1080×1920: acceptable on a server, plan A) go to the
        // blocking task, the runtime stays responsive.
        let out_width = self.stitch.out_width;
        let params = pnex_stitcher::Params {
            hfov_deg: job.hfov_deg,
            out_width,
            // 0.75: the top rows of the tilted ring are partially covered
            // by nature (79 % measured on a real 2-ring capture) — an
            // azimuthal hole (missing frame) costs several % and is still
            // detected. Aligned with the device threshold (pipeline.rs).
            min_band_coverage: 0.75,
            pipeline: pnex_stitcher::Pipeline::Quality,
            quality: pnex_stitcher::QualityParams::default(),
        };
        let stitched = tokio::task::spawn_blocking(move || {
            pnex_stitcher::stitch_with_model(&frames, &params, model.as_ref())
        })
        .await
        .map_err(|e| format!("stitch task interrupted: {e}"))?
        .map_err(|e| e.to_string())?;

        // The panorama becomes version n+1 of the asset (current) —
        // write_version is reused as is (storage first, DB next, GPano
        // sniff included). Never duplicate this logic.
        write_version(
            &self.ctx,
            job.org_id,
            &asset,
            IncomingVersion {
                filename: format!("panorama-server-{out_width}.jpg"),
                content_type: "image/jpeg".to_string(),
                note: Some(format!("server stitch {out_width} (Take 360 V2)")),
                bytes: stitched.jpeg.into(),
            },
            &self.media,
        )
        .await
        .map_err(|_| "failed to publish the version".to_string())?;

        // Alignment report (Quality pipeline) — server logging only
        // (plan D: filelog/residual diagnostics). `repli_ncc` = the pose
        // model did not anchor enough pairs and the legacy NCC path took
        // over.
        if let Some(report) = &stitched.align_report {
            tracing::info!(
                job = %job.id,
                paires_acceptees = report.accepted.len(),
                paires_rejetees = report.rejected,
                converge = report.converged,
                repli_ncc = report.fallback,
                correction_max_deg = report
                    .corrections_deg
                    .iter()
                    .fold(0.0f32, |m, c| m.max(c[0].abs()).max(c[1].abs()).max(c[2].abs())),
                "server stitch done"
            );
        }
        Ok(())
    }
}

#[async_trait]
impl BackgroundWorker<StitchPanoramaArgs> for StitchPanoramaWorker {
    fn build(ctx: &AppContext) -> Self {
        Self {
            ctx: ctx.clone(),
            media: MediaSettings::from_config(&ctx.config),
            stitch: StitchSettings::from_config(&ctx.config),
        }
    }

    async fn perform(&self, args: StitchPanoramaArgs) -> Result<()> {
        let job = stitch_jobs::Entity::find_by_id(args.job_id)
            .one(&self.ctx.db)
            .await?
            .ok_or_else(|| Error::Message(format!("stitch job {} not found", args.job_id)))?;
        // Idempotent claim: only a queued job, or a running one whose claim
        // went stale (worker died, queue reaper re-queued it), is taken. A
        // duplicate delivery of a succeeded/failed or live-running job is a
        // no-op instead of a second stitch and a second asset version.
        if !Self::claim(&self.ctx.db, job.id, self.stitch.timeout_secs).await? {
            tracing::info!(job = %args.job_id, state = %job.state, "stitch job already handled, skipped");
            return Ok(());
        }
        let timeout = std::time::Duration::from_secs(self.stitch.timeout_secs);
        let outcome = match tokio::time::timeout(timeout, self.run(&job)).await {
            Ok(outcome) => outcome,
            // The blocking stitch thread cannot be interrupted: it finishes
            // in the background, its result is discarded.
            Err(_) => Err(format!("timeout after {} s", self.stitch.timeout_secs)),
        };
        match outcome {
            Ok(()) => {
                // The alignment report is already logged in `run` —
                // here we trace the job success (correlation with the queue).
                tracing::info!(job = %args.job_id, "stitch job succeeded");
                StitchPanoramaWorker::set_state(&self.ctx.db, job.id, STATE_SUCCEEDED, None)
                    .await?;
            }
            Err(msg) => {
                // Short message in the `error` column (read by the front) —
                // the full message stays in the server logs.
                tracing::error!(job = %args.job_id, error = %msg, "stitch job failed");
                StitchPanoramaWorker::set_state(
                    &self.ctx.db,
                    job.id,
                    STATE_FAILED,
                    Some(msg.chars().take(200).collect()),
                )
                .await?;
            }
        }
        Ok(())
    }
}
