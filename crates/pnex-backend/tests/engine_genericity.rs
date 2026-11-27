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
    ("watch", "a site's reserved path (/watch) in the browser agent"),
    ("before", "the journal's before/after documents"),
    (
        "finding",
        "result of a watch check, ok | unknown | finding (partners.md D426)",
    ),
    (
        "meeting",
        "mail category of a calendar invitation (mail/invitations.rs)",
    ),
    (
        "quote",
        "block quote of the doc editor, quoted text of a media citation",
    ),
    (
        "agent_run",
        "kind of the flow node that starts a run (D713), not the agent_ops type",
    ),
    (
        "skill",
        "lexicon term of agent skills (D711), not a pack type",
    ),
    (
        "maintenance",
        "category of a time range read as a maintenance window by the causal chains (cases.md D904)",
    ),
];

/// Existing couplings to a shipped pack, frozen: each is debt to move to
/// the pack or to a declaration (external-apps.md §3.6). (file, keys, reason).
const COUPLED: &[(&str, &[&str], &str)] = &[
    (
        "pnex-core/src/ontology/roles.rs",
        &[
            "person",
            "organization",
            "place",
            "identifier",
            "statement",
            "hypothesis",
            "claim",
            "piece",
            "party",
            "uses_identifier",
            "appears_in",
            "said_by",
            "published_by",
            "supports",
            "contradicts",
            "sourced_by",
            "knows",
            "employs",
            "manages",
            "member_of",
            "recommended",
            "owns",
        ],
        "the role vocabulary (D441): packs declare these roles on their types and link types, the engine names only the roles",
    ),
    (
        "pnex-backend/src/services/ontology/cases/",
        &["hypothesis", "statement"],
        "`hypothesis` is also an event attachment role (K21); a retained causal chain fills the `statement` property of the hypothesis and claim roles (D909): property keys are not roles",
    ),
    (
        "pnex-core/src/ontology/mod.rs",
        &["hypothesis"],
        "event attachment role (K21), also an investigation type",
    ),
    (
        "pnex-backend/src/services/software_supply.rs",
        &[
            "software_supply",
            "software_image",
            "software_bom",
            "software_component",
            "software_vulnerability",
            "image_has_bom",
            "bom_lists_component",
            "image_exposed_to",
        ],
        "the fabric projects what it scans into its own pack (agents.md D708, D728)",
    ),
    (
        "pnex-backend/src/services/agent_ops.rs",
        &[
            "agent_ops",
            "agent_environment",
            "hosted_agent",
            "agent_run",
            "agent_file",
            "run_of_agent",
            "run_in_environment",
            "file_produced_by",
            "run_read_file",
            "agent_merge_request",
            "merge_request_of_run",
            "work_item",
            "run_works_on",
            "merge_request_for",
        ],
        "the fabric projects its ended runs into its own pack (agents.md D708, D761, D1055)",
    ),
    ("pnex-core/src/vision.rs", &["person"], "COCO class name"),
    (
        "pnex-backend/src/services/device_liveness.rs",
        &["claim"],
        "device session lease, not the investigation claim",
    ),
    (
        "pnex-recorder/src/calls.rs",
        &["activity"],
        "CSS class of the recorder's panel, not the CRM activity",
    ),
    (
        "pnex-backend/src/services/profiles.rs",
        &["lead", "list_contact"],
        "the retention queue also proposes what names a person before the graph knows them: a lead, a list contact (crm.md D514, D610)",
    ),
    (
        "pnex-pipeline-worker/src/engine.rs",
        &["lead"],
        "SQL window function allowed in a pipeline, not the CRM lead",
    ),
    (
        "pnex-frontend/src/pages/ontology/object.rs",
        &["macro_series", "instrument", "paper_position", "market_event", "geo_event", "work_order"],
        "object tabs of the markets pack (markets-research.md D541) and the session tab of its moves and world events (D738); the parts export of a maintenance work order (parts-docs.md PD1)",
    ),
    (
        "pnex-frontend/src/pages/equipment_parts.rs",
        &["instance_of", "bom_line", "work_order", "work_order_on", "uses_part"],
        "parts list of an installed equipment and its work order basket, built on the maintenance pack (parts-docs.md PD1, Q-PD4)",
    ),
    (
        "pnex-frontend/src/pages/field_note.rs",
        &["field_note"],
        "field note form on an object page, shown when the org has the maintenance pack's type (parts-docs.md PD1d)",
    ),
    (
        "pnex-frontend/src/state/field_notes.rs",
        &["field_note", "note_on", "note_photo"],
        "offline queue of field notes, sent as proposals of the maintenance pack with their links (parts-docs.md PD1d)",
    ),
    (
        "pnex-frontend/src/pages/parts.rs",
        &["part_reference", "refers_to", "supersedes"],
        "spare parts search page built on the maintenance pack (parts-docs.md PD0, D961): its references, what they designate and the supersessions; a pack-declared reference search would lift it",
    ),
    (
        "pnex-backend/src/controllers/markets.rs",
        &["markets", "paper_position"],
        "market routes built on the markets pack: OpenAPI tag, paper positions",
    ),
    (
        "pnex-frontend/src/api/markets.rs",
        &["instrument"],
        "the `instrument` query parameter of the model decisions (markets-research.md D894), a column of the price tables, not the pack type",
    ),
    (
        "pnex-backend/src/controllers/market_decisions.rs",
        &["markets", "instrument"],
        "bars and decisions of an external model (markets-research.md D892, D894): OpenAPI tag, and `instrument` the column of the price tables, not the pack type",
    ),
    (
        "pnex-backend/src/services/markets/",
        &[
            "instrument",
            "filing",
            "filing_document",
            "macro_series",
            "market_event",
            "event_about",
            "geo_event",
            "organization",
            "subsidiary_of",
            "filed_by",
            "same_company_as",
            "prediction_markets",
            "prediction_market",
            "prediction_resolution",
            "bears_on",
            "wallet",
            "traded",
            "trading_accounts",
            "trading_account",
            "trades",
        ],
        "market services built on the markets and world_events packs: detector, session, map, confirmation (markets-research.md D734-D743); subsidiaries of a 10-K's Exhibit 21 written as companies-pack organizations (F6); SEC companies matched to their LEI by proposal (D812); dated resolutions, model inputs and early bet detector built on the prediction_markets pack (appendix J, D918-D924); public trading accounts: imported trades, their account objects and links, entry timing detector built on the trading_accounts pack (appendix K, D926-D933)",
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

/// Non-test code of every crate: (path under `crates/`, text).
fn engine_sources() -> Vec<(String, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in std::fs::read_dir(&crates).unwrap().flatten() {
        rust_files(&c.path().join("src"), &mut files);
    }
    assert!(files.len() > 100, "sources not found under {crates:?}");
    files
        .iter()
        .filter_map(|f| {
            let rel = f
                .strip_prefix(&crates)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if rel.ends_with("/tests.rs") || rel.contains("/tests/") {
                return None;
            }
            let text = std::fs::read_to_string(f).unwrap();
            Some((rel, non_test(&text).to_string()))
        })
        .collect()
}

#[test]
fn engine_names_no_pack_type_or_link() {
    let keys = pack_keys();
    assert!(keys.len() > 20, "shipped packs not read: {keys:?}");
    let mut found = Vec::new();
    for (rel, text) in engine_sources() {
        for (n, line) in text.lines().enumerate() {
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

/// Hosts the engine itself talks to or cites, whatever the org's data: a
/// transport, a format namespace, a documentation link.
const ENGINE_HOSTS: &[(&str, &str)] = &[
    ("www.w3.org", "SVG and XML namespaces"),
    ("schemas.openxmlformats.org", "Office document namespaces"),
    ("ns.adobe.com", "XMP namespace of a panorama"),
    ("ns.google.com", "photo sphere namespace of a panorama"),
    (
        "api.anthropic.com",
        "default address of an AI provider kind",
    ),
    ("api.telegram.org", "notification channel kind"),
    ("discord.com", "notification channel kind"),
    ("discordapp.com", "notification channel kind"),
    ("hooks.slack.com", "notification channel kind"),
    ("ntfy.sh", "notification channel kind"),
    ("google.serper.dev", "web search provider kind"),
    ("api.met.no", "provider of the weather node"),
    ("api.open-meteo.com", "provider of the weather node"),
    (
        "github.com",
        "forge kind of an agent run, documentation links",
    ),
    (
        "bellard.org",
        "documentation link of the functions reference",
    ),
    (
        "developer.mozilla.org",
        "documentation link of the functions reference",
    ),
];

/// Hosts written as `"http(s)://host…` on a line.
// shortcut: a host written without its scheme is not seen; scan bare host
// literals too if one slips through.
fn hosts_of(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for scheme in ["\"https://", "\"http://"] {
        for (i, _) in line.match_indices(scheme) {
            let host: String = line[i + scheme.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            out.push(host.trim_end_matches('.').to_ascii_lowercase());
        }
    }
    out
}

/// Not a public host: a placeholder, a local name, an address, PneX itself.
fn reserved(host: &str) -> bool {
    !host.contains('.')
        || host.parse::<std::net::IpAddr>().is_ok()
        || host == "pnex.io"
        || [".example", ".invalid", ".test", ".local", ".home"]
            .iter()
            .any(|s| host.ends_with(s))
        || ["example.com", "example.org", "example.net"]
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

#[test]
fn engine_names_no_data_source_host() {
    let mut found = Vec::new();
    for (rel, text) in engine_sources() {
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for host in hosts_of(line) {
                if reserved(&host) || ENGINE_HOSTS.iter().any(|(h, _)| *h == host) {
                    continue;
                }
                found.push(format!("{rel}:{}: {host}", n + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "data source hosts in the engine (D951): declare the source in a pipeline of a pack:\n{}",
        found.join("\n")
    );
}

#[test]
fn hosts_are_read_and_sorted() {
    assert_eq!(
        hosts_of(r#"get("https://Data.Example.io/a", "http://10.0.0.1:8080")"#),
        ["data.example.io", "10.0.0.1"]
    );
    assert!(reserved("10.0.0.1") && reserved("api.example.com") && reserved("localhost"));
    assert!(reserved("pnex.local") && !reserved("data.example.io"));
}
