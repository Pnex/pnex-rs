//! Canaux livrés — [`websocket`](self::websocket) (bus interne),
//! [`webhook`](self::webhook) (POST générique), [`ntfy`](self::ntfy)
//! (push), [`telegram`](self::telegram) / [`slack`](self::slack) /
//! [`discord`](self::discord) (chat), [`smtp`](self::smtp) (e-mail).
//! Nouveau kind = 1 module + 1 impl [`Channel`](crate::Channel) + 1
//! entrée dans [`registry::CHANNELS`](crate::registry).

pub mod discord;
pub mod ntfy;
pub mod slack;
pub mod smtp;
pub mod telegram;
pub mod webhook;
pub mod websocket;

use crate::error::NotifyError;

use std::sync::LazyLock;

/// Client HTTP partagé — timeout 10 s par tentative (D53 : le retry
/// multiplie les tentatives, jamais leur durée).
pub(crate) static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    // Egress guard (R8): webhook URLs are user-chosen; internal addresses
    // are refused at resolution, every redirect re-checked.
    pnex_core::egress::guarded(reqwest::Client::builder())
        .redirect(pnex_core::egress::redirect_policy(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client (rustls)")
});

/// Client of the platform's own delivery endpoint (`websocket` channel):
/// an internal URL set by the server, never chosen by a user, so it is not
/// under the egress guard.
pub(crate) static INTERNAL_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client (rustls)")
});

/// Champ chaîne non vide d'une config de canal.
pub(crate) fn str_field<'a>(config: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    config
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Champ de `field_spec` — helper commun aux impls de canaux.
pub(crate) fn field(
    id: &str,
    label_i18n: &str,
    r#type: pnex_core::FieldType,
    required: bool,
    placeholder: Option<&str>,
    help_i18n: Option<&str>,
    options: &[&str],
) -> pnex_core::FieldSpec {
    pnex_core::FieldSpec {
        id: id.into(),
        label_i18n: label_i18n.into(),
        r#type,
        required,
        placeholder: placeholder.map(str::to_string),
        help_i18n: help_i18n.map(str::to_string),
        options: options.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Compose le texte des canaux « chat » (telegram/slack/discord) — le
/// sujet en première ligne (D62), corps brut ensuite.
pub(crate) fn compose_text(subject: Option<&str>, body: &str) -> String {
    match subject {
        Some(s) if !s.is_empty() => format!("{s}\n{body}"),
        _ => body.to_string(),
    }
}

/// Garde de taille fail-loud (D63) — jamais de troncature silencieuse en
/// alerting : le message dépasse la borne du canal ⇒ `TooLarge`.
pub(crate) fn size_guard(text: &str, max: usize) -> Result<(), NotifyError> {
    let n = text.chars().count();
    (n <= max).then_some(()).ok_or(NotifyError::TooLarge(n))
}

/// Tests: the mock servers listen on loopback, which the egress guard
/// refuses by default.
#[cfg(test)]
pub(crate) fn open_egress_for_tests() {
    pnex_core::egress::init(pnex_core::egress::EgressPolicy::Open);
}
