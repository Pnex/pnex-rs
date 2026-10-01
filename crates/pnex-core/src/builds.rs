//! Firmware builds domain DTOs — parity with the legacy `firmware_builder`
//! contracts (Phase 6), org scoping (D2) instead of the `user`.
//!
//! Deliberate adaptations vs the legacy stack (recorded in
//! `docs/contracts/build.http`):
//! - plus de champs `backend`/`job_name`/`argo_wf_job_name` (pas de k8s/Argo
//!   en Rust : queue PostgreSQL + worker in-process) — la réponse de
//!   création expose `build_id` + `status` ;
//! - `build_phase` in canonical lowercase: `queued` (new — the legacy
//!   stack had no queued state, submit was synchronous) | `running`
//!   (legacy `Running`) | `succeeded` (`Succeeded`) | `failed`
//!   (`Failed`); `Deleted` dropped (no job left to claim);
//! - paginated list (D14) — the legacy stack returned a bare list.
//!
//! Champs dates en chaînes RFC 3339 (sérialisation SeaORM), pas de chrono
//! dans le core (wasm32).

use serde::{Deserialize, Serialize};

/// Corps du `POST /api/v1/build-firmware`.
///
/// `insecure`, `server_port`, `force_rebuild` and `metadata` from the
/// legacy contract are accepted and ignored (serde tolerates unknown
/// fields): the current firmware only reads WiFi, host and WS scheme.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBuild {
    /// WiFi entry of the org's referential (secrets.md S6): the build
    /// references its vault secret, the password never travels. When set,
    /// `wifi_ssid` / `wifi_password` are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi_credential_id: Option<i64>,
    /// Legacy: typed SSID + password, saved into the referential (and the
    /// vault) before the build.
    #[serde(default)]
    pub wifi_ssid: String,
    #[serde(default)]
    pub wifi_password: String,
    /// Doit correspondre au modèle d'un device enregistré dans l'org (le
    /// contrôleur vérifie la cohérence avec le device).
    pub predefined_device_name: String,
    /// Hôte du serveur PNEX (ex. `dev1.pnex.io`) — passé au firmware en
    /// base64. Deviation from the legacy stack: passed as-is, no `_extract_hostname`.
    pub pnex_host: String,
    pub device_id: String,
    /// Ignored by the server since D70: every firmware is built for
    /// `wss://` through the TLS edge. Kept in the contract for compatibility.
    #[serde(default = "default_ws_ssl")]
    pub ws_ssl: bool,
}

fn default_ws_ssl() -> bool {
    true
}

/// Réponse 201 du `POST /api/v1/build-firmware`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBuildResponse {
    /// Vrai si un nouveau record a été inséré (faux = rebuild du même
    /// device → record reused, legacy `update_or_create` parity).
    pub build_record_created: bool,
    /// Id du `build_records`.
    pub build_id: i64,
    /// Phase au moment de la soumission — `"queued"`.
    pub status: String,
    pub message: String,
}

/// Record de build — `GET /api/v1/build-records` (paginé D14).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRecord {
    pub id: i64,
    /// D2: owning org instead of the legacy `user`.
    pub org_id: i64,
    pub device_id: Option<String>,
    /// Vrai ssi `build_phase == "succeeded"` (le binaire est prêt).
    pub success: bool,
    /// `queued` | `running` | `succeeded` | `failed` (cf. doc module).
    pub build_phase: Option<String>,
    /// Clé de l'artefact dans l'`ArtifactStore` (absente si échec/en cours).
    #[serde(default)]
    pub firmware_bin_s3_key: Option<String>,
    /// Version du firmware compilé (id du record, stampée dans le binaire
    /// `PNEX_FW_VERSION` et annoncée par le device) — absente des records
    /// antérieurs à l'OTA.
    #[serde(default)]
    pub fw_version: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339 — dernier changement de phase.
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forme de sortie exacte d'un record (parité BuildRecordSerializer,
    /// org_id à la place de user, plus d'argo_wf_job_name).
    #[test]
    fn build_record_shape_roundtrip() {
        let json = r#"{
            "id": 12,
            "org_id": 4,
            "device_id": "capteur-jardin",
            "success": true,
            "build_phase": "succeeded",
            "firmware_bin_s3_key": "org_4/firmware/capteur-jardin-firmware.bin",
            "fw_version": "12",
            "created_at": "2026-08-16T12:00:00+00:00",
            "updated_at": "2026-08-16T12:03:00+00:00"
        }"#;
        let record: BuildRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.org_id, 4);
        assert!(record.success);
        assert_eq!(record.build_phase.as_deref(), Some("succeeded"));
        assert_eq!(record.fw_version.as_deref(), Some("12"));
        // Legacy payload without fw_version still deserializes (serde default).
        let legacy: BuildRecord = serde_json::from_str(
            r#"{
            "id": 12,
            "org_id": 4,
            "device_id": "capteur-jardin",
            "success": true,
            "build_phase": "succeeded",
            "created_at": "2026-08-16T12:00:00+00:00",
            "updated_at": "2026-08-16T12:03:00+00:00"
        }"#,
        )
        .unwrap();
        assert_eq!(legacy.fw_version, None);
        let back = serde_json::to_value(&record).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    /// Minimal POST payload; inherited legacy fields are tolerated.
    #[test]
    fn create_build_minimal_et_champs_herites_ignores() {
        let payload: CreateBuild = serde_json::from_str(
            r#"{
                "wifi_ssid": "coloc",
                "wifi_password": "ZaFjX9",
                "device_id": "dev-11",
                "predefined_device_name": "soil_sensor",
                "pnex_host": "dev1.pnex.io",
                "ws_ssl": false,
                "insecure": 1,
                "server_port": 443,
                "force_rebuild": true,
                "metadata": ""
            }"#,
        )
        .unwrap();
        assert_eq!(payload.wifi_ssid, "coloc");
        assert_eq!(payload.pnex_host, "dev1.pnex.io");
        // ws_ssl explicite dans la charge.
        assert!(!payload.ws_ssl);
    }

    /// ws_ssl absent du corps → défaut true (parité firmware qui parlait
    /// toujours wss ; le front local envoie explicitement false).
    #[test]
    fn create_build_ws_ssl_defaut_true() {
        let payload: CreateBuild = serde_json::from_str(
            r#"{
                "wifi_ssid": "coloc",
                "wifi_password": "ZaFjX9",
                "device_id": "dev-11",
                "predefined_device_name": "soil_sensor",
                "pnex_host": "dev1.pnex.io"
            }"#,
        )
        .unwrap();
        assert!(payload.ws_ssl);
    }

    /// Réponse de création : sans backend/job_name (adaptation Rust).
    #[test]
    fn create_response_sans_champs_k8s() {
        let res = CreateBuildResponse {
            build_record_created: true,
            build_id: 5,
            status: "queued".into(),
            message: "Build firmware job created".into(),
        };
        let json = serde_json::to_value(&res).unwrap();
        assert!(json.get("backend").is_none());
        assert!(json.get("job_name").is_none());
        assert_eq!(json["build_id"], 5);
    }
}
