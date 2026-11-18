//! The person search functions of the `prospecting_b2b` pack (pipeline-engine.md
//! D999), run in the sandbox the flows use: the subject built from the graph,
//! the score of what a search kept, and the follow-up of a human decision.
#![cfg(feature = "starlark")]

use pnex_core::starlark_sandbox::execute_with;
use pnex_core::FunctionSignature;
use serde_json::{json, Map, Value};

const PACK: &str = include_str!("../../pnex-backend/packs/prospecting_b2b.yaml");

/// The code of a pack function, read from its `code: |` block.
fn code_of(name: &str) -> String {
    let start = PACK
        .find(&format!("  - name: {name}\n"))
        .unwrap_or_else(|| panic!("{name} not in the pack"));
    let rest = &PACK[start..];
    let at = rest.find("    code: |\n").expect("a code block") + "    code: |\n".len();
    rest[at..]
        .lines()
        .take_while(|l| l.is_empty() || l.starts_with("      "))
        .map(|l| l.get(6..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(name: &str, msg: Value) -> Value {
    let settings: Map<String, Value> = Map::new();
    execute_with(
        &code_of(name),
        &FunctionSignature::default(),
        &msg,
        2_000,
        &settings,
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
    .return_json
    .unwrap_or(Value::Null)
}

fn link(direction: &str, title: &str, properties: Value) -> Value {
    json!({"direction": direction, "valid_from": "2026-01-01T00:00:00Z",
           "other": {"id": format!("id-{title}"), "title": title, "properties": properties}})
}

#[test]
fn the_subject_reads_company_group_and_site_cities() {
    let out = run(
        "pm_subject",
        json!({
            "person_id": "p1", "person_name": "Élodie Martin",
            "object": {"id": "o1", "title": "ACME INDUSTRIES SAS"},
            "payload": {"press_names": "ACME Group"},
            "links": {
                "subsidiary_of": [link("out", "Globex Holding", json!({}))],
                "establishment_of": [link("in", "site", json!({"address": "10 RUE X 31000 TOULOUSE", "head_office": true}))],
            },
        }),
    );
    let row = &out["payload"];
    assert_eq!(row["query"], "elodie martin acme industries");
    assert_eq!(
        (row["step"].as_str(), row["words"].as_str()),
        (Some("0"), Some("elodie martin"))
    );
    let subject: Value = serde_json::from_str(out["subject"].as_str().unwrap()).unwrap();
    let queries: Vec<&str> = subject["queries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        queries.last(),
        Some(&"elodie martin"),
        "the name alone ends the ladder"
    );
    assert!(
        queries.contains(&"elodie martin globex holding"),
        "{queries:?}"
    );
    assert!(queries.contains(&"elodie martin toulouse"), "{queries:?}");
    assert!(queries.len() <= 5);
}

fn decide(candidates: Value, step: i64) -> Value {
    let subject = json!({"name": ["elodie", "martin"], "orgs": [["acme", "industries"]], "groups": [],
                         "cities": [["toulouse"]], "queries": ["elodie martin acme industries", "elodie martin"]});
    run(
        "pm_decide",
        json!({
            "person": {"id": "p1", "title": "Élodie Martin"},
            "person_props": {"match_subject": subject.to_string()},
            "person_links": {},
            "payload": {"text": json!({"query": "q", "step": step, "candidates": candidates}).to_string()},
        }),
    )
}

#[test]
fn a_profile_at_the_company_is_captured_a_past_one_proposed_none_asks_the_next_query() {
    let at = json!([{"url": "https://www.linkedin.com/in/elodie-martin-1", "card": "Élodie Martin · Head of purchasing at ACME Industries", "profile": "Elodie Martin\nToulouse"}]);
    let out = decide(at, 0);
    assert_eq!(out[0]["payload"]["path"], "/in/elodie-martin-1", "{out}");
    assert_eq!(out[3]["payload"]["match_state"], "found");
    assert_eq!(out[3]["payload"]["match_to_score"], "");

    let filler = "lorem ipsum ".repeat(80);
    let past = json!([{"url": "https://www.linkedin.com/in/elodie-m", "card": "Elodie Martin · Engineer at Initech",
                       "profile": format!("Elodie Martin\nEngineer at Initech\n{filler}\nBuyer · ACME Industries · 2018 - 2022")}]);
    let out = decide(past, 0);
    assert!(out[0].is_null(), "a job change is never attached: {out}");
    assert_eq!(
        out[1]["payload"][0]["url"],
        "https://www.linkedin.com/in/elodie-m"
    );
    assert_eq!(out[3]["payload"]["match_state"], "proposed");

    let out = decide(json!([]), 0);
    assert_eq!(out[2]["payload"]["query"], "elodie martin", "{out}");
    assert_eq!(out[2]["payload"]["step"], "1");
    let out = decide(json!([]), 1);
    assert_eq!(out[3]["payload"]["match_state"], "not_found", "{out}");
}

#[test]
fn a_rejected_proposal_asks_the_next_query_once_none_is_left() {
    let props = |pending: &str| {
        json!({"match_state": "proposed", "match_pending": pending, "match_step": 0,
               "match_subject": json!({"name": ["elodie", "martin"], "queries": ["a", "b"]}).to_string()})
    };
    let msg = |pending: &str, decision: &str| json!({"payload": props(pending), "object": {"id": "p1"}, "url": "https://x/in/a", "decision": decision});
    let out = run(
        "pm_follow",
        msg("https://x/in/a https://x/in/b", "rejected"),
    );
    assert_eq!(
        out[2]["payload"]["match_pending"], "https://x/in/b",
        "{out}"
    );
    let out = run("pm_follow", msg("https://x/in/a", "rejected"));
    assert_eq!(out[1]["payload"]["query"], "b", "{out}");
    let out = run("pm_follow", msg("https://x/in/a", "accepted"));
    assert_eq!(out[0]["payload"]["path"], "/in/a", "{out}");
    assert!(run("pm_follow", msg("https://x/in/z", "accepted")).is_null());
}

#[test]
fn a_search_asked_sets_its_next_date() {
    let out = run(
        "pm_asked",
        json!({"person_id": "p1", "today": "2026-11-18", "step": 0, "subject": "{}"}),
    );
    assert_eq!(out["payload"]["match_next_on"], "2027-02-16", "{out}");
    assert_eq!(out["payload"]["match_state"], "searching");
    let out = run(
        "pm_asked",
        json!({"person_id": "p1", "today": "2024-02-28", "step": 1}),
    );
    assert_eq!(out["payload"]["match_next_on"], "2024-05-28", "{out}");
}
