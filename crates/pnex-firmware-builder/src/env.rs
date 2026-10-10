//! Assemblage des variables d'environnement du sous-process `pio run`.
//!
//! Contrat du dépôt firmware (vérifié, firmware-build.md §2.1) : la config
//! device se lit en **variables d'environnement** (platformio.ini
//! `-D WIFI_SSID="${sysenv.WIFI_SSID}"`…) — WIFI_SSID, WIFI_PASSWORD,
//! HOST, TOKEN et DEVICE_ID **en base64** (le firmware les décode ; le
//! base64 ne contient ni espace ni quote, un SSID littéral comme
//! « Chez Shan » casserait le flag `-D`). Always wss (D154): no scheme
//! variable. Jamais en argv : `ps` expose les arguments.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// Secrets d'un build. `token` et `encryption_key` viennent de la base au
/// moment du `perform` (ils ne transitent jamais par la queue) ; le WiFi et
/// l'hôte viennent de la requête utilisateur.
#[derive(Debug, Clone)]
pub struct BuildSecrets {
    pub wifi_ssid: String,
    pub wifi_password: String,
    /// Hôte du serveur PNEX (tel que saisi, ex. `dev1.pnex.io`).
    pub host: String,
    /// Token du device (`device_tokens.token`).
    pub token: String,
    pub device_id: String,
    /// Noise pre-shared key, base64 (`device_tokens.encryption_key`), passed
    /// as is — the firmware decodes it.
    pub encryption_key: String,
    /// Root CA (PEM) the firmware pins for wss + OTA https (`pnex_tls`) —
    /// the TLS edge's device root (D70). `None` = a firmware that never
    /// connects (no setInsecure): refused by the server outside tests.
    pub ca_cert_pem: Option<String>,
    /// Ed25519 public key (hex) of the instance OTA signer (SEC-18).
    pub ota_pubkey: String,
    /// Device client certificate + private key (PEM), issued by the org CA
    /// for this build (D153).
    pub client_cert: (String, String),
}

fn b64(v: &str) -> String {
    STANDARD.encode(v)
}

/// Variables injectées au sous-process `pio run` (par-dessus l'env réduite).
pub fn child_env(secrets: &BuildSecrets) -> Vec<(String, String)> {
    let mut vars = vec![
        // Everything is base64 on the server side and decoded by the
        // firmware — SSID spaces/quotes cannot go through a -D flag.
        ("WIFI_SSID".into(), b64(&secrets.wifi_ssid)),
        ("WIFI_PASSWORD".into(), b64(&secrets.wifi_password)),
        ("HOST".into(), b64(&secrets.host)),
        ("TOKEN".into(), b64(&secrets.token)),
        ("DEVICE_ID".into(), b64(&secrets.device_id)),
        ("ENCRYPTION_KEY".into(), secrets.encryption_key.clone()),
    ];
    // Always set (empty = no pin) so `${sysenv.PNEX_CA_CERT}` never leaks
    // an unrelated value.
    let ca = secrets
        .ca_cert_pem
        .as_deref()
        .filter(|pem| !pem.trim().is_empty())
        .map(b64)
        .unwrap_or_default();
    vars.push(("PNEX_CA_CERT".into(), ca));
    // Always set too (PIO fails on a missing `${sysenv.*}`); a public key,
    // not a secret. Hex only, so it can never break the `-D` flag.
    let ota_pubkey = Some(secrets.ota_pubkey.as_str())
        .filter(|k| k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_default()
        .to_string();
    vars.push(("PNEX_OTA_PUBKEY".into(), ota_pubkey));
    // Client certificate + key in base64 (no quote or newline can break the
    // `-D` flag).
    let (cert, key) = &secrets.client_cert;
    vars.push(("PNEX_CLIENT_CERT".into(), b64(cert)));
    vars.push(("PNEX_CLIENT_KEY".into(), b64(key)));
    vars
}

/// Masks the credentials of `secrets` in a tool output shown to users
/// (O4: failure reason of a build). The WiFi password, the device token
/// and its encryption key are replaced, raw and in the base64 form the
/// build passes them in. Values shorter than 4 characters are left alone
/// (they would mask ordinary text).
pub fn scrub_secrets(text: &str, secrets: &BuildSecrets) -> String {
    let mut out = text.to_string();
    let values = [
        secrets.wifi_password.as_str(),
        secrets.token.as_str(),
        secrets.encryption_key.as_str(),
        secrets.client_cert.1.as_str(),
    ];
    for v in values.into_iter().filter(|v| v.len() >= 4) {
        out = out.replace(&b64(v), "***").replace(v, "***");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_masks_raw_and_base64_credentials() {
        let s = secrets();
        let text = format!(
            "flags -DWIFI_PASSWORD={} -DTOKEN={} key {} raw p@ss w0rd and tok-secret",
            b64("p@ss w0rd"),
            b64("tok-secret"),
            "Y2xlLWI2NC1zMk8="
        );
        let clean = scrub_secrets(&text, &s);
        for leak in [
            "p@ss w0rd",
            "tok-secret",
            "Y2xlLWI2NC1zMk8=",
            &b64("tok-secret"),
        ] {
            assert!(!clean.contains(leak), "{leak} leaked in {clean}");
        }
        assert!(clean.contains("flags -DWIFI_PASSWORD=***"));
    }

    fn secrets() -> BuildSecrets {
        BuildSecrets {
            wifi_ssid: "coloc".into(),
            wifi_password: "p@ss w0rd".into(),
            host: "dev1.pnex.io".into(),
            token: "tok-secret".into(),
            device_id: "capteur-jardin".into(),
            encryption_key: "Y2xlLWI2NC1zMk8=".into(),
            ca_cert_pem: None,
            ota_pubkey: String::new(),
            client_cert: ("CERT PEM".into(), "KEY PEM".into()),
        }
    }

    /// The 5 device vars in base64 (round trip), the key as is.
    #[test]
    fn env_conforme_au_contrat_firmware() {
        let vars = child_env(&secrets());
        let get = |k: &str| {
            vars.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        for (name, expected) in [
            ("WIFI_SSID", "coloc"),
            ("WIFI_PASSWORD", "p@ss w0rd"),
            ("HOST", "dev1.pnex.io"),
            ("TOKEN", "tok-secret"),
            ("DEVICE_ID", "capteur-jardin"),
        ] {
            let decoded = STANDARD.decode(get(name)).expect("b64");
            assert_eq!(String::from_utf8(decoded).expect("utf8"), expected);
        }
        assert!(vars.iter().all(|(n, _)| n != "WS_SSL"));
        assert_eq!(get("ENCRYPTION_KEY"), "Y2xlLWI2NC1zMk8=");
    }

    /// SSID avec espaces : l'env transmise ne contient ni espace ni quote
    /// (le flag `-D` de platformio.ini ne peut pas se casser).
    #[test]
    fn ssid_avec_espaces_devient_base64() {
        let mut s = secrets();
        s.wifi_ssid = "Chez Shan".into();
        let vars = child_env(&s);
        let value = vars
            .iter()
            .find(|(n, _)| n == "WIFI_SSID")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        assert!(!value.contains(' ') && !value.contains('"'));
        let decoded = STANDARD.decode(value).expect("b64");
        assert_eq!(String::from_utf8(decoded).expect("utf8"), "Chez Shan");
    }

    fn get_var(vars: &[(String, String)], name: &str) -> String {
        vars.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// The CA travels base64-encoded, and is always present (empty without
    /// a CA).
    #[test]
    fn ca_cert_is_base64_and_always_set() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
        let mut s = secrets();
        s.ca_cert_pem = Some(pem.into());
        let decoded = STANDARD
            .decode(get_var(&child_env(&s), "PNEX_CA_CERT"))
            .expect("b64");
        assert_eq!(String::from_utf8(decoded).expect("utf8"), pem);

        s.ca_cert_pem = None;
        let vars = child_env(&s);
        assert!(vars
            .iter()
            .any(|(n, v)| n == "PNEX_CA_CERT" && v.is_empty()));
    }

    #[test]
    fn ota_pubkey_is_always_set_and_hex_only() {
        let get = |s: &BuildSecrets| {
            child_env(s)
                .into_iter()
                .find(|(n, _)| n == "PNEX_OTA_PUBKEY")
                .map(|(_, v)| v)
        };
        let mut s = secrets();
        assert_eq!(get(&s).as_deref(), Some(""));
        s.ota_pubkey = "ab".repeat(32);
        assert_eq!(get(&s), Some("ab".repeat(32)));
        // Anything else never reaches the `-D` flag.
        s.ota_pubkey = "\"; rm -rf /".into();
        assert_eq!(get(&s).as_deref(), Some(""));
    }
}
