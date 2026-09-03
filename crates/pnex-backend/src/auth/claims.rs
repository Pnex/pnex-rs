//! Claims IdP attendus dans les access tokens.
//!
//! Continuité Django (`KeycloakJWTAuthentication`) : le provisioning JIT lit
//! `email`, `given_name`, `family_name`, `preferred_username`. **Tous les
//! champs d'identité sont optionnels** : Rauthy émet des access tokens lean
//! (pas de `preferred_username`/`given_name`/`family_name` — ces claims
//! vivent dans l'id_token et `/userinfo`). La validation (signature, `iss`,
//! `aud`, `exp`) est faite dans [`super::jwks`].

use serde::Deserialize;

/// `aud` peut être une string ou une liste selon l'IdP/client.
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
    /// `sub` de l'IdP (Rauthy : 24 caractères alphanumériques) — clé du JIT
    /// provisioning (`users.idp_sub`).
    pub sub: String,
    /// Rauthy ne l'émet pas dans l'access token (claim du profil/id_token).
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub iss: String,
    pub exp: i64,
    pub aud: Option<Aud>,
}

impl Claims {
    /// Nom affiché : `given_name family_name`, sinon le username de l'IdP,
    /// sinon l'email (Rauthy : access tokens lean, souvent email seul).
    pub fn display_name(&self) -> String {
        match (&self.given_name, &self.family_name) {
            (Some(g), Some(f)) => format!("{g} {f}"),
            (Some(g), None) => g.clone(),
            (None, Some(f)) => f.clone(),
            (None, None) => self
                .preferred_username
                .clone()
                .or_else(|| self.email.clone())
                .unwrap_or_else(|| self.sub.clone()),
        }
    }
}
