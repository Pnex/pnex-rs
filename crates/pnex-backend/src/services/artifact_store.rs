//! Backends réels de l'`ArtifactStore` (D5 v2) — la crate builder ne porte
//! que le trait ; les implémentations vivent ici :
//!
//! - [`DbStore`] (`db`, défaut) : binaires en base, table
//!   `firmware_artifacts` — tiers sqlite (tout-en-un) et postgres (pods API
//!   stateless, n'importe quel réplica sert le download). La clé logique est
//!   stable par device (`org_{id}/firmware/{device}-firmware.bin`) → `put()`
//!   est un upsert `ON CONFLICT (key) DO UPDATE` : un rebuild écrase la
//!   ligne précédente, zéro artefact orphelin. Portable PG (BYTEA) / sqlite
//!   (BLOB).
//! - [`S3Store`] (`s3`, tier industriel) : binaires sur stockage compatible
//!   S3 (AWS, RustFS, Scaleway…) via opendal. La base (PG ou sqlite) garde
//!   données + queue ; seuls les artefacts partent sur S3. Clés logiques
//!   identiques — la sémantique par device se mappe 1:1 sur PutObject
//!   (écrasement natif, delete idempotent).

use async_trait::async_trait;
use opendal::layers::RetryLayer;
use opendal::services::S3;
use opendal::{ErrorKind, Operator};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use sha2::{Digest, Sha256};

use pnex_firmware_builder::{ArtifactStore, BuildError};

use crate::models::_entities::firmware_artifacts;

/// Backend `db` : magasin d'artefacts adossé à la base de l'app.
#[derive(Clone)]
pub struct DbStore {
    db: DatabaseConnection,
}

/// Hypothèse documentée du builder : binaires mergés 1–4 Mo. Plafond dur
/// défensif — au-delà, quelque chose ne va pas (image corrompue, merge raté).
const MAX_BYTES: usize = 50 * 1024 * 1024;

impl DbStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

#[async_trait]
impl ArtifactStore for DbStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError> {
        if key.is_empty() {
            return Err(BuildError::Store("clé vide".into()));
        }
        if bytes.len() > MAX_BYTES {
            return Err(BuildError::Store(format!(
                "artefact {key} trop volumineux : {} o (plafond {MAX_BYTES})",
                bytes.len()
            )));
        }
        let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();
        // `updated_at` posé explicitement (les deux branches insert/update) :
        // le hook `before_save` ne s'exécute pas sur le chemin on-conflict.
        let am = firmware_artifacts::ActiveModel {
            key: Set(key.to_string()),
            bytes: Set(bytes.to_vec()),
            size_bytes: Set(bytes.len() as i64),
            sha256: Set(Some(sha256_hex(bytes))),
            updated_at: Set(now),
            ..Default::default()
        };
        firmware_artifacts::Entity::insert(am)
            .on_conflict(
                OnConflict::column(firmware_artifacts::Column::Key)
                    .update_columns([
                        firmware_artifacts::Column::Bytes,
                        firmware_artifacts::Column::SizeBytes,
                        firmware_artifacts::Column::Sha256,
                        firmware_artifacts::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await
            .map_err(|e| BuildError::Store(format!("upsert artefact {key} : {e}")))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError> {
        firmware_artifacts::Entity::find()
            .filter(firmware_artifacts::Column::Key.eq(key))
            .one(&self.db)
            .await
            .map_err(|e| BuildError::Store(format!("get artefact {key} : {e}")))?
            .map(|m| m.bytes)
            .ok_or_else(|| BuildError::NotFound(key.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), BuildError> {
        // Idempotent : absent ≠ erreur (parité S3).
        firmware_artifacts::Entity::delete_many()
            .filter(firmware_artifacts::Column::Key.eq(key))
            .exec(&self.db)
            .await
            .map_err(|e| BuildError::Store(format!("delete artefact {key} : {e}")))?;
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, BuildError> {
        firmware_artifacts::Entity::find()
            .filter(firmware_artifacts::Column::Key.eq(key))
            .one(&self.db)
            .await
            .map_err(|e| BuildError::Store(format!("exists artefact {key} : {e}")))
            .map(|row| row.is_some())
    }
}

// ─────────────────── Backend s3 (opendal) ───────────────────

/// Backend `s3` : magasin d'artefacts sur stockage compatible S3 (AWS S3,
/// RustFS, Scaleway…) — tier industriel (D5 v2).
pub struct S3Store {
    operator: Operator,
}

/// Réglages de connexion S3 — remplis par [`super::firmware::FirmwareSettings`]
/// depuis `settings.firmware.storage` (env `PNEX_S3_*`).
#[derive(Clone, Debug, Default)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Adressage path-style (`http://host/bucket/key`) — RustFS et la
    /// plupart des S3 auto-hébergés ; AWS utilise le host virtuel par défaut.
    pub path_style: bool,
}

impl S3Config {
    /// Depuis l'env `PNEX_S3_*` (mêmes vars que la config yaml) — pour les
    /// tests e2e contre un vrai service. `None` si l'endpoint n'est pas
    /// défini (service absent → test saute).
    pub fn from_env_if_set() -> Option<Self> {
        let endpoint = std::env::var("PNEX_S3_ENDPOINT").ok()?;
        let flag = |v: String| v.eq_ignore_ascii_case("true") || v == "1";
        Some(Self {
            endpoint,
            bucket: std::env::var("PNEX_S3_BUCKET").unwrap_or_default(),
            region: std::env::var("PNEX_S3_REGION").unwrap_or_default(),
            access_key: std::env::var("PNEX_S3_ACCESS_KEY").unwrap_or_default(),
            secret_key: std::env::var("PNEX_S3_SECRET_KEY").unwrap_or_default(),
            path_style: std::env::var("PNEX_S3_PATH_STYLE")
                .map(flag)
                .unwrap_or(true),
        })
    }
}

impl S3Store {
    /// Construit l'operator. Aucun I/O réseau ici — juste l'assemblage du
    /// client : une config incomplète échoue maintenant (message explicite),
    /// pas au premier build.
    pub fn connect(config: &S3Config) -> Result<Self, String> {
        if config.bucket.is_empty() {
            return Err("S3 : bucket requis (PNEX_S3_BUCKET)".into());
        }
        if config.endpoint.is_empty() {
            return Err("S3 : endpoint requis (PNEX_S3_ENDPOINT)".into());
        }
        if config.access_key.is_empty() || config.secret_key.is_empty() {
            return Err("S3 : credentials requis (PNEX_S3_ACCESS_KEY / PNEX_S3_SECRET_KEY)".into());
        }
        let mut builder = S3::default()
            .bucket(&config.bucket)
            .endpoint(&config.endpoint)
            .access_key_id(&config.access_key)
            .secret_access_key(&config.secret_key);
        // opendal 0.57 : le path-style (`http://host/bucket/key`) est le DÉFAUT
        // — `enable_path_style` a disparu. PNEX_S3_PATH_STYLE=false = host
        // virtuel AWS (`http://bucket.host/key`) via l'opt-in inverse.
        if !config.path_style {
            builder = builder.enable_virtual_host_style();
        }
        // Région requise par le signer — défaut us-east-1 : valeur ignorée
        // par RustFS & la plupart des S3 auto-hébergés, valide pour AWS.
        let region = if config.region.is_empty() {
            "us-east-1"
        } else {
            config.region.as_str()
        };
        builder = builder.region(region);
        // Retry des erreurs transitives (5xx, timeout) — uploads d'artefacts
        // idempotents (même clé, même contenu), sans risque de doublon.
        let operator = Operator::new(builder)
            .map_err(|e| format!("S3 : initialisation impossible : {e}"))?
            .layer(RetryLayer::default())
            .finish();
        Ok(Self { operator })
    }
}

#[async_trait]
impl ArtifactStore for S3Store {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError> {
        if key.is_empty() {
            return Err(BuildError::Store("clé vide".into()));
        }
        if bytes.len() > MAX_BYTES {
            return Err(BuildError::Store(format!(
                "artefact {key} trop volumineux : {} o (plafond {MAX_BYTES})",
                bytes.len()
            )));
        }
        self.operator
            .write(key, bytes.to_vec())
            .await
            .map_err(|e| BuildError::Store(format!("S3 put {key} : {e}")))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError> {
        let buf = self.operator.read(key).await.map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                BuildError::NotFound(key.to_string())
            } else {
                BuildError::Store(format!("S3 get {key} : {e}"))
            }
        })?;
        Ok(buf.to_vec())
    }

    async fn delete(&self, key: &str) -> Result<(), BuildError> {
        // Delete S3 idempotent nativement (204 même si absent) — parité
        // DbStore::delete.
        self.operator
            .delete(key)
            .await
            .map_err(|e| BuildError::Store(format!("S3 delete {key} : {e}")))
    }

    async fn exists(&self, key: &str) -> Result<bool, BuildError> {
        match self.operator.stat(key).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(BuildError::Store(format!("S3 stat {key} : {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{Database, PaginatorTrait};

    /// Base sqlite mémoire + migrations complètes — double emploi : premier
    /// test des migrations sur sqlite (tiers hobbyist).
    async fn migrated_sqlite() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        use pnex_migration::MigratorTrait;
        pnex_migration::Migrator::up(&db, None)
            .await
            .expect("migrations sqlite");
        db
    }

    fn oversized() -> Vec<u8> {
        vec![0u8; MAX_BYTES + 1]
    }

    #[tokio::test]
    async fn cycle_complet_et_upsert() {
        let db = migrated_sqlite().await;
        let store = DbStore::new(db);
        let key = "org_7/firmware/dev-1-firmware.bin";

        assert!(!store.exists(key).await.expect("exists"));
        store.put(key, b"octets-v1").await.expect("put v1");
        assert!(store.exists(key).await.expect("exists"));
        assert_eq!(store.get(key).await.expect("get"), b"octets-v1");

        // Rebuild = même clé → upsert, pas de doublon.
        store.put(key, b"octets-v2").await.expect("put v2");
        assert_eq!(store.get(key).await.expect("get"), b"octets-v2");
        let count = firmware_artifacts::Entity::find()
            .filter(firmware_artifacts::Column::Key.eq(key))
            .count(&store.db)
            .await
            .expect("count");
        assert_eq!(count, 1, "l'upsert ne doit pas créer de doublon");

        store.delete(key).await.expect("delete");
        assert!(!store.exists(key).await.expect("exists"));
        store.delete(key).await.expect("delete idempotent");
        assert!(matches!(store.get(key).await, Err(BuildError::NotFound(_))));
    }

    #[tokio::test]
    async fn garde_fous_taille_et_cle() {
        let db = migrated_sqlite().await;
        let store = DbStore::new(db);

        assert!(matches!(
            store
                .put("org_1/firmware/x-firmware.bin", &oversized())
                .await,
            Err(BuildError::Store(_))
        ));
        assert!(matches!(
            store.put("", b"x").await,
            Err(BuildError::Store(_))
        ));
    }

    fn s3_config() -> S3Config {
        S3Config {
            endpoint: "http://localhost:9000".into(),
            bucket: "pnex-test".into(),
            region: String::new(),
            access_key: "rustfsadmin".into(),
            secret_key: "rustfsadmin".into(),
            path_style: true,
        }
    }

    /// Validation de la config S3 sans réseau : chaque champ requis manquant
    /// → erreur explicite ; config complète → operator constructible.
    #[test]
    fn s3_validation_configuration() {
        for (mutant, attendu) in [
            (
                {
                    let mut c = s3_config();
                    c.bucket.clear();
                    c
                },
                "bucket",
            ),
            (
                {
                    let mut c = s3_config();
                    c.endpoint.clear();
                    c
                },
                "endpoint",
            ),
            (
                {
                    let mut c = s3_config();
                    c.access_key.clear();
                    c
                },
                "credentials",
            ),
            (
                {
                    let mut c = s3_config();
                    c.secret_key.clear();
                    c
                },
                "credentials",
            ),
        ] {
            // S3Store n'implémente pas Debug — pas de expect_err, un let-else.
            let Err(err) = S3Store::connect(&mutant) else {
                panic!("config incomplète ({attendu} vide) doit être rejetée");
            };
            assert!(err.contains(attendu), "{attendu} attendu dans : {err}");
        }
        assert!(S3Store::connect(&s3_config()).is_ok());
    }

    /// Cycle complet contre un vrai S3-compatible. Ignored par défaut
    /// (nécessite un service externe + bucket) — le plus simple : la stack
    /// compose locale (service `rustfs`, buckets auto-créés) :
    /// ```sh
    /// docker compose up -d rustfs rustfs-init
    /// PNEX_S3_ENDPOINT=http://localhost:9000 PNEX_S3_BUCKET=pnex-test \
    /// PNEX_S3_ACCESS_KEY=rustfsadmin PNEX_S3_SECRET_KEY=rustfsadmin \
    /// PNEX_S3_PATH_STYLE=true \
    ///   cargo test -p pnex-backend --lib s3_cycle_reel -- --ignored
    /// ```
    /// (tout S3-compatible avec les mêmes vars d'env fait l'affaire).
    #[tokio::test]
    #[ignore = "nécessite un S3-compatible (rustfs via compose) + bucket pnex-test"]
    async fn s3_cycle_reel() {
        let Some(config) = S3Config::from_env_if_set() else {
            eprintln!("PNEX_S3_ENDPOINT non défini — test ignoré");
            return;
        };
        let store = S3Store::connect(&config).expect("connect");
        let key = "org_7/firmware/s3-test-firmware.bin";

        store.delete(key).await.expect("delete initial");
        assert!(!store.exists(key).await.expect("exists"));
        store.put(key, b"octets-v1").await.expect("put v1");
        assert!(store.exists(key).await.expect("exists"));
        assert_eq!(store.get(key).await.expect("get"), b"octets-v1");

        // Même clé → écrasement (sémantique par device).
        store.put(key, b"octets-v2").await.expect("put v2");
        assert_eq!(store.get(key).await.expect("get"), b"octets-v2");

        store.delete(key).await.expect("delete");
        assert!(!store.exists(key).await.expect("exists"));
        // Delete idempotent + absent → NotFound (pas Store).
        store.delete(key).await.expect("delete idempotent");
        assert!(matches!(store.get(key).await, Err(BuildError::NotFound(_))));
    }
}
