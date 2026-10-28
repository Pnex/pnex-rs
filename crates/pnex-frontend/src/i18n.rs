//! PNEX front i18n — Fluent via `dioxus-i18n`.
//!
//! Zero hard-coded label in components: everything goes through `t!("key")`.
//! Embedded locales (`include_str!`, wasm-compatible): `fr-FR` + `en-US`,
//! fallback `en-US`. The crate choice is isolated in THIS module — the rest
//! of the code only uses `init`, `t!`, `set_locale` and `current_tag`.
//!
//! Initial language resolution: stored preference (`pnex.locale`,
//! see `storage`) > `profile.language` (after login, see session) >
//! `navigator.language` (web) > `en-US`.

use dioxus_i18n::prelude::*;
use dioxus_i18n::unic_langid::{langid, LanguageIdentifier};

use crate::storage::KeyValueStorage;

/// Initialises the i18n provider — to call only once, at the root of
/// the app (hook).
pub fn init() -> I18n {
    use_init_i18n(|| {
        I18nConfig::new(resolve_locale())
            .with_locale((langid!("en-US"), include_str!("../locales/en-US.ftl")))
            .with_locale((langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")))
            .with_fallback(langid!("en-US"))
    })
}

/// Initial language: browser (web), else en-US.
/// The local preference (`pnex.locale`) and the profile one are applied by
/// the session (boot / login) via `set_locale`.
fn resolve_locale() -> LanguageIdentifier {
    if let Some(tag) = stored_locale() {
        return tag;
    }
    #[cfg(target_arch = "wasm32")]
    {
        // navigator.language looks like "fr", "fr-FR", "en-US"…
        if let Some(nav) = web_sys::window().map(|w| w.navigator().language()) {
            if let Some(tag) = nav.as_deref().and_then(locale_from_tag) {
                return tag;
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Native apps: follow the OS language (fr-BE, fr_FR.UTF-8… → fr-FR).
        if let Some(tag) = system_locale_tag()
            .as_deref()
            .and_then(language_from_system_tag)
        {
            return tag;
        }
    }
    langid!("en-US")
}

/// Maps an OS locale tag on a supported locale by its language subtag only
/// ("fr-BE", "fr_FR.UTF-8", "en-GB" …); `None` for unsupported languages.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn language_from_system_tag(tag: &str) -> Option<LanguageIdentifier> {
    let language = tag.split(['-', '_', '.', '@']).next().unwrap_or_default();
    locale_from_tag(language)
}

/// OS locale tag of the device (Android: `Locale.getDefault()`, which also
/// honours the per-app language setting).
#[cfg(target_os = "android")]
fn system_locale_tag() -> Option<String> {
    let env = crate::capture::attach_env()?;
    unsafe {
        let class = ((**env).v1_1.FindClass)(env, c"java/util/Locale".as_ptr());
        if class.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        let get_default = ((**env).v1_1.GetStaticMethodID)(
            env,
            class,
            c"getDefault".as_ptr(),
            c"()Ljava/util/Locale;".as_ptr(),
        );
        let to_tag = ((**env).v1_1.GetMethodID)(
            env,
            class,
            c"toLanguageTag".as_ptr(),
            c"()Ljava/lang/String;".as_ptr(),
        );
        if get_default.is_null() || to_tag.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        let locale =
            ((**env).v1_1.CallStaticObjectMethodA)(env, class, get_default, std::ptr::null());
        if locale.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        let jtag = ((**env).v1_1.CallObjectMethodA)(env, locale, to_tag, std::ptr::null());
        if jtag.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        let chars = ((**env).v1_1.GetStringUTFChars)(env, jtag, std::ptr::null_mut());
        if chars.is_null() {
            return None;
        }
        let tag = std::ffi::CStr::from_ptr(chars)
            .to_string_lossy()
            .into_owned();
        ((**env).v1_1.ReleaseStringUTFChars)(env, jtag, chars);
        Some(tag)
    }
}

/// OS locale tag of the desktop session (POSIX precedence; Windows: none yet,
/// falls back to en-US).
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
fn system_locale_tag() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
}

/// Language preference persisted locally (key `pnex.locale`).
fn stored_locale() -> Option<LanguageIdentifier> {
    crate::storage::local()
        .get(crate::storage::KEY_LOCALE)
        .as_deref()
        .and_then(locale_from_tag)
}

/// Maps any tag ("fr", "fr-FR", "en", "en-US"…) to a supported
/// locale; `None` otherwise.
pub fn locale_from_tag(tag: &str) -> Option<LanguageIdentifier> {
    let lower = tag.to_ascii_lowercase();
    match lower.as_str() {
        "fr" | "fr-fr" => Some(langid!("fr-FR")),
        "en" | "en-us" => Some(langid!("en-US")),
        _ => None,
    }
}

/// Current tag ("fr-FR" / "en-US") — for display and persistence.
pub fn current_tag() -> String {
    i18n().language().to_string()
}

/// Changes the current language (no-op if the tag is unknown) and persists the choice.
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

    /// `t!` panics on a key missing from the current language: both
    /// locales must define exactly the same keys.
    fn keys(source: &'static str) -> Vec<String> {
        let resource = fluent_syntax::parser::parse(source).expect("valid .ftl file");
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
    fn system_tags_map_by_language() {
        assert_eq!(
            super::language_from_system_tag("fr-BE"),
            Some(langid!("fr-FR"))
        );
        assert_eq!(
            super::language_from_system_tag("fr_FR.UTF-8"),
            Some(langid!("fr-FR"))
        );
        assert_eq!(
            super::language_from_system_tag("en-GB"),
            Some(langid!("en-US"))
        );
        assert_eq!(super::language_from_system_tag("de-DE"), None);
    }

    #[test]
    fn parite_cles_fr_en() {
        let mut en = keys(include_str!("../locales/en-US.ftl"));
        let mut fr = keys(include_str!("../locales/fr-FR.ftl"));
        en.sort();
        fr.sort();
        assert_eq!(en, fr, "fr-FR and en-US must define exactly the same keys");
    }

    /// `$variables` used by each message (continuation lines included).
    fn variables(source: &str) -> std::collections::BTreeMap<String, BTreeSet<String>> {
        let mut out = std::collections::BTreeMap::new();
        let mut current: Option<String> = None;
        for line in source.lines() {
            let starts_entry = line.split_once(" =").filter(|(k, _)| {
                !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            });
            if let Some((key, _)) = starts_entry {
                current = Some(key.to_string());
            } else if !line.starts_with(|c: char| " \t}[*".contains(c)) {
                current = None;
            }
            let Some(key) = &current else { continue };
            let vars: &mut BTreeSet<String> = out.entry(key.clone()).or_default();
            for part in line.split('$').skip(1) {
                let name: String = part
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if !name.is_empty() {
                    vars.insert(name);
                }
            }
        }
        out
    }

    /// A variable renamed in one locale only makes `t!` fail at render time
    /// (wasm panic, whole page down): both locales use the same variables.
    #[test]
    fn variables_match_across_locales() {
        let en = variables(include_str!("../locales/en-US.ftl"));
        let fr = variables(include_str!("../locales/fr-FR.ftl"));
        let diff: Vec<String> = en
            .iter()
            .filter_map(|(k, v)| {
                let other = fr.get(k)?;
                (other != v).then(|| format!("{k}: en {v:?} / fr {other:?}"))
            })
            .collect();
        assert!(diff.is_empty(), "variables differ: {diff:#?}");
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

    /// Every service and verb of the IAM registry has its label in BOTH
    /// locales (`access-denied` names them, iam.md §8).
    #[test]
    fn iam_permission_labels_en_fr() {
        let en = keys(include_str!("../locales/en-US.ftl"));
        let fr = keys(include_str!("../locales/fr-FR.ftl"));
        for (permission, _) in pnex_api_contract::iam::PERMISSIONS {
            let (service, verb) = permission.split_once(':').unwrap();
            for key in [
                format!("iam-service-{}", service.replace('.', "-")),
                format!("iam-verb-{verb}"),
            ] {
                assert!(
                    en.contains(&key) && fr.contains(&key),
                    "`{permission}`: key `{key}` missing from a locale — add it to BOTH .ftl files"
                );
            }
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

    /// Sweep needle built in pieces: a literal `t!(` in
    /// THIS file would sweep itself.
    const NEEDLE: &str = concat!("t!", "(");

    /// Neutralises comments (`//` and `/* */`) so as not to sweep
    /// `t!` examples quoted in comments; string literals are
    /// preserved (escapes handled). Block comments are not
    /// assumed to nest here.
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

    /// Recursive collection of **literal** `t!` keys under a folder, after
    /// stripping comments. Macro boundary checked (the previous character
    /// must not extend an identifier, otherwise `format!(`/`concat!(`
    /// would match); `t!(variable)` (e.g. label_i18n of the backend field_spec)
    /// is out of scope — not statically checkable.
    fn collect_t_keys(dir: &Path, out: &mut BTreeSet<String>) {
        let entries =
            fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("readable src/ entry").path();
            if path.is_dir() {
                collect_t_keys(&path, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = strip_comments(&fs::read_to_string(&path).expect("readable .rs source"));
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
                // Literal key only: `"` as the first token after `t!(`.
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

    /// Every literal key passed to `t!` must exist in **both**
    /// locales: `t!` panics on a missing key (real crash 2026-09-14 —
    /// `common-create` missing → panic "App panicked" when clicking
    /// "add a notification channel" on /notifications).
    #[test]
    fn cles_litterales_t_presentes_dans_les_locales() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut used = BTreeSet::new();
        collect_t_keys(&src, &mut used);
        assert!(used.len() > 100, "sterile sweep: {} keys found", used.len());

        let en = keys(include_str!("../locales/en-US.ftl"));
        let fr = keys(include_str!("../locales/fr-FR.ftl"));
        let missing: Vec<&String> = used
            .iter()
            .filter(|k| !en.contains(k) || !fr.contains(k))
            .collect();
        assert!(
            missing.is_empty(),
            "keys missing from the locales (panic at render): {missing:?}"
        );
    }

    /// A Fluent message with a variable rendered **without** its argument makes
    /// `t!` panic (FluentErrorsDetected → unwrap_or_else(panic!) in dioxus-i18n):
    /// real crash 2026-09-06 — `flows-metric-labels-help` (variable `$id`)
    /// rendered without `id` when adding a Metric node in the flow editor.
    /// The crash message must stay resolvable in both locales with
    /// the exact argument `MetricForm` now passes.
    #[test]
    fn flows_metric_labels_help_resout_avec_son_argument_id() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource = FluentResource::try_new(source.to_string()).expect("valid .ftl file");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("isolated resource: no overlap error");
            bundle.set_use_isolating(false);
            let message = bundle
                .get_message("flows-metric-labels-help")
                .expect("key defined (see test parite_cles_fr_en)");
            let pattern = message.value().expect("message without attribute");
            let mut args = FluentArgs::new();
            args.set("id", FluentValue::from(7_i64));
            let mut errors = vec![];
            let rendered = bundle
                .format_pattern(pattern, Some(&args), &mut errors)
                .to_string();
            assert!(errors.is_empty(), "{locale} : {errors:?}");
            assert!(rendered.contains("flow_7"), "{locale}: \"{rendered}\"");
        }
    }

    /// `flows-deploy-stale-notify { $count }` (D50 deploy toast) must
    /// resolve with its exact argument in both locales.
    #[test]
    fn flows_deploy_stale_notify_resout_avec_son_argument_count() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource = FluentResource::try_new(source.to_string()).expect("valid .ftl file");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("isolated resource: no overlap error");
            bundle.set_use_isolating(false);
            let message = bundle
                .get_message("flows-deploy-stale-notify")
                .expect("key defined (see test parite_cles_fr_en)");
            let pattern = message.value().expect("message without attribute");
            let mut args = FluentArgs::new();
            args.set("count", FluentValue::from(2_i64));
            let mut errors = vec![];
            let rendered = bundle
                .format_pattern(pattern, Some(&args), &mut errors)
                .to_string();
            assert!(errors.is_empty(), "{locale} : {errors:?}");
            assert!(rendered.contains('2'), "{locale}: \"{rendered}\"");
        }
    }

    /// The notify-inspector var rows embed literal braces (`{{ msg.x }}`) in
    /// the user-facing syntax hint. Written bare, fluent parses `{{ msg.x }}`
    /// as a message reference (`msg` + attribute `x`) → ResolverError → `t!`
    /// panics at render (real crash 2026-09-23 — clicking the var row of a
    /// Notification node). They must use fluent string literals (`{'{{'}`)
    /// and resolve error-free, showing the `{msg.x}`-style syntax.
    #[test]
    fn notify_var_syntax_hints_resolvent_sans_erreur() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource = FluentResource::try_new(source.to_string()).expect("valid .ftl file");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("isolated resource: no overlap error");
            bundle.set_use_isolating(false);
            for key in ["flows-inspector-var-value-placeholder", "flows-notify-vars"] {
                let message = bundle.get_message(key).expect("key defined (parity)");
                let pattern = message.value().expect("message without attribute");
                let mut errors = vec![];
                let rendered = bundle
                    .format_pattern(pattern, None, &mut errors)
                    .to_string();
                assert!(errors.is_empty(), "{locale} {key} : {errors:?}");
                assert!(rendered.contains("msg.x"), "{locale} {key}: \"{rendered}\"");
            }
        }
    }

    /// `flows-notify-antispam-hint { $max } / { $window }` must resolve
    /// with its two exact arguments in both locales.
    #[test]
    fn flows_notify_antispam_hint_resout_avec_ses_arguments() {
        for (locale, source) in [
            (langid!("en-US"), include_str!("../locales/en-US.ftl")),
            (langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")),
        ] {
            let resource = FluentResource::try_new(source.to_string()).expect("valid .ftl file");
            let mut bundle = FluentBundle::new(vec![locale.clone()]);
            bundle
                .add_resource(resource)
                .expect("isolated resource: no overlap error");
            bundle.set_use_isolating(false);
            let message = bundle
                .get_message("flows-notify-antispam-hint")
                .expect("key defined (see test parite_cles_fr_en)");
            let pattern = message.value().expect("message without attribute");
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
                "{locale}: \"{rendered}\""
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
