//! PNEX interoperability contract — single source of truth app ↔ server.
//!
//! Embedded by the backend AND the front (wasm32 and native): the contract
//! number compiled into each binary is compared at runtime via
//! `GET {META_VERSION_PATH}` — an outdated app facing a newer server
//! (or the reverse) is rejected cleanly at boot instead of producing opaque
//! API errors. Bump rule: any API break increments
//! `CONTRACT` here, both sides switch together on the next build.
//!
//! wasm-safe (serde only) — same requirement as `pnex-core`.

use serde::{Deserialize, Serialize};

/// What the MCP exposes, and what the `pnex` CLI asks `--yes` for.
pub mod exposure;

/// Who may call each operation and assistant tool (iam.md D289).
pub mod iam;

/// Contract version expected by THIS binary. A bump = API break.
///
/// 2 (2026-09-12, D43): `Poi.device_id` (single column) replaced by
/// `Poi.devices: Vec<DevicePlacement>` — several devices per POI;
/// attach via `POST /pois/{id}/devices`, move via
/// `PATCH /pois/placements/{id}` (no more PATCH `device_id` on the POI).
///
/// 3 (2026-10-17, D238–D244): harmonized contract — one error shape
/// (`ErrorDetail`, `errors.fields` / `errors.violations`), every list
/// paginated, typed ids/dates/enums, GET for reads, English descriptions.
///
/// 4 (2026-10-24, web-presence.md D359): the flow node kind `ingest` and
/// `GET /flows/{id}/ingest-steps` are removed; an import carries its steps.
///
/// 5 (2026-10-28): the data classification enum is English on the wire
/// (`public`, `internal`, `personal`, `sensitive`).
///
/// 6 (2026-10-29, flow-engine.md D497): a function's interface is sent as
/// `inputs`/`outputs` (create, save, ad-hoc test), never read from `@input`/
/// `@output` comments in the code; `function-directive-invalid` is replaced
/// by field errors (`inputs.<i>.name`…).
pub const CONTRACT: u32 = 6;

/// Identity announced by the meta endpoint — aligned with
/// `pnex_core::SERVICE_NAME` (the service is `pnex-server`). The LAN scan compares this field to tell a PNEX
/// server apart from anything else answering on :5150.
pub const SERVICE: &str = "pnex-server";

/// Path of the meta endpoint (LAN scan + compatibility gate).
pub const META_VERSION_PATH: &str = "/api/v1/meta/version";

/// Public download of the root CA that signs the TLS edge certificate
/// (D70, local mode). Native apps pin it after a trust-on-first-use
/// confirmation; phones and browsers install it from there. 404 when the
/// server has no local CA to hand out (no edge, or public Let's Encrypt).
pub const META_CA_PATH: &str = "/api/v1/meta/ca";

/// Identity card of the server, served by `GET {META_VERSION_PATH}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ServerInfo {
    /// Always [`SERVICE`] for a PNEX server.
    pub service: String,
    /// Composite version (`"0.1.0 (dev)"`: semver + build sha).
    pub version: String,
    /// API contract version served by this binary.
    pub contract: u32,
    /// Canonical public origin of the deployment (`https://pnex.local`),
    /// when the server knows it. A native app that reached the server by IP
    /// (LAN scan, typed address) switches to it: OIDC redirect URIs and the
    /// certificate name are registered for this origin, not for the IP.
    /// Absent when the server does not know its public name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// App ↔ server compatibility: strict equality of contracts (no
/// tolerance window — a mismatch is a break, we name it).
#[must_use]
pub fn compatible(a: u32, b: u32) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_info_serde_roundtrip() {
        let info = ServerInfo {
            service: SERVICE.into(),
            version: "0.1.0 (dev)".into(),
            contract: CONTRACT,
            origin: Some("https://pnex.local".into()),
        };
        let back: ServerInfo =
            serde_json::from_str(&serde_json::to_string(&info).unwrap()).expect("roundtrip");
        assert_eq!(back, info);
    }

    #[test]
    fn server_info_without_origin_still_parses() {
        // Body of a server that predates `origin`: same contract, no field.
        let old = r#"{"service":"pnex-server","version":"0.1.0 (x)","contract":2}"#;
        let info: ServerInfo = serde_json::from_str(old).expect("old body");
        assert_eq!(info.origin, None);
        assert!(!serde_json::to_string(&info).unwrap().contains("origin"));
    }

    #[test]
    fn strict_compatibility() {
        assert!(compatible(CONTRACT, CONTRACT));
        assert!(!compatible(1, 2));
        assert!(!compatible(2, 1));
    }

    #[test]
    fn service_identity_is_pnex_server() {
        // The LAN scan rejects anything that does not announce itself exactly so.
        assert_eq!(SERVICE, "pnex-server");
    }
}
