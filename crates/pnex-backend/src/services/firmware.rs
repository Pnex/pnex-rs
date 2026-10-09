//! Réglages firmware (Phase 6) — `settings.firmware` de la config Loco,
//! champ par champ avec défauts (pattern `IngestSettings`).
//!
//! Source du firmware : **embarquée dans le binaire** (convergence
//! monorepo — cf. `pnex_firmware_builder::embedded`). Pas de sélecteur :
//! une version du serveur compile la version du firmware qui l'accompagne.
//!
//! Storage (D5 v2, trois tiers de déploiement) : `db` (défaut — les
//! binaires vivent dans la base, table `firmware_artifacts` ; tiers sqlite
//! tout-en-un et postgres multi-pods stateless) ou `s3` (tier industriel —
//! artefacts sur S3-compatible via opendal, cf. `services::artifact_store`).
//! `PNEX_STORAGE_BACKEND` (env) **surcharge** `storage.backend` de la config.
//! Aucun système de migration/réconciliation entre backends : on choisit
//! son tier à l'installation.

use std::sync::Arc;

use loco_rs::config::Config;
use pnex_firmware_builder::ArtifactStore;
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use crate::services::artifact_store::{DbStore, S3Config, S3Store};

/// Phases canoniques d'un build (colonne `build_phase`, minuscules).
/// Legacy mapping: Pending→queued, Running→running, Succeeded→succeeded,
/// Failed→failed (Deleted removed — no k8s job left to reclaim).
pub const PHASE_QUEUED: &str = "queued";
pub const PHASE_RUNNING: &str = "running";
pub const PHASE_SUCCEEDED: &str = "succeeded";
pub const PHASE_FAILED: &str = "failed";

/// Réglages résolus du worker de build.
///
/// Debug manuel : le secret S3 n'apparaît jamais dans les logs (même par
/// accident via `?settings`).
#[derive(Clone)]
pub struct FirmwareSettings {
    /// `db` | `s3` (surchargeable par `PNEX_STORAGE_BACKEND`).
    pub storage_backend: String,
    // Connexion S3 (tier industriel — cf. services::artifact_store).
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_path_style: bool,
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

impl std::fmt::Debug for FirmwareSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirmwareSettings")
            .field("storage_backend", &self.storage_backend)
            .field("s3_endpoint", &self.s3_endpoint)
            .field("s3_bucket", &self.s3_bucket)
            .field("s3_region", &self.s3_region)
            .field("s3_access_key", &self.s3_access_key)
            .field(
                "s3_secret_key",
                &if self.s3_secret_key.is_empty() {
                    "<vide>"
                } else {
                    "<masqué>"
                },
            )
            .field("s3_path_style", &self.s3_path_style)
            .field("pio_cmd", &self.pio_cmd)
            .field("esptool_cmd", &self.esptool_cmd)
            .field("timeout_secs", &self.timeout_secs)
            .field("custom", &self.custom)
            .finish()
    }
}

/// Forme sérialisable partielle de `settings.firmware` (tout optionnel).
#[derive(Default, Deserialize)]
struct FirmwarePartial {
    storage: Option<StoragePartial>,
    pio_cmd: Option<String>,
    esptool_cmd: Option<String>,
    timeout_secs: Option<u64>,
    custom: Option<CustomPartial>,
    require_device_ca: Option<bool>,
}

#[derive(Default, Deserialize)]
struct StoragePartial {
    backend: Option<String>,
    s3_endpoint: Option<String>,
    s3_bucket: Option<String>,
    s3_region: Option<String>,
    s3_access_key: Option<String>,
    s3_secret_key: Option<String>,
    s3_path_style: Option<bool>,
}

impl Default for FirmwareSettings {
    fn default() -> Self {
        Self {
            storage_backend: "db".into(),
            s3_endpoint: String::new(),
            s3_bucket: String::new(),
            s3_region: String::new(),
            s3_access_key: String::new(),
            s3_secret_key: String::new(),
            s3_path_style: false,
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
            s3_access_key: partial
                .storage
                .as_ref()
                .and_then(|s| s.s3_access_key.clone())
                .unwrap_or_default(),
            s3_secret_key: partial
                .storage
                .as_ref()
                .and_then(|s| s.s3_secret_key.clone())
                .unwrap_or_default(),
            s3_path_style: partial
                .storage
                .as_ref()
                .and_then(|s| s.s3_path_style)
                .unwrap_or(false),
            pio_cmd: partial.pio_cmd.unwrap_or(defaults.pio_cmd),
            esptool_cmd: partial.esptool_cmd.unwrap_or(defaults.esptool_cmd),
            timeout_secs: partial.timeout_secs.unwrap_or(defaults.timeout_secs),
            custom: CustomFirmwareSettings::from_partial(partial.custom),
            require_device_ca: partial
                .require_device_ca
                .unwrap_or(defaults.require_device_ca),
        };
        // Décision utilisateur : l'env surcharge la config.
        if let Ok(backend) = std::env::var("PNEX_STORAGE_BACKEND") {
            if !backend.is_empty() {
                settings.storage_backend = backend;
            }
        }
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

    /// Magasin d'artefacts selon le backend (`db` | `s3`). Le backend `db`
    /// a besoin de la connexion de l'app (tiers sqlite ou postgres) ; le
    /// backend `s3` valide sa configuration à la construction.
    pub fn store(&self, db: &DatabaseConnection) -> Result<Arc<dyn ArtifactStore>, String> {
        match self.storage_backend.as_str() {
            "db" => Ok(Arc::new(DbStore::new(db.clone())) as Arc<dyn ArtifactStore>),
            "s3" => {
                let store = S3Store::connect(&S3Config {
                    endpoint: self.s3_endpoint.clone(),
                    bucket: self.s3_bucket.clone(),
                    region: self.s3_region.clone(),
                    access_key: self.s3_access_key.clone(),
                    secret_key: self.s3_secret_key.clone(),
                    path_style: self.s3_path_style,
                })?;
                Ok(Arc::new(store) as Arc<dyn ArtifactStore>)
            }
            other => Err(format!("backend de stockage inconnu : {other} (db | s3)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sélecteur de magasin : db opérationnel (put/get réels sur sqlite
    /// mémoire migrée), s3 validé à la construction (config incomplète →
    /// erreur explicite, complète → operator), inconnu rejeté. Défauts :
    /// db / pio / 900 s.
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

        // s3 sans config → refus explicite (pas d'opé silencieusement cassées).
        settings.storage_backend = "s3".into();
        // Arc<dyn ArtifactStore> n'implémente pas Debug — let-else, pas expect_err.
        let Err(err) = settings.store(&db) else {
            panic!("s3 non configuré doit être refusé à la construction");
        };
        assert!(err.contains("bucket"), "{err}");

        // Config complète → operator constructible (aucun I/O).
        settings.s3_endpoint = "http://localhost:9000".into();
        settings.s3_bucket = "pnex".into();
        settings.s3_access_key = "rustfsadmin".into();
        settings.s3_secret_key = "rustfsadmin".into();
        settings.s3_path_style = true;
        settings.store(&db).expect("s3 configuré → operator");

        settings.storage_backend = "gcs".into();
        assert!(settings.store(&db).is_err());
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

    /// Le Debug de FirmwareSettings ne fuite jamais le secret S3.
    #[test]
    fn debug_masque_le_secret_s3() {
        let settings = FirmwareSettings {
            s3_secret_key: "super-secret".into(),
            ..Default::default()
        };
        let repr = format!("{settings:?}");
        assert!(!repr.contains("super-secret"), "{repr}");
        assert!(repr.contains("<masqué>"), "{repr}");
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
