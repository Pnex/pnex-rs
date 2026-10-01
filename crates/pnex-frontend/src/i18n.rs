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
    let Some(id) = locale_from_tag(tag) else {
        return;
    };
    let mut current = i18n();
    if current.language() != id {
        current.set_language(id);
    }
    crate::storage::local().set(crate::storage::KEY_LOCALE, tag);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use dioxus_i18n::fluent::{FluentArgs, FluentBundle, FluentResource, FluentValue};
    use dioxus_i18n::unic_langid::langid;

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

    /// Cross-check (i18n chantier): every machine code registered in
    /// `pnex_core::err_codes::ALL` must have its `err-<kebab>` key in BOTH
    /// locales — the backend guard `error_codes.rs` enforces the converse
    /// (every ErrorDetail code in ALL).
    #[test]
    fn err_codes_resolvent_en_fr() {
        let en = keys(include_str!("../locales/en-US.ftl"));
        let fr = keys(include_str!("../locales/fr-FR.ftl"));
        for code in pnex_core::err_codes::ALL {
            let key = pnex_core::err_codes::fluent_key(code);
            assert!(
                en.contains(&key) && fr.contains(&key),
                "code `{code}`: key `{key}` missing from a locale — add it to BOTH .ftl files"
            );
        }
    }

    /// Every catalog library of the firmware IDE has its description key
    /// (`fw-lib-<id>`, rendered with a dynamic `t!` — a missing key panics).
    #[test]
    fn firmware_lib_catalog_keys_en_fr() {
        let en = keys(include_str!("../locales/en-US.ftl"));
        let fr = keys(include_str!("../locales/fr-FR.ftl"));
        for lib in pnex_core::firmware::LIB_CATALOG {
            let key = format!("fw-lib-{}", lib.id);
            assert!(
                en.contains(&key) && fr.contains(&key),
                "catalog library `{}`: key `{key}` missing from a locale",
                lib.id
            );
        }
    }

    /// Every CoolProp node quantity has its label key (rendered with a
    /// dynamic `t!` — a missing key panics).
    #[test]
    fn thermo_quantity_label_keys_en_fr() {
        let en = keys(include_str!("../locales/en-US.ftl"));
        let fr = keys(include_str!("../locales/fr-FR.ftl"));
        for q in pnex_core::THERMO_QUANTITIES {
            assert!(
                en.contains(&q.label_key.to_string()) && fr.contains(&q.label_key.to_string()),
                "quantity `{}`: key `{}` missing from a locale",
                q.id,
                q.label_key
            );
        }
    }

    /// Aiguille du balayage construite par morceaux : un `t!(` littéral dans
    /// CE fichier se balaierait lui-même.
    const NEEDLE: &str = concat!("t!", "(");

    /// Neutralise les commentaires (`//` et `/* */`) pour ne pas balayer des
    /// exemples `t!` cités en commentaire ; les littéraux de chaîne sont
    /// préservés (échappements gérés). Les commentaires de bloc ne sont pas
    /// supposés imbriqués ici.
    fn strip_comments(src: &str) -> String {
        let b = src.as_bytes();
        let mut out = String::with_capacity(src.len());
        let mut copied = 0usize;
        let mut i = 0usize;
        let mut in_string = false;
        while i < b.len() {
            if in_string {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    in_string = false;
                }
                i += 1;
                continue;
            }
            match b[i] {
                b'"' => {
                    in_string = true;
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    out.push_str(&src[copied..i]);
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                    copied = i;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    out.push_str(&src[copied..i]);
                    i += 2;
                    while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                        i += 1;
                    }
                    i = (i + 2).min(b.len());
                    copied = i;
                }
                _ => i += 1,
            }
        }
        out.push_str(&src[copied..]);
        out
    }

    /// Collecte récursive des clés `t!` **littérales** sous un dossier, après
    /// retrait des commentaires. Bord de macro vérifié (le caractère précédent
    /// ne doit pas prolonger un identifiant, sinon `format!(`/`concat!(`
    /// matcheraient) ; `t!(variable)` (ex. label_i18n du field_spec backend)
    /// est hors périmètre — non vérifiable statiquement.
    fn collect_t_keys(dir: &Path, out: &mut BTreeSet<String>) {
        let entries =
            fs::read_dir(dir).unwrap_or_else(|e| panic!("lecture de {} : {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("entrée src/ lisible").path();
            if path.is_dir() {
                collect_t_keys(&path, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = strip_comments(&fs::read_to_string(&path).expect("source .rs lisible"));
            let bytes = source.as_bytes();
            let mut cursor = 0;
            while let Some(rel) = source[cursor..].find(NEEDLE) {
                let at = cursor + rel;
                cursor = at + NEEDLE.len();
                let boundary = match bytes[..at].iter().rev().find(|b| !b.is_ascii_whitespace()) {
                    Some(b) => !b.is_ascii_alphanumeric() && *b != b'_',
                    None => true,
                };
                if !boundary {
                    continue;
                }
                // Clé littérale uniquement : `"` en premier token après `t!(`.
                let Some(rest) = source[cursor..].trim_start().strip_prefix('"') else {
                    continue;
                };
                let mut key = String::new();
                let mut escaped = false;
                for ch in rest.chars() {
                    if escaped {
                        key.push(ch);
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        break;
                    } else {
                        key.push(ch);
                    }
                }
                out.insert(key);
            }
        }
    }

    /// Toute clé littérale passée à `t!` doit exister dans les **deux**
    /// locales : `t!` panique sur une clé absente (crash réel 2026-09-14 —
    /// `common-create` manquant → panic « App panicked » en cliquant
    /// « ajouter un canal de notification » sur /notifications).
    #[test]
    fn cles_litterales_t_presentes_dans_les_locales() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut used = BTreeSet::new();
        collect_t_keys(&src, &mut used);
        assert!(
            used.len() > 100,
            "balayage stérile : {} clés trouvées",
            used.len()
        );

        let en = keys(include_str!("../locales/en-US.ftl"));
        let fr = keys(include_str!("../locales/fr-FR.ftl"));
        let missing: Vec<&String> = used
            .iter()
            .filter(|k| !en.contains(k) || !fr.contains(k))
            .collect();
        assert!(
            missing.is_empty(),
            "clés manquantes dans les locales (panique au rendu) : {missing:?}"
        );
    }

    /// Un message Fluent à variable rendu **sans** son argument fait paniquer
    /// `t!` (FluentErrorsDetected → unwrap_or_else(panic!) dans dioxus-i18n) :
    /// crash réel 2026-09-06 — `flows-metric-labels-help` (variable `$id`)
    /// rendu sans `id` à l'ajout d'un nœud Métrique dans l'éditeur de flows.
    /// Le message du crash doit rester résoluble dans les deux locales avec
    /// l'argument exact que passe désormais `MetricForm`.
    #[test]
    fn flows_metric_labels_help_resout_avec_son_argument_id() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource =
                FluentResource::try_new(source.to_string()).expect("fichier .ftl valide");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("ressource isolée : aucune erreur de chevauchement");
            bundle.set_use_isolating(false);
            let message = bundle
                .get_message("flows-metric-labels-help")
                .expect("clé définie (cf. test parite_cles_fr_en)");
            let pattern = message.value().expect("message sans attribut");
            let mut args = FluentArgs::new();
            args.set("id", FluentValue::from(7_i64));
            let mut errors = vec![];
            let rendered = bundle
                .format_pattern(pattern, Some(&args), &mut errors)
                .to_string();
            assert!(errors.is_empty(), "{locale} : {errors:?}");
            assert!(rendered.contains("flow_7"), "{locale} : « {rendered} »");
        }
    }

    /// `flows-deploy-stale-notify { $count }` (toast de deploy D50) doit se
    /// résoudre avec son argument exact dans les deux locales.
    #[test]
    fn flows_deploy_stale_notify_resout_avec_son_argument_count() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource =
                FluentResource::try_new(source.to_string()).expect("fichier .ftl valide");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("ressource isolée : aucune erreur de chevauchement");
            bundle.set_use_isolating(false);
            let message = bundle
                .get_message("flows-deploy-stale-notify")
                .expect("clé définie (cf. test parite_cles_fr_en)");
            let pattern = message.value().expect("message sans attribut");
            let mut args = FluentArgs::new();
            args.set("count", FluentValue::from(2_i64));
            let mut errors = vec![];
            let rendered = bundle
                .format_pattern(pattern, Some(&args), &mut errors)
                .to_string();
            assert!(errors.is_empty(), "{locale} : {errors:?}");
            assert!(rendered.contains('2'), "{locale} : « {rendered} »");
        }
    }

    /// The notify-inspector var rows embed literal braces (`{{ msg.x }}`) in
    /// the user-facing syntax hint. Written bare, fluent parses `{{ msg.x }}`
    /// as a message reference (`msg` + attribute `x`) → ResolverError → `t!`
    /// panics at render (crash réel 2026-09-23 — clicking the var row of a
    /// Notification node). They must use fluent string literals (`{'{{'}`)
    /// and resolve error-free, showing the `{msg.x}`-style syntax.
    #[test]
    fn notify_var_syntax_hints_resolvent_sans_erreur() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource =
                FluentResource::try_new(source.to_string()).expect("fichier .ftl valide");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("ressource isolée : aucune erreur de chevauchement");
            bundle.set_use_isolating(false);
            for key in ["flows-inspector-var-value-placeholder", "flows-notify-vars"] {
                let message = bundle.get_message(key).expect("clé définie (parité)");
                let pattern = message.value().expect("message sans attribut");
                let mut errors = vec![];
                let rendered = bundle
                    .format_pattern(pattern, None, &mut errors)
                    .to_string();
                assert!(errors.is_empty(), "{locale} {key} : {errors:?}");
                assert!(
                    rendered.contains("msg.x"),
                    "{locale} {key} : « {rendered} »"
                );
            }
        }
    }

    /// `flows-notify-antispam-hint { $max } / { $window }` doit se résoudre
    /// avec ses deux arguments exacts dans les deux locales.
    #[test]
    fn flows_notify_antispam_hint_resout_avec_ses_arguments() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource =
                FluentResource::try_new(source.to_string()).expect("fichier .ftl valide");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("ressource isolée : aucune erreur de chevauchement");
            bundle.set_use_isolating(false);
            let message = bundle
                .get_message("flows-notify-antispam-hint")
                .expect("clé définie (cf. test parite_cles_fr_en)");
            let pattern = message.value().expect("message sans attribut");
            let mut args = FluentArgs::new();
            args.set("max", FluentValue::from(3_i64));
            args.set("window", FluentValue::from(10_i64));
            let mut errors = vec![];
            let rendered = bundle
                .format_pattern(pattern, Some(&args), &mut errors)
                .to_string();
            assert!(errors.is_empty(), "{locale} : {errors:?}");
            assert!(
                rendered.contains('3') && rendered.contains("10"),
                "{locale} : « {rendered} »"
            );
        }
    }

    /// Predictive node hints carry `$min` / `$max`: each must resolve with
    /// exactly the arguments `inspector/predict.rs` passes (a missing
    /// argument panics `t!` at render).
    #[test]
    fn predict_hints_resolve_with_their_arguments() {
        let cases: [(&str, &[&str]); 4] = [
            ("flows-predict-window-hint", &["min", "max"]),
            ("flows-predict-min-samples-hint", &["min"]),
            ("flows-forecast-horizon-hint", &["max"]),
            ("flows-forecast-horizon-span", &["at_1hz", "at_1min"]),
        ];
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource = FluentResource::try_new(source.to_string()).expect("valid .ftl file");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle.add_resource(resource).expect("isolated resource");
            bundle.set_use_isolating(false);
            for (key, names) in cases {
                let pattern = bundle
                    .get_message(key)
                    .and_then(|m| m.value())
                    .unwrap_or_else(|| panic!("{locale} : {key} missing"));
                let mut args = FluentArgs::new();
                for name in names {
                    args.set(*name, FluentValue::from(42_i64));
                }
                let mut errors = vec![];
                let rendered = bundle
                    .format_pattern(pattern, Some(&args), &mut errors)
                    .to_string();
                assert!(errors.is_empty(), "{locale} {key} : {errors:?}");
                assert!(rendered.contains("42"), "{locale} {key} : {rendered}");
            }
        }
    }
}
