//! Réglages firmware (Phase 6) — `settings.firmware` de la config Loco,
//! champ par champ avec défauts (pattern `IngestSettings`).
//!
//! Source du firmware : **embarquée dans le binaire** (convergence
//! monorepo — cf. `pnex_firmware_builder::embedded`). Pas de sélecteur :
//! une version du serveur compile la version du firmware qui l'accompagne.
//!
//! Artifacts live in the S3 object store (`services::artifact_store`,
//! `settings.storage`).

use std::sync::Arc;

use loco_rs::config::Config;
use pnex_firmware_builder::ArtifactStore;
use serde::Deserialize;

use crate::services::artifact_store::S3Config;

/// Phases canoniques d'un build (colonne `build_phase`, minuscules).
pub const PHASE_QUEUED: &str = "queued";
pub const PHASE_RUNNING: &str = "running";
pub const PHASE_SUCCEEDED: &str = "succeeded";
pub const PHASE_FAILED: &str = "failed";

/// Réglages résolus du worker de build.
#[derive(Clone, Debug)]
pub struct FirmwareSettings {
    /// Artifact object store (`settings.storage`, secret masked in Debug).
    pub storage: S3Config,
    /// `pio` ou `uv run pio` (split sur espaces, pas de guillemets).
    pub pio_cmd: String,
    /// `esptool` ou `python -m esptool`.
    pub esptool_cmd: String,
    /// Budget global du build.
    pub timeout_secs: u64,
    /// Custom firmware IDE (custom-firmware.md D91): disabled unless the
    /// operator turns it on — compiling user C++ runs user code on the
    /// worker.
    pub custom: CustomFirmwareSettings,
    /// Refuse a `wss` build when no device CA can be pinned (SEC-W6):
    /// without it the firmware falls back to `setInsecure`. Default true;
    /// only the test configuration turns it off.
    pub require_device_ca: bool,
}

/// `settings.firmware.custom` — gate + build sandbox of custom firmware.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomFirmwareSettings {
    /// Master switch (default false). When false the project endpoints
    /// still work (editing, history) but compiling is refused.
    pub enabled: bool,
    /// Build isolation; `None` = unsandboxed (only acceptable on a
    /// single-user dev machine — logged loudly at each build).
    pub sandbox: Option<pnex_firmware_builder::Sandbox>,
}

impl CustomFirmwareSettings {
    fn from_partial(p: Option<CustomPartial>) -> Self {
        let p = p.unwrap_or_default();
        let sandbox = match p.sandbox.as_deref().unwrap_or("bwrap") {
            "none" => None,
            _ => Some(pnex_firmware_builder::Sandbox::Bwrap {
                program: p.bwrap_cmd.unwrap_or_else(|| "bwrap".into()),
                allow_network: p.allow_network.unwrap_or(false),
            }),
        };
        Self {
            enabled: p.enabled.unwrap_or(false),
            sandbox,
        }
    }
}

impl CustomFirmwareSettings {
    /// Env overrides: `enabled` = `true|false`, `sandbox` = `bwrap|none`
    /// (empty/unknown values are ignored).
    fn apply_env(&mut self, enabled: Option<&str>, sandbox: Option<&str>) {
        match enabled {
            Some("true") => self.enabled = true,
            Some("false") => self.enabled = false,
            _ => {}
        }
        match sandbox {
            Some("none") => self.sandbox = None,
            Some("bwrap") if self.sandbox.is_none() => {
                self.sandbox = Some(pnex_firmware_builder::Sandbox::Bwrap {
                    program: "bwrap".into(),
                    allow_network: false,
                })
            }
            _ => {}
        }
    }
}

#[derive(Default, Deserialize)]
struct CustomPartial {
    enabled: Option<bool>,
    /// `bwrap` (default) | `none`.
    sandbox: Option<String>,
    bwrap_cmd: Option<String>,
    /// Let `pio` reach the library registry inside the sandbox (catalog
    /// not prefetched on this worker).
    allow_network: Option<bool>,
}

/// Forme sérialisable partielle de `settings.firmware` (tout optionnel).
#[derive(Default, Deserialize)]
struct FirmwarePartial {
    pio_cmd: Option<String>,
    esptool_cmd: Option<String>,
    timeout_secs: Option<u64>,
    custom: Option<CustomPartial>,
    require_device_ca: Option<bool>,
}

impl Default for FirmwareSettings {
    fn default() -> Self {
        Self {
            storage: S3Config::default(),
            pio_cmd: "pio".into(),
            esptool_cmd: "esptool".into(),
            timeout_secs: 900,
            custom: CustomFirmwareSettings::default(),
            require_device_ca: true,
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
            storage: S3Config::from_config(config),
            pio_cmd: partial.pio_cmd.unwrap_or(defaults.pio_cmd),
            esptool_cmd: partial.esptool_cmd.unwrap_or(defaults.esptool_cmd),
            timeout_secs: partial.timeout_secs.unwrap_or(defaults.timeout_secs),
            custom: CustomFirmwareSettings::from_partial(partial.custom),
            require_device_ca: partial
                .require_device_ca
                .unwrap_or(defaults.require_device_ca),
        };
        // Custom firmware gate and sandbox, same env-over-config rule
        // (container deployments toggle them without editing the yaml).
        settings.custom.apply_env(
            std::env::var("PNEX_FIRMWARE_CUSTOM_ENABLED")
                .ok()
                .as_deref(),
            std::env::var("PNEX_FIRMWARE_SANDBOX").ok().as_deref(),
        );
        settings
    }

    /// The artifact store; an incomplete S3 configuration is rejected here.
    pub fn store(&self) -> Result<Arc<dyn ArtifactStore>, String> {
        Ok(Arc::new(self.storage.store()?) as Arc<dyn ArtifactStore>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Defaults (pio / 900 s); the store needs a complete S3 configuration,
    /// the test one (`settings.storage` of test.yaml) is.
    #[test]
    fn store_is_s3_only() {
        let settings = FirmwareSettings::default();
        assert_eq!(settings.timeout_secs, 900);
        assert_eq!(settings.pio_cmd, "pio");
        // Arc<dyn ArtifactStore> is not Debug: let-else, not expect_err.
        let Err(err) = settings.store() else {
            panic!("an unconfigured S3 must be refused at construction");
        };
        assert!(err.contains("bucket"), "{err}");

        let config = Config::new(&loco_rs::environment::Environment::Test).unwrap();
        FirmwareSettings::from_config(&config)
            .store()
            .expect("test S3 configured");
    }

    /// Custom firmware is off by default; the sandbox defaults to bwrap
    /// without network, `none` must be explicit.
    #[test]
    fn custom_firmware_defaults_are_safe() {
        let d = CustomFirmwareSettings::from_partial(None);
        assert!(!d.enabled);
        assert_eq!(
            d.sandbox,
            Some(pnex_firmware_builder::Sandbox::Bwrap {
                program: "bwrap".into(),
                allow_network: false
            })
        );
        let p: CustomPartial =
            serde_json::from_value(serde_json::json!({"enabled": true, "sandbox": "none"}))
                .unwrap();
        let s = CustomFirmwareSettings::from_partial(Some(p));
        assert!(s.enabled);
        assert!(s.sandbox.is_none());
    }

    #[test]
    fn custom_firmware_env_overrides() {
        let mut s = CustomFirmwareSettings::from_partial(None);
        s.apply_env(Some("true"), Some("none"));
        assert!(s.enabled);
        assert!(s.sandbox.is_none());
        s.apply_env(Some("nope"), Some("bwrap"));
        assert!(s.enabled, "unknown value ignored");
        assert!(s.sandbox.is_some());
    }

    /// SEC-W6: a build without a pinned device CA is refused by default.
    #[test]
    fn device_ca_is_required_by_default() {
        assert!(FirmwareSettings::default().require_device_ca);
    }

    /// The Debug of FirmwareSettings never leaks the S3 secret.
    #[test]
    fn debug_masks_the_s3_secret() {
        let mut settings = FirmwareSettings::default();
        settings.storage.secret_key = "super-secret".into();
        let repr = format!("{settings:?}");
        assert!(!repr.contains("super-secret"), "{repr}");
    }
}

/// Device endpoint of the deployment (D158), from `PNEX_DEVICE_HOST`.
pub fn device_endpoint_env(host: &str) -> String {
    device_endpoint(host, std::env::var("PNEX_DEVICE_HOST").ok().as_deref())
}

/// Device endpoint (D158) compiled into firmware and handed to edge
/// agents, from the `PNEX_DEVICE_HOST` setting: `name[:port]` = its own
/// name (cloud: `devices.dev.pnex.io`); `:port` = the instance host on its
/// own port (LAN edge, where the host is a bare IP: `:4443`); unset = the
/// instance host itself. The instance host's own port, if any, is dropped
/// by `:port`.
pub fn device_endpoint(host: &str, setting: Option<&str>) -> String {
    let host = host.trim();
    match setting.map(str::trim).filter(|s| !s.is_empty()) {
        Some(port) if port.starts_with(':') => {
            let bare = match host.rsplit_once(':') {
                Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) && !h.ends_with(':') => h,
                _ => host,
            };
            format!("{bare}{port}")
        }
        Some(own) => own.to_string(),
        None => host.to_string(),
    }
}

#[cfg(test)]
mod device_endpoint_tests {
    use super::device_endpoint;

    #[test]
    fn setting_names_the_host_the_port_or_nothing() {
        assert_eq!(
            device_endpoint("192.168.1.185", Some(":4443")),
            "192.168.1.185:4443"
        );
        assert_eq!(device_endpoint("host:8443", Some(":4443")), "host:4443");
        assert_eq!(
            device_endpoint("dev.pnex.io", Some("devices.dev.pnex.io")),
            "devices.dev.pnex.io"
        );
        assert_eq!(device_endpoint("host", Some(" ")), "host");
        assert_eq!(device_endpoint("host", None), "host");
    }
}
