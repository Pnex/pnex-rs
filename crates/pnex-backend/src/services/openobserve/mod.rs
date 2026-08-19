//! Intégration OpenObserve (D1/D2) : stockage télémétrie, 1 org O2 par org
//! PNEX, credentials d'ingestion provisionnés automatiquement et conservés
//! en base (`openobserve_orgs`) — le chemin d'ingestion y puise le couple
//! org/token correlé.
//!
//! Contrat API vérifié contre openobserve v0.92.1 (dev compose) :
//! - `GET /api/organizations` (Basic root) → `data[] {identifier, name}` ;
//! - `POST /api/organizations {name}` → **ne dédoublonne pas par nom**
//!   (recréer = 2e org) → toujours chercher par nom avant de créer ;
//!   nom : alphanumérique + underscore uniquement ;
//! - `POST /api/{identifier}/users {email,password,role:"admin"}` (seul
//!   rôle natif hors root) → 400 « User already exists » si présent ;
//! - `PUT /api/{identifier}/users/{email} {new_password,change_password}`
//!   (root) → reprend un user sans son ancien mot de passe ;
//! - `GET /api/{identifier}/passcode` (Basic email:password du user) →
//!   `data.passcode` (o2oi_…) — Bearer passcode NON supporté en ingestion ;
//! - ingestion `POST /api/{identifier}/{stream}/_json` (Basic
//!   email:password **ou** email:passcode, corps = tableau JSON).

pub mod client;
pub mod promwrite;
pub mod provisioning;
pub mod sink;

pub use client::Client;
pub use provisioning::{ensure_org_credentials, OrgCredentials};
pub use sink::spawn_batcher;

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct OpenobserveSettings {
    pub base_url: String,
    /// Compte service admin (provisioning des orgs/users — jamais utilisé
    /// pour ingérer).
    pub root_email: String,
    pub root_password: String,
}

impl OpenobserveSettings {
    /// `settings.openobserve` optionnelle — absente = fonctionnalité
    /// désactivée (sink noop, health « not-configured » ; cas des tests).
    pub fn from_config(config: &Config) -> Option<Self> {
        config
            .settings
            .as_ref()
            .and_then(|s| s.get("openobserve"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}
