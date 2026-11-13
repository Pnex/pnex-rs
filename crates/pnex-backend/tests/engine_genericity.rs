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
        "finding",
        "result of a watch check, ok | unknown | finding (network-check.md D426)",
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
        "category of a time range read as a maintenance window by the causal chains (cases-workspace.md D904)",
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
            "statement",
        ],
        "case workspace built on the investigation pack (cases.md); a retained causal chain writes a hypothesis and its claims (D909)",
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
        "the fabric projects what it scans into its own pack (agent-environments.md D708, D728)",
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
        ],
        "the fabric projects its ended runs into its own pack (agent-environments.md D708, D761)",
    ),
    (
        "pnex-backend/src/services/profiles.rs",
        &["person", "identifier", "uses_identifier"],
        "browser-agent profiles write investigation identities",
    ),
    (
        "pnex-backend/src/services/person_match.rs",
        &["person", "uses_identifier", "employs", "subsidiary_of", "establishment_of"],
        "the person match reads who works where (investigation employs, companies group and sites) and writes investigation identities (D788); to move into keys the prospecting pack declares",
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
        &["macro_series", "instrument", "paper_position", "market_event", "geo_event"],
        "object tabs of the markets pack (markets-research.md D541) and the session tab of its moves and world events (D738)",
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
        "market services built on the markets and world_events packs: detector, session, map, confirmation (markets-research.md D734-D743); subsidiaries of a 10-K's Exhibit 21 written as companies-pack organizations (F6); SEC companies matched to their LEI by proposal (D812); prediction markets collector, dated resolutions, model inputs and early bet detector built on the prediction_markets pack (appendix J, D918-D924); public trading accounts: imported trades, their account objects and links, entry timing detector built on the trading_accounts pack (appendix K, D926-D933)",
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

/// Data sources and sites named by the engine, frozen: each line leaves with
/// the lot that moves it to a pipeline of a pack (pipeline-engine.md D951,
/// lots PE2-PE9). (file, hosts, reason). A new one fails the guard; one that
/// is gone must be removed from here.
const HOST_DEBT: &[(&str, &[&str], &str)] = &[
    (
        "pnex-backend/src/services/ingest/resolve.rs",
        &["www.data.gouv.fr"],
        "catalogue mode of source_resolve (PE4)",
    ),
    (
        "pnex-core/src/markets/prediction.rs",
        &[
            "clob.polymarket.com",
            "data-api.polymarket.com",
            "gamma-api.polymarket.com",
        ],
        "prediction markets collector (PE4)",
    ),
    (
        "pnex-backend/src/services/org_resolve.rs",
        &["recherche-entreprises.api.gouv.fr"],
        "company lookup of a flow node, to become a declared connector (external-apps.md D441)",
    ),
    (
        "pnex-core/src/browser_agent.rs",
        &[
            "www.linkedin.com",
            "x.com",
            "www.facebook.com",
            "www.instagram.com",
            "www.tiktok.com",
            "www.youtube.com",
            "video.twimg.com",
        ],
        "profile networks of the browser agents, to become rules of a pack (PE5)",
    ),
    (
        "pnex-frontend/src/pages/ontology/profile_capture.rs",
        &["www.linkedin.com"],
        "placeholder of the profile capture form (PE5)",
    ),
    (
        "pnex-extension/src/",
        &["www.tiktok.com", "www.youtube.com", "x.com"],
        "site adapters of the extension, to become rules (agent-unification.md D780)",
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
    let (mut found, mut used) = (Vec::new(), BTreeSet::new());
    for (rel, text) in engine_sources() {
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for host in hosts_of(line) {
                if reserved(&host) || ENGINE_HOSTS.iter().any(|(h, _)| *h == host) {
                    continue;
                }
                match HOST_DEBT
                    .iter()
                    .position(|(file, hs, _)| rel.starts_with(file) && hs.contains(&host.as_str()))
                {
                    Some(i) => {
                        used.insert((i, host));
                    }
                    None => found.push(format!("{rel}:{}: {host}", n + 1)),
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "data source hosts in the engine (D951): declare the source in a pipeline of a pack:\n{}",
        found.join("\n")
    );
    let gone: Vec<String> = HOST_DEBT
        .iter()
        .enumerate()
        .flat_map(|(i, (file, hs, _))| {
            let used = &used;
            hs.iter()
                .filter(move |h| !used.contains(&(i, h.to_string())))
                .map(move |h| format!("{file}: {h}"))
        })
        .collect();
    assert!(
        gone.is_empty(),
        "hosts no longer in the engine: remove them from HOST_DEBT:\n{}",
        gone.join("\n")
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
