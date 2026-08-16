//! Claims Keycloak attendus dans les access tokens.
//!
//! Parité fonctionnelle Django (`KeycloakJWTAuthentication`) : le provisioning
//! JIT lit `preferred_username` (obligatoire), `email`, `given_name`,
//! `family_name`. Les champs d'identité absents restent `None` — la validation
//! (signature, `iss`, `aud`, `exp`) est faite dans [`super::jwks`].

use serde::Deserialize;

/// `aud` peut être une string ou une liste selon les mappers du client.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Aud {
    One(String),
    Many(Vec<String>),
}

impl Aud {
    pub fn contains(&self, expected: &str) -> bool {
        match self {
            Aud::One(s) => s == expected,
            Aud::Many(list) => list.iter().any(|s| s == expected),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Claims {
    /// UUID Keycloak — clé de JIT provisioning (`users.keycloak_uuid`).
    pub sub: String,
    pub preferred_username: String,
    pub email: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub iss: String,
    pub exp: i64,
    pub aud: Option<Aud>,
}

impl Claims {
    /// Nom affiché : `given_name family_name`, sinon le username Keycloak.
    pub fn display_name(&self) -> String {
        match (&self.given_name, &self.family_name) {
            (Some(g), Some(f)) => format!("{g} {f}"),
            (Some(g), None) => g.clone(),
            (None, Some(f)) => f.clone(),
            (None, None) => self.preferred_username.clone(),
        }
    }
}
