//! Lexicon guard (design-system.md §7, D218–D224): one word per concept.
//! Scans the values of both locales and the assistant knowledge cards for
//! words that name another concept (« label » for an étiquette, « couche »
//! for an annotation layer, « Audio streams » for the streams page…).
//! A new string that drifts fails here; extend `RULES` when the lexicon
//! gains a concept, and justify every exemption.

#![cfg(test)]

use std::path::PathBuf;

/// Where a rule applies.
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Fr,
    En,
    /// English locale and the assistant cards (written in English).
    EnAndCards,
}

struct Rule {
    scope: Scope,
    /// Lowercase word or phrase, matched on word boundaries.
    term: &'static str,
    /// The lexicon word to use instead.
    use_instead: &'static str,
    /// Fluent keys (prefixes) where the term is legitimate, with the reason
    /// in a comment next to the rule.
    allowed_keys: &'static [&'static str],
}

const RULES: &[Rule] = &[
    // iam.md §3: members grouped for access are a « team » (« Groupe »
    // already names cold groups, company groups, node groups), and a role
    // given to a member or a team is an « attribution ».
    Rule {
        scope: Scope::Fr,
        term: "groupe d'utilisateurs",
        use_instead: "équipe (iam.md §3)",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "user group",
        use_instead: "team (iam.md §3)",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "affectation",
        use_instead: "attribution (iam.md §3)",
        allowed_keys: &[],
    },
    // D255: the extension that runs collection missions is an « agent
    // navigateur », never a « collecteur » (no collector domain, D254).
    Rule {
        scope: Scope::Fr,
        term: "collecteur",
        use_instead: "agent navigateur (D255)",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "collecteurs",
        use_instead: "agents navigateur (D255)",
        allowed_keys: &[],
    },
    // The storyline view of a case is the « Histoire » (its tab name).
    Rule {
        scope: Scope::Fr,
        term: "storyline",
        use_instead: "histoire (vue d'un dossier)",
        allowed_keys: &[],
    },
    // D218: free classification of an object is an « étiquette ». CSV/JSON
    // column and field names quoted to the user stay as they are.
    Rule {
        scope: Scope::Fr,
        term: "label",
        use_instead: "étiquette (D42), dimension (série), classe (modèle), libellé (nom affiché)",
        allowed_keys: &[
            "ranges-import-help",
            "err-time-range-import-invalid",
            "flows-range-upsert-help",
        ],
    },
    Rule {
        scope: Scope::Fr,
        term: "labels",
        use_instead: "étiquettes (D42), dimensions (série), classes (modèle)",
        allowed_keys: &[],
    },
    // D219: the versioned unit of annotations is a « calque ». Neural
    // network layers (training help) are not annotations.
    Rule {
        scope: Scope::Fr,
        term: "couche",
        use_instead: "calque (d'annotations)",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "couches",
        use_instead: "calques (d'annotations)",
        allowed_keys: &["lab-train-help"],
    },
    Rule {
        scope: Scope::Fr,
        term: "ensemble d'annotations",
        use_instead: "calque d'annotations",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "calque de détection",
        use_instead: "détections",
        allowed_keys: &[],
    },
    // Annotating is what one does on a visual media (annot-*); the Vision
    // Lab « étiquette » its training images.
    Rule {
        scope: Scope::Fr,
        term: "annoter",
        use_instead: "étiqueter (Vision Lab)",
        allowed_keys: &["annot-"],
    },
    Rule {
        scope: Scope::Fr,
        term: "annotez",
        use_instead: "étiquetez (Vision Lab)",
        allowed_keys: &["annot-"],
    },
    Rule {
        scope: Scope::Fr,
        term: "annotée",
        use_instead: "étiquetée (Vision Lab)",
        allowed_keys: &["annot-"],
    },
    Rule {
        scope: Scope::Fr,
        term: "annotées",
        use_instead: "étiquetées (Vision Lab)",
        allowed_keys: &["annot-"],
    },
    // Streams carry audio and/or video: the page is « Flux ».
    Rule {
        scope: Scope::Fr,
        term: "flux audio",
        use_instead: "flux",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "audio streams",
        use_instead: "streams",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "annotation set",
        use_instead: "annotation layer",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "annotation sets",
        use_instead: "annotation layers",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "detection layer",
        use_instead: "recorded detections",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "detection layers",
        use_instead: "recorded detections",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "camera layer",
        use_instead: "recorded detections",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "series labels",
        use_instead: "series dimensions",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::En,
        term: "annotate",
        use_instead: "label (Vision Lab)",
        allowed_keys: &["annot-"],
    },
    Rule {
        scope: Scope::En,
        term: "annotated",
        use_instead: "labeled (Vision Lab)",
        allowed_keys: &["annot-"],
    }, // cases-money.md: money moved by an event is a « versement » shown in
    // the « Argent » view; « flux » is the stream (Flux / Streams).
    Rule {
        scope: Scope::Fr,
        term: "flux financiers",
        use_instead: "versements (vue Argent)",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "flux d'argent",
        use_instead: "versements (vue Argent)",
        allowed_keys: &[],
    },
    // D231: the media of an object are its « galerie »; a skeleton's
    // parts are « articulations ».
    Rule {
        scope: Scope::Fr,
        term: "assets",
        use_instead: "galerie (médias d'un objet)",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "bone",
        use_instead: "articulation",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "paiement",
        use_instead: "versement",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::Fr,
        term: "transaction",
        use_instead: "versement",
        allowed_keys: &[],
    },
    Rule {
        scope: Scope::EnAndCards,
        term: "money flows",
        use_instead: "payments (Money view)",
        allowed_keys: &[],
    },
];

/// Fluent placeables (`{ $label }`, selectors) are code, not words.
fn strip_placeables(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// `term` occurs in `text` (lowercased) on word boundaries.
fn has_term(text: &str, term: &str) -> bool {
    let hay = text.to_lowercase();
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut from = 0;
    while let Some(pos) = hay[from..].find(term) {
        let start = from + pos;
        let end = start + term.len();
        let before = hay[..start].chars().next_back();
        let after = hay[end..].chars().next();
        if !is_word(before) && !is_word(after) {
            return true;
        }
        from = start + term.chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// (key, value) pairs of a locale; continuation lines join their key.
fn entries(ftl: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in ftl.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((key, value)) = line.split_once(" = ") {
            out.push((key.trim().to_string(), value.to_string()));
        }
    }
    out
}

fn applies(rule: &Rule, locale: Scope) -> bool {
    match (rule.scope, locale) {
        (Scope::Fr, Scope::Fr) => true,
        (Scope::En | Scope::EnAndCards, Scope::En) => true,
        (Scope::EnAndCards, Scope::EnAndCards) => true,
        _ => false,
    }
}

fn check_locale(file: &str, locale: Scope, failures: &mut Vec<String>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("locales")
        .join(file);
    let ftl = std::fs::read_to_string(&path).expect("locale file");
    for (key, value) in entries(&ftl) {
        let text = strip_placeables(&value);
        for rule in RULES.iter().filter(|r| applies(r, locale)) {
            if rule.allowed_keys.iter().any(|k| key.starts_with(k)) {
                continue;
            }
            if has_term(&text, rule.term) {
                failures.push(format!(
                    "{file} `{key}`: « {} » → use « {} »",
                    rule.term, rule.use_instead
                ));
            }
        }
    }
}

#[test]
fn locales_follow_the_lexicon() {
    let mut failures = Vec::new();
    check_locale("fr-FR.ftl", Scope::Fr, &mut failures);
    check_locale("en-US.ftl", Scope::En, &mut failures);
    assert!(
        failures.is_empty(),
        "lexicon (design-system.md §7):\n{}",
        failures.join("\n")
    );
}

#[test]
fn assistant_cards_follow_the_lexicon() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pnex-backend/assistant-kb");
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assistant-kb dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("card");
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        for line in text.lines() {
            // Search synonyms of the front matter may list older words.
            if line.starts_with("tags:") {
                continue;
            }
            for rule in RULES.iter().filter(|r| r.scope == Scope::EnAndCards) {
                if has_term(line, rule.term) {
                    failures.push(format!(
                        "{name}: « {} » → use « {} »",
                        rule.term, rule.use_instead
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "lexicon (design-system.md §7):\n{}",
        failures.join("\n")
    );
}

#[test]
fn matcher_respects_word_boundaries_and_placeables() {
    assert!(has_term("Enregistrer les labels", "labels"));
    assert!(!has_term("Libellé", "label"));
    assert!(!has_term(
        &strip_placeables("Supprimer « { $label } » ?"),
        "label"
    ));
    assert!(has_term("Créer une couche", "couche"));
    assert!(has_term("Audio streams page", "audio streams"));
    assert!(!has_term("audio streamsX", "audio streams"));
}
