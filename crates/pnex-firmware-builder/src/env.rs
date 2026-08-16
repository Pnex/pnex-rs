//! Assemblage des variables d'environnement du sous-process `pio run`.
//!
//! Contrat du dépôt firmware (vérifié, firmware-build.md §2.1) : la config
//! device se lit en **variables d'environnement** (platformio.ini
//! `-D WIFI_SSID="${sysenv.WIFI_SSID}"`…) — WIFI_SSID et WIFI_PASSWORD en
//! clair, HOST/TOKEN/DEVICE_ID **en base64** (le firmware les décode).
//! Jamais en argv : `ps` expose les arguments.

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
        ("WIFI_SSID".into(), secrets.wifi_ssid.clone()),
        ("WIFI_PASSWORD".into(), secrets.wifi_password.clone()),
        // Base64 côté serveur, décodés par le firmware (parité build.sh).
        ("HOST".into(), b64(&secrets.host)),
        ("TOKEN".into(), b64(&secrets.token)),
        ("DEVICE_ID".into(), b64(&secrets.device_id)),
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
            token: "tok-secret".into(),
            device_id: "capteur-jardin".into(),
            encryption_key: Some("Y2xlLWI2NC1zMk8=".into()),
        }
    }

    /// WiFi en clair, HOST/TOKEN/DEVICE_ID en base64 (round-trip), clé
    /// telle quelle, absente si `None`.
    #[test]
    fn env_conforme_au_contrat_firmware() {
        let vars = child_env(&secrets());
        let get = |k: &str| {
            vars.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        assert_eq!(get("WIFI_SSID"), "coloc");
        assert_eq!(get("WIFI_PASSWORD"), "p@ss w0rd");
        for (name, expected) in [
            ("HOST", "dev1.pnex.io"),
            ("TOKEN", "tok-secret"),
            ("DEVICE_ID", "capteur-jardin"),
        ] {
            let decoded = STANDARD.decode(get(name)).expect("b64");
            assert_eq!(String::from_utf8(decoded).expect("utf8"), expected);
        }
        assert_eq!(get("ENCRYPTION_KEY"), "Y2xlLWI2NC1zMk8=");
    }

    #[test]
    fn cle_absente_si_none() {
        let mut s = secrets();
        s.encryption_key = None;
        assert!(!child_env(&s).iter().any(|(n, _)| n == "ENCRYPTION_KEY"));
    }
}
