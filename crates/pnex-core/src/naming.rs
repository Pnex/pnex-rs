//! Nommage des métriques et clés de payload — **source de vérité unique**
//! partagée backend ↔ runtime de flows (Phase 6 ETL).
//!
//! Pourquoi centraliser : la lecture device du runtime cible les séries
//! écrites par l'ingestion (`last_over_time(<label>{device_id="…"})`) — si les
//! deux côtés ne normalisaient pas le label du pin à l'identique, le PromQL
//! chercherait une série inexistante. Même exigence pour l'écriture `etl_` :
//! le préfixe et le sanitize doivent coïncider partout.
//!
//! Tout est pur et sans dépendance (wasm32 inclus) — sauf
//! [`normalize_measurement_name`], derrière la feature `naming` (table
//! deunicode, inutile au bundle wasm du front).

/// Nom de métrique Prometheus valide (`[a-zA-Z_:][a-zA-Z0-9_:]*`) — sert de
/// validation anti-injection avant toute interpolation dans une requête
/// PromQL ; les noms hors charset sont rejetés plutôt que d'ouvrir une
/// faille.
pub fn valid_metric_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

/// Nom de métrique Prometheus valide : `[a-zA-Z_:][a-zA-Z0-9_:]*` — les
/// caractères interdits deviennent `_`, un préfixe `_` interdit est
/// évité (interdiction de ressembler aux séries internes).
pub fn sanitize_metric_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, c) in name.chars().enumerate() {
        let valid = c.is_ascii_alphanumeric() || c == '_' || c == ':';
        if valid && !(i == 0 && c.is_ascii_digit()) {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('m');
    }
    out
}

/// Valeur de label `device_id` sûre à interpoler dans un sélecteur
/// PromQL : charset fermé (nos device_id sont des slugs), aucune
/// quote/brace/backslash possible — l'injection PromQL est bloquée en
/// amont, pas échappée.
pub fn valid_device_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Assainit une clé de payload device → identifiant de variable calc
/// (`[A-Za-z_][A-Za-z0-9_]*`) : « capteur-1 » + « D1 » → `capteur_1_D1`.
/// Les caractères hors charset deviennent `_` (répétitions fondues ? non —
/// un-à-un, même règle que [`sanitize_metric_name`]), un chiffre initial
/// devient `_`, et une clé vide ressort « k » (jamais vide — une clé vide
/// ne serait pas une variable calc référençable).
pub fn sanitize_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (i, c) in raw.chars().enumerate() {
        let valid = c.is_ascii_alphanumeric() || c == '_';
        if valid && !(i == 0 && c.is_ascii_digit()) {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('k');
    }
    out
}

/// Clé de payload d'une lecture device : `sanitize_key(device_id) + "_" +
/// sanitize_key(pin)` — l'identifiant que les variables du nœud `calc`
/// référencent, calculé par le runtime ET prévisualisé par l'éditeur (même
/// fonction, donc même valeur).
pub fn device_payload_key(device_id: &str, pin: &str) -> String {
    format!("{}_{}", sanitize_key(device_id), sanitize_key(pin))
}

/// Nom final de la métrique écrite par un nœud `metric` : préfixe `etl_`
/// forcé (l'« index dédié » des résultats ETL — idempotent si l'utilisateur
/// a déjà saisi le préfixe), minuscules (même philosophie que la
/// normalisation D16 des mesures), puis sanitize Prometheus.
pub fn etl_metric_name(name: &str) -> String {
    let lowered = name.to_lowercase();
    let stripped = lowered.strip_prefix("etl_").unwrap_or(&lowered);
    format!("etl_{}", sanitize_metric_name(stripped))
}

/// Measurement label normalization (D16): one source of truth, so the
/// device link and the runtime's device reads produce the same series name.
///
/// Trim, pliage des accents (deunicode), minuscules, tout non
/// `[a-z0-9_:]` → `_` (répétitions fondues, `_` de bord supprimés).
/// `Soil-Moisture`, `soil moisture` et `soil_moisture` → `soil_moisture`.
/// Vide si le nom n'est que des séparateurs.
#[cfg(feature = "naming")]
pub fn normalize_measurement_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_sep = false; // séparateur fondu, flushé devant du contenu
    for c in deunicode::deunicode(raw.trim()).chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == ':' {
            if pending_sep {
                out.push('_');
                pending_sep = false;
            }
            out.push(c);
        } else {
            pending_sep = !out.is_empty();
        }
    }
    out
}

// ───────────── Free series labels (D171): shared write/read rules ─────────────

/// Max free labels on one series (metric node config and dashboard source).
pub const SERIES_LABELS_MAX: usize = 5;
/// Max length of a free label value.
pub const SERIES_LABEL_VALUE_MAX: usize = 64;
/// Labels owned by the platform: a free label never overrides them.
pub const RESERVED_SERIES_LABELS: &[&str] = &[
    "__name__",
    "device_id",
    "pred_dev",
    "source_type",
    "ts_source",
];

/// Free label name: `^[a-z_][a-z0-9_]{0,31}$`, not reserved, no `__` prefix.
pub fn valid_series_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= 32
        && (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with("__")
        && !RESERVED_SERIES_LABELS.contains(&name)
}

/// Free label value safe to interpolate in a PromQL selector: 1..=64 chars
/// of `[A-Za-z0-9_.:-]` (no quote, brace, backslash or newline possible).
pub fn valid_series_label_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= SERIES_LABEL_VALUE_MAX
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

/// Runtime label value: trimmed, every char outside the value charset
/// becomes `_`, cut to 64 chars; empty → `unknown`. Always passes
/// [`valid_series_label_value`].
pub fn sanitize_series_label_value(raw: &str) -> String {
    let out: String = raw
        .trim()
        .chars()
        .take(SERIES_LABEL_VALUE_MAX)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

/// Selector check of a dashboard label set (read side): count, names and
/// values. `Some((code, message))` = refused before any query.
pub fn check_series_selector_labels(
    labels: &std::collections::BTreeMap<String, String>,
) -> Option<(&'static str, String)> {
    if labels.len() > SERIES_LABELS_MAX {
        return Some((
            "series_labels_too_many",
            format!("at most {SERIES_LABELS_MAX} labels"),
        ));
    }
    for (k, v) in labels {
        if !valid_series_label_name(k) {
            return Some((
                "series_label_name_invalid",
                format!("invalid label name `{k}`"),
            ));
        }
        if !valid_series_label_value(v) {
            return Some((
                "series_label_value_invalid",
                format!("invalid value for label `{k}`"),
            ));
        }
    }
    None
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn label_names_and_values() {
        for ok in ["stream", "_x", "entity_1", "taxonomy_version"] {
            assert!(valid_series_label_name(ok), "{ok}");
        }
        let long_name = "a".repeat(33);
        for bad in [
            "",
            "Stream",
            "1a",
            "__x",
            "device_id",
            "pred_dev",
            "a-b",
            "a\"",
            long_name.as_str(),
        ] {
            assert!(!valid_series_label_name(bad), "{bad}");
        }
        assert!(valid_series_label_value("france-inter:v1.2_x"));
        let long_value = "x".repeat(65);
        for bad in [
            "",
            "a\"}",
            "a\nb",
            "a b",
            "a\\",
            "a{b}",
            long_value.as_str(),
        ] {
            assert!(!valid_series_label_value(bad), "{bad:?}");
        }
        assert_eq!(sanitize_series_label_value("  Le Monde\"} "), "Le_Monde__");
        assert_eq!(sanitize_series_label_value("   "), "unknown");
        assert_eq!(sanitize_series_label_value(&"é".repeat(70)).len(), 64);
        let mut m = std::collections::BTreeMap::new();
        m.insert("stream".to_string(), "inter".to_string());
        assert_eq!(check_series_selector_labels(&m), None);
        m.insert("x".into(), "a\"}".into());
        assert_eq!(
            check_series_selector_labels(&m).map(|c| c.0),
            Some("series_label_value_invalid")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noms_de_metriques_assainis() {
        assert_eq!(sanitize_metric_name("soil-moisture"), "soil_moisture");
        assert_eq!(sanitize_metric_name("temp extérieure"), "temp_ext_rieure");
        assert_eq!(sanitize_metric_name("2ph"), "_ph");
        assert_eq!(sanitize_metric_name(""), "m");
    }

    #[test]
    fn noms_valides_et_labels_devices() {
        assert!(valid_metric_name("soil_moisture"));
        assert!(valid_metric_name("d1"));
        assert!(!valid_metric_name("soil-moisture"));
        assert!(!valid_metric_name(""));
        assert!(valid_device_label("fuzzy-zebra"));
        assert!(valid_device_label("a.b_c-d"));
        assert!(!valid_device_label(""));
        assert!(!valid_device_label("deux mots"));
        assert!(!valid_device_label(&"x".repeat(129)));
        assert!(valid_device_label(&"x".repeat(128)));
    }

    #[test]
    fn cles_payload_devices() {
        assert_eq!(sanitize_key("capteur-1"), "capteur_1");
        assert_eq!(sanitize_key("D1"), "D1");
        assert_eq!(sanitize_key("2hab"), "_hab");
        assert_eq!(sanitize_key(""), "k");
        assert_eq!(device_payload_key("capteur-1", "D1"), "capteur_1_D1");
        assert_eq!(device_payload_key("a", "b"), "a_b");
        // Unicité vérifiée en validation : deux lectures aux clés égales.
        assert_eq!(
            device_payload_key("a-b", "c"),
            device_payload_key("a", "b-c")
        );
    }

    #[test]
    fn metriques_etl_prefixees() {
        assert_eq!(etl_metric_name("moyenne_serre"), "etl_moyenne_serre");
        // Idempotent : l'utilisateur peut avoir déjà tapé le préfixe.
        assert_eq!(etl_metric_name("etl_temp"), "etl_temp");
        assert_eq!(etl_metric_name("Temp extérieure!"), "etl_temp_ext_rieure_");
        assert_eq!(etl_metric_name(""), "etl_m");
    }

    #[cfg(feature = "naming")]
    #[test]
    fn normalisation_mesures() {
        assert_eq!(normalize_measurement_name("Soil-Moisture"), "soil_moisture");
        assert_eq!(normalize_measurement_name("soil moisture"), "soil_moisture");
        // Accent folding (D16 contract): é→e,
        // °C→degc — pas un filtrage naïf des caractères non-ASCII.
        assert_eq!(
            normalize_measurement_name("Température Extérieure"),
            "temperature_exterieure"
        );
        assert_eq!(
            normalize_measurement_name("  Température °C "),
            "temperature_degc"
        );
        assert_eq!(normalize_measurement_name("---"), "");
    }
}
