//! Front app version — `option_env!` pattern identical to
//! `pnex-backend/src/app.rs` (semver + optional build sha). Shown on
//! the login page and in the profile "About" card.

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build sha injected as an environment variable at build (BUILD_SHA, else
/// GITHUB_SHA, else "dev").
pub fn build_sha() -> &'static str {
    option_env!("BUILD_SHA")
        .or(option_env!("GITHUB_SHA"))
        .unwrap_or("dev")
}

/// "0.1.0 (dev)" — composed version shown to the user.
#[must_use]
pub fn full() -> String {
    format!("{} ({})", APP_VERSION, build_sha())
}
