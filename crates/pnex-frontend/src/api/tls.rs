//! TLS trust for native targets (D70: TLS everywhere through the nginx edge).
//!
//! A self-hosted server in `local` edge mode presents a certificate signed by
//! its own root CA, unknown to the bundled webpki roots. Native builds pin
//! that root after the user confirms its fingerprint (trust on first use,
//! `pages/trust_ca.rs`); every HTTP client then trusts webpki roots + the
//! pinned CA. The web build is same-origin: the browser owns trust, these
//! helpers degrade to a plain client.

use std::time::Duration;

use base64::Engine;
use sha2::{Digest, Sha256};

#[cfg(not(target_arch = "wasm32"))]
use crate::storage::{self, KeyValueStorage, KEY_SERVER_CA};

/// Pinned root CA (PEM), if any. Always `None` on the web.
#[must_use]
pub fn pinned_ca() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        storage::local()
            .get(KEY_SERVER_CA)
            .filter(|pem| !pem.is_empty())
    }
}

/// Pins `pem` as the server's root CA (replaces any previous one).
#[cfg(not(target_arch = "wasm32"))]
pub fn pin_ca(pem: &str) {
    storage::local().set(KEY_SERVER_CA, pem);
}

/// Client trusting webpki roots + the pinned CA. `timeout` is ignored on the
/// web (reqwest's wasm builder has no global timeout).
#[must_use]
pub fn client(timeout: Option<Duration>) -> reqwest::Client {
    client_with_ca(pinned_ca().as_deref(), timeout)
}

/// Client trusting webpki roots + `ca` (an unparsable PEM is skipped: the
/// handshake then fails on its own, which is the safe outcome).
#[must_use]
pub fn client_with_ca(ca: Option<&str>, timeout: Option<Duration>) -> reqwest::Client {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (ca, timeout);
        reqwest::Client::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut builder = reqwest::Client::builder();
        if let Some(cert) = ca.and_then(|pem| reqwest::Certificate::from_pem(pem.as_bytes()).ok()) {
            builder = builder.add_root_certificate(cert);
        }
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        builder.build().unwrap_or_default()
    }
}

/// Discovery client: accepts ANY certificate. Only for public, non-secret
/// reads that must happen before trust exists — the LAN scan's identity
/// probe and the CA download that the user then confirms by fingerprint.
/// Never carries tokens.
#[must_use]
pub fn discovery_client() -> reqwest::Client {
    #[cfg(target_arch = "wasm32")]
    {
        reqwest::Client::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_default()
    }
}

/// SHA-256 fingerprint of the first certificate of `pem`, as uppercase hex
/// pairs joined by `:` (the format `openssl x509 -fingerprint -sha256` and
/// the Android certificate screens display).
#[must_use]
pub fn fingerprint(pem: &str) -> Option<String> {
    let body: String = pem
        .lines()
        .skip_while(|line| !line.starts_with("-----BEGIN CERTIFICATE-----"))
        .skip(1)
        .take_while(|line| !line.starts_with("-----END CERTIFICATE-----"))
        .map(str::trim)
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    if der.is_empty() {
        return None;
    }
    let digest = Sha256::digest(&der);
    Some(
        digest
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_hashes_the_der_body() {
        // DER = b"hello" → SHA-256 2CF24DBA…
        let pem = "junk\n-----BEGIN CERTIFICATE-----\naGVs\nbG8=\n-----END CERTIFICATE-----\n";
        let fp = fingerprint(pem).expect("fingerprint");
        assert!(fp.starts_with("2C:F2:4D:BA:5F:B0:A3:0E"), "{fp}");
        assert_eq!(fp.split(':').count(), 32);
    }

    #[test]
    fn fingerprint_rejects_non_pem() {
        assert_eq!(fingerprint("not a certificate"), None);
        assert_eq!(
            fingerprint("-----BEGIN CERTIFICATE-----\n!!\n-----END CERTIFICATE-----"),
            None
        );
    }
}
