//! Backend `db` de l'`ArtifactStore` (D5 v2) : les binaires firmware vivent
//! en base (table `firmware_artifacts`) — tiers sqlite (tout-en-un) et
//! postgres (pods API stateless, n'importe quel réplica sert le download).
//!
//! La clé logique est stable par device (`org_{id}/firmware/{device}-firmware.bin`)
//! → `put()` est un upsert `ON CONFLICT (key) DO UPDATE` : un rebuild
//! écrase la ligne précédente, zéro artefact orphelin. Portable PG (BYTEA) /
//! sqlite (BLOB). L'implémentation vit ici — la crate builder reste sans DB.

use async_trait::async_trait;
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
}
