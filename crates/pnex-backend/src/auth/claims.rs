//! IdP claims expected in access tokens.
//!
//! JIT provisioning reads
//! `email`, `given_name`, `family_name`, `preferred_username`. **All identity
//! fields are optional**: Rauthy emits lean access tokens (no
//! `preferred_username`/`given_name`/`family_name` — those claims live in the
//! id_token and `/userinfo`). Validation (signature, `iss`,
//! `aud`, `exp`) is done in [`super::jwks`].

use serde::Deserialize;

/// `aud` can be a string or a list depending on the IdP/client.
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
    /// IdP `sub` (Rauthy: 24 alphanumeric characters) — JIT
    /// provisioning key (`users.idp_sub`).
    pub sub: String,
    /// Rauthy does not emit it in the access token (profile/id_token claim).
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    /// The IdP verified `email` (Rauthy emits it in the access token).
    /// Required to relink an existing account by email (SEC-W5).
    #[serde(default)]
    pub email_verified: Option<bool>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub iss: String,
    pub exp: i64,
    pub aud: Option<Aud>,
}

impl Claims {
    /// Display name: `given_name family_name`, otherwise the IdP username,
    /// otherwise the email (Rauthy: lean access tokens, often email only).
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
