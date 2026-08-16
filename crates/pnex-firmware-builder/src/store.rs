//! Magasin d'artefacts (D5) : abstraction à deux backends — `local`
//! (système de fichiers, déploiement edge) et `s3` (cloud, **différé** :
//! plomberie de configuration seulement, méthodes `NotImplemented`).
//!
//! Clés logiques D6 : `org_{id}/firmware/{device_id}-firmware.bin`. Les
//! octets transitent en RAM (binaires fusionnés de 1–4 Mo ; hypothèse
//! documentée < ~50 Mo) — mappe 1:1 sur Put/GetObject S3 plus tard.

use std::path::PathBuf;

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

/// Backend FS : `root` + segments sanitizés de la clé. Sélectionné par
/// `STORAGE_BACKEND=local` (défaut).
pub struct LocalStore {
    root: PathBuf,
}

impl LocalStore {
    /// Crée le répertoire racine s'il manque (`create_dir_all`).
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, BuildError> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|e| BuildError::Store(format!("création de {} : {e}", root.display())))?;
        Ok(Self { root })
    }

    fn resolve(&self, key: &str) -> Result<PathBuf, BuildError> {
        if key.is_empty() {
            return Err(BuildError::Store("clé vide".into()));
        }
        let mut path = self.root.clone();
        for seg in key.split('/') {
            let seg = sanitize_segment(seg);
            if seg.is_empty() {
                return Err(BuildError::Store(format!("clé invalide : {key}")));
            }
            path.push(seg);
        }
        Ok(path)
    }
}

#[async_trait]
impl ArtifactStore for LocalStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError> {
        let path = self.resolve(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| BuildError::Store(format!("mkdir {} : {e}", parent.display())))?;
        }
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| BuildError::Store(format!("écriture {} : {e}", path.display())))
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError> {
        let path = self.resolve(key)?;
        tokio::fs::read(&path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    BuildError::NotFound(format!("{key} ({})", path.display()))
                }
                _ => BuildError::Store(format!("lecture {} : {e}", path.display())),
            })
    }

    async fn delete(&self, key: &str) -> Result<(), BuildError> {
        let path = self.resolve(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            // Idempotent, parité sémantique S3.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(BuildError::Store(format!("suppression {} : {e}", path.display()))),
        }
    }

    async fn exists(&self, key: &str) -> Result<bool, BuildError> {
        Ok(self.resolve(key)?.is_file())
    }
}

/// Backend S3 (ou MinIO compatible) — **différé** (D5, tranche ultérieure) :
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
            "S3 put non implémenté (D5 — tranche ultérieure)".into(),
        ))
    }

    async fn get(&self, _key: &str) -> Result<Vec<u8>, BuildError> {
        Err(BuildError::NotImplemented(
            "S3 get non implémenté (D5 — tranche ultérieure)".into(),
        ))
    }

    async fn delete(&self, _key: &str) -> Result<(), BuildError> {
        Err(BuildError::NotImplemented(
            "S3 delete non implémenté (D5 — tranche ultérieure)".into(),
        ))
    }

    async fn exists(&self, _key: &str) -> Result<bool, BuildError> {
        Err(BuildError::NotImplemented(
            "S3 exists non implémenté (D5 — tranche ultérieure)".into(),
        ))
    }
}

/// Sanitise un segment de clé : tout caractère hors `[A-Za-z0-9._-]` → `_`,
/// `.` et `..` → `_` (anti-traversal ; un `device_id` contenant `/` est
/// réduit à un segment sûr).
pub fn sanitize_segment(seg: &str) -> String {
    if seg == "." || seg == ".." {
        return "_".into();
    }
    seg.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect()
}

/// Clé logique D6 de l'artefact : `org_{org_id}/firmware/{device_id}-firmware.bin`.
pub fn artifact_key(org_id: i64, device_id: &str) -> String {
    format!("org_{org_id}/firmware/{}-firmware.bin", sanitize_segment(device_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Put/get/exists/delete sur le backend FS, sous-répertoires créés.
    #[tokio::test]
    async fn local_store_cycle_complet() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = LocalStore::new(dir.path()).expect("store");
        let key = "org_7/firmware/dev-1-firmware.bin";

        assert!(!store.exists(key).await.expect("exists"));
        store.put(key, b"octets").await.expect("put");
        assert!(store.exists(key).await.expect("exists"));
        assert_eq!(store.get(key).await.expect("get"), b"octets");

        store.delete(key).await.expect("delete");
        assert!(!store.exists(key).await.expect("exists"));
        // Idempotent.
        store.delete(key).await.expect("delete absent");
        // Clé absente → NotFound (pas Store).
        assert!(matches!(store.get(key).await, Err(BuildError::NotFound(_))));
    }

    /// Un device_id hostile ne s'échappe pas de la racine.
    #[tokio::test]
    async fn local_store_anti_traversal() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = LocalStore::new(dir.path()).expect("store");
        // Les `..` deviennent `_`, les `/` sont des séparateurs sanitizés.
        store.put("../etc/passwd", b"x").await.expect("put");
        assert!(dir.path().join("_/etc/passwd").is_file());
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
