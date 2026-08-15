//! PNEX firmware-builder — orchestration des builds firmware ESP32.
//!
//! Phase 1 : squelette uniquement. L'implémentation réelle arrive en Phase 6 :
//! parité du script k8s_job Django (git clone → `pio run` → `esptool merge-bin`
//! → ArtifactStore MinIO/S3, décision D5), timeout dur, secrets scopés.

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
