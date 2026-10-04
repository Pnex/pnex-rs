//! Média (D21) — bibliothèque d'assets versionnée par org. Les octets ne
//! vivent JAMAIS en base : chaque version référence une clé logique dans ce
//! magasin dédié (école `artifact_store` mais trait local — les backends
//! firmware portent `ArtifactStore` de la crate builder, mauvais couplage
//! pour les médias).
//!
//! - [`FsStore`] (`fs`, défaut) : disque local (dev, tier hobbyist) —
//!   opendal `services::Fs`, répertoire `settings.media.dir`
//!   (env `PNEX_MEDIA_DIR`).
//! - [`S3Store`] (`s3`, tier industriel) : stockage compatible S3 (RustFS…)
//!   via opendal, même recette que `artifact_store::S3Store` (copie assumée
//!   ~40 lignes : les S3Store firmware implémentent `ArtifactStore`, un
//!   générique à type-erasure serait sur-ingénierie pour deux backends).
//!
//! Clés logiques : `org_{org}/media/{asset}/{version}_{filename}` — purge
//! par clés connues en DB, idempotente (parité S3, école `artifact_store`).

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use loco_rs::config::Config;
use opendal::layers::RetryLayer;
use opendal::services::S3;
use opendal::{ErrorKind, Operator};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use pnex_firmware_builder::sanitize_segment;

use crate::services::artifact_store::S3Config;

/// Plafond d'un upload média — relevé explicitement sur les routes POST
/// (`DefaultBodyLimit` ; aucun précédent dans le repo, premier chemin
/// body-bytes).
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Réglages média — `settings.media` (env `MEDIA_BACKEND`, `PNEX_MEDIA_DIR`,
/// `PNEX_MEDIA_MAX_BYTES`), école `settings.firmware.*`.
pub struct MediaSettings {
    /// `fs` (défaut) | `s3` — l'env `MEDIA_BACKEND` surcharge (école
    /// `STORAGE_BACKEND`).
    pub storage_backend: String,
    /// Racine du magasin `fs`.
    pub dir: String,
    /// Plafond d'upload média (réponse 413 JSON propre au-delà ; la limite
    /// axum, posée à max+1 Mo, reste le filet).
    pub max_bytes: usize,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_path_style: bool,
    /// The `fs` root is a volume shared by every pod (RWX). Required to
    /// boot a multi-pod deployment on the `fs` backend
    /// (`settings.media.storage.fs_shared`, env `PNEX_MEDIA_FS_SHARED`).
    pub fs_shared: bool,
}

impl std::fmt::Debug for MediaSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaSettings")
            .field("storage_backend", &self.storage_backend)
            .field("dir", &self.dir)
            .field("max_bytes", &self.max_bytes)
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
            .field("fs_shared", &self.fs_shared)
            .finish()
    }
}

/// Forme sérialisable partielle de `settings.media` (tout optionnel).
#[derive(Default, Deserialize)]
struct MediaPartial {
    storage: Option<MediaStoragePartial>,
    max_bytes: Option<usize>,
}

#[derive(Default, Deserialize)]
struct MediaStoragePartial {
    backend: Option<String>,
    dir: Option<String>,
    s3_endpoint: Option<String>,
    s3_bucket: Option<String>,
    s3_region: Option<String>,
    s3_access_key: Option<String>,
    s3_secret_key: Option<String>,
    s3_path_style: Option<bool>,
    fs_shared: Option<bool>,
}

impl Default for MediaSettings {
    fn default() -> Self {
        Self {
            storage_backend: "fs".into(),
            dir: "./storage/media".into(),
            max_bytes: DEFAULT_MAX_BYTES,
            s3_endpoint: String::new(),
            s3_bucket: String::new(),
            s3_region: String::new(),
            s3_access_key: String::new(),
            s3_secret_key: String::new(),
            s3_path_style: false,
            fs_shared: false,
        }
    }
}

impl MediaSettings {
    /// `settings.media` optionnelle — défauts champ par champ.
    pub fn from_config(config: &Config) -> Self {
        let partial: MediaPartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("media"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let defaults = Self::default();
        let mut settings = Self {
            storage_backend: partial
                .storage
                .as_ref()
                .and_then(|s| s.backend.clone())
                .unwrap_or(defaults.storage_backend),
            dir: partial
                .storage
                .as_ref()
                .and_then(|s| s.dir.clone())
                .unwrap_or(defaults.dir),
            max_bytes: partial.max_bytes.unwrap_or(defaults.max_bytes),
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
            fs_shared: partial
                .storage
                .as_ref()
                .and_then(|s| s.fs_shared)
                .unwrap_or(false),
        };
        if let Some(shared) = env_flag("PNEX_MEDIA_FS_SHARED") {
            settings.fs_shared = shared;
        }
        // Décision utilisateur : l'env surcharge la config (école firmware).
        if let Ok(backend) = std::env::var("MEDIA_BACKEND") {
            if !backend.is_empty() {
                settings.storage_backend = backend;
            }
        }
        if let Ok(dir) = std::env::var("PNEX_MEDIA_DIR") {
            if !dir.is_empty() {
                settings.dir = dir;
            }
        }
        if let Ok(max) = std::env::var("PNEX_MEDIA_MAX_BYTES") {
            if let Ok(parsed) = max.parse::<usize>() {
                settings.max_bytes = parsed;
            }
        }
        settings
    }

    /// Magasin média selon le backend (`fs` | `s3`). Aucun I/O réseau à la
    /// construction (l'opendal fs crée les répertoires à l'écriture) ; le
    /// backend `s3` valide sa configuration à la construction.
    pub fn store(&self) -> Result<Arc<dyn MediaStore>, String> {
        match self.storage_backend.as_str() {
            "fs" => FsStore::connect(&self.dir).map(|s| Arc::new(s) as Arc<dyn MediaStore>),
            "s3" => S3Store::connect(&S3Config {
                endpoint: self.s3_endpoint.clone(),
                bucket: self.s3_bucket.clone(),
                region: self.s3_region.clone(),
                access_key: self.s3_access_key.clone(),
                secret_key: self.s3_secret_key.clone(),
                path_style: self.s3_path_style,
            })
            .map(|s| Arc::new(s) as Arc<dyn MediaStore>),
            other => Err(format!(
                "backend de stockage média inconnu : {other} (fs | s3)"
            )),
        }
    }

    /// Clé logique d'une version média.
    pub fn storage_key(org_id: i64, asset_id: Uuid, version_number: i64, filename: &str) -> String {
        format!(
            "org_{org_id}/media/{asset_id}/{version_number}_{}",
            sanitize_segment(filename)
        )
    }

    /// Clé logique d'une frame d'un job de stitch serveur (Take 360 V2) —
    /// même magasin que les médias (fs | s3 selon `settings.media`), espace
    /// de noms dédié. Les frames sont conservées après succès (re-stitch
    /// hôte / futur tuning — plan B2) ; purge par clés connues en DB.
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

/// Outcome of the media storage boot check.
#[derive(Debug, PartialEq, Eq)]
pub enum StorageBootCheck {
    /// Object storage, or a single-node `fs`.
    Ok,
    /// Multi-pod on `fs` declared shared (RWX volume): boot, but warn.
    SharedFs,
    /// Multi-pod on a per-pod `fs` disk: media, video segments, stitch
    /// frames and model bytes written by one pod are invisible to others.
    Refuse,
}

impl MediaSettings {
    /// Pure decision of [`media_boot_guard`].
    pub fn boot_check(&self, multi_pod: bool) -> StorageBootCheck {
        if self.storage_backend != "fs" || !multi_pod {
            StorageBootCheck::Ok
        } else if self.fs_shared {
            StorageBootCheck::SharedFs
        } else {
            StorageBootCheck::Refuse
        }
    }
}

/// Boot guard (from `App::after_routes`): refuses to start a multi-pod
/// deployment whose media store is a pod-local disk, unless the operator
/// declares the `fs` root a shared volume (`PNEX_MEDIA_FS_SHARED=true`).
/// Single-node deployments (dev, hobbyist) are untouched.
pub fn media_boot_guard(config: &Config) -> Result<(), String> {
    let settings = MediaSettings::from_config(config);
    match settings.boot_check(multi_pod_configured(config)) {
        StorageBootCheck::Ok => Ok(()),
        StorageBootCheck::SharedFs => {
            tracing::warn!(
                dir = %settings.dir,
                "multi-pod deployment on the fs media backend: PNEX_MEDIA_DIR must be the SAME shared (RWX) volume on every pod and worker — prefer MEDIA_BACKEND=s3"
            );
            Ok(())
        }
        StorageBootCheck::Refuse => {
            let msg = format!(
                "refusing to boot: multi-pod deployment (flow cluster advertise_url/token or PNEX_CLUSTER_MODE) with the pod-local fs media backend ({}) — media, video segments, stitch frames and ML models would not be visible across pods. Set MEDIA_BACKEND=s3, or PNEX_MEDIA_FS_SHARED=true if PNEX_MEDIA_DIR is a shared RWX volume",
                settings.dir
            );
            tracing::error!("{msg}");
            Err(msg)
        }
    }
}

/// Erreur du magasin média (école `BuildError` — trait local, pas de
/// couplage à pnex-firmware-builder).
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("média introuvable : {0}")]
    NotFound(String),
    #[error("magasin média : {0}")]
    Store(String),
}

/// Magasin d'octets média (même forme que `ArtifactStore`, trait local).
#[async_trait]
pub trait MediaStore: Send + Sync {
    /// Écrit (écrase si présent).
    ///
    /// Takes ownership of a ref-counted buffer: the upload body is handed
    /// to the store without any copy (a 256 MiB upload stays 256 MiB).
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), MediaError>;
    /// `MediaError::NotFound` si absent.
    async fn get(&self, key: &str) -> Result<Vec<u8>, MediaError>;
    /// Idempotent (absent ≠ erreur) — parité S3.
    async fn delete(&self, key: &str) -> Result<(), MediaError>;
    async fn exists(&self, key: &str) -> Result<bool, MediaError>;
}

/// Backend `fs` : disque local (dev, tier hobbyist) — opendal `services::Fs`.
#[derive(Clone)]
pub struct FsStore {
    operator: Operator,
}

impl FsStore {
    /// Construit l'operator. Aucun I/O réseau — juste la création du
    /// répertoire racine.
    pub fn connect(dir: &str) -> Result<Self, String> {
        if dir.is_empty() {
            return Err("MediaStore fs : répertoire requis (PNEX_MEDIA_DIR)".into());
        }
        // L'opendal fs crée les sous-répertoires à l'écriture ; la racine,
        // elle, doit exister.
        std::fs::create_dir_all(dir).map_err(|e| format!("MediaStore fs : {dir} : {e}"))?;
        let builder = opendal::services::Fs::default().root(dir);
        // Retry des erreurs transitoires — école artifact_store.
        let operator = Operator::new(builder)
            .map_err(|e| format!("MediaStore fs : initialisation impossible : {e}"))?
            .layer(RetryLayer::default());
        Ok(Self { operator })
    }
}

#[async_trait]
impl MediaStore for FsStore {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), MediaError> {
        if key.is_empty() {
            return Err(MediaError::Store("clé vide".into()));
        }
        self.operator
            .write(key, bytes)
            .await
            .map_err(|e| MediaError::Store(format!("fs put {key} : {e}")))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, MediaError> {
        let buf = self.operator.read(key).await.map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                MediaError::NotFound(key.to_string())
            } else {
                MediaError::Store(format!("fs get {key} : {e}"))
            }
        })?;
        Ok(buf.to_vec())
    }

    async fn delete(&self, key: &str) -> Result<(), MediaError> {
        // Delete idempotent nativement (fs + S3) — école artifact_store.
        self.operator
            .delete(key)
            .await
            .map_err(|e| MediaError::Store(format!("fs delete {key} : {e}")))
    }

    async fn exists(&self, key: &str) -> Result<bool, MediaError> {
        self.operator.stat(key).await.map(|_| true).or_else(|e| {
            if e.kind() == ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(MediaError::Store(format!("fs stat {key} : {e}")))
            }
        })
    }
}

/// Backend `s3` : stockage compatible S3 (RustFS…) — même recette que
/// `artifact_store::S3Store` (duplication assumée, voir doc de module).
#[derive(Clone)]
pub struct S3Store {
    operator: Operator,
}

impl S3Store {
    /// Construit l'operator. Aucun I/O réseau ici — juste l'assemblage du
    /// client : une config incomplète échoue maintenant (message explicite),
    /// pas au premier put (école artifact_store).
    pub fn connect(config: &S3Config) -> Result<Self, String> {
        if config.bucket.is_empty() {
            return Err("MediaStore s3 : bucket requis (PNEX_S3_BUCKET)".into());
        }
        if config.endpoint.is_empty() {
            return Err("MediaStore s3 : endpoint requis (PNEX_S3_ENDPOINT)".into());
        }
        if config.access_key.is_empty() || config.secret_key.is_empty() {
            return Err(
                "MediaStore s3 : credentials requis (PNEX_S3_ACCESS_KEY / PNEX_S3_SECRET_KEY)"
                    .into(),
            );
        }
        let mut builder = S3::default()
            .bucket(&config.bucket)
            .endpoint(&config.endpoint)
            .access_key_id(&config.access_key)
            .secret_access_key(&config.secret_key);
        if !config.path_style {
            builder = builder.enable_virtual_host_style();
        }
        let region = if config.region.is_empty() {
            "us-east-1"
        } else {
            config.region.as_str()
        };
        builder = builder.region(region);
        // Retry des erreurs transitoires (5xx, timeout) — école artifact_store.
        crate::services::artifact_store::ensure_http_transport();
        let operator = Operator::new(builder)
            .map_err(|e| format!("MediaStore s3 : initialisation impossible : {e}"))?
            .layer(RetryLayer::default());
        Ok(Self { operator })
    }
}

#[async_trait]
impl MediaStore for S3Store {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), MediaError> {
        if key.is_empty() {
            return Err(MediaError::Store("clé vide".into()));
        }
        self.operator
            .write(key, bytes)
            .await
            .map_err(|e| MediaError::Store(format!("s3 put {key} : {e}")))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, MediaError> {
        let buf = self.operator.read(key).await.map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                MediaError::NotFound(key.to_string())
            } else {
                MediaError::Store(format!("s3 get {key} : {e}"))
            }
        })?;
        Ok(buf.to_vec())
    }

    async fn delete(&self, key: &str) -> Result<(), MediaError> {
        // Delete S3 idempotent nativement — école artifact_store.
        self.operator
            .delete(key)
            .await
            .map_err(|e| MediaError::Store(format!("s3 delete {key} : {e}")))
    }

    async fn exists(&self, key: &str) -> Result<bool, MediaError> {
        self.operator.stat(key).await.map(|_| true).or_else(|e| {
            if e.kind() == ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(MediaError::Store(format!("s3 stat {key} : {e}")))
            }
        })
    }
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
    "video/webm",
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

/// sha256 hexadécimal (école artifact_store).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use loco_rs::environment::Environment;

    /// Défauts : backend fs, ./storage/media, plafond 256 Mo.
    #[test]
    fn defauts_media_settings() {
        let config = Config::new(&Environment::Test).unwrap();
        let settings = MediaSettings::from_config(&config);
        assert_eq!(settings.storage_backend, "fs");
        assert_eq!(settings.dir, "./storage/media");
        assert_eq!(settings.max_bytes, DEFAULT_MAX_BYTES);
    }

    /// Bloc `settings.media` partiel : seul `max_bytes` présent → le reste
    /// garde les défauts.
    #[test]
    fn partiel_max_bytes_seul() {
        let mut config = Config::new(&Environment::Test).unwrap();
        config.settings = Some(serde_json::json!({
            "media": { "max_bytes": 1024 }
        }));
        let settings = MediaSettings::from_config(&config);
        assert_eq!(settings.max_bytes, 1024);
        assert_eq!(settings.storage_backend, "fs");
        assert_eq!(settings.dir, "./storage/media");
    }

    /// Backend inconnu rejeté à la construction du magasin.
    #[test]
    fn backend_inconnu_rejete() {
        let config = Config::new(&Environment::Test).unwrap();
        let mut settings = MediaSettings::from_config(&config);
        settings.storage_backend = "gdrive".into();
        assert!(settings.store().is_err());
    }

    #[test]
    fn multi_pod_fs_needs_shared_volume() {
        let mut settings = MediaSettings::default();
        assert_eq!(settings.boot_check(false), StorageBootCheck::Ok);
        assert_eq!(settings.boot_check(true), StorageBootCheck::Refuse);
        settings.fs_shared = true;
        assert_eq!(settings.boot_check(true), StorageBootCheck::SharedFs);
        settings.fs_shared = false;
        settings.storage_backend = "s3".into();
        assert_eq!(settings.boot_check(true), StorageBootCheck::Ok);
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

    /// Le secret S3 ne fuit pas dans le Debug.
    #[test]
    fn debug_sans_fuite_secret() {
        let settings = MediaSettings {
            s3_secret_key: "topsecret".into(),
            ..Default::default()
        };
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("topsecret"), "secret dans le Debug !");
        assert!(rendered.contains("<masqué>"));
    }

    /// Format de clé : org_{id}/media/{asset}/{version}_{filename} avec
    /// sanitize du nom de fichier (hors [A-Za-z0-9._-] → `_`).
    #[test]
    fn cle_de_stockage_sanitize() {
        let asset = Uuid::new_v4();
        let key = MediaSettings::storage_key(7, asset, 3, "mon panorama (1).jpg");
        assert_eq!(key, format!("org_7/media/{asset}/3_mon_panorama__1_.jpg"));
    }

    /// Cycle put/get/exists/delete réel sur le backend fs (répertoire
    /// temporaire jetable).
    #[tokio::test]
    async fn fs_store_cycle_complet() {
        let dir = std::env::temp_dir().join(format!("pnex-media-unit-{}", std::process::id()));
        let store = FsStore::connect(dir.to_str().unwrap()).unwrap();
        let key = "org_1/media/0197..../1_photo.jpg";
        store.put(key, Bytes::from_static(b"hello")).await.unwrap();
        assert!(store.exists(key).await.unwrap());
        assert_eq!(store.get(key).await.unwrap(), b"hello");
        store.delete(key).await.unwrap();
        assert!(!store.exists(key).await.unwrap());
        // Absent → NotFound.
        assert!(matches!(store.get(key).await, Err(MediaError::NotFound(_))));
        std::fs::remove_dir_all(dir).ok();
    }
}
