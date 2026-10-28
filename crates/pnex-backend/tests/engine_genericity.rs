//! The engine knows no business (external-apps.md D432, D441): no key of
//! a shipped pack, of its object types or of its link types appears as a
//! string literal in the non-test code of `crates/*/src`. Business logic
//! lives in packs and in applications.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Engine words that a pack happens to use as a key too: allowed anywhere.
const ENGINE_WORDS: &[(&str, &str)] = &[
    ("media", "media library, routes and ingestion"),
    ("site", "site recipes and crawls name the website they read"),
    ("line", "chart widget kind, error line numbers"),
    ("controls", "surfaces controls (D123)"),
    ("about", "about: URLs in the browser guard"),
    ("before", "the journal's before/after documents"),
    (
        "meeting",
        "mail category of a calendar invitation (mail/invitations.rs)",
    ),
];

/// Existing couplings to a shipped pack, frozen: each is debt to move to
/// the pack or to a declaration (external-apps.md §11). (file, keys, reason).
const COUPLED: &[(&str, &[&str], &str)] = &[
    (
        "pnex-backend/src/services/ontology/cases/",
        &[
            "person",
            "organization",
            "place",
            "hypothesis",
            "claim",
            "supports",
            "contradicts",
            "sourced_by",
            "piece",
            "identifier",
        ],
        "case workspace built on the investigation pack (cases.md)",
    ),
    (
        "pnex-frontend/src/pages/cases/",
        &[
            "person",
            "organization",
            "place",
            "hypothesis",
            "claim",
            "statement",
            "party",
            "piece",
            "identifier",
            "owns",
        ],
        "case workspace built on the investigation pack (cases.md)",
    ),
    (
        "pnex-backend/src/services/ontology/objects.rs",
        &["claim"],
        "claims of the investigation pack keep a status rule",
    ),
    (
        "pnex-core/src/ontology/mod.rs",
        &["hypothesis"],
        "event attachment role (K21), also an investigation type",
    ),
    (
        "pnex-backend/src/services/profiles.rs",
        &["person", "identifier", "uses_identifier"],
        "browser-agent profiles write investigation identities",
    ),
    (
        "pnex-backend/src/services/mail/invitations.rs",
        &["identifier", "uses_identifier"],
        "meeting attendees matched by the investigation pack's e-mail identifiers (mail.md D466)",
    ),
    (
        "pnex-backend/src/services/media_attributions.rs",
        &["appears_in"],
        "media attributions write an investigation link",
    ),
    (
        "pnex-backend/src/services/extension.rs",
        &["published_by"],
        "collected videos link their channel (investigation)",
    ),
    (
        "pnex-frontend/src/pages/media/review.rs",
        &["person", "statement", "said_by"],
        "media review writes investigation statements",
    ),
    (
        "pnex-frontend/src/pages/browser_agents.rs",
        &["person", "organization"],
        "browser-agent profiles pick an investigation identity",
    ),
    (
        "pnex-core/src/ontology/api.rs",
        &["knows", "recommended", "employs", "manages", "member_of"],
        "introduction paths of account mapping (D329) name pack links",
    ),
    (
        "pnex-core/src/voice_id.rs",
        &["person"],
        "voice profiles attach to an investigation person",
    ),
    ("pnex-core/src/vision.rs", &["person"], "COCO class name"),
    (
        "pnex-backend/src/services/device_liveness.rs",
        &["claim"],
        "device session lease, not the investigation claim",
    ),
];

fn pack_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for pack in pnex_backend::services::ontology::packs::shipped() {
        keys.insert(pack.key);
        keys.extend(pack.types.into_iter().map(|t| t.key));
        keys.extend(pack.link_types.into_iter().map(|l| l.key));
    }
    keys
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The code of a file before its first test module.
fn non_test(text: &str) -> &str {
    text.find("#[cfg(test)]").map_or(text, |i| &text[..i])
}

#[test]
fn engine_names_no_pack_type_or_link() {
    let keys = pack_keys();
    assert!(keys.len() > 20, "shipped packs not read: {keys:?}");
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in std::fs::read_dir(&crates).unwrap().flatten() {
        rust_files(&c.path().join("src"), &mut files);
    }
    assert!(files.len() > 100, "sources not found under {crates:?}");
    let mut found = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&crates)
            .unwrap()
            .to_string_lossy()
            .to_string();
        if rel.ends_with("/tests.rs") || rel.contains("/tests/") {
            continue;
        }
        let text = std::fs::read_to_string(f).unwrap();
        for (n, line) in non_test(&text).lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            for k in &keys {
                if code.contains(&format!("\"{k}\""))
                    && !ENGINE_WORDS.iter().any(|(w, _)| w == k)
                    && !COUPLED
                        .iter()
                        .any(|(file, ks, _)| rel.starts_with(file) && ks.contains(&k.as_str()))
                {
                    found.push(format!("{rel}:{}: \"{k}\"", n + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "pack names in the engine (D441): move this to the pack, or add an exception with its reason:\n{}",
        found.join("\n")
    );
}
