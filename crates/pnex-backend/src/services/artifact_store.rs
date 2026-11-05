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

    /// Aborts the multipart uploads started more than `older_than` ago whose key
    /// contains `key_part`. A killed process leaves its upload behind: the parts
    /// take disk space, no object listing shows them, and opendal has no call
    /// for them. Returns how many were aborted.
    pub async fn abort_stale_uploads(
        &self,
        key_part: &str,
        older_than: std::time::Duration,
    ) -> Result<usize, String> {
        let base = if self.path_style {
            format!("{}/{}", self.endpoint.trim_end_matches('/'), self.bucket)
        } else {
            self.endpoint
                .trim_end_matches('/')
                .replacen("://", &format!("://{}.", self.bucket), 1)
        };
        let limit = chrono::Utc::now()
            - chrono::Duration::from_std(older_than).map_err(|e| e.to_string())?;
        let mut stale = Vec::new();
        let mut page =
            reqwest::Url::parse(&format!("{base}?uploads")).map_err(|e| e.to_string())?;
        // Aborted once listed: deleting while listing would move the markers.
        loop {
            let listing = self.signed(http::Method::GET, page.as_str()).await?;
            let (found, next) = stale_uploads(&listing, key_part, limit)?;
            stale.extend(found);
            let Some((key, id)) = next else { break };
            page.set_query(Some("uploads"));
            page.query_pairs_mut()
                .append_pair("key-marker", &key)
                .append_pair("upload-id-marker", &id);
        }
        for upload in &stale {
            let mut url = reqwest::Url::parse(&base).map_err(|e| e.to_string())?;
            url.path_segments_mut()
                .map_err(|()| "S3: endpoint cannot have a path".to_string())?
                .extend(upload.key.split('/'));
            url.query_pairs_mut().append_pair("uploadId", &upload.id);
            self.signed(http::Method::DELETE, url.as_str()).await?;
        }
        Ok(stale.len())
    }

    /// One SigV4-signed request, its body on success.
    async fn signed(&self, method: http::Method, url: &str) -> Result<String, String> {
        let region = if self.region.is_empty() {
            "us-east-1"
        } else {
            &self.region
        };
        let signer = reqsign_core::Signer::new(
            reqsign_core::Context::new(),
            reqsign_aws_v4::StaticCredentialProvider::new(&self.access_key, &self.secret_key),
            reqsign_aws_v4::RequestSigner::new("s3", region),
        );
        let (mut parts, ()) = http::Request::builder()
            .method(method)
            .uri(url)
            .body(())
            .map_err(|e| e.to_string())?
            .into_parts();
        signer
            .sign(&mut parts, None)
            .await
            .map_err(|e| format!("S3 sign: {e}"))?;
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .send()
            .await
            .map_err(|e| format!("S3 uploads: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("S3 uploads: status {status}"));
        }
        response
            .text()
            .await
            .map_err(|e| format!("S3 uploads: {e}"))
    }
}

#[derive(Debug, Default, PartialEq)]
struct StaleUpload {
    key: String,
    id: String,
}

/// The uploads of a `ListMultipartUploads` answer started before `limit`, and the markers
/// of the next page if there is one. An upload with a date that does not read stays:
/// aborting a live upload loses data.
fn stale_uploads(
    xml: &str,
    key_part: &str,
    limit: chrono::DateTime<chrono::Utc>,
) -> Result<(Vec<StaleUpload>, Option<(String, String)>), String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut found = Vec::new();
    let (mut current, mut tag, mut initiated) = (StaleUpload::default(), Vec::new(), String::new());
    let (mut truncated, mut next_key, mut next_id) = (false, String::new(), String::new());
    loop {
        match reader
            .read_event()
            .map_err(|e| format!("S3 uploads: {e}"))?
        {
            Event::Start(e) => tag = e.local_name().as_ref().to_vec(),
            Event::Text(t) => {
                let text = t.decode().map_err(|e| format!("S3 uploads: {e}"))?;
                match tag.as_slice() {
                    b"Key" => current.key = text.into_owned(),
                    b"UploadId" => current.id = text.into_owned(),
                    b"Initiated" => initiated = text.into_owned(),
                    b"IsTruncated" => truncated = text == "true",
                    b"NextKeyMarker" => next_key = text.into_owned(),
                    b"NextUploadIdMarker" => next_id = text.into_owned(),
                    _ => {}
                }
            }
            Event::End(e) => {
                tag.clear();
                if e.local_name().as_ref() == b"Upload" {
                    let old =
                        chrono::DateTime::parse_from_rfc3339(&initiated).is_ok_and(|at| at < limit);
                    let upload = std::mem::take(&mut current);
                    if old && upload.key.contains(key_part) {
                        found.push(upload);
                    }
                    initiated.clear();
                }
            }
            Event::Eof => {
                // No marker to go on from: stop rather than read the same page forever.
                let next = (truncated && !next_key.is_empty()).then_some((next_key, next_id));
                return Ok((found, next));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod stale_upload_tests {
    use super::*;

    #[test]
    fn only_old_uploads_of_the_right_keys_are_stale() {
        let row = |key: &str, id: &str, at: &str| {
            format!("<Upload><Initiator><ID>x</ID></Initiator><Key>{key}</Key><UploadId>{id}</UploadId><Initiated>{at}</Initiated></Upload>")
        };
        let xml = format!(
            "<?xml version=\"1.0\"?><ListMultipartUploadsResult><Bucket>b</Bucket><IsTruncated>true</IsTruncated><NextKeyMarker>k</NextKeyMarker><NextUploadIdMarker>u</NextUploadIdMarker>{}{}{}{}</ListMultipartUploadsResult>",
            row("org_1/datasets/a.parquet", "old", "2026-11-01T00:00:00.000Z"),
            row("org_1/datasets/b.parquet", "young", "2026-11-05T00:00:00.000Z"),
            row("org_1/media/c.mp4", "other", "2026-11-01T00:00:00.000Z"),
            row("org_1/datasets/d.parquet", "unreadable", "yesterday"),
        );
        let limit = "2026-11-04T00:00:00Z".parse().unwrap();
        assert_eq!(
            stale_uploads(&xml, "/datasets/", limit).unwrap(),
            (
                vec![StaleUpload {
                    key: "org_1/datasets/a.parquet".into(),
                    id: "old".into()
                }],
                Some(("k".to_string(), "u".to_string()))
            )
        );
        let last =
            "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>";
        assert_eq!(stale_uploads(last, "", limit).unwrap(), (vec![], None));
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

    /// Multipart writer of a large object (8 MiB parts), for bodies streamed
    /// from elsewhere (ingestion staging, D318). Its sha256 is not known
    /// up front: the caller journals it instead of the object metadata.
    pub async fn writer(&self, key: &str) -> Result<opendal::Writer, String> {
        if key.is_empty() {
            return Err("empty key".into());
        }
        let mut meta = object_metadata(key, String::new(), chrono::Utc::now());
        meta.retain(|(k, _)| k != "pnex-sha256");
        self.operator
            .writer_with(key)
            .chunk(8 * 1024 * 1024)
            .user_metadata(meta)
            .await
            .map_err(|e| format!("S3 writer {key}: {e}"))
    }

    /// Writes a whole object (hashed into its metadata).
    pub async fn put_bytes(&self, key: &str, bytes: Bytes) -> Result<(), String> {
        self.write(key, bytes).await
    }

    /// Size of an object, `Ok(None)` when absent.
    pub async fn size(&self, key: &str) -> Result<Option<u64>, String> {
        match self.operator.stat(key).await {
            Ok(m) => Ok(Some(m.content_length())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("S3 stat {key}: {e}")),
        }
    }

    /// The whole object, `Ok(None)` when absent.
    pub async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.read(key).await
    }

    /// A byte range of an object.
    pub async fn get_range(&self, key: &str, range: std::ops::Range<u64>) -> Result<Bytes, String> {
        self.operator
            .read_with(key)
            .range(range)
            .await
            .map(|b| b.to_bytes())
            .map_err(|e| format!("S3 get {key}: {e}"))
    }

    /// Idempotent delete.
    pub async fn delete_key(&self, key: &str) -> Result<(), String> {
        self.remove(key).await
    }

    /// Objects under `prefix`: key, size, last modification.
    pub async fn list_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, u64, Option<chrono::DateTime<chrono::Utc>>)>, String> {
        let entries = match self.operator.list_with(prefix).recursive(true).await {
            Ok(e) => e,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("S3 list {prefix}: {e}")),
        };
        Ok(entries
            .into_iter()
            .filter(|e| !e.path().ends_with('/'))
            .map(|e| {
                let m = e.metadata();
                let at = m
                    .last_modified()
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::from(t)));
                (e.path().to_string(), m.content_length(), at)
            })
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
