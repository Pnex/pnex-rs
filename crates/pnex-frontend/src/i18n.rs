//! i18n du front PNEX — Fluent via `dioxus-i18n`.
//!
//! Zéro libellé en dur dans les composants : tout passe par `t!("clé")`.
//! Locales embarquées (`include_str!`, compatibles wasm) : `fr-FR` + `en-US`,
//! fallback `en-US`. Le choix de la crate est isolé dans CE module — le reste
//! du code n'utilise que `init`, `t!`, `set_locale` et `current_tag`.
//!
//! Résolution de la langue initiale : préférence stockée (`pnex.locale`,
//! cf. `storage`) > `profile.language` (après login, cf. session) >
//! `navigator.language` (web) > `en-US`.

use dioxus_i18n::prelude::*;
use dioxus_i18n::unic_langid::{langid, LanguageIdentifier};

use crate::storage::KeyValueStorage;

/// Tags de locale persistés/échangés avec le backend (`profile.language`).
/// Deviennent utilisés au branchement session/profil.
#[allow(dead_code)]
pub const LOCALE_EN: &str = "en-US";
#[allow(dead_code)]
pub const LOCALE_FR: &str = "fr-FR";

/// Initialise le provider i18n — à appeler une seule fois, à la racine de
/// l'app (hook).
pub fn init() -> I18n {
    use_init_i18n(|| {
        I18nConfig::new(resolve_locale())
            .with_locale((langid!("en-US"), include_str!("../locales/en-US.ftl")))
            .with_locale((langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")))
            .with_fallback(langid!("en-US"))
    })
}

/// Langue initiale : navigateur (web), sinon en-US.
/// La préférence locale (`pnex.locale`) et celle du profil sont intégrées par
/// la session (boot / login) via `set_locale`.
fn resolve_locale() -> LanguageIdentifier {
    if let Some(tag) = stored_locale() {
        return tag;
    }
    #[cfg(target_arch = "wasm32")]
    {
        // navigator.language ressemble à "fr", "fr-FR", "en-US"…
        if let Some(nav) = web_sys::window().map(|w| w.navigator().language()) {
            if let Some(tag) = nav.as_deref().and_then(locale_from_tag) {
                return tag;
            }
        }
    }
    langid!("en-US")
}

/// Préférence de langue persistée localement (clé `pnex.locale`).
fn stored_locale() -> Option<LanguageIdentifier> {
    crate::storage::local()
        .get(crate::storage::KEY_LOCALE)
        .as_deref()
        .and_then(locale_from_tag)
}

/// Mappe un tag quelconque ("fr", "fr-FR", "en", "en-US"…) sur une locale
/// supportée ; `None` sinon.
pub fn locale_from_tag(tag: &str) -> Option<LanguageIdentifier> {
    let lower = tag.to_ascii_lowercase();
    match lower.as_str() {
        "fr" | "fr-fr" => Some(langid!("fr-FR")),
        "en" | "en-us" => Some(langid!("en-US")),
        _ => None,
    }
}

/// Tag courant ("fr-FR" / "en-US") — pour l'affichage et la persistance.
/// Devient utilisé au branchement session/profil.
#[allow(dead_code)]
pub fn current_tag() -> String {
    i18n().language().to_string()
}

/// Change la langue courante (no-op si tag inconnu) et persiste le choix.
#[allow(dead_code)]
pub fn set_locale(tag: &str) {
    let Some(id) = locale_from_tag(tag) else { return };
    let mut current = i18n();
    if current.language() != id {
        current.set_language(id);
    }
    crate::storage::local().set(crate::storage::KEY_LOCALE, tag);
}

#[cfg(test)]
mod tests {
    /// `t!` panique sur une clé absente de la langue courante : les deux
    /// locales doivent définir exactement les mêmes clés.
    fn keys(source: &'static str) -> Vec<String> {
        let resource = fluent_syntax::parser::parse(source).expect("fichier .ftl valide");
        resource
            .body
            .iter()
            .filter_map(|entry| match entry {
                fluent_syntax::ast::Entry::Message(message) => Some(message.id.name.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parite_cles_fr_en() {
        let mut en = keys(include_str!("../locales/en-US.ftl"));
        let mut fr = keys(include_str!("../locales/fr-FR.ftl"));
        en.sort();
        fr.sort();
        assert_eq!(
            en, fr,
            "fr-FR et en-US doivent définir exactement les mêmes clés"
        );
    }
}
