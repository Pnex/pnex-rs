//! Object storage — the single home of every stored byte: firmware
//! artifacts and media (assets, video segments, stitch frames, models).
//! S3-compatible only (AWS, RustFS, Scaleway…) via opendal; the database
//! keeps metadata and logical keys, never bytes, and nothing is written to
//! a local disk.
//!
//! Configuration: `settings.storage` (env `PNEX_S3_*`), one bucket for
//! everything. Keys are namespaced (`org_{id}/firmware/…`,
//! `org_{id}/media/…`, `stitch_jobs/…`); writes overwrite, deletes are
//! idempotent.

use std::collections::HashMap;

use async_trait::async_trait;
use axum::body::Bytes;
use loco_rs::config::Config;
use opendal::layers::RetryLayer;
use opendal::services::S3;
use opendal::{ErrorKind, Operator};
use serde::Deserialize;

use pnex_firmware_builder::{ArtifactStore, BuildError};

use crate::services::media::{MediaError, MediaStore};

/// Documented builder assumption: merged binaries are 1–4 MB. Defensive
/// hard cap — beyond it something is wrong (corrupt image, failed merge).
const MAX_BYTES: usize = 50 * 1024 * 1024;

/// S3 store shared by firmware artifacts and media.
#[derive(Clone)]
pub struct S3Store {
    operator: Operator,
}

/// S3 connection settings — `settings.storage` (env `PNEX_S3_*`).
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Path-style addressing (`http://host/bucket/key`): RustFS and most
    /// self-hosted S3; AWS uses virtual hosts.
    pub path_style: bool,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key", &self.access_key)
            .field(
                "secret_key",
                &if self.secret_key.is_empty() {
                    "<empty>"
                } else {
                    "<masked>"
                },
            )
            .field("path_style", &self.path_style)
            .finish()
    }
}

impl S3Config {
    /// `settings.storage` of the loaded configuration (missing = empty,
    /// rejected by [`S3Store::connect`]).
    pub fn from_config(config: &Config) -> Self {
        config
            .settings
            .as_ref()
            .and_then(|s| s.get("storage"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    /// The store of this configuration.
    pub fn store(&self) -> Result<S3Store, String> {
        S3Store::connect(self)
    }
}

/// opendal 0.58 serves HTTP services (S3) through a process-wide transport
/// that its `auto-register-services` feature installs before `main` — off
/// here (`default-features = false`). Without it every S3 call fails with
/// "default HTTP transport is not installed". Called before building any
/// S3 operator; idempotent.
pub(crate) fn ensure_http_transport() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(opendal::install_default);
}

impl S3Store {
    /// Live connectivity probe of the bucket (opendal `check`: a cheap
    /// list/stat round-trip). Used by the platform status page (D72).
    pub async fn check(&self) -> Result<(), String> {
        self.operator.check().await.map_err(|e| e.to_string())
    }

    /// Builds the operator. No network I/O: an incomplete configuration
    /// fails now with an explicit message, not at the first write.
    pub fn connect(config: &S3Config) -> Result<Self, String> {
        if config.bucket.is_empty() {
            return Err("S3: bucket required (PNEX_S3_BUCKET)".into());
        }
        if config.endpoint.is_empty() {
            return Err("S3: endpoint required (PNEX_S3_ENDPOINT)".into());
        }
        if config.access_key.is_empty() || config.secret_key.is_empty() {
            return Err(
                "S3: credentials required (PNEX_S3_ACCESS_KEY / PNEX_S3_SECRET_KEY)".into(),
            );
        }
        let mut builder = S3::default()
            .bucket(&config.bucket)
            .endpoint(&config.endpoint)
            .access_key_id(&config.access_key)
            .secret_access_key(&config.secret_key);
        // opendal 0.57+: path-style is the default; virtual hosts are opt-in.
        if !config.path_style {
            builder = builder.enable_virtual_host_style();
        }
        // The signer needs a region: us-east-1 is ignored by RustFS and most
        // self-hosted S3, and valid for AWS.
        let region = if config.region.is_empty() {
            "us-east-1"
        } else {
            config.region.as_str()
        };
        builder = builder.region(region);
        // Retries transient errors (5xx, timeouts): writes are idempotent
        // (same key, same content).
        ensure_http_transport();
        let operator = Operator::new(builder)
            .map_err(|e| format!("S3: cannot initialise: {e}"))?
            .layer(RetryLayer::default());
        Ok(Self { operator })
    }

    async fn write(&self, key: &str, bytes: Bytes) -> Result<(), String> {
        if key.is_empty() {
            return Err("empty key".into());
        }
        // Hashing up to 256 MiB is CPU bound: off the async runtime.
        let hashed = bytes.clone();
        let sha256 =
            tokio::task::spawn_blocking(move || crate::services::media::sha256_hex(&hashed))
                .await
                .map_err(|e| format!("S3 put {key}: hash: {e}"))?;
        self.operator
            .write_with(key, bytes)
            .user_metadata(object_metadata(key, sha256, chrono::Utc::now()))
            .await
            .map(|_| ())
            .map_err(|e| format!("S3 put {key}: {e}"))
    }

    /// User metadata of an object (`pnex-*` entries), `Ok(None)` when absent.
    pub async fn metadata(&self, key: &str) -> Result<Option<HashMap<String, String>>, String> {
        match self.operator.stat(key).await {
            Ok(m) => Ok(Some(m.user_metadata().cloned().unwrap_or_default())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("S3 stat {key}: {e}")),
        }
    }

    /// Every object key of the bucket.
    ///
    /// shortcut: collects the whole listing in memory, fine up to a few
    /// million objects; stream it (`lister_with`) beyond.
    pub async fn list_keys(&self) -> Result<Vec<String>, String> {
        let entries = self
            .operator
            .list_with("")
            .recursive(true)
            .await
            .map_err(|e| format!("S3 list: {e}"))?;
        Ok(entries
            .into_iter()
            .filter(|e| !e.path().ends_with('/'))
            .map(|e| e.path().to_string())
            .collect())
    }

    /// `Ok(None)` when absent.
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self.operator.read(key).await {
            Ok(buf) => Ok(Some(buf.to_vec())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("S3 get {key}: {e}")),
        }
    }

    /// Idempotent: S3 answers 204 on an absent key.
    async fn remove(&self, key: &str) -> Result<(), String> {
        self.operator
            .delete(key)
            .await
            .map_err(|e| format!("S3 delete {key}: {e}"))
    }

    async fn stat(&self, key: &str) -> Result<bool, String> {
        match self.operator.stat(key).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("S3 stat {key}: {e}")),
        }
    }
}

/// Self-describing metadata written with every object, so a bucket can be
/// audited, garbage-collected or partly re-indexed without the database:
/// `pnex-kind` and `pnex-org` (from the key namespace), `pnex-sha256`,
/// `pnex-written-at`. Stable facts only (S3 metadata is immutable); never a
/// secret, an email or a user-supplied file name (R4, R16).
fn object_metadata(
    key: &str,
    sha256: String,
    written_at: chrono::DateTime<chrono::Utc>,
) -> Vec<(String, String)> {
    let mut parts = key.split('/');
    let first = parts.next().unwrap_or_default();
    let (org, kind) = match first.strip_prefix("org_") {
        Some(org) => (Some(org), parts.next().unwrap_or_default()),
        None => (None, first),
    };
    let mut meta = vec![
        ("pnex-kind".to_string(), kind.to_string()),
        ("pnex-sha256".to_string(), sha256),
        ("pnex-written-at".to_string(), written_at.to_rfc3339()),
    ];
    if let Some(org) = org {
        meta.push(("pnex-org".to_string(), org.to_string()));
    }
    meta
}

#[async_trait]
impl ArtifactStore for S3Store {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError> {
        if bytes.len() > MAX_BYTES {
            return Err(BuildError::Store(format!(
                "artifact {key} too large: {} B (cap {MAX_BYTES})",
                bytes.len()
            )));
        }
        self.write(key, Bytes::copy_from_slice(bytes))
            .await
            .map_err(BuildError::Store)
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError> {
        self.read(key)
            .await
            .map_err(BuildError::Store)?
            .ok_or_else(|| BuildError::NotFound(key.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), BuildError> {
        self.remove(key).await.map_err(BuildError::Store)
    }

    async fn exists(&self, key: &str) -> Result<bool, BuildError> {
        self.stat(key).await.map_err(BuildError::Store)
    }
}

#[async_trait]
impl MediaStore for S3Store {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), MediaError> {
        self.write(key, bytes).await.map_err(MediaError::Store)
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, MediaError> {
        self.read(key)
            .await
            .map_err(MediaError::Store)?
            .ok_or_else(|| MediaError::NotFound(key.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), MediaError> {
        self.remove(key).await.map_err(MediaError::Store)
    }

    async fn exists(&self, key: &str) -> Result<bool, MediaError> {
        self.stat(key).await.map_err(MediaError::Store)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The test S3 (compose `rustfs`, bucket `pnex-test`), same values as
    /// `config/test.yaml`.
    pub(crate) fn test_config() -> S3Config {
        let config = Config::new(&loco_rs::environment::Environment::Test).unwrap();
        S3Config::from_config(&config)
    }

    /// Each required field missing → explicit error; complete → operator.
    #[test]
    fn connect_validates_configuration() {
        let base = test_config();
        for (field, expected) in [
            ("bucket", "bucket"),
            ("endpoint", "endpoint"),
            ("access_key", "credentials"),
            ("secret_key", "credentials"),
        ] {
            let mut c = base.clone();
            match field {
                "bucket" => c.bucket.clear(),
                "endpoint" => c.endpoint.clear(),
                "access_key" => c.access_key.clear(),
                _ => c.secret_key.clear(),
            }
            let Err(err) = S3Store::connect(&c) else {
                panic!("{field} empty must be rejected");
            };
            assert!(err.contains(expected), "{expected} expected in: {err}");
        }
        assert!(S3Store::connect(&base).is_ok());
    }

    /// The secret never leaks through Debug.
    #[test]
    fn debug_masks_secret() {
        let rendered = format!(
            "{:?}",
            S3Config {
                secret_key: "topsecret".into(),
                ..Default::default()
            }
        );
        assert!(!rendered.contains("topsecret"));
    }

    /// Regression: without the process-wide opendal HTTP transport, an S3
    /// write failed with "default HTTP transport is not installed" before
    /// any I/O. Against an unreachable endpoint it must now fail on the
    /// network instead.
    #[tokio::test]
    async fn s3_write_reaches_the_network() {
        let mut config = test_config();
        config.endpoint = "http://127.0.0.1:9".into();
        let store = S3Store::connect(&config).expect("connect");
        let Err(err) = ArtifactStore::put(&store, "org_1/firmware/x.bin", b"x").await else {
            panic!("nothing listens on 127.0.0.1:9");
        };
        let msg = format!("{err:?}");
        assert!(
            !msg.contains("transport is not installed"),
            "the S3 operator has no HTTP transport: {msg}"
        );
    }

    /// Full cycle against the test S3 (`docker compose up -d rustfs
    /// rustfs-init`): overwrite, idempotent delete, absent → NotFound, size
    /// cap.
    #[tokio::test]
    async fn s3_full_cycle() {
        let store = test_config().store().expect("connect");
        let key = format!("org_7/firmware/{}-firmware.bin", uuid::Uuid::new_v4());

        assert!(!ArtifactStore::exists(&store, &key).await.expect("exists"));
        ArtifactStore::put(&store, &key, b"bytes-v1")
            .await
            .expect("put v1");
        assert_eq!(
            ArtifactStore::get(&store, &key).await.expect("get"),
            b"bytes-v1"
        );
        ArtifactStore::put(&store, &key, b"bytes-v2")
            .await
            .expect("put v2");
        assert_eq!(
            ArtifactStore::get(&store, &key).await.expect("get"),
            b"bytes-v2"
        );
        // Self-describing metadata, refreshed by the overwrite.
        let meta = store.metadata(&key).await.expect("stat").expect("present");
        assert_eq!(meta["pnex-kind"], "firmware");
        assert_eq!(meta["pnex-org"], "7");
        assert_eq!(
            meta["pnex-sha256"],
            crate::services::media::sha256_hex(b"bytes-v2")
        );
        assert!(store.list_keys().await.expect("list").contains(&key));

        ArtifactStore::delete(&store, &key).await.expect("delete");
        ArtifactStore::delete(&store, &key)
            .await
            .expect("delete idempotent");
        assert!(matches!(
            ArtifactStore::get(&store, &key).await,
            Err(BuildError::NotFound(_))
        ));
        assert!(matches!(
            MediaStore::get(&store, &key).await,
            Err(MediaError::NotFound(_))
        ));
        assert!(matches!(
            ArtifactStore::put(&store, &key, &vec![0u8; MAX_BYTES + 1]).await,
            Err(BuildError::Store(_))
        ));
        assert!(matches!(
            ArtifactStore::put(&store, "", b"x").await,
            Err(BuildError::Store(_))
        ));
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::object_metadata;

    #[test]
    fn kind_and_org_come_from_the_key_namespace() {
        let at = chrono::DateTime::from_timestamp(0, 0).unwrap();
        let m = object_metadata("org_3/media/a/1_x.jpg", "abc".into(), at);
        assert!(m.contains(&("pnex-kind".into(), "media".into())));
        assert!(m.contains(&("pnex-org".into(), "3".into())));
        assert!(m.contains(&("pnex-sha256".into(), "abc".into())));
        let m = object_metadata("stitch_jobs/j/frame_000.jpg", "abc".into(), at);
        assert!(m.contains(&("pnex-kind".into(), "stitch_jobs".into())));
        assert!(!m.iter().any(|(k, _)| k == "pnex-org"));
    }
}
