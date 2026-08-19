//! PNEX firmware-builder — orchestration des builds firmware ESP32/ESP8266.
//!
//! La source du firmware est **embarquée dans le binaire**
//! ([`embedded`]) : l'arborescence `firmware/` du monorepo est compilée
//! dedans (`include_dir!`, convergence monorepo) — une version du serveur
//! build exactement la version du firmware qui l'accompagne. Pipeline :
//! extraction de la source embarquée → `pio run` → `esptool merge-bin` →
//! dépôt de l'artefact dans un [`ArtifactStore`] (décision D5 v2 : backend
//! `db` par défaut, implémenté côté backend ; `s3` = tier industriel différé).
//!
//! Contrat de build du firmware (vérifié, cf.
//! `docs/architecture/firmware-build.md` §2) : la config device passe en
//! **variables d'environnement** du sous-process `pio run` — WIFI_SSID,
//! WIFI_PASSWORD, HOST, TOKEN et DEVICE_ID en base64 (un SSID littéral
//! avec espaces casserait le flag `-D`), WS_SSL en true/false (wss/ws) —
//! jamais en argv (lisible via `ps`). Le workspace est un tmp par job,
//! effacé au drop (secrets compilés dans les artefacts intermédiaires).

mod embedded;
mod env;
mod merge;
mod pipeline;
mod store;

pub use env::{child_env, BuildSecrets};
pub use merge::{merge_args, merge_offsets};
pub use pipeline::{run_build, BuildConfig, DeviceSpec};
pub use store::{artifact_key, sanitize_segment, ArtifactStore, InMemoryStore, S3Store};

/// Étapes d'un build firmware (tracing par étape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStep {
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
