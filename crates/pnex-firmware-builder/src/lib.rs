//! PNEX firmware-builder — orchestration des builds firmware ESP32.
//!
//! Phase 1 : squelette uniquement. L'implémentation réelle arrive en Phase 6 :
//! parité du script k8s_job Django (git clone → `pio run` → `esptool merge-bin`
//! → ArtifactStore MinIO/S3, décision D5), timeout dur, secrets scopés.
//!
//! Contrat de build du dépôt firmware (vérifié) : la config device passe en
//! **variables d'environnement** du sous-process `pio run` (WIFI_SSID,
//! WIFI_PASSWORD, HOST/TOKEN/DEVICE_ID en base64) — cf.
//! `docs/architecture/firmware-build.md`.

/// Étapes d'un build firmware (parité du pipeline Django).
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
}

/// Résultat d'un build réussi — chemin logique dans l'`ArtifactStore`.
#[derive(Debug, Clone)]
pub struct BuildArtifact {
    pub key: String,
    pub size_bytes: u64,
}
