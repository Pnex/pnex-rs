//! Media (D21): org-versioned asset library. Bytes NEVER live in the
//! database: each version references a logical key of the S3 object store
//! (`services::artifact_store::S3Store`, `settings.storage`).
//!
//! Logical keys: `org_{org}/media/{asset}/{version}_{filename}`, purged by
//! the keys known in DB, idempotently.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use loco_rs::config::Config;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use pnex_firmware_builder::sanitize_segment;

use crate::services::artifact_store::S3Config;

/// Media upload cap — raised explicitly on the POST routes
/// (`DefaultBodyLimit`; no precedent in the repo, first
/// body-bytes path).
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Media settings — `settings.media` (env `PNEX_MEDIA_MAX_BYTES`); the
/// bytes go to the object store of `settings.storage`.
#[derive(Clone, Debug)]
pub struct MediaSettings {
    /// Object store (secret masked in Debug).
    pub storage: S3Config,
    /// Media upload cap (clean JSON 413 beyond it; the axum
    /// limit, set to max+1 MB, remains the safety net).
    pub max_bytes: usize,
}

/// Partial serializable form of `settings.media` (all optional).
#[derive(Default, Deserialize)]
struct MediaPartial {
    max_bytes: Option<usize>,
}

impl Default for MediaSettings {
    fn default() -> Self {
        Self {
            storage: S3Config::default(),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

impl MediaSettings {
    /// Optional `settings.media` — defaults field by field.
    pub fn from_config(config: &Config) -> Self {
        let partial: MediaPartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("media"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let mut settings = Self {
            storage: S3Config::from_config(config),
            max_bytes: partial.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
        };
        if let Ok(max) = std::env::var("PNEX_MEDIA_MAX_BYTES") {
            if let Ok(parsed) = max.parse::<usize>() {
                settings.max_bytes = parsed;
            }
        }
        settings
    }

    /// The media store; an incomplete S3 configuration is rejected here
    /// (no network I/O).
    pub fn store(&self) -> Result<Arc<dyn MediaStore>, String> {
        Ok(Arc::new(self.storage.store()?) as Arc<dyn MediaStore>)
    }

    /// Logical key of a media version.
    pub fn storage_key(org_id: i64, asset_id: Uuid, version_number: i64, filename: &str) -> String {
        format!(
            "org_{org_id}/media/{asset_id}/{version_number}_{}",
            sanitize_segment(filename)
        )
    }

    /// Logical key of a server stitch job frame (Take 360 V2): same object
    /// store as media, dedicated namespace. Frames are kept after success
    /// (host re-stitch, future tuning, plan B2); purged by keys known in DB.
    pub fn stitch_key(job_id: Uuid, k: u32) -> String {
        format!("stitch_jobs/{job_id}/frame_{k:03}.jpg")
    }
}

/// Parses a boolean env flag (`true/1/yes/on` | `false/0/no/off`); `None`
/// when unset or unrecognised.
fn env_flag(key: &str) -> Option<bool> {
    let raw = std::env::var(key).ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// True when this process is configured as one pod of a multi-pod
/// deployment: explicit `PNEX_CLUSTER_MODE=true`, or a flow cluster with an
/// `advertise_url` or a cluster token (only useful across processes).
pub fn multi_pod_configured(config: &Config) -> bool {
    if env_flag("PNEX_CLUSTER_MODE") == Some(true) {
        return true;
    }
    let cluster = crate::services::flow_cluster::ClusterSettings::from_config(config);
    !cluster.advertise_url.is_empty() || !cluster.token.is_empty()
}

/// Media store error (`BuildError` pattern — local trait, no
/// coupling to pnex-firmware-builder).
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("media not found: {0}")]
    NotFound(String),
    #[error("media store: {0}")]
    Store(String),
}

/// Media byte store (same shape as `ArtifactStore`, local trait).
#[async_trait]
pub trait MediaStore: Send + Sync {
    /// Writes (overwrites if present).
    ///
    /// Takes ownership of a ref-counted buffer: the upload body is handed
    /// to the store without any copy (a 256 MiB upload stays 256 MiB).
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), MediaError>;
    /// `MediaError::NotFound` if absent.
    async fn get(&self, key: &str) -> Result<Vec<u8>, MediaError>;
    /// Idempotent (absent ≠ error) — S3 parity.
    async fn delete(&self, key: &str) -> Result<(), MediaError>;
    async fn exists(&self, key: &str) -> Result<bool, MediaError>;
}

// ───────────────────── Served content type (SEC-5) ─────────────────────

/// Content types a stored media version may be served with: raster images
/// and video render inline; every other declared type (HTML, SVG, XML, …)
/// is stored and served as opaque bytes (docs/architecture/security.md R12).
const INLINE_CONTENT_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/gif",
    "image/avif",
    "video/mp4",
    "video/quicktime",
    "video/webm",
    // Voice clips (voice-id.md), played by an <audio> element.
    "audio/wav",
    "audio/mpeg",
    "audio/ogg",
    "audio/flac",
    "audio/mp4",
];

/// Fallback of any content type outside [`INLINE_CONTENT_TYPES`].
pub const OPAQUE_CONTENT_TYPE: &str = "application/octet-stream";

/// Normalised, allowlisted content type of a media version: the client's
/// declared value is never trusted (an uploaded `text/html` or SVG would
/// otherwise run script on the app origin).
pub fn safe_content_type(declared: &str) -> &'static str {
    let essence = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    INLINE_CONTENT_TYPES
        .iter()
        .find(|t| **t == essence)
        .copied()
        .unwrap_or(OPAQUE_CONTENT_TYPE)
}

/// Response headers of user-supplied bytes (media, public tour assets):
/// allowlisted type, `nosniff`, a sandboxing CSP, and `attachment` for
/// anything that is not an inline image/video.
pub fn user_content_headers(declared: &str, filename: &str) -> [(&'static str, String); 4] {
    let content_type = safe_content_type(declared);
    let disposition = if content_type == OPAQUE_CONTENT_TYPE {
        "attachment"
    } else {
        "inline"
    };
    let filename = sanitize_segment(filename);
    [
        ("content-type", content_type.to_string()),
        (
            "content-disposition",
            format!("{disposition}; filename=\"{filename}\""),
        ),
        ("x-content-type-options", "nosniff".to_string()),
        (
            "content-security-policy",
            "default-src 'none'; sandbox".to_string(),
        ),
    ]
}

/// Hex sha256 (artifact_store pattern).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use loco_rs::environment::Environment;

    /// Default upload cap: 256 MB; the test S3 builds a store.
    #[test]
    fn defaults_media_settings() {
        let config = Config::new(&Environment::Test).unwrap();
        let settings = MediaSettings::from_config(&config);
        assert_eq!(settings.max_bytes, DEFAULT_MAX_BYTES);
        assert!(settings.store().is_ok());
        assert!(MediaSettings::default().store().is_err());
    }

    /// Partial `settings.media` block: `max_bytes` only.
    #[test]
    fn partial_max_bytes_only() {
        let mut config = Config::new(&Environment::Test).unwrap();
        config.settings = Some(serde_json::json!({
            "media": { "max_bytes": 1024 }
        }));
        assert_eq!(MediaSettings::from_config(&config).max_bytes, 1024);
    }

    /// SEC-5: script-capable types are never served as such.
    #[test]
    fn content_type_allowlist() {
        assert_eq!(safe_content_type("image/JPEG; q=1"), "image/jpeg");
        for bad in [
            "text/html",
            "image/svg+xml",
            "application/xhtml+xml",
            "text/xml",
            "",
        ] {
            assert_eq!(safe_content_type(bad), OPAQUE_CONTENT_TYPE, "{bad}");
        }
        let h = user_content_headers("text/html", "x.html");
        assert_eq!(h[0].1, OPAQUE_CONTENT_TYPE);
        assert!(h[1].1.starts_with("attachment"));
        assert_eq!(h[2], ("x-content-type-options", "nosniff".to_string()));
        assert!(user_content_headers("image/png", "a.png")[1]
            .1
            .starts_with("inline"));
    }

    /// Key format: org_{id}/media/{asset}/{version}_{filename} with
    /// the file name sanitized (outside [A-Za-z0-9._-] → `_`).
    #[test]
    fn cle_de_stockage_sanitize() {
        let asset = Uuid::new_v4();
        let key = MediaSettings::storage_key(7, asset, 3, "mon panorama (1).jpg");
        assert_eq!(key, format!("org_7/media/{asset}/3_mon_panorama__1_.jpg"));
    }
}
