//! The Starlark rules shipped by the `quali_watch` pack (partner-watch.md
//! D314, D316) and the target score of `prospecting_b2b`
//! (account-mapping.md D336) run as the flow runtime runs them.

use pnex_node_starlark::executor::execute;
use serde_json::json;

const PACK: &str = include_str!("../../pnex-backend/packs/quali_watch.yaml");
const PROSPECTING: &str = include_str!("../../pnex-backend/packs/prospecting_b2b.yaml");
const PRESENCE: &str = include_str!("../../pnex-backend/packs/web_presence.yaml");
const PROSPECTING_FR: &str = include_str!("../../pnex-backend/packs/prospecting_b2b_fr.yaml");
const CRM: &str = include_str!("../../pnex-backend/packs/crm.yaml");

/// The `code: |` block of the `quali_watch` function `name`.
fn code_of(name: &str) -> String {
    code_in(PACK, name)
}

/// The `code: |` block of the pack function `name` (6-space indent).
fn code_in(pack: &str, name: &str) -> String {
    let start = pack
        .find(&format!("- name: {name}\n"))
        .unwrap_or_else(|| panic!("function {name}"));
    let rest = &pack[start..];
    let body = &rest[rest.find("code: |\n").expect("code block") + 8..];
    body.lines()
        .take_while(|l| l.is_empty() || l.starts_with("      "))
        .map(|l| l.get(6..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every Starlark function of every shipped pack evaluates as a module
/// (names resolve at compile time: a dangling one refuses the whole flow).
#[test]
fn every_pack_starlark_function_compiles() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../pnex-backend/packs");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "yaml") {
            continue;
        }
        let pack = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = pack.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let Some(name) = l.strip_prefix("  - name: ") else {
                continue;
            };
            if lines.get(i + 1) != Some(&"    language: starlark") {
                continue;
            }
            pnex_node_starlark::executor::compile_check(&code_in(&pack, name))
                .unwrap_or_else(|e| panic!("{}: {name}: {e}", path.display()));
            checked += 1;
        }
    }
    assert!(checked > 10, "only {checked} functions found");
}

#[test]
fn next_reminder_moves_the_procedure_and_waits_for_a_human_at_r3() {
    let msg = json!({
        "payload": {"level": "r2", "next_due": "2026-12-20", "contact_email": "conformite@of.fr"},
        "object": {"id": "p1", "title": "OF Formation", "version": 3},
        "today": "2026-12-25", "now": "2026-12-25T06:00:00+00:00"
    });
    let out = execute(&code_of("qw_next_reminder"), &msg, 2000).unwrap();
    let r = out.return_json.expect("two messages");
    assert_eq!(r[0]["payload"]["level"], "r3");
    assert_eq!(r[0]["payload"]["next_due"], "2027-01-09");
    assert_eq!(r[0]["payload"]["state"], "awaiting_validation");
    assert_eq!(
        (r[0]["object_id"].clone(), r[0]["expected_version"].clone()),
        (json!("p1"), json!(3))
    );
    assert_eq!(r[1]["payload"]["ref"], "p1:r3");
    assert_eq!(r[1]["payload"]["state"], "to_validate");
    assert_eq!(r[1]["payload"]["recipient"], "conformite@of.fr");
    // r1 → r2 is automatic; r4 is the last reminder.
    let first = json!({"payload": {"level": "r1"}, "object": {"id": "p", "title": "T", "version": 1}, "today": "2028-02-20", "now": "x"});
    let r = execute(&code_of("qw_next_reminder"), &first, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (
            r[0]["payload"]["next_due"].clone(),
            r[1]["payload"]["state"].clone()
        ),
        (json!("2028-03-06"), json!("sent"))
    );
    let last =
        json!({"payload": {"level": "r4"}, "object": {"id": "p", "title": "T", "version": 1}});
    assert!(execute(&code_of("qw_next_reminder"), &last, 2000)
        .unwrap()
        .return_json
        .is_none());
}

#[test]
fn adm_01_finding_and_link_payload() {
    let msg = json!({
        "payload": {"siren": "123456789", "ceased_on": "2026-09-30"},
        "object": {"id": "o1", "title": "OF Fermé", "version": 2},
        "now": "2026-10-19T05:00:00+00:00"
    });
    let r = execute(&code_of("qw_rule_adm_01"), &msg, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["ref"], "ADM-01:123456789");
    assert_eq!(r["payload"]["state"], "proposed");
    assert_eq!(r["payload"]["observed"], "Legal unit ceased on 2026-09-30");
    assert_eq!(r["organization_id"], "o1");
    let linked = json!({"object": {"id": "e1"}, "organization_id": "o1"});
    let l = execute(&code_of("qw_link"), &linked, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(l["payload"], json!({"source_id": "e1", "target_id": "o1"}));
}

#[test]
fn target_score_adds_the_points_of_the_criteria_met() {
    let crit = |title: &str, prop: &str, op: &str, value: &str, points: i64| json!({"title": title, "properties": {"property": prop, "operator": op, "value": value, "points": points}});
    let criteria = json!([
        crit("Industrie visée", "activity_code", "starts_with", "25, 30.3, 21", 30),
        crit("ETI et plus", "headcount_min", "at_least", "1000", 20),
        crit("CA > 10 M€", "last_revenue", "at_least", "10000000", 15),
        crit("Déjà client", "account_status", "equals", "customer", -100),
        crit("Pays", "country", "one_of", "FR,BE", 5),
        {"title": "Inactif", "properties": {"property": "country", "operator": "filled", "points": 99, "active": false}},
    ]);
    let msg = json!({
        "payload": {"activity_code": "25.62", "headcount_min": 1000, "country": "FR",
                    "last_revenue": {"amount": 343838577.0, "currency": "EUR", "estimated": false},
                    "account_status": "target"},
        "object": {"id": "o1", "title": "MECACHROME FRANCE", "version": 4},
        "criteria": criteria,
        "today": "2026-10-22"
    });
    let r = execute(&code_in(PROSPECTING, "pb_score"), &msg, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["target_score"], 70);
    assert_eq!(r["payload"]["scored_on"], "2026-10-22");
    assert_eq!(r["object_id"], "o1");
    let detail = r["payload"]["score_detail"].as_str().unwrap();
    assert_eq!(detail.lines().count(), 4, "{detail}");
    assert!(detail.contains("ETI et plus: +20"), "{detail}");
    // A client loses its points; a missing property meets nothing.
    let mut client = msg.clone();
    client["payload"] = json!({"account_status": "customer", "country": "BE"});
    let r = execute(&code_in(PROSPECTING, "pb_score"), &client, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["target_score"], -95);
    // The criteria travel with the message.
    let keep = execute(
        &code_in(PROSPECTING, "pb_criteria"),
        &json!({"payload": [1, 2]}),
        2000,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert_eq!(keep["criteria"], json!([1, 2]));
}

#[test]
fn website_search_prepares_a_query_and_an_email_candidate() {
    let run = |code: &str, msg: serde_json::Value| {
        execute(&code_in(PRESENCE, code), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let msg = json!({
        "payload": {"siren": "123456789", "compliance_contact_email": "Qualite@ACME-Formation.fr", "other": "x@gmail.com"},
        "object": {"id": "o1", "title": "ACME Formation", "version": 2},
        "today": "2026-10-22"
    });
    let r = run("pw_prepare", msg);
    assert_eq!(r[0]["payload"], "ACME Formation");
    assert_eq!(r[0]["org"]["siren"], "123456789");
    assert_eq!(
        (
            r[1]["object_id"].clone(),
            r[1]["payload"]["website_searched_on"].clone()
        ),
        (json!("o1"), json!("2026-10-22"))
    );
    assert_eq!(r[2]["payload"]["url"], "https://acme-formation.fr/");
    assert_eq!(r[2]["candidate"]["source"], "email");
    // A webmail names no website.
    let r = run(
        "pw_prepare",
        json!({"payload": {"mail": "jean@orange.fr"}, "object": {"id": "o", "title": "T", "version": 1}}),
    );
    assert!(r[2].is_null(), "{r}");

    let r = run(
        "pw_results",
        json!({"payload": {"query": "ACME", "provider": "serper",
        "results": [{"url": "https://acme.fr/", "rank": 1}, {"url": "https://b.fr/", "rank": 2}]}}),
    );
    assert_eq!(
        (
            r[0]["payload"]["url"].clone(),
            r[1]["candidate"]["rank"].clone()
        ),
        (json!("https://acme.fr/"), json!(2))
    );
    assert!(r[2].is_null());
}

#[test]
fn website_score_confirms_on_the_siren_and_proposes_on_the_name() {
    let score = |text: &str, source: &str| {
        let msg = json!({
            "payload": {"url": "https://www.acme.fr/", "final_url": "https://www.acme.fr/accueil",
                        "title": "Accueil", "text": "", "legal_text": text},
            "org": {"id": "o1", "title": "ACME Formation", "siren": "123456789", "nda": "84691234569"},
            "candidate": {"source": source, "query": "ACME", "rank": 2, "provider": "serper"}
        });
        execute(&code_in(PRESENCE, "pw_score"), &msg, 2000)
            .unwrap()
            .return_json
    };
    let r = score(
        "ACME Formation SAS, SIREN 123 456 789, NDA 84 69 12345 69",
        "search",
    )
    .unwrap();
    assert_eq!(
        r[0]["payload"],
        json!({"root_url": "https://www.acme.fr/", "domain": "acme.fr"})
    );
    // Port 0: accepted by the org rule (D377); port 1 (proposed) empty.
    assert!(r[1].is_null(), "{r}");
    assert_eq!(
        r[0]["accept"],
        "web_presence.confirm_threshold >= 80 (score 100)"
    );
    assert_eq!(r[0]["link"]["score"], 100);
    assert_eq!(r[0]["confidence"], 100);
    assert_eq!(
        (
            r[0]["link"]["siren_proof"].clone(),
            r[0]["link"]["rank"].clone()
        ),
        (json!(true), json!(2))
    );
    // The name and the org's own e-mail domain: for a human to check.
    let r = score("ACME Formation, organisme à Lyon", "email").unwrap();
    assert!(r[0].is_null(), "{r}");
    assert_eq!(r[1]["link"]["score"], json!(40));
    assert!(r[1].get("accept").is_none(), "a human decides: {r}");
    // The name alone: nothing written.
    assert!(score("ACME Formation", "search").is_none());
    // D368: the org lowered the confirming score to 40.
    let msg = json!({
        "payload": {"url": "https://acme.fr/", "title": "", "text": "", "legal_text": "ACME Formation, Lyon"},
        "org": {"id": "o1", "title": "ACME Formation"},
        "candidate": {"source": "email"}
    });
    let mut settings = serde_json::Map::new();
    settings.insert("web_presence.confirm_threshold".into(), json!(40));
    let r = pnex_node_starlark::executor::execute_with(
        &code_in(PRESENCE, "pw_score"),
        &msg,
        2000,
        &settings,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert!(r[1].is_null() && r[0]["link"]["score"] == 40, "{r}");
    assert_eq!(
        r[0]["accept"],
        "web_presence.confirm_threshold >= 40 (score 40)"
    );
}

#[test]
fn model_opinion_becomes_evidence_and_moves_the_confidence() {
    let opinion = |text: &str| {
        let msg = json!({
            "payload": {"text": text, "model": "m1"},
            "proposal": {"source_id": "o1", "target_id": "s1", "attributes": {}, "confidence": 70}
        });
        execute(&code_in(PRESENCE, "pw_opinion"), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let r = opinion("YES\nThe legal notice cites the SIREN.");
    assert_eq!(
        r["payload"]["confidence"], 79,
        "never above the proposal band"
    );
    assert_eq!(
        r["payload"]["evidence"]["llm_opinion"],
        "YES\nThe legal notice cites the SIREN."
    );
    assert_eq!(opinion("no, a directory")["payload"]["confidence"], 50);
    assert_eq!(opinion("MAYBE")["payload"]["confidence"], 70);
}

#[test]
fn signals_digest_lists_the_new_ones_newest_first() {
    let row = |title: &str, date: &str| {
        json!({"id": title, "title": title, "version": 1,
        "properties": {"date": date, "summary": "Modifications diverses"}})
    };
    let msg =
        json!({"payload": [row("ACME · Modif", "2026-10-01"), row("BETA · Vente", "2026-10-20")]});
    let r = execute(&code_in(PROSPECTING, "pb_digest"), &msg, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (
            r[0]["payload"]["trigger"].clone(),
            r[0]["payload"]["count"].clone()
        ),
        (json!(true), json!(2))
    );
    let text = r[0]["payload"]["text"].as_str().unwrap();
    assert!(text.starts_with("- 2026-10-20 BETA · Vente"), "{text}");
    assert_eq!(text.lines().count(), 2);
    assert!(execute(
        &code_in(PROSPECTING, "pb_digest"),
        &json!({"payload": []}),
        2000
    )
    .unwrap()
    .return_json
    .is_none());
}

#[test]
fn list_contacts_ask_their_company_and_keep_the_candidates() {
    let run = |f: &str, msg: serde_json::Value| {
        execute(&code_in(PROSPECTING_FR, f), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let r = run(
        "pl_prepare",
        json!({
        "payload": {"company": "Mecachrome", "city": "Amboise", "postal_code": "37530"},
        "object": {"id": "c1", "title": "Jeanne Martin", "version": 1}}),
    );
    assert_eq!(
        r["payload"],
        json!({"name": "Mecachrome", "city": "Amboise", "postcode": "37530"})
    );
    assert_eq!(r["contact"]["id"], "c1");
    let found = json!({"payload": {"siren": "429853351",
        "candidates": [{"siren": "429853351", "name": "MECACHROME FRANCE", "city": "AMBOISE", "score": 95}]},
        "contact": {"id": "c1"}});
    let r = run("pl_found", found.clone());
    assert_eq!(
        (r[0]["payload"]["siren"].clone(), r[0]["title"].clone()),
        (json!("429853351"), json!("MECACHROME FRANCE"))
    );
    assert_eq!(
        (
            r[1]["object_id"].clone(),
            r[1]["payload"]["matching"].clone()
        ),
        (json!("c1"), json!("found"))
    );
    let r = run("pl_review", found);
    assert_eq!(r["payload"]["matching"], "to_review");
    assert_eq!(
        r["payload"]["candidates"],
        "MECACHROME FRANCE (429853351, AMBOISE, 95)"
    );
    let r = run(
        "pl_review",
        json!({"payload": {"candidates": []}, "contact": {"id": "c2"}}),
    );
    assert_eq!(r["payload"]["matching"], "not_found");
}

#[test]
fn a_changed_page_becomes_a_signal_only_on_a_listed_topic() {
    let run = |f: &str, msg: serde_json::Value| {
        execute(&code_in(PROSPECTING, f), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let prep = run(
        "ps_prepare",
        json!({
        "payload": {"url": "https://acme.fr/actus", "text": "ACME ouvre une usine à Lyon.", "account": "org-1"},
        "object": {"id": "p1", "title": "Actualités", "version": 2}}),
    );
    assert_eq!(prep["payload"]["text"], "ACME ouvre une usine à Lyon.");
    let page = prep["page"].clone();
    let r = run(
        "ps_signal",
        json!({"payload": {"text": "new_site\n« ACME ouvre une usine à Lyon. »"},
        "page": page, "today": "2026-10-22"}),
    );
    assert_eq!(r[0]["payload"]["topic"], "new_site");
    assert_eq!(
        r[0]["payload"]["ref"],
        "page:https://acme.fr/actus:2026-10-22"
    );
    assert_eq!(r[0]["payload"]["state"], "to_validate");
    assert_eq!(
        r[0]["payload"]["excerpt"],
        "« ACME ouvre une usine à Lyon. »"
    );
    assert_eq!(
        (
            r[1]["object_id"].clone(),
            r[1]["payload"]["changed"].clone()
        ),
        (json!("p1"), json!(false))
    );
    // Nothing announced, or a topic off the list: only marked read.
    for answer in ["none", "météo", ""] {
        let r = run(
            "ps_signal",
            json!({"payload": {"text": answer}, "page": page}),
        );
        assert!(r[0].is_null(), "{answer}: {r}");
        assert_eq!(r[1]["payload"]["changed"], false);
    }
    let link = run(
        "ps_link",
        json!({"payload": {}, "object": {"id": "s1"}, "page": page}),
    );
    assert_eq!(
        link["payload"],
        json!({"source_id": "s1", "target_id": "org-1"})
    );
}

/// One ontology_query row (array output) with joined links.
fn row(
    id: &str,
    title: &str,
    props: serde_json::Value,
    links: serde_json::Value,
) -> serde_json::Value {
    json!({"id": id, "title": title, "version": 1, "properties": props, "links": links})
}

fn joined(
    other_id: &str,
    title: &str,
    props: serde_json::Value,
    attrs: serde_json::Value,
) -> serde_json::Value {
    json!({"direction": "in", "attributes": attrs, "valid_from": "2026-01-01T00:00:00Z",
           "other": {"id": other_id, "type": "x", "title": title, "properties": props}})
}

fn rules_of(out: &serde_json::Value) -> Vec<String> {
    let mut r: Vec<String> = out["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["props"]["ref"]
                .as_str()
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    r.sort();
    r
}

#[test]
fn network_rules_read_registry_facts() {
    let cert = row(
        "c1",
        "RS0101",
        json!({"code": "RS0101"}),
        json!({
            "holds": [joined("w", "CERTIF EXEMPLE", json!({"siren": "200000008"}), json!({}))],
            "accredited_for": [
                joined("ceased", "OF CESSE", json!({"siren": "111111111", "administrative_state": "ceased", "qualiopi": true, "qualiopi_updated": "2026-04-11"}), json!({"declared_fc": true, "scope": "both"})),
                joined("eval", "OF EVAL", json!({"siren": "222222222", "qualiopi": false, "qualiopi_updated": "2026-04-11"}), json!({"declared_fc": true, "scope": "assessment"})),
                joined("old", "OF ANCIEN", json!({"siren": "333333333"}), json!({"scope": "both", "fc_state": "inactive", "ends_on": "2026-01-31", "source": "page_fc"})),
                joined("ok", "OF EXEMPLE", json!({"siren": "100000009", "qualiopi": true, "qualiopi_updated": "2026-04-11"}), json!({"declared_fc": true, "scope": "both", "fc_state": "active"})),
            // Accredited to prepare: its offers from another establishment
            // (SIRET) are matched by SIREN and are not CPF-01.
            joined("aud", "OF AUTRE SITE", json!({"siren": "520388901", "qualiopi": true, "qualiopi_updated": "2026-04-11"}), json!({"declared_fc": true, "scope": "preparation", "fc_state": "active"}))
            ],
            "offer_for": [
                joined("o1", "OF HORS RESEAU · SEO", json!({"siret": "88024436300011", "id_action": "S_SEO", "seen_on": "2026-10-23"}), json!({})),
                joined("o2", "OF EVAL · SEO", json!({"siret": "22222222200010", "id_action": "E_SEO", "seen_on": "2026-10-23"}), json!({})),
                joined("o3", "OF ANCIEN · SEO", json!({"siret": "33333333300010", "id_action": "A_SEO", "seen_on": "2026-10-23"}), json!({})),
                joined("o4", "OF EXEMPLE · SEO", json!({"siret": "10000000900024", "id_action": "P_SEO", "seen_on": "2026-10-23"}), json!({})),
                joined("o5", "CERTIF EXEMPLE · SEO", json!({"siret": "20000000800010", "id_action": "W_SEO", "seen_on": "2026-10-23"}), json!({})),
            joined("o6", "OF AUTRE SITE · SEO", json!({"siret": "52038890100065", "id_action": "AUD_SEO", "seen_on": "2026-10-23"}), json!({}))
            ]
        }),
    );
    // A certification no certifier of the org holds is not watched.
    let other = row(
        "c2",
        "RS1",
        json!({"code": "RS1"}),
        json!({"offer_for": [joined("o9", "X", json!({"siret": "99999999900010"}), json!({}))]}),
    );
    let msg = json!({"payload": [cert, other], "now": "2026-10-24T05:30:00+00:00", "today": "2026-10-24"});
    let out = execute(&code_of("qw_network_rules"), &msg, 5000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        rules_of(&out),
        vec!["ADM-01", "ADM-02", "CPF-01", "CPF-02", "CPF-03", "REG-01", "REG-02"],
        "{out}"
    );
    let e = out["findings"].as_array().unwrap();
    let cpf1 = e
        .iter()
        .find(|x| x["props"]["ref"].as_str().unwrap().starts_with("CPF-01"))
        .unwrap();
    assert_eq!(
        cpf1["props"]["ref"], "CPF-01:880244363:RS0101:S_SEO",
        "the certifier's own offer is not one"
    );
    assert_eq!(
        (
            cpf1["props"]["state"].clone(),
            cpf1["props"]["severity"].clone()
        ),
        (json!("proposed"), json!("critical"))
    );
    assert_eq!(cpf1["link_targets"]["observed_in"], "o1");
    assert!(
        e.iter()
            .all(|x| !x["props"]["ref"].as_str().unwrap().contains("100000009")),
        "OF EXEMPLE is clean: {out}"
    );
}

/// The partner message of the website check: a partner reached from a
/// certification, its site and accreditation joined.
fn partner_msg() -> serde_json::Value {
    json!({
        "payload": {"siren": "100000009", "qualiopi": true, "qualiopi_updated": "2026-04-11", "qualiopi_assessments": false},
        "object": {"id": "ls", "type": "organization", "title": "OF EXEMPLE", "version": 3},
        "links": {
            "has_website": [{"direction": "out", "attributes": {}, "other": {"id": "site1", "title": "of-exemple.example", "properties": {"root_url": "https://www.of-exemple.example/", "domain": "of-exemple.example"}}}],
            "accredited_for": [{"direction": "out", "attributes": {"fc_state": "active", "scope": "both"}, "other": {"id": "c1", "title": "RS0101", "properties": {"code": "RS0101"}}}]
        },
        "network": {
            "RS0101": {"id": "c1", "official_title": "Piloter le référencement d'un site", "status": "active", "certifiers": ["CERTIF EXEMPLE"]},
            "RS9999": {"id": "c9", "official_title": "Piloter un projet", "status": "active", "certifiers": ["CERTIF EXEMPLE"]}
        },
        "today": "2026-10-26", "now": "2026-10-26T02:00:00+00:00"
    })
}

#[test]
fn crawl_targets_carry_codes_titles_and_the_partner() {
    let out = execute(&code_of("qw_targets"), &partner_msg(), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(out["payload"]["url"], "https://www.of-exemple.example/");
    assert_eq!(out["payload"]["link_to"], "site1");
    let rules = out["payload"]["match"].to_string();
    assert!(
        rules.contains("RS\\\\s?7511")
            && rules.contains("référencement naturel")
            && rules.contains("CERTIF EXEMPLE"),
        "{rules}"
    );
    assert_eq!(
        out["partner"]["accreditations"]["RS0101"]["fc_state"],
        "active"
    );
    assert!(out["partner"]["accreditations"].get("RS9999").is_none());
    // No website: no target.
    let mut bare = partner_msg();
    bare["links"]["has_website"] = json!([]);
    assert!(execute(&code_of("qw_targets"), &bare, 2000)
        .unwrap()
        .return_json
        .is_none());
}

#[test]
fn website_rules_quote_the_page() {
    let target = execute(&code_of("qw_targets"), &partner_msg(), 2000)
        .unwrap()
        .return_json
        .unwrap();
    let mut msg = partner_msg();
    msg["partner"] = target["partner"].clone();
    let page = |text: &str| {
        let mut m = msg.clone();
        m["payload"] = json!({"url": "https://www.of-exemple.example/seo", "title": "SEO", "text": text, "page_id": "p1", "object_id": "po1", "changed": true});
        execute(&code_of("qw_web_rules"), &m, 5000)
            .unwrap()
            .return_json
            .unwrap()
    };
    // A clean page: code, exact title, certifier, CPF while accredited.
    let clean = page(
        "Formation Piloter le référencement d'un site (RS0101), certificateur CERTIF EXEMPLE. Éligible au CPF.",
    );
    assert_eq!(rules_of(&clean), Vec::<String>::new(), "{clean}");
    // RS9999 (not accredited) offered with the CPF; a free gift; Qualiopi
    // on skills assessments not certified; RS0101 under a wrong title and
    // without its certifier.
    let bad = page(
        "Préparez le RS 9999 Piloter un projet, éligible au CPF ! Tablette offerte pour toute inscription. \
        Organisme certifié Qualiopi pour vos bilans de compétences. Formation SEO RS0101 en 2 jours.",
    );
    assert_eq!(
        rules_of(&bad),
        vec!["WEB-01", "WEB-03", "WEB-03", "WEB-04", "WEB-05", "WEB-06", "WEB-08"],
        "{bad}"
    );
    let e = bad["findings"].as_array().unwrap();
    for x in e {
        assert_eq!(x["props"]["state"], "to_validate");
        let quote = x["props"]["excerpt"].as_str().unwrap();
        assert!(
            !quote.is_empty() && msg_text_holds(&bad, quote),
            "word for word: {quote}"
        );
    }
    let web08 = e
        .iter()
        .find(|x| x["props"]["ref"].as_str().unwrap().starts_with("WEB-08"))
        .unwrap();
    assert_eq!(web08["props"]["ref"], "WEB-08:100000009:RS9999:p1");
    assert_eq!(
        (
            web08["link_targets"]["finding_on"].clone(),
            web08["link_targets"]["observed_in"].clone()
        ),
        (json!("c9"), json!("po1"))
    );
    // The title without its code (WEB-02).
    let no_code = page(
        "Notre parcours Piloter le référencement d'un site, par CERTIF EXEMPLE.",
    );
    assert_eq!(rules_of(&no_code), vec!["WEB-02"], "{no_code}");
    // The site summary: nothing relevant (COL-01), a finding to propose.
    let mut m = msg.clone();
    m["payload"] = json!({"url": "https://www.of-exemple.example", "pages": 40, "relevant": 0, "truncated": false, "mission_id": "m1"});
    let col = execute(&code_of("qw_web_rules"), &m, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(rules_of(&col), vec!["COL-01"]);
    assert_eq!(col["findings"][0]["props"]["state"], "proposed");
}

/// WEB-06 needs an explicit Qualiopi category claim, quoted word for word,
/// and the website rules stay on pages citing the client's certifications
/// unless the whole-site setting is on.
#[test]
fn website_rules_need_an_explicit_claim_and_the_clients_certifications() {
    let target = execute(&code_of("qw_targets"), &partner_msg(), 2000)
        .unwrap()
        .return_json
        .unwrap();
    let mut msg = partner_msg();
    msg["partner"] = target["partner"].clone();
    let run = |text: &str, settings: serde_json::Value| {
        let mut m = msg.clone();
        m["payload"] = json!({"url": "https://www.of-exemple.example/seo", "title": "SEO", "text": text, "page_id": "p1", "object_id": "po1", "changed": true});
        pnex_node_starlark::executor::execute_with(
            &code_of("qw_web_rules"),
            &m,
            5000,
            settings.as_object().unwrap(),
        )
        .unwrap()
        .return_json
        .unwrap()
    };
    let page = |text: &str| run(text, json!({}));
    // Expert review 2026-10-27, false positives: "apprentissage" in its
    // pedagogical sense (wording of the stored GOBITWIN pages).
    for text in [
        "Formation RS0101 Piloter le référencement d'un site, CERTIF EXEMPLE. \
         Formation animée par un formateur expert, certifié Qualiopi. Des évaluations formatives \
         soutiennent l'apprentissage des apprenants.",
        "RS0101 Piloter le référencement d'un site (CERTIF EXEMPLE) : \
         organisme certifié Qualiopi, une pédagogie centrée sur l’apprentissage par la pratique.",
    ] {
        assert_eq!(page(text), json!({"findings": []}), "{text}");
    }
    // An explicit claim: quoted where the claim is, not where Qualiopi is.
    let claim = format!(
        // english-guard: allow (French page text the rule reads)
        "Organisme certifié Qualiopi. RS0101 Piloter le référencement d'un site, \
         CERTIF EXEMPLE. {} Cette formation est aussi proposée en contrat d’apprentissage avec notre CFA.",
        // english-guard: allow (French page text the rule reads)
        "Programme détaillé, modalités, délais d'accès. ".repeat(10)
    );
    let out = page(&claim);
    assert_eq!(rules_of(&out), vec!["WEB-06"], "{out}");
    let quote = out["findings"][0]["props"]["excerpt"].as_str().unwrap();
    assert!(
        quote.contains("en contrat d’apprentissage")
            && !quote.contains("Qualiopi")
            && claim.contains(quote),
        "{quote}"
    );
    // A page citing none of the client's certifications: nothing by default,
    // the whole site under the setting.
    let other = "Organisme certifié Qualiopi pour vos bilans de compétences. Éligible au CPF, formation gratuite.";
    assert_eq!(page(other), json!({"findings": []}));
    assert_eq!(
        rules_of(&run(other, json!({"quali_watch.whole_site": true}))),
        vec!["WEB-05", "WEB-06"]
    );
}

/// Every excerpt is cut from the page text of the test (word for word).
fn msg_text_holds(_out: &serde_json::Value, quote: &str) -> bool {
    "Préparez le RS 9999 Piloter un projet, éligible au CPF ! Tablette offerte pour toute inscription. \
        Organisme certifié Qualiopi pour vos bilans de compétences. Formation SEO RS0101 en 2 jours."
        .contains(quote)
}

#[test]
fn a_finding_becomes_a_write_then_its_links() {
    let e = json!({"payload": {"props": {"ref": "WEB-08:1:RS1:p", "state": "to_validate"}, "title": "WEB-08 · X", "link_targets": {"concerns": "o", "finding_on": "c", "observed_in": ""}},
                   "object": {"id": "stale"}, "links": {"big": []}});
    let w = execute(&code_of("qw_finding"), &e, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (
            w["payload"]["ref"].clone(),
            w["title"].clone(),
            w["object"].clone()
        ),
        (json!("WEB-08:1:RS1:p"), json!("WEB-08 · X"), json!(null))
    );
    let written = json!({"object": {"id": "e1"}, "link_targets": w["link_targets"].clone()});
    let l = execute(&code_of("qw_links"), &written, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(l["concerns"], json!({"source_id": "e1", "target_id": "o"}));
    assert_eq!(l["finding_on"]["target_id"], "c");
    assert!(l.get("observed_in").is_none());
}

/// `crm` (crm.md D450): one activity per step of the plan, due on the
/// start day plus the step's day, across month and leap-year ends.
#[test]
fn follow_up_steps_become_activities_due_on_their_day() {
    let act = |day0: &str, day: i64| {
        let msg = json!({
            "payload": {"day": day, "kind": "mail", "instruction": "Follow up", "active": true},
            "object": {"id": "s1", "title": "D3"},
            "opp": {"id": "o1", "name": "Acme 2027", "context": "Account: Acme", "day0": day0}
        });
        execute(&code_in(CRM, "crm_cadence_activity"), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let r = act("2026-10-30", 3);
    assert_eq!(r["payload"]["due_on"], "2026-11-02");
    assert_eq!(r["payload"]["ref"], "cadence:o1:s1");
    assert_eq!(r["payload"]["state"], "to_do");
    assert_eq!(r["title"], "D3 · Acme 2027", "the step's own title");
    assert_eq!(act("2028-02-27", 2)["payload"]["due_on"], "2028-02-29");
    assert_eq!(act("2026-12-31", 14)["payload"]["due_on"], "2027-01-14");
    assert_eq!(act("2026-01-01", 0)["payload"]["due_on"], "2026-01-01");
    let off = json!({"payload": {"day": 1, "kind": "call", "active": false}, "object": {"id": "s"}, "opp": {"id": "o", "name": "T", "context": "", "day0": "2026-01-01"}});
    assert!(execute(&code_in(CRM, "crm_cadence_activity"), &off, 2000)
        .unwrap()
        .return_json
        .is_none());
}

#[test]
fn a_plan_never_starts_when_every_person_objected() {
    let msg = |opposed: bool| {
        json!({
            "payload": {"stage": "qualification"},
            "object": {"id": "o1", "title": "Acme 2027"},
            "today": "2026-10-26",
            "links": {
                "involved_in": [{"direction": "in", "attributes": {"role": "decision_maker"},
                              "other": {"id": "p1", "type": "person", "title": "A",
                                        "properties": if opposed { json!({"opposed_on": "2026-10-01"}) } else { json!({}) }}}],
                "opportunity_with": [{"direction": "out", "other": {"id": "a", "type": "organization", "title": "Acme", "properties": {}}}]
            }
        })
    };
    let r = execute(&code_in(CRM, "crm_cadence_start"), &msg(false), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r[0]["payload"]["follow_up_started_on"], "2026-10-26");
    assert_eq!(
        r[1]["opp"]["context"],
        "Account: Acme · contacts: decision_maker"
    );
    assert_eq!(r[1]["opp"]["day0"], "2026-10-26");
    let r = execute(&code_in(CRM, "crm_cadence_start"), &msg(true), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r[0]["object_id"], "o1");
    assert!(r[1].is_null(), "marked, no activity");
}

#[test]
fn missing_criteria_follow_the_stage_and_valid_links() {
    let crit = |id: &str, title: &str, stage: &str, order: i64| json!({"id": id, "title": title, "properties": {"stage": stage, "order": order, "active": true}});
    let msg = |stage: &str, current: &str| {
        json!({
            "payload": {"stage": stage, "missing_criteria": current},
            "object": {"id": "o1", "title": "Acme"},
            "criteria": [crit("c2", "Budget", "qualification", 2), crit("c1", "Pain", "discovery", 1), crit("c3", "Decision maker", "proposal", 3)],
            "links": {"qualified_on": [{"direction": "out", "attributes": {"status": "valid"}, "other": {"id": "c1"}},
                                   {"direction": "out", "attributes": {"status": "partial"}, "other": {"id": "c2"}}]}
        })
    };
    let r = execute(
        &code_in(CRM, "crm_missing"),
        &msg("qualification", ""),
        2000,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert_eq!(r["payload"]["missing_criteria"], "Budget");
    let r = execute(&code_in(CRM, "crm_missing"), &msg("negotiation", ""), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["missing_criteria"], "Budget\nDecision maker");
    assert!(
        execute(
            &code_in(CRM, "crm_missing"),
            &msg("qualification", "Budget"),
            2000
        )
        .unwrap()
        .return_json
        .is_none(),
        "unchanged: no write"
    );
}

#[test]
fn a_done_activity_only_moves_the_last_activity_forward() {
    let msg = |last: &str| {
        json!({
            "payload": {"state": "done", "done_at": "2026-10-20T09:00:00Z"},
            "object": {"id": "a1", "title": "Appel"},
            "now": "2026-10-26T08:00:00Z",
            "links": {"activity_on": [{"direction": "out", "other": {"id": "o1", "type": "opportunity", "title": "Acme", "properties": {"last_activity": last}}}]}
        })
    };
    let r = execute(
        &code_in(CRM, "crm_report"),
        &msg("2026-10-01T00:00:00Z"),
        2000,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert_eq!(r[0]["payload"]["last_activity"], "2026-10-20T09:00:00Z");
    assert_eq!(r[0]["object_id"], "o1");
    assert_eq!(r[1]["payload"]["reported"], true);
    let r = execute(
        &code_in(CRM, "crm_report"),
        &msg("2026-10-25T00:00:00Z"),
        2000,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert!(r[0].is_null(), "an older activity never moves it back");
}

#[test]
fn my_day_lists_due_activities_and_dormant_opportunities() {
    let msg = json!({
        "dues": [{"title": "mail D3 · Acme", "properties": {"due_on": "2026-10-26"}}],
        "payload": [{"title": "Beta 2027", "properties": {"last_activity": "2026-09-30T10:00:00Z"}}]
    });
    let r = execute(&code_in(CRM, "crm_day"), &msg, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["count"], 2);
    let text = r["payload"]["text"].as_str().unwrap();
    assert!(
        text.contains("2026-10-26 mail D3 · Acme")
            && text.contains("Beta 2027 (last activity 2026-09-30)"),
        "{text}"
    );
    let empty = json!({"dues": [], "payload": []});
    assert!(execute(&code_in(CRM, "crm_day"), &empty, 2000)
        .unwrap()
        .return_json
        .is_none());
}

/// `crm` « mails received » (mail.md D462–D463): the correspondent of a
/// trusted forward, authentication deciding between a link and a proposal.
#[test]
fn a_mail_is_filed_under_its_correspondent() {
    let mail = |forward_trusted: bool, dmarc: &str| {
        json!({"payload": {
            "message_id": "<m1@x>", "subject": "Fwd: Quote", "text": "Hello", "date": "2026-10-26T09:00:00+00:00",
            "from": {"address": "Bob@Example.com"}, "auth": {"spf": "pass", "dkim": "none", "dmarc": dmarc},
            "automatic": false, "from_pnex": false,
            "forwarded": {"from": {"address": "Elodie@Acme.fr"}, "subject": "Quote line 3", "trusted": forward_trusted}
        }})
    };
    let r = execute(&code_in(CRM, "crm_mail_prep"), &mail(true, "pass"), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["addr"], "elodie@acme.fr");
    assert_eq!(r["mail"]["subject"], "Quote line 3");
    assert_eq!(r["mail"]["trusted"], true);
    let r = execute(&code_in(CRM, "crm_mail_prep"), &mail(false, "fail"), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        r["addr"], "bob@example.com",
        "an untrusted forward stays with its sender"
    );
    assert_eq!(r["mail"]["trusted"], false);
    let auto = json!({"payload": {"automatic": true, "from": {"address": "a@b.fr"}}});
    assert!(execute(&code_in(CRM, "crm_mail_prep"), &auto, 2000)
        .unwrap()
        .return_json
        .is_none());

    let linked = |trusted: bool| json!({"object": {"id": "act"}, "person": {"id": "p1"}, "opportunities": ["o1"], "mail": {"trusted": trusted, "subject": "Quote"}});
    let r = execute(&code_in(CRM, "crm_mail_link"), &linked(true), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        r[0]["payload"],
        json!([{"source_id": "act", "target_id": "p1"}, {"source_id": "act", "target_id": "o1"}])
    );
    assert!(r[1].is_null());
    let r = execute(&code_in(CRM, "crm_mail_link"), &linked(false), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert!(r[0].is_null());
    assert_eq!(r[1]["payload"][1]["confidence"], 40);

    let opp = |id: &str, stage: &str| json!({"direction": "out", "other": {"id": id, "type": "opportunity", "properties": {"stage": stage}}});
    let act = json!({"object": {"id": "p1"}, "links": {"involved_in": [opp("o1", "proposal"), opp("o2", "lost")]},
        "mail": {"message_id": "<m1@x>", "subject": "Quote", "text": "Hello", "date": "2026-10-26T13:35:00+00:00"}});
    let r = execute(&code_in(CRM, "crm_mail_activity"), &act, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (
            r["payload"]["ref"].clone(),
            r["payload"]["direction"].clone(),
            r["person"]["id"].clone()
        ),
        (json!("mail:<m1@x>"), json!("inbound"), json!("p1"))
    );
    // Only the open opportunity; the mail date moves its last activity.
    assert_eq!(r["opportunities"], json!(["o1"]));
    assert_eq!(r["payload"]["done_at"], "2026-10-26T13:35:00+00:00");
    assert!(r["payload"].get("reported").is_none());
}

/// An unknown sender from an own domain is proposed under the account
/// whose website has that domain; a webmail never is.
#[test]
fn an_unknown_sender_is_proposed_under_the_account_of_its_domain() {
    let mail =
        json!({"message_id": "<m2@x>", "subject": "Quote", "text": "Hello", "trusted": true});
    let q = |found: serde_json::Value, addr: &str| json!({"payload": found, "addr": addr, "mail": mail});
    let r = execute(
        &code_in(CRM, "crm_mail_domain"),
        &q(json!([]), "new@acme.fr"),
        2000,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert_eq!(r["domain"], "acme.fr");
    for (found, addr) in [
        (json!([{"id": "p1"}]), "known@acme.fr"),
        (json!([]), "someone@gmail.com"),
    ] {
        assert!(
            execute(&code_in(CRM, "crm_mail_domain"), &q(found, addr), 2000)
                .unwrap()
                .return_json
                .is_none()
        );
    }
    let opp = |id: &str, stage: &str| json!({"direction": "in", "other": {"id": id, "type": "opportunity", "properties": {"stage": stage}}});
    let acc = json!({"object": {"id": "org1"}, "links": {"opportunity_with": [opp("o1", "proposal"), opp("o2", "won")]},
        "mail": mail, "domain": "acme.fr"});
    let r = execute(&code_in(CRM, "crm_mail_account"), &acc, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (r["person"]["id"].clone(), r["opportunities"].clone()),
        (json!("org1"), json!(["o1"]))
    );
    let mut linked = r.clone();
    linked["object"] = json!({"id": "act"});
    let r = execute(&code_in(CRM, "crm_mail_link"), &linked, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert!(
        r[0].is_null(),
        "a domain match is never linked on its own, even trusted"
    );
    assert_eq!(r[1]["payload"][0]["evidence"]["domain"], "acme.fr");
}

/// `crm` « meetings received » (D484): an invitation becomes a meeting
/// keyed by its UID, each address once, a reply is ignored, and
/// authentication decides between a participation and a proposal.
#[test]
fn an_invitation_becomes_a_meeting_with_its_people() {
    let mail = |method: &str, dmarc: &str| {
        json!({"payload": {
            "subject": "Invitation: Review", "auth": {"spf": "pass", "dkim": "pass", "dmarc": dmarc},
            "invitation": {"uid": "m1@acme.fr", "method": method, "summary": "Quote review",
                "start": "2026-10-27T20:00:00+01:00", "location": "Teams",
                "organizer": {"address": "Elodie@Acme.fr"},
                "attendees": [{"address": "alice@example.com"}, {"address": "elodie@acme.fr"}]}
        }})
    };
    let prep = code_in(CRM, "crm_meeting_prep");
    let r = execute(&prep, &mail("REQUEST", "pass"), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        r["payload"],
        json!({"invitation_uid": "m1@acme.fr", "purpose": "Quote review", "status": "planned",
            "date": "2026-10-27T20:00:00+01:00", "place": "Teams"})
    );
    assert_eq!(
        r["addresses"],
        json!(["elodie@acme.fr", "alice@example.com"])
    );
    assert_eq!(r["trusted"], true);
    let r = execute(&prep, &mail("CANCEL", "fail"), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (r["payload"]["status"].clone(), r["trusted"].clone()),
        (json!("cancelled"), json!(false))
    );
    for m in [
        mail("REPLY", "pass"),
        json!({"payload": {"subject": "Hello"}}),
    ] {
        assert!(execute(&prep, &m, 2000).unwrap().return_json.is_none());
    }

    let r = execute(
        &code_in(CRM, "crm_meeting_participants"),
        &json!({"object": {"id": "m1", "title": "meeting · Review"}, "addresses": ["a@x.fr", "b@y.fr"]}),
        2000,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert_eq!(
        r["payload"],
        json!([{"address": "a@x.fr"}, {"address": "b@y.fr"}])
    );
    assert_eq!(r["meeting"]["id"], "m1");

    let found = |trusted: bool| json!({"object": {"id": "p1"}, "meeting": {"id": "m1", "purpose": "Review"}, "trusted": trusted});
    let link = code_in(CRM, "crm_meeting_link");
    let r = execute(&link, &found(true), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        r[0]["payload"],
        json!({"source_id": "p1", "target_id": "m1"})
    );
    assert!(r[1].is_null());
    let r = execute(&link, &found(false), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert!(r[0].is_null());
    assert_eq!(r[1]["payload"]["confidence"], 40);
}
