//! PNEX firmware-builder — orchestration des builds firmware ESP32/ESP8266.
//!
//! Parité du script k8s_job Django (Phase 6) : mise en place de la source
//! (copie locale ou `git clone`) → `pio run` → `esptool merge-bin` → dépôt
//! de l'artefact dans un [`ArtifactStore`] (décision D5 : backend `local`
//! d'abord, `s3` différé).
//!
//! Contrat de build du dépôt firmware (vérifié, cf.
//! `docs/architecture/firmware-build.md` §2) : la config device passe en
//! **variables d'environnement** du sous-process `pio run` — WIFI_SSID et
//! WIFI_PASSWORD en clair, HOST/TOKEN/DEVICE_ID en base64 — jamais en argv
//! (lisible via `ps`). Le workspace est un tmp par job, effacé au drop
//! (secrets compilés dans les artefacts intermédiaires).

mod env;
mod merge;
mod pipeline;
mod store;

pub use env::{child_env, BuildSecrets};
pub use merge::{merge_args, merge_offsets};
pub use pipeline::{run_build, BuildConfig, DeviceSpec, FirmwareSource};
pub use store::{artifact_key, sanitize_segment, ArtifactStore, LocalStore, S3Store};

/// Étapes d'un build firmware (parité du pipeline Django, tracing par étape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStep {
    Clone,
    Compile,
    MergeBin,
    Upload,
}

/// Erreurs possibles du builder.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("timeout du build")]
    Timeout,
    #[error("outil externe: {0}")]
    Tool(String),
    #[error("source du firmware: {0}")]
    Source(String),
    #[error("artefact introuvable: {0}")]
    NotFound(String),
    #[error("magasin d'artefacts: {0}")]
    Store(String),
    #[error("backend non implémenté: {0}")]
    NotImplemented(String),
}

/// Résultat d'un build réussi — chemin logique dans l'`ArtifactStore`.
#[derive(Debug, Clone)]
pub struct BuildArtifact {
    pub key: String,
    pub size_bytes: u64,
}
