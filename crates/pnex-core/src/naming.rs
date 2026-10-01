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

/// Normalisation des labels de mesures (D16) — déplacée de
/// `pnex-backend/src/controllers/ws_ingest.rs` (une seule vérité : l'ingest
/// et la lecture device du runtime doivent produire le même nom de série).
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
        // Pliage des accents (contrat D16 hérité de ws_ingest.rs) : é→e,
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
