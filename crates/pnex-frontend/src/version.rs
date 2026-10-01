//! Version de l'app front — pattern `option_env!` identique à
//! `pnex-backend/src/app.rs` (semver + sha de build optionnel). Affichée sur
//! la page de login et dans la carte « À propos » du profil.

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Sha de build injecté en var d'environnement au build (BUILD_SHA, sinon
/// GITHUB_SHA, sinon « dev »).
pub fn build_sha() -> &'static str {
    option_env!("BUILD_SHA")
        .or(option_env!("GITHUB_SHA"))
        .unwrap_or("dev")
}

/// « 0.1.0 (dev) » — version composée affichée à l'utilisateur.
#[must_use]
pub fn full() -> String {
    format!("{} ({})", APP_VERSION, build_sha())
}
