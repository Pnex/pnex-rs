//! Réglages firmware (Phase 6) — `settings.firmware` de la config Loco,
//! champ par champ avec défauts (pattern `IngestSettings`).
//!
//! Storage : `STORAGE_BACKEND` (env) **surcharge** `storage.backend` de la
//! config — décision utilisateur : abstraction à deux backends, `local`
//! (FS, edge) d'abord, `s3` (cloud) différé (D5).

use std::path::PathBuf;
use std::sync::Arc;

use loco_rs::config::Config;
use pnex_firmware_builder::{ArtifactStore, FirmwareSource, LocalStore, S3Store};
use serde::Deserialize;

/// Phases canoniques d'un build (colonne `build_phase`, minuscules).
/// Mapping Django : Pending→queued, Running→running, Succeeded→succeeded,
/// Failed→failed (Deleted supprimé — plus de job k8s à réclamer).
pub const PHASE_QUEUED: &str = "queued";
pub const PHASE_RUNNING: &str = "running";
pub const PHASE_SUCCEEDED: &str = "succeeded";
pub const PHASE_FAILED: &str = "failed";

/// Source du firmware : arborescence locale (dev/edge) ou dépôt git
/// (branche/tag, clone `--depth 1` par job). Variantes en minuscules dans
/// la config (`kind: local` / `kind: git`).
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FirmwareSourceCfg {
    Local { path: String },
    Git { repo: String, git_ref: String },
}

/// Réglages résolus du worker de build.
#[derive(Clone, Debug)]
pub struct FirmwareSettings {
    pub source: FirmwareSourceCfg,
    /// `local` | `s3` (surchargeable par `STORAGE_BACKEND`).
    pub storage_backend: String,
    /// Racine du backend local (créée au premier dépôt).
    pub local_root: PathBuf,
    // Plomberie S3 (lue depuis la config, opérations différées — D5).
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
    source: Option<FirmwareSourceCfg>,
    storage: Option<StoragePartial>,
    pio_cmd: Option<String>,
    esptool_cmd: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(Default, Deserialize)]
struct StoragePartial {
    backend: Option<String>,
    local_root: Option<String>,
    s3_endpoint: Option<String>,
    s3_bucket: Option<String>,
    s3_region: Option<String>,
    s3_path_style: Option<bool>,
}

impl Default for FirmwareSettings {
    fn default() -> Self {
        Self {
            // Défaut Django : dépôt firmware public, branche main.
            source: FirmwareSourceCfg::Git {
                repo: "https://github.com/Pnex/iot-firmware.git".into(),
                git_ref: "main".into(),
            },
            storage_backend: "local".into(),
            local_root: PathBuf::from("./artifacts"),
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
            source: partial.source.unwrap_or(defaults.source),
            storage_backend: partial
                .storage
                .as_ref()
                .and_then(|s| s.backend.clone())
                .unwrap_or(defaults.storage_backend),
            local_root: partial
                .storage
                .as_ref()
                .and_then(|s| s.local_root.clone())
                .map(PathBuf::from)
                .unwrap_or(defaults.local_root),
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

    /// Magasin d'artefacts selon le backend (`local` | `s3`).
    pub fn store(&self) -> Result<Arc<dyn ArtifactStore>, String> {
        match self.storage_backend.as_str() {
            "local" => LocalStore::new(&self.local_root)
                .map(|s| Arc::new(s) as Arc<dyn ArtifactStore>)
                .map_err(|e| e.to_string()),
            "s3" => Ok(Arc::new(S3Store {
                endpoint: self.s3_endpoint.clone(),
                bucket: self.s3_bucket.clone(),
                region: self.s3_region.clone(),
                path_style: self.s3_path_style,
            }) as Arc<dyn ArtifactStore>),
            other => Err(format!("backend de stockage inconnu : {other}")),
        }
    }

    /// Source du pipeline (type de la crate firmware-builder).
    pub fn source(&self) -> FirmwareSource {
        match &self.source {
            FirmwareSourceCfg::Local { path } => FirmwareSource::Local {
                path: PathBuf::from(path),
            },
            FirmwareSourceCfg::Git { repo, git_ref } => FirmwareSource::Git {
                repo: repo.clone(),
                git_ref: git_ref.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sélecteur de magasin : local ok, s3 différé (NotImplemented), inconnu
    /// rejeté. Défauts : local / pio / 900 s / dépôt git public.
    #[tokio::test]
    async fn selecteur_de_magasin() {
        let mut settings = FirmwareSettings::default();
        assert_eq!(settings.storage_backend, "local");
        assert_eq!(settings.timeout_secs, 900);
        assert_eq!(settings.pio_cmd, "pio");
        assert!(matches!(
            settings.source,
            FirmwareSourceCfg::Git { ref git_ref, .. } if git_ref == "main"
        ));

        settings.storage_backend = "s3".into();
        let store = settings.store().expect("s3 plomberie");
        assert!(matches!(
            store.put("k", b"x").await,
            Err(pnex_firmware_builder::BuildError::NotImplemented(_))
        ));

        settings.storage_backend = "gcs".into();
        assert!(settings.store().is_err());

        settings.storage_backend = "local".into();
        assert!(settings.store().is_ok());
    }
}
