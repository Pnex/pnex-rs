//! Assemblage des variables d'environnement du sous-process `pio run`.
//!
//! Contrat du dépôt firmware (vérifié, firmware-build.md §2.1) : la config
//! device se lit en **variables d'environnement** (platformio.ini
//! `-D WIFI_SSID="${sysenv.WIFI_SSID}"`…) — WIFI_SSID, WIFI_PASSWORD,
//! HOST, TOKEN et DEVICE_ID **en base64** (le firmware les décode ; le
//! base64 ne contient ni espace ni quote, un SSID littéral comme
//! « Chez Shan » casserait le flag `-D`), WS_SSL en true/false (schéma
//! wss/ws). Jamais en argv : `ps` expose les arguments.

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
    /// WebSocket en `wss://` (TLS) ou `ws://` (local sans TLS) — pas un
    /// secret, injecté par le même canal que le WiFi.
    pub ws_ssl: bool,
    /// Token du device (`device_tokens.token`).
    pub token: String,
    pub device_id: String,
    /// Clé ChaCha20 b64 (`device_tokens.encryption_key`), passée telle
    /// quelle — le firmware décodera lui-même.
    pub encryption_key: Option<String>,
    /// Root CA (PEM) the firmware pins for wss + OTA https (`pnex_tls`) —
    /// the TLS edge's device root (D70). `None` = no pinning (setInsecure,
    /// on both cores).
    pub ca_cert_pem: Option<String>,
}

fn b64(v: &str) -> String {
    STANDARD.encode(v)
}

/// Variables injectées au sous-process `pio run` (par-dessus l'env réduite).
pub fn child_env(secrets: &BuildSecrets) -> Vec<(String, String)> {
    let mut vars = vec![
        // Tout en base64 côté serveur, décodé par le firmware (parité
        // build.sh) — espaces/quotes des SSID impossibles pour le flag -D.
        ("WIFI_SSID".into(), b64(&secrets.wifi_ssid)),
        ("WIFI_PASSWORD".into(), b64(&secrets.wifi_password)),
        ("HOST".into(), b64(&secrets.host)),
        ("TOKEN".into(), b64(&secrets.token)),
        ("DEVICE_ID".into(), b64(&secrets.device_id)),
        // Schéma WebSocket du firmware : "true" → wss, "false" → ws.
        ("WS_SSL".into(), secrets.ws_ssl.to_string()),
    ];
    if let Some(key) = &secrets.encryption_key {
        vars.push(("ENCRYPTION_KEY".into(), key.clone()));
    }
    // Always set (empty = no pin) so `${sysenv.PNEX_CA_CERT}` never leaks
    // an unrelated value; only meaningful over TLS.
    let ca = secrets
        .ca_cert_pem
        .as_deref()
        .filter(|pem| secrets.ws_ssl && !pem.trim().is_empty())
        .map(b64)
        .unwrap_or_default();
    vars.push(("PNEX_CA_CERT".into(), ca));
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
        Some(secrets.wifi_password.as_str()),
        Some(secrets.token.as_str()),
        secrets.encryption_key.as_deref(),
    ];
    for v in values.into_iter().flatten().filter(|v| v.len() >= 4) {
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
            ws_ssl: true,
            token: "tok-secret".into(),
            device_id: "capteur-jardin".into(),
            encryption_key: Some("Y2xlLWI2NC1zMk8=".into()),
            ca_cert_pem: None,
        }
    }

    /// Les 5 vars device en base64 (round-trip), WS_SSL en true/false,
    /// clé telle quelle, absente si `None`.
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
        assert_eq!(get("WS_SSL"), "true");
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

    /// WS_SSL=false → ws:// (déploiement local sans TLS).
    #[test]
    fn ws_ssl_false_pour_local() {
        let mut s = secrets();
        s.ws_ssl = false;
        let vars = child_env(&s);
        let ssl = vars
            .iter()
            .find(|(n, _)| n == "WS_SSL")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        assert_eq!(ssl, "false");
    }

    fn get_var(vars: &[(String, String)], name: &str) -> String {
        vars.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// The CA travels base64-encoded over TLS, and is always present (empty
    /// without a CA or over plain ws).
    #[test]
    fn ca_cert_is_base64_over_tls_only() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
        let mut s = secrets();
        s.ca_cert_pem = Some(pem.into());
        let decoded = STANDARD
            .decode(get_var(&child_env(&s), "PNEX_CA_CERT"))
            .expect("b64");
        assert_eq!(String::from_utf8(decoded).expect("utf8"), pem);

        s.ws_ssl = false;
        assert_eq!(get_var(&child_env(&s), "PNEX_CA_CERT"), "");

        s.ws_ssl = true;
        s.ca_cert_pem = None;
        let vars = child_env(&s);
        assert!(vars
            .iter()
            .any(|(n, v)| n == "PNEX_CA_CERT" && v.is_empty()));
    }

    #[test]
    fn cle_absente_si_none() {
        let mut s = secrets();
        s.encryption_key = None;
        assert!(!child_env(&s).iter().any(|(n, _)| n == "ENCRYPTION_KEY"));
    }
}
