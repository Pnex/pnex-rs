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
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> BuildSecrets {
        BuildSecrets {
            wifi_ssid: "coloc".into(),
            wifi_password: "p@ss w0rd".into(),
            host: "dev1.pnex.io".into(),
            ws_ssl: true,
            token: "tok-secret".into(),
            device_id: "capteur-jardin".into(),
            encryption_key: Some("Y2xlLWI2NC1zMk8=".into()),
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

    #[test]
    fn cle_absente_si_none() {
        let mut s = secrets();
        s.encryption_key = None;
        assert!(!child_env(&s).iter().any(|(n, _)| n == "ENCRYPTION_KEY"));
    }
}
