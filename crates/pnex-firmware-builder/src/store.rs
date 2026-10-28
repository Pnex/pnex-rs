//! Artifact store (D5 v2): the abstraction only — the real
//! implementations (`db` on postgres, `s3` via opendal) live in
//! pnex-backend; this crate deliberately stays without DB or
//! cloud dependency, [`InMemoryStore`] serves the pipeline tests.
//!
//! D6 logical keys: `org_{id}/firmware/{device_id}-firmware.bin`. The
//! bytes go through RAM (merged binaries of 1–4 MB; documented
//! assumption < ~50 MB) — maps 1:1 onto SQL BLOBs and S3 Put/GetObject.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::BuildError;

/// Firmware artifact store (final flashable binary).
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Stores the bytes under the logical key (overwrites if present).
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError>;
    /// Reads the bytes — [`BuildError::NotFound`] if the key does not exist.
    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError>;
    /// Deletes the key (idempotent: absent ≠ error, S3 parity).
    async fn delete(&self, key: &str) -> Result<(), BuildError>;
    async fn exists(&self, key: &str) -> Result<bool, BuildError>;
}

/// In-memory backend — pipeline tests (no FS I/O or DB) and test
/// doubles on the backend side. Same semantics as the real backends:
/// `put` overwrites, `get` absent → `NotFound`, `delete` idempotent.
#[derive(Default)]
pub struct InMemoryStore {
    map: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ArtifactStore for InMemoryStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BuildError> {
        if key.is_empty() {
            return Err(BuildError::Store("empty key".into()));
        }
        self.map
            .lock()
            .expect("InMemoryStore lock poisoned")
            .insert(key.to_string(), bytes.to_vec());
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BuildError> {
        self.map
            .lock()
            .expect("InMemoryStore lock poisoned")
            .get(key)
            .cloned()
            .ok_or_else(|| BuildError::NotFound(key.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), BuildError> {
        self.map
            .lock()
            .expect("InMemoryStore lock poisoned")
            .remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, BuildError> {
        Ok(self
            .map
            .lock()
            .expect("InMemoryStore lock poisoned")
            .contains_key(key))
    }
}

/// Sanitises a key segment: any character outside `[A-Za-z0-9._-]` → `_`,
/// `.` and `..` → `_` (the key stays a safe identifier whatever the
/// backend; a `device_id` containing `/` is reduced to a safe segment).
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

/// D6 logical key of the artifact: `org_{org_id}/firmware/{device_id}-firmware.bin`.
pub fn artifact_key(org_id: i64, device_id: &str) -> String {
    format!(
        "org_{org_id}/firmware/{}-firmware.bin",
        sanitize_segment(device_id)
    )
}

/// OTA logical key — **versioned, immutable** (the D6 key above stays a
/// per-device upsert for USB flashing; this one keeps the history of
/// deployable versions): `org_{org_id}/ota/{device_id}/{version}.bin`.
pub fn ota_artifact_key(org_id: i64, device_id: &str, version: &str) -> String {
    // Both segments sanitized (SEC-W4, R18): a version is a build id today,
    // but the key must never depend on that.
    format!(
        "org_{org_id}/ota/{}/{}.bin",
        sanitize_segment(device_id),
        sanitize_segment(version)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ota_key_sanitizes_every_segment() {
        assert_eq!(ota_artifact_key(3, "dev-1", "42"), "org_3/ota/dev-1/42.bin");
        assert_eq!(
            ota_artifact_key(3, "dev-1", "../../org_9/ota/x/1"),
            "org_3/ota/dev-1/.._.._org_9_ota_x_1.bin"
        );
        assert_eq!(ota_artifact_key(3, "..", ".."), "org_3/ota/_/_.bin");
    }

    /// Put/get/exists/delete on the memory backend, semantics of the real backends.
    #[tokio::test]
    async fn in_memory_store_cycle_complet() {
        let store = InMemoryStore::default();
        let key = "org_7/firmware/dev-1-firmware.bin";

        assert!(!store.exists(key).await.expect("exists"));
        store.put(key, b"octets").await.expect("put");
        assert!(store.exists(key).await.expect("exists"));
        assert_eq!(store.get(key).await.expect("get"), b"octets");

        // put overwrites (upsert per device).
        store.put(key, b"octets-v2").await.expect("put");
        assert_eq!(store.get(key).await.expect("get"), b"octets-v2");

        store.delete(key).await.expect("delete");
        assert!(!store.exists(key).await.expect("exists"));
        // Idempotent.
        store.delete(key).await.expect("delete absent");
        // Missing key → NotFound (not Store).
        assert!(matches!(store.get(key).await, Err(BuildError::NotFound(_))));
        // Empty key rejected.
        assert!(matches!(
            store.put("", b"x").await,
            Err(BuildError::Store(_))
        ));
    }

    /// D6 key: shape `org_{id}/firmware/{sanitized}-firmware.bin`.
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
        assert_eq!(
            ota_artifact_key(7, "capteur-jardin", "42"),
            "org_7/ota/capteur-jardin/42.bin"
        );
        assert_eq!(sanitize_segment(".."), "_");
        assert_eq!(sanitize_segment("."), "_");
    }
}
