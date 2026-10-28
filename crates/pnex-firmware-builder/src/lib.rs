//! PNEX firmware-builder — orchestration of ESP32/ESP8266 firmware builds.
//!
//! The firmware source is **embedded in the binary**
//! ([`embedded`]): the monorepo `firmware/` tree is compiled
//! into it (`include_dir!`, monorepo convergence) — a server version
//! builds exactly the firmware version that ships with it. Pipeline:
//! extraction of the embedded source → `pio run` → `esptool merge-bin` →
//! artifact stored in an [`ArtifactStore`] (decision D5 v2: `db` and `s3`
//! backends, implemented in pnex-backend; only the contract here).
//!
//! Firmware build contract (verified, see
//! `docs/architecture/firmware-build.md` §2): the device config goes through
//! **environment variables** of the `pio run` subprocess — WIFI_SSID,
//! WIFI_PASSWORD, HOST, TOKEN and DEVICE_ID in base64 (a literal SSID
//! with spaces would break the `-D` flag), always wss (D154) —
//! never in argv (readable via `ps`). The workspace is a tmp per job,
//! wiped on drop (secrets compiled into the intermediate artifacts).

mod custom;
mod embedded;
pub use embedded::source_fingerprint;
mod env;
mod merge;
mod pipeline;
mod store;

pub use custom::{sandbox_argv, BuildOptions, CustomSource, Sandbox};
pub use env::{child_env, scrub_secrets, BuildSecrets};
pub use merge::{chip_name, merge_args, merge_offsets};
pub use pipeline::{
    compile_check, run_build, run_build_with, BuildConfig, CompileOutcome, DeviceSpec, ScreenSpec,
};
pub use store::{artifact_key, ota_artifact_key, sanitize_segment, ArtifactStore, InMemoryStore};

/// Steps of a firmware build (tracing per step).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStep {
    Compile,
    MergeBin,
    Upload,
}

/// Possible builder errors.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("build timeout")]
    Timeout,
    #[error("external tool: {0}")]
    Tool(String),
    #[error("firmware source: {0}")]
    Source(String),
    #[error("artifact not found: {0}")]
    NotFound(String),
    #[error("artifact store: {0}")]
    Store(String),
}

/// Result of a successful build — logical path in the `ArtifactStore`.
#[derive(Debug, Clone)]
pub struct BuildArtifact {
    pub key: String,
    pub size_bytes: u64,
    /// Raw app image (OTA payload, pre-merge) — versioned key
    /// `org_{id}/ota/{device}/{version}.bin`, immutable per version.
    pub ota_key: String,
    /// sha256 digest (lowercase hex) of the raw app image — the OTA
    /// deployment integrity gate (device compares before booting it).
    pub ota_sha256: String,
    pub ota_size_bytes: u64,
}
