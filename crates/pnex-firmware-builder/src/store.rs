//! Magasin d'artefacts (D5 v2) : abstraction à backends — `db` (base de
//! données, implémenté côté pnex-backend, défaut des tiers sqlite/postgres)
//! et `s3` (cloud industriel, **différé** : plomberie de configuration
//! seulement, méthodes `NotImplemented`). Cette crate reste volontairement
//! sans dépendance DB : les implémentations réelles vivent dans le backend,
//! [`InMemoryStore`] sert aux tests du pipeline.
//!
//! Clés logiques D6 : `org_{id}/firmware/{device_id}-firmware.bin`. Les
//! octets transitent en RAM (binaires fusionnés de 1–4 Mo ; hypothèse
//! documentée < ~50 Mo) — mappe 1:1 sur Put/GetObject S3 plus tard.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::BuildError;

/// Magasin d'artefacts de firmware (binaire final flashable).
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Dépose les octets sous la clé logique (écrase si présent).
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError>;
    /// Lit les octets — [`BuildError::NotFound`] si la clé n'existe pas.
    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError>;
    /// Supprime la clé (idempotent : absent ≠ erreur, parité S3).
    async fn delete(&self, key: &str) -> Result<(), BuildError>;
    async fn exists(&self, key: &str) -> Result<bool, BuildError>;
}

/// Backend en mémoire — tests du pipeline (sans I/O FS ni DB) et doubles
/// de test côté backend. Sémantique identique aux backends réels :
/// `put` écrase, `get` absent → `NotFound`, `delete` idempotent.
#[derive(Default)]
pub struct InMemoryStore {
    map: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ArtifactStore for InMemoryStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError> {
        if key.is_empty() {
            return Err(BuildError::Store("clé vide".into()));
        }
        self.map
            .lock()
            .expect("InMemoryStore verrou empoisonné")
            .insert(key.to_string(), bytes.to_vec());
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError> {
        self.map
            .lock()
            .expect("InMemoryStore verrou empoisonné")
            .get(key)
            .cloned()
            .ok_or_else(|| BuildError::NotFound(key.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), BuildError> {
        self.map
            .lock()
            .expect("InMemoryStore verrou empoisonné")
            .remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, BuildError> {
        Ok(self
            .map
            .lock()
            .expect("InMemoryStore verrou empoisonné")
            .contains_key(key))
    }
}

/// Backend S3 (ou MinIO compatible) — **différé** (D5 v2, tier industriel) :
/// la plomberie de configuration existe pour valider les réglages maintenant,
/// les opérations renvoient [`BuildError::NotImplemented`].
pub struct S3Store {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub path_style: bool,
}

#[async_trait]
impl ArtifactStore for S3Store {
    async fn put(&self, _key: &str, _bytes: &[u8]) -> Result<(), BuildError> {
        Err(BuildError::NotImplemented(
            "S3 put non implémenté (D5 — tier industriel, tranche ultérieure)".into(),
        ))
    }

    async fn get(&self, _key: &str) -> Result<Vec<u8>, BuildError> {
        Err(BuildError::NotImplemented(
            "S3 get non implémenté (D5 — tier industriel, tranche ultérieure)".into(),
        ))
    }

    async fn delete(&self, _key: &str) -> Result<(), BuildError> {
        Err(BuildError::NotImplemented(
            "S3 delete non implémenté (D5 — tier industriel, tranche ultérieure)".into(),
        ))
    }

    async fn exists(&self, _key: &str) -> Result<bool, BuildError> {
        Err(BuildError::NotImplemented(
            "S3 exists non implémenté (D5 — tier industriel, tranche ultérieure)".into(),
        ))
    }
}

/// Sanitise un segment de clé : tout caractère hors `[A-Za-z0-9._-]` → `_`,
/// `.` et `..` → `_` (la clé reste un identifiant sûr quel que soit le
/// backend ; un `device_id` contenant `/` est réduit à un segment sûr).
pub fn sanitize_segment(seg: &str) -> String {
    if seg == "." || seg == ".." {
        return "_".into();
    }
    seg.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Clé logique D6 de l'artefact : `org_{org_id}/firmware/{device_id}-firmware.bin`.
pub fn artifact_key(org_id: i64, device_id: &str) -> String {
    format!(
        "org_{org_id}/firmware/{}-firmware.bin",
        sanitize_segment(device_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Put/get/exists/delete sur le backend mémoire, sémantique des backends réels.
    #[tokio::test]
    async fn in_memory_store_cycle_complet() {
        let store = InMemoryStore::default();
        let key = "org_7/firmware/dev-1-firmware.bin";

        assert!(!store.exists(key).await.expect("exists"));
        store.put(key, b"octets").await.expect("put");
        assert!(store.exists(key).await.expect("exists"));
        assert_eq!(store.get(key).await.expect("get"), b"octets");

        // put écrase (upsert par device).
        store.put(key, b"octets-v2").await.expect("put");
        assert_eq!(store.get(key).await.expect("get"), b"octets-v2");

        store.delete(key).await.expect("delete");
        assert!(!store.exists(key).await.expect("exists"));
        // Idempotent.
        store.delete(key).await.expect("delete absent");
        // Clé absente → NotFound (pas Store).
        assert!(matches!(store.get(key).await, Err(BuildError::NotFound(_))));
        // Clé vide rejetée.
        assert!(matches!(
            store.put("", b"x").await,
            Err(BuildError::Store(_))
        ));
    }

    /// Clé D6 : forme `org_{id}/firmware/{sanitized}-firmware.bin`.
    #[test]
    fn cles_artefact_sanitisees() {
        assert_eq!(
            artifact_key(7, "capteur-jardin"),
            "org_7/firmware/capteur-jardin-firmware.bin"
        );
        assert_eq!(
            artifact_key(1, "a/b c:é"),
            "org_1/firmware/a_b_c__-firmware.bin"
        );
        assert_eq!(sanitize_segment(".."), "_");
        assert_eq!(sanitize_segment("."), "_");
    }

    /// S3 : plomberie présente, opérations explicitement différées.
    #[tokio::test]
    async fn s3_differe() {
        let store = S3Store {
            endpoint: "http://minio:9000".into(),
            bucket: "pnex".into(),
            region: "fr-par".into(),
            path_style: true,
        };
        for res in [
            store.put("k", b"x").await.map(|_| ()),
            store.get("k").await.map(|_| ()),
            store.delete("k").await,
            store.exists("k").await.map(|_| ()),
        ] {
            assert!(
                matches!(res, Err(BuildError::NotImplemented(_))),
                "S3 doit être explicitement différé"
            );
        }
    }
}
