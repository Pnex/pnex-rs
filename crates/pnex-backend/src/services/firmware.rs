//! Réglages firmware (Phase 6) — `settings.firmware` de la config Loco,
//! champ par champ avec défauts (pattern `IngestSettings`).
//!
//! Source du firmware : **embarquée dans le binaire** (convergence
//! monorepo — cf. `pnex_firmware_builder::embedded`). Pas de sélecteur :
//! une version du serveur compile la version du firmware qui l'accompagne.
//!
//! Storage (D5 v2, trois tiers de déploiement) : `db` (défaut — les
//! binaires vivent dans la base, table `firmware_artifacts` ; tiers sqlite
//! tout-en-un et postgres multi-pods stateless) ou `s3` (tier industriel,
//! différé). `STORAGE_BACKEND` (env) **surcharge** `storage.backend` de la
//! config. Aucun système de migration/réconciliation entre backends : on
//! choisit son tier à l'installation.

use std::sync::Arc;

use loco_rs::config::Config;
use pnex_firmware_builder::{ArtifactStore, S3Store};
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use crate::services::artifact_store::DbStore;

/// Phases canoniques d'un build (colonne `build_phase`, minuscules).
/// Mapping Django : Pending→queued, Running→running, Succeeded→succeeded,
/// Failed→failed (Deleted supprimé — plus de job k8s à réclamer).
pub const PHASE_QUEUED: &str = "queued";
pub const PHASE_RUNNING: &str = "running";
pub const PHASE_SUCCEEDED: &str = "succeeded";
pub const PHASE_FAILED: &str = "failed";

/// Réglages résolus du worker de build.
#[derive(Clone, Debug)]
pub struct FirmwareSettings {
    /// `db` | `s3` (surchargeable par `STORAGE_BACKEND`).
    pub storage_backend: String,
    // Plomberie S3 (lue depuis la config, opérations différées — D5 v2).
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_path_style: bool,
    /// `pio` ou `uv run pio` (split sur espaces, pas de guillemets).
    pub pio_cmd: String,
    /// `esptool` ou `python -m esptool`.
    pub esptool_cmd: String,
    /// Budget global du build.
    pub timeout_secs: u64,
}

/// Forme sérialisable partielle de `settings.firmware` (tout optionnel).
#[derive(Default, Deserialize)]
struct FirmwarePartial {
    storage: Option<StoragePartial>,
    pio_cmd: Option<String>,
    esptool_cmd: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(Default, Deserialize)]
struct StoragePartial {
    backend: Option<String>,
    s3_endpoint: Option<String>,
    s3_bucket: Option<String>,
    s3_region: Option<String>,
    s3_path_style: Option<bool>,
}

impl Default for FirmwareSettings {
    fn default() -> Self {
        Self {
            storage_backend: "db".into(),
            s3_endpoint: String::new(),
            s3_bucket: String::new(),
            s3_region: String::new(),
            s3_path_style: false,
            pio_cmd: "pio".into(),
            esptool_cmd: "esptool".into(),
            timeout_secs: 900,
        }
    }
}

impl FirmwareSettings {
    /// `settings.firmware` optionnelle — défauts champ par champ.
    pub fn from_config(config: &Config) -> Self {
        let partial: FirmwarePartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("firmware"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let defaults = Self::default();
        let mut settings = Self {
            storage_backend: partial
                .storage
                .as_ref()
                .and_then(|s| s.backend.clone())
                .unwrap_or(defaults.storage_backend),
            s3_endpoint: partial
                .storage
                .as_ref()
                .and_then(|s| s.s3_endpoint.clone())
                .unwrap_or_default(),
            s3_bucket: partial
                .storage
                .as_ref()
                .and_then(|s| s.s3_bucket.clone())
                .unwrap_or_default(),
            s3_region: partial
                .storage
                .as_ref()
                .and_then(|s| s.s3_region.clone())
                .unwrap_or_default(),
            s3_path_style: partial
                .storage
                .as_ref()
                .and_then(|s| s.s3_path_style)
                .unwrap_or(false),
            pio_cmd: partial.pio_cmd.unwrap_or(defaults.pio_cmd),
            esptool_cmd: partial.esptool_cmd.unwrap_or(defaults.esptool_cmd),
            timeout_secs: partial.timeout_secs.unwrap_or(defaults.timeout_secs),
        };
        // Décision utilisateur : l'env surcharge la config.
        if let Ok(backend) = std::env::var("STORAGE_BACKEND") {
            if !backend.is_empty() {
                settings.storage_backend = backend;
            }
        }
        settings
    }

    /// Magasin d'artefacts selon le backend (`db` | `s3`). Le backend `db`
    /// a besoin de la connexion de l'app (tiers sqlite ou postgres).
    pub fn store(&self, db: &DatabaseConnection) -> Result<Arc<dyn ArtifactStore>, String> {
        match self.storage_backend.as_str() {
            "db" => Ok(Arc::new(DbStore::new(db.clone())) as Arc<dyn ArtifactStore>),
            "s3" => Ok(Arc::new(S3Store {
                endpoint: self.s3_endpoint.clone(),
                bucket: self.s3_bucket.clone(),
                region: self.s3_region.clone(),
                path_style: self.s3_path_style,
            }) as Arc<dyn ArtifactStore>),
            other => Err(format!("backend de stockage inconnu : {other} (db | s3)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sélecteur de magasin : db opérationnel (put/get réels sur sqlite
    /// mémoire migrée), s3 différé (NotImplemented), inconnu rejeté.
    /// Défauts : db / pio / 900 s.
    #[tokio::test]
    async fn selecteur_de_magasin() {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("sqlite");
        use pnex_migration::MigratorTrait;
        pnex_migration::Migrator::up(&db, None)
            .await
            .expect("migrations");

        let mut settings = FirmwareSettings::default();
        assert_eq!(settings.storage_backend, "db");
        assert_eq!(settings.timeout_secs, 900);
        assert_eq!(settings.pio_cmd, "pio");

        let store = settings.store(&db).expect("store db");
        store
            .put("org_1/firmware/dev-1-firmware.bin", b"octets")
            .await
            .expect("put");
        assert_eq!(
            store
                .get("org_1/firmware/dev-1-firmware.bin")
                .await
                .expect("get"),
            b"octets"
        );

        settings.storage_backend = "s3".into();
        let store = settings.store(&db).expect("s3 plomberie");
        assert!(matches!(
            store.put("k", b"x").await,
            Err(pnex_firmware_builder::BuildError::NotImplemented(_))
        ));

        settings.storage_backend = "gcs".into();
        assert!(settings.store(&db).is_err());
    }
}
