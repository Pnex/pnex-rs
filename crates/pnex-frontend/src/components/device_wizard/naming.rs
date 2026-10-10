//! Random device identifier generation (adjective-noun shuffle, in the style
//! of unique-names-generator) and the catalog multi-field search matcher.

// ── Identifier generator (adjective-noun shuffle) ──

const ADJECTIVES: [&str; 24] = [
    "amber", "brave", "calm", "dusk", "eager", "fuzzy", "gentle", "happy", "icy", "jolly", "keen",
    "lucky", "mellow", "nimble", "olive", "proud", "quiet", "rapid", "silly", "tidy", "urban",
    "vivid", "witty", "young",
];
const ANIMALS: [&str; 24] = [
    "otter", "falcon", "ibex", "lynx", "marmot", "newt", "osprey", "puffin", "quokka", "robin",
    "serval", "tapir", "urchin", "viper", "walrus", "yak", "zebra", "badger", "crane", "dolphin",
    "ermine", "ferret", "gannet", "heron",
];

/// Identifiant kebab-case aléatoire (adjectif-animal, ≤ 16 chars).
pub(super) fn random_device_id() -> String {
    let mut bytes = [0u8; 2];
    let _ = getrandom::getrandom(&mut bytes);
    let raw = format!(
        "{}-{}",
        ADJECTIVES[(bytes[0] as usize) % ADJECTIVES.len()],
        ANIMALS[(bytes[1] as usize) % ANIMALS.len()],
    );
    truncate_chars(&raw, 16)
}

/// Troncature respectant les frontières UTF-8.
fn truncate_chars(input: &str, max: usize) -> String {
    match input.char_indices().nth(max) {
        Some((idx, _)) => input[..idx].to_string(),
        None => input.to_string(),
    }
}

/// Recherche multi-champs du catalogue (insensible à la casse).
pub(super) fn model_matches(pd: &pnex_core::PredefinedDevice, term: &str) -> bool {
    let haystack = format!(
        "{} {} {} {} {} {}",
        pd.name,
        pd.pretty_name.as_deref().unwrap_or_default(),
        pd.description.as_deref().unwrap_or_default(),
        pd.device_type,
        pd.board,
        pd.capabilities.join(" "),
    )
    .to_lowercase();
    haystack.contains(term)
}
