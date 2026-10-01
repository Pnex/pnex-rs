//! Référentiels Edge — credentials WiFi + hosts serveur PNeX (org-scoped).
//!
//! Alimente l'étape Config du wizard device : fin de la ressaisie WiFi/host
//! à chaque ajout, l'entrée vit en base par org. Le payload `CreateBuild`
//! est **inchangé** (le wizard résout entrée → valeurs) — changement
//! purement additif, CONTRACT inchangé (2).
//!
//! WiFi password: a vault secret (secrets.md lot S6) — the API only carries
//! its reference and name, never the value; builds reference the
//! credential and the build worker decrypts at run time. wasm-safe (serde
//! seul, dates en String RFC 3339).

use serde::{Deserialize, Serialize};

/// Credential WiFi enregistré — `GET /api/v1/edge/wifi-credentials`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiCredential {
    pub id: i64,
    pub org_id: i64,
    /// Case-sensitive (byte-sensitive 802.11) — l'upsert l'est aussi.
    pub ssid: String,
    /// Vault secret of the password (`None` = not set).
    #[serde(default)]
    pub password: Option<crate::SecretFieldView>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
}

/// Corps du `POST /api/v1/edge/wifi-credentials` (upsert sur (org, ssid)).
///
/// `password`: pick a vault secret or type a value (owner/admin, stored in
/// the dedicated secret `wifi/<ssid>/password`). Absent on an update =
/// unchanged. `wifi_password` = legacy typed value, same as `{"value"}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WifiCredentialInput {
    pub ssid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<crate::SecretFieldInput>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub wifi_password: String,
}

/// Host serveur PNeX enregistré — `GET /api/v1/edge/hosts`.
/// `GET /api/v1/edge/hosts/locked` — server host imposed by the deployment
/// (`PNEX_PROD_HOST`). `host: None` = free choice from the org's referential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LockedHost {
    pub host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PnexHost {
    pub id: i64,
    pub org_id: i64,
    /// Hôte NU sans schéma (`192.168.1.16:5150`, `dev1.pnex.io`) — le
    /// schéma websocket suit [`PnexHost::ws_ssl`].
    pub host: String,
    /// Always true since D70: devices connect over `wss://` through the TLS
    /// edge (kept in the contract for compatibility).
    pub ws_ssl: bool,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
}

/// Corps du `POST /api/v1/edge/hosts` (upsert sur (org, host)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PnexHostInput {
    pub host: String,
    /// Ignored since D70 — the server stores `true` (wss only).
    pub ws_ssl: bool,
}

/// Un serveur PNeX détecté par le scan LAN côté serveur —
/// `GET /api/v1/edge/lan-scan`. L'identité `pnex-server` est vérifiée par
/// la sonde meta (champ `service` du contrat), pas un simple port ouvert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanScanHit {
    /// Hôte NU sans schéma, prêt pour [`PnexHost::host`] (`192.168.1.16:5150`).
    pub host: String,
    /// Version du serveur détecté (badge d'aide, pas bloquant).
    pub version: String,
    /// Contrat API du serveur détecté (info).
    pub contract: u32,
}

/// Réponse du scan LAN — les préfixes « a.b.c. » effectivement sondés
/// (auto-détection des interfaces privées du serveur, ou préfixe passé en
/// query) et les serveurs trouvés (triés par hôte).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanScanResult {
    pub prefixes: Vec<String>,
    pub hits: Vec<LanScanHit>,
}
