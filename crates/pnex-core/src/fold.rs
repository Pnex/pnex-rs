//! Accent- and case-insensitive text (crm.md D447): one folding table for
//! the Rust side (fingerprints, search patterns, page texts) and for SQL
//! (`translate(lower(col), …)`), so « Élodie », « elodie » and « ÉLODIE »
//! compare equal everywhere without a database extension. Pure, wasm-safe.

/// Lowercase Latin letters with diacritics → ASCII (`é` → `e`, `œ` → `oe`).
const FOLDS: &[(&str, &str)] = &[
    ("àáâãäåāăą", "a"),
    ("æ", "ae"),
    ("çćč", "c"),
    ("ďđ", "d"),
    ("èéêëēėęě", "e"),
    ("ìíîïīį", "i"),
    ("ł", "l"),
    ("ñńň", "n"),
    ("òóôõöøōő", "o"),
    ("œ", "oe"),
    ("ř", "r"),
    ("śšş", "s"),
    ("ß", "ss"),
    ("ťţ", "t"),
    ("ùúûüūůű", "u"),
    ("ýÿ", "y"),
    ("źżž", "z"),
    // Typographic apostrophe, as typed by word processors.
    ("\u{2019}", "'"),
];

fn folded(c: char) -> Option<&'static str> {
    FOLDS
        .iter()
        .find(|(from, _)| from.contains(c))
        .map(|(_, to)| *to)
}

/// Lower case, Latin accents folded; other characters kept.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match folded(c) {
            Some(to) => out.push_str(to),
            None => out.push(c),
        }
    }
    out
}

/// Folded words of a text, split on every non-alphanumeric character
/// (apostrophes and hyphens included).
pub fn fold_words(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The folding table as SQL pieces: the ligatures for `replace`, then the
/// one-to-one letters for `translate`. Upper-case letters are included
/// (`lower` leaves them under a `C` collation).
pub struct SqlFold {
    pub ligatures: Vec<(String, &'static str)>,
    pub from: String,
    pub to: String,
}

pub fn sql_fold() -> SqlFold {
    let mut f = SqlFold {
        ligatures: Vec::new(),
        from: String::new(),
        to: String::new(),
    };
    for (chars, ascii) in FOLDS {
        for c in chars.chars() {
            let mut variants = vec![c];
            let up: Vec<char> = c.to_uppercase().collect();
            // `ß` upper-cases to « SS »: never a key of the table.
            if let [u] = up[..] {
                if u != c {
                    variants.push(u);
                }
            }
            for v in variants {
                if ascii.chars().count() == 1 {
                    f.from.push(v);
                    f.to.push_str(ascii);
                } else {
                    f.ligatures.push((v.to_string(), ascii));
                }
            }
        }
    }
    f
}

/// The SQL form of [`fold`] for one column expression (a trusted SQL
/// fragment, never user text).
pub fn sql(expr: &str) -> String {
    let f = sql_fold();
    let quote = |s: &str| s.replace('\'', "''");
    let mut out = format!("lower({expr})");
    for (v, ascii) in &f.ligatures {
        out = format!("replace({out}, '{v}', '{ascii}')");
    }
    format!("translate({out}, '{}', '{}')", quote(&f.from), quote(&f.to))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_and_case_fold_to_one_form() {
        assert_eq!(fold("Élodie"), "elodie");
        assert_eq!(fold("ÉLODIE"), fold("elodie"));
        assert_eq!(fold("Cœur l’Ours"), "coeur l'ours");
        assert_eq!(fold("Zoë Łukasz"), "zoe lukasz");
        assert_eq!(fold("東京"), "東京");
    }

    #[test]
    fn words_split_on_separators() {
        assert_eq!(
            fold_words("Cœur-de-Bœuf, ÇA!"),
            ["coeur", "de", "boeuf", "ca"]
        );
        assert!(fold_words(" -- ").is_empty());
    }

    #[test]
    fn sql_covers_upper_case_and_ligatures() {
        let s = sql("o.title");
        assert!(s.starts_with("translate("), "{s}");
        assert!(s.contains("lower(o.title)"));
        assert!(s.contains("'œ', 'oe'") && s.contains("'Œ', 'oe'") && s.contains("'ß', 'ss'"));
        assert!(!s.contains("'S', 'ss'"), "ß upper-cases to two letters");
        // One character to one: both sides of `translate` have the same length.
        let f = sql_fold();
        assert_eq!(f.from.chars().count(), f.to.chars().count());
        assert!(f.from.contains('É') && f.from.contains('é'));
        // The apostrophe is quoted for SQL.
        assert!(s.ends_with("'''')") || s.contains("''"), "{s}");
    }
}
