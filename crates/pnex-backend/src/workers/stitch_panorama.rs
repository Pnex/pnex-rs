//! Worker de stitch serveur (Take 360 V2, plan cheerful-booping-teacup B3)
//! — consomme la queue PostgreSQL, pattern exact de [`super::build_firmware`]
//! : `perform_later` avec des args minuscules, `perform` qui écrit
//! directement les transitions `running → succeeded|failed`.
//!
//! Pipeline : lecture des frames du MediaStore (mêmes réglages que les
//! médias, clés `stitch_jobs/{job_id}/frame_{k:03}.jpg`) → décodage JPEG →
//! [`pnex_stitcher::stitch`] (pipeline `Quality`, alignement + coutures DP +
//! multi-bandes) → publication du JPEG (GPano inclus) comme **version n+1**
//! de l'asset via [`crate::controllers::media::write_version`] (réutilisée,
//! pas dupliquée : `current_version_id` suit).
//!
//! CPU-bound (minutes à 4096) : le stitch tourne dans
//! `tokio::task::spawn_blocking` — le worker est le seul gros consommateur
//! RAM (~170 Mo pour 28 frames, plan A3 : ~200 Mo pic côté blend), un seul
//! worker loco (`PNEX_QUEUE_NUM_WORKERS=1`).
//!
//! Échec déterministe (couverture/décodage/…) → job `failed` + message
//! court dans `error` (jamais de chemin disque — le détail part dans les
//! logs serveur), `perform` retourne `Ok(())` : pas de rejeu, même école
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

/// Sémaphore global de concurrence du stitch — knob FIXE (décision
/// 2026-09-10 : 2 jobs simultanés max sur RPi 8 Go, ~300 Mo de pic par job).
/// Initialisé au premier `perform` avec les settings (les workers partagent
/// les mêmes, la première initialisation gagne).
static STITCH_PERMITS: OnceLock<Semaphore> = OnceLock::new();

/// Une pose au déclenchement (une entrée de `poses_json`, une par frame).
/// Définie ici : le contrôleur valide/crée avec la même forme que le worker
/// relit — une seule définition, pas de divergence de champs.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PoseDeg {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
}

/// Charge utile du job — le reste (poses, hfov, asset, frames) est relu du
/// row `stitch_jobs` + du MediaStore au moment du `perform` (rien de lourd
/// ne transite par `pg_loco_queue`).
#[derive(Debug, Serialize, Deserialize)]
pub struct StitchPanoramaArgs {
    pub job_id: Uuid,
    pub org_id: i64,
}

pub struct StitchPanoramaWorker {
    /// Contexte complet (pas seulement la DB) : la publication de la version
    /// passe par `write_version`, qui veut un `&AppContext` (école
    /// flow_supervisor : le ctx est cloné hors requête HTTP).
    ctx: AppContext,
    media: MediaSettings,
    stitch: StitchSettings,
}

impl StitchPanoramaWorker {
    /// Pose une transition d'état (`running`/`succeeded`/`failed` — `queued`
    /// est posé par le contrôleur) ; le worker est l'unique écrivain des
    /// états terminaux.
    async fn set_state(
        db: &sea_orm::DatabaseConnection,
        id: Uuid,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        let model = stitch_jobs::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or_else(|| Error::Message(format!("stitch job {id} introuvable")))?;
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

    /// Exécute le pipeline complet pour un job : frames → stitch → version
    /// n+1 de l'asset. `Err(msg)` = message court destiné à la colonne
    /// `error` (le détail complet va dans les logs).
    async fn run(&self, job: &stitch_jobs::Model) -> std::result::Result<(), String> {
        // Asset de l'org (le scoping est porté par le row job : l'asset
        // appartient à l'org du job — défense en profondeur).
        let asset = media_assets::Entity::find()
            .filter(media_assets::Column::Id.eq(job.asset_id))
            .filter(media_assets::Column::OrgId.eq(job.org_id))
            .one(&self.ctx.db)
            .await
            .map_err(|e| format!("db : {e}"))?
            .ok_or_else(|| "asset introuvable".to_string())?;

        // Poses figées à la création du job — une entrée par frame.
        let poses: Vec<PoseDeg> =
            serde_json::from_value(job.poses_json.clone()).map_err(|e| format!("poses : {e}"))?;
        if poses.len() != job.frames_total as usize {
            return Err(format!(
                "poses : {} entrées pour {} frames",
                poses.len(),
                job.frames_total
            ));
        }

        let store = self
            .media
            .store()
            .map_err(|e| format!("magasin média : {e}"))?;
        let mut frames: Vec<pnex_stitcher::Frame> = Vec::with_capacity(job.frames_total as usize);
        for k in 0..job.frames_total {
            let key = MediaSettings::stitch_key(job.id, k as u32);
            let bytes = store
                .get(&key)
                .await
                .map_err(|_| format!("frame {k} absente du magasin"))?;
            let (rgb, width, height) = pnex_stitcher::decode_jpeg_rgb(&bytes)
                .map_err(|_| format!("frame {k} : décodage impossible"))?;
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

        // Modèle de pose (SuperPoint+LightGlue) : chargé par job (le chargement
        // des sessions ~1 s est négligeable devant l'inférence). Absent ou
        // défaillant → None → repli NCC côté stitcher (dégradation gracieuse).
        // Résolution : dir configurée (relatif au CWD du process) puis
        // chemin du dépôt (le serveur tourne souvent depuis
        // crates/pnex-backend — constat 2026-09-10 : deploy/models n'y
        // résout pas).
        let model_dirs = [
            self.stitch.models_dir.clone(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/models").to_string(),
        ];
        let model = model_dirs.iter().find_map(|dir| {
            match pnex_stitcher::pose_model::PoseModel::load(dir) {
                Ok(m) => {
                    tracing::info!(job = %job.id, dir = %dir, "modèle de pose chargé");
                    Some(m)
                }
                Err(_) => None,
            }
        });
        if model.is_none() {
            tracing::warn!(
                job = %job.id,
                essayé = ?model_dirs,
                "modèle de pose indisponible → alignement NCC"
            );
        }

        // Concurrence bornée : le sémaphore porte le knob fixe (2 par
        // défaut) indépendamment du nombre de workers de queue.
        let semaphore = STITCH_PERMITS
            .get_or_init(|| Semaphore::new(self.stitch.max_concurrent.clamp(1, 8) as usize));
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|_| "sémaphore de stitch fermé".to_string())?;

        // CPU-bound : spawn_blocking — les frames (clonées une fois, ~170 Mo
        // pour 28×1080×1920 : acceptable serveur, plan A) passent à la tâche
        // blocante, le runtime reste réactif.
        let out_width = self.stitch.out_width;
        let params = pnex_stitcher::Params {
            hfov_deg: job.hfov_deg,
            out_width,
            // 0,75 : les rangées hautes de l'anneau incliné sont
            // partiellement couvertes par nature (79 % constaté sur une
            // capture 2 anneaux réelle) — un trou azimuthal (frame
            // manquante) coûte plusieurs % et reste détecté. Aligné sur le
            // seuil device (pipeline.rs).
            min_band_coverage: 0.75,
            pipeline: pnex_stitcher::Pipeline::Quality,
            quality: pnex_stitcher::QualityParams::default(),
        };
        let stitched = tokio::task::spawn_blocking(move || {
            pnex_stitcher::stitch_with_model(&frames, &params, model.as_ref())
        })
        .await
        .map_err(|e| format!("tâche de stitch interrompue : {e}"))?
        .map_err(|e| e.to_string())?;

        // Le panorama devient la version n+1 de l'asset (courante) —
        // write_version est réutilisée telle quelle (storage d'abord, DB
        // ensuite, sniff GPano inclus). Jamais de doublon de cette logique.
        write_version(
            &self.ctx,
            job.org_id,
            &asset,
            IncomingVersion {
                filename: format!("panorama-serveur-{out_width}.jpg"),
                content_type: "image/jpeg".to_string(),
                note: Some(format!("assemblage serveur {out_width} (Take 360 V2)")),
                bytes: stitched.jpeg.into(),
            },
            &self.media,
        )
        .await
        .map_err(|_| "échec de la publication de la version".to_string())?;

        // Rapport d'alignement (pipeline Quality) — journalisation serveur
        // uniquement (plan D : filelog/diagnostic des résidus). `repli_ncc`
        // = le modèle de pose n'a pas ancré assez de paires et le chemin
        // NCC historique a pris le relais.
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
                "stitch serveur terminé"
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
            .ok_or_else(|| Error::Message(format!("stitch job {} introuvable", args.job_id)))?;
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
                // Le rapport d'alignement est déjà journalisé dans `run` —
                // ici on trace le succès du job (corrélation avec la queue).
                tracing::info!(job = %args.job_id, "stitch job réussi");
                StitchPanoramaWorker::set_state(&self.ctx.db, job.id, STATE_SUCCEEDED, None)
                    .await?;
            }
            Err(msg) => {
                // Message court dans la colonne `error` (lu par le front) —
                // le message complet reste dans les logs serveur.
                tracing::error!(job = %args.job_id, erreur = %msg, "stitch job échoué");
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
