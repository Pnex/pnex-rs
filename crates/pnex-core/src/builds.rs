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

/// Body of `POST /api/v1/build-firmware`. Unknown fields are refused.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBuild {
    /// Registered device of the org; its model and frozen board drive the
    /// build.
    pub device_id: String,
    /// WiFi entry of the org's referential (secrets.md S6): the build
    /// references its vault secret, the password never travels.
    pub wifi_credential_id: i64,
    /// PneX server host (e.g. `dev1.pnex.io`), passed to the firmware in
    /// base64. An imposed production host (`PNEX_PROD_HOST`) wins.
    #[serde(default)]
    pub pnex_host: String,
}

/// 201 answer of `POST /api/v1/build-firmware`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBuildResponse {
    /// Id of the new `build_records` row (also the firmware version).
    pub build_id: i64,
    /// Phase at submission — `"queued"`.
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
    /// `queued` | `running` | `succeeded` | `failed` (cf. doc module).
    pub build_phase: String,
    /// Clé de l'artefact dans l'`ArtifactStore` (absente si échec/en cours).
    #[serde(default)]
    pub firmware_bin_s3_key: Option<String>,
    /// Version du firmware compilé (id du record, stampée dans le binaire
    /// `PNEX_FW_VERSION` et annoncée par le device) — absente des records
    /// antérieurs à l'OTA.
    #[serde(default)]
    pub fw_version: Option<String>,
    /// Why a failed build failed (O4): one of [`BUILD_FAILURE_CODES`],
    /// shown translated; absent while the build runs or succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    /// Last lines of the failing tool output (compiler errors), the
    /// device credentials masked by the worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339 — dernier changement de phase.
    pub updated_at: String,
}

impl BuildRecord {
    /// The binary is ready (`build_phase == "succeeded"`).
    pub fn succeeded(&self) -> bool {
        self.build_phase == "succeeded"
    }
}

/// Failure reasons of a firmware build (O4), stored on the record by the
/// worker; the UI shows the fluent key `build-fail-<code>` (code with `-`).
pub const BUILD_FAILURE_CODES: &[&str] = &[
    // The compiler rejected the sources (the detail holds its errors).
    "build_compile",
    // Merging the flash image (esptool) failed.
    "build_merge",
    // Another external tool could not run.
    "build_tool",
    // No device CA to pin: a wss firmware would not verify the server
    // (SEC-W6).
    "build_no_ca",
    // The device CA is larger than the firmware of this chip can pin
    // (SEC-16, see `device_ca_max_pem_bytes`).
    "build_ca_too_large",
    // The OTA signing key of the instance could not be read or created
    // (SEC-18): a firmware without it would refuse every update.
    "build_ota_key",
    // The device certificate could not be issued by the org CA (D153).
    "build_device_cert",
    // The build exceeded its time budget.
    "build_timeout",
    // The firmware sources could not be staged on the worker.
    "build_source",
    // The compiler produced no firmware file.
    "build_artifact",
    // The firmware could not be stored.
    "build_store",
    // The WiFi password could not be read from the vault.
    "build_wifi",
    // The device or its token was deleted meanwhile.
    "build_device",
    // Custom firmware builds are disabled on the worker.
    "build_custom_disabled",
    // The custom firmware project or its revision is missing or invalid.
    "build_custom_project",
    // Unexpected server error (database…).
    "build_internal",
];

/// Longest failure detail kept on a record (characters, tail kept).
pub const BUILD_FAILURE_DETAIL_MAX: usize = 4000;

/// Largest device CA PEM (bytes, without the terminating NUL) the firmware
/// of a SoC can pin. Mirrors `PNEX_CA_PEM_MAX - 1` in
/// `firmware/lib/pnex/src/pnex_tls.h`: 4 KB buffer on the ESP32 family
/// (two roots fit), 2 KB on the ESP8266 (heap budget, one root).
pub fn device_ca_max_pem_bytes(soc: &str) -> usize {
    if soc.eq_ignore_ascii_case("esp8266") {
        2047
    } else {
        4095
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact output shape of a record.
    #[test]
    fn build_record_shape_roundtrip() {
        let json = r#"{
            "id": 12,
            "org_id": 4,
            "device_id": "capteur-jardin",
            "build_phase": "succeeded",
            "firmware_bin_s3_key": "org_4/firmware/capteur-jardin-firmware.bin",
            "fw_version": "12",
            "created_at": "2026-08-16T12:00:00+00:00",
            "updated_at": "2026-08-16T12:03:00+00:00"
        }"#;
        let record: BuildRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.org_id, 4);
        assert!(record.succeeded());
        assert_eq!(record.fw_version.as_deref(), Some("12"));
        let back = serde_json::to_value(&record).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    /// Minimal POST payload; any other field is refused.
    #[test]
    fn create_build_refuses_unknown_fields() {
        let payload: CreateBuild = serde_json::from_str(
            r#"{"device_id": "dev-11", "wifi_credential_id": 3, "pnex_host": "dev1.pnex.io"}"#,
        )
        .unwrap();
        assert_eq!(payload.wifi_credential_id, 3);
        assert_eq!(payload.pnex_host, "dev1.pnex.io");
        for legacy in [
            r#""ws_ssl": true"#,
            r#""wifi_password": "x""#,
            r#""insecure": 1"#,
        ] {
            let body = format!(r#"{{"device_id": "d", "wifi_credential_id": 3, {legacy}}}"#);
            assert!(
                serde_json::from_str::<CreateBuild>(&body).is_err(),
                "{legacy}"
            );
        }
    }

    /// Réponse de création : sans backend/job_name (adaptation Rust).
    #[test]
    fn create_response_sans_champs_k8s() {
        let res = CreateBuildResponse {
            build_id: 5,
            status: "queued".into(),
            message: "Build firmware job created".into(),
        };
        let json = serde_json::to_value(&res).unwrap();
        assert!(json.get("backend").is_none());
        assert!(json.get("job_name").is_none());
        assert_eq!(json["build_id"], 5);
    }

    /// The server limit stays the firmware buffer minus its NUL (SEC-16):
    /// parsed from pnex_tls.h so a buffer change cannot drift silently.
    #[test]
    fn device_ca_limit_matches_the_firmware_buffer() {
        let header = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../firmware/lib/pnex/src/pnex_tls.h"
        ))
        .expect("pnex_tls.h");
        let sizes: Vec<usize> = header
            .lines()
            .filter_map(|l| l.trim().strip_prefix("#define PNEX_CA_PEM_MAX "))
            .map(|v| v.trim().parse().expect("numeric PNEX_CA_PEM_MAX"))
            .collect();
        // ESP32 branch first, ESP8266 second (header order).
        assert_eq!(sizes, vec![4096, 2048]);
        assert_eq!(device_ca_max_pem_bytes("esp32-c6"), sizes[0] - 1);
        assert_eq!(device_ca_max_pem_bytes("esp32"), sizes[0] - 1);
        assert_eq!(device_ca_max_pem_bytes("esp8266"), sizes[1] - 1);
    }
}
