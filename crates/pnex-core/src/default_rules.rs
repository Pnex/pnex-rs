//! Navigation rules shipped with PneX (agent-unification.md D780, lot U4):
//! what used to be mission types coded in the agent is now a rule like any
//! other, played by the same engines. Each rule is YAML, parsed and checked
//! by the backend like an org's own (tested there); the server fills its
//! parameters and acts on the run's outcome by the purpose it started it
//! for, never by a kind of mission. Pure, wasm-safe.

/// Screens scrolled on an employee list before reading it (D779): the cards
/// a reader would see, never the whole list.
pub const PEOPLE_SCROLLS: usize = 5;

/// Site of a profile network (D333): the host its canonical profile
/// addresses carry ([`crate::browser_agent::profile_of`]).
pub fn network_site(network: &str) -> Option<&'static str> {
    Some(match network {
        "linkedin" => "www.linkedin.com",
        "x" => "x.com",
        "facebook" => "www.facebook.com",
        "instagram" => "www.instagram.com",
        "tiktok" => "www.tiktok.com",
        "youtube" => "www.youtube.com",
        _ => return None,
    })
}

/// Capture of one profile page in the user's session (D333): the page is
/// opened, two screens scrolled, its text kept as `profile`. A redirect to
/// a sign-in or a check leaves the profile address: the assertion fails
/// the run instead of keeping that page. Params: `url` (canonical), `path`
/// (its path, checked after the load).
pub fn profile_capture(network: &str) -> Option<String> {
    let site = network_site(network)?;
    Some(format!(
        "name: \"Profile capture · {network}\"\n\
         site: \"{site}\"\n\
         session: \"required\"\n\
         params: [url, path]\n\
         steps:\n\
         \x20 - open: \"{{{{url}}}}\"\n\
         \x20 - wait: {{ms: 2500}}\n\
         \x20 - assert: {{url_contains: \"{{{{path}}}}\"}}\n\
         \x20 - scroll: \"bottom\"\n\
         \x20 - wait: {{ms: 2000}}\n\
         \x20 - extract: {{all_text: true, as: \"profile\"}}\n"
    ))
}

/// Capture of a company's employee list on LinkedIn (D779): the first
/// screens of cards at a reader's pace, the text kept as `people`.
/// Param: `url` (canonical `/company/<name>/people/`).
pub fn people_capture() -> String {
    let mut out = String::from(
        "name: \"Employee list · linkedin\"\n\
         site: \"www.linkedin.com\"\n\
         session: \"required\"\n\
         params: [url]\n\
         steps:\n\
         \x20 - open: \"{{url}}\"\n\
         \x20 - wait: {ms: 3000}\n\
         \x20 - assert: {url_contains: \"/people\"}\n",
    );
    for _ in 0..PEOPLE_SCROLLS {
        out += "  - scroll: \"bottom\"\n  - wait: {ms: 3000}\n";
    }
    out += "  - extract: {all_text: true, as: \"people\"}\n";
    out
}

/// Search of a person through LinkedIn's search bar (D788): the query typed
/// where a person types it, the People results kept as cards (the whole
/// card text and its profile link), and the first profiles opened, each
/// text kept as `profile.<n>`. Param: `query`. The selectors aim at what a
/// reader sees (a list item holding a profile link), not at the network's
/// class names; a rule copied into the org can be adjusted in the studio.
pub fn person_search() -> String {
    "name: \"Person search · linkedin\"\n\
     site: \"www.linkedin.com\"\n\
     session: \"required\"\n\
     params: [query]\n\
     steps:\n\
     \x20 - open: \"/feed/\"\n\
     \x20 - wait: {ms: 2500}\n\
     \x20 - type: {text: \"{{query}}\"}\n\
     \x20 - press: \"Enter\"\n\
     \x20 - wait: {ms: 3000}\n\
     \x20 - click: {role: \"button\", name: \"People\", optional: true}\n\
     \x20 - click: {role: \"button\", name: \"Personnes\", optional: true}\n\
     \x20 - wait: {ms: 3000}\n\
     \x20 - extract: {cards: {selector: \"main li:has(a[href*='/in/'])\", max: 5, fields: {card: {}, link: {selector: \"a[href*='/in/']\", attr: \"href\"}}}, as: \"results\", optional: true}\n\
     \x20 - for_each: {from: \"results\", link: \"link\", max: 3, steps: [{wait: {ms: 2500}}, {scroll: \"bottom\"}, {wait: {ms: 1500}}, {extract: {all_text: true, as: \"profile\"}}]}\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_network_has_a_capture_rule() {
        for n in crate::browser_agent::PROFILE_NETWORKS {
            let y = profile_capture(n).expect("rule");
            assert!(y.contains("session: \"required\""), "{y}");
            assert!(y.contains("{{url}}") && y.contains("{{path}}"), "{y}");
        }
        assert!(profile_capture("myspace").is_none());
        assert_eq!(people_capture().matches("scroll").count(), PEOPLE_SCROLLS);
        assert!(person_search().contains("type: {text: \"{{query}}\"}"));
    }
}
