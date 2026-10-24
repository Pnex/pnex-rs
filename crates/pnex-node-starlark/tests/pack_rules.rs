//! The Starlark rules shipped by the `quali_watch` pack (partner-watch.md
//! D314, D316) and the target score of `prospection_b2b`
//! (account-mapping.md D336) run as the flow runtime runs them.

use pnex_node_starlark::executor::execute;
use serde_json::json;

const PACK: &str = include_str!("../../pnex-backend/packs/quali_watch.yaml");
const PROSPECTION: &str = include_str!("../../pnex-backend/packs/prospection_b2b.yaml");
const PRESENCE: &str = include_str!("../../pnex-backend/packs/presence_web.yaml");
const PROSPECTION_FR: &str = include_str!("../../pnex-backend/packs/prospection_fr.yaml");

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

#[test]
fn next_reminder_moves_the_procedure_and_waits_for_a_human_at_r3() {
    let msg = json!({
        "payload": {"niveau": "r2", "prochaine_echeance": "2026-12-20", "contact_email": "conformite@of.fr"},
        "object": {"id": "p1", "title": "OF Formation", "version": 3},
        "today": "2026-12-25", "now": "2026-12-25T06:00:00+00:00"
    });
    let out = execute(&code_of("qw_relance_suivante"), &msg, 2000).unwrap();
    let r = out.return_json.expect("two messages");
    assert_eq!(r[0]["payload"]["niveau"], "r3");
    assert_eq!(r[0]["payload"]["prochaine_echeance"], "2027-01-09");
    assert_eq!(r[0]["payload"]["etat"], "en_attente_validation");
    assert_eq!(
        (r[0]["object_id"].clone(), r[0]["expected_version"].clone()),
        (json!("p1"), json!(3))
    );
    assert_eq!(r[1]["payload"]["cle"], "p1:r3");
    assert_eq!(r[1]["payload"]["etat"], "a_valider");
    assert_eq!(r[1]["payload"]["destinataire"], "conformite@of.fr");
    // r1 → r2 is automatic; r4 is the last reminder.
    let first = json!({"payload": {"niveau": "r1"}, "object": {"id": "p", "title": "T", "version": 1}, "today": "2028-02-20", "now": "x"});
    let r = execute(&code_of("qw_relance_suivante"), &first, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (
            r[0]["payload"]["prochaine_echeance"].clone(),
            r[1]["payload"]["etat"].clone()
        ),
        (json!("2028-03-06"), json!("envoyee"))
    );
    let last =
        json!({"payload": {"niveau": "r4"}, "object": {"id": "p", "title": "T", "version": 1}});
    assert!(execute(&code_of("qw_relance_suivante"), &last, 2000)
        .unwrap()
        .return_json
        .is_none());
}

#[test]
fn adm_01_finding_and_link_payload() {
    let msg = json!({
        "payload": {"siren": "123456789", "date_cessation": "2026-09-30"},
        "object": {"id": "o1", "title": "OF Fermé", "version": 2},
        "now": "2026-10-19T05:00:00+00:00"
    });
    let r = execute(&code_of("qw_regle_adm_01"), &msg, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["cle"], "ADM-01:123456789");
    assert_eq!(r["payload"]["etat"], "propose");
    assert_eq!(
        r["payload"]["constate"],
        "Unité légale cessée le 2026-09-30"
    );
    assert_eq!(r["organization_id"], "o1");
    let linked = json!({"object": {"id": "e1"}, "organization_id": "o1"});
    let l = execute(&code_of("qw_lien"), &linked, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(l["payload"], json!({"source_id": "e1", "target_id": "o1"}));
}

#[test]
fn target_score_adds_the_points_of_the_criteria_met() {
    let crit = |title: &str, prop: &str, op: &str, value: &str, points: i64| json!({"title": title, "properties": {"propriete": prop, "operateur": op, "valeur": value, "points": points}});
    let criteria = json!([
        crit("Industrie visée", "code_activite", "commence_par", "25, 30.3, 21", 30),
        crit("ETI et plus", "effectif_min", "au_moins", "1000", 20),
        crit("CA > 10 M€", "ca_dernier", "au_moins", "10000000", 15),
        crit("Déjà client", "statut_compte", "egal", "client", -100),
        crit("Pays", "pays", "parmi", "FR,BE", 5),
        {"title": "Inactif", "properties": {"propriete": "pays", "operateur": "renseigne", "points": 99, "actif": false}},
    ]);
    let msg = json!({
        "payload": {"code_activite": "25.62", "effectif_min": 1000, "pays": "FR",
                    "ca_dernier": {"amount": 343838577.0, "currency": "EUR", "estimated": false},
                    "statut_compte": "cible"},
        "object": {"id": "o1", "title": "MECACHROME FRANCE", "version": 4},
        "criteres": criteria,
        "today": "2026-10-22"
    });
    let r = execute(&code_in(PROSPECTION, "pb_score"), &msg, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["score_cible"], 70);
    assert_eq!(r["payload"]["score_le"], "2026-10-22");
    assert_eq!(r["object_id"], "o1");
    let detail = r["payload"]["score_detail"].as_str().unwrap();
    assert_eq!(detail.lines().count(), 4, "{detail}");
    assert!(detail.contains("ETI et plus : +20"), "{detail}");
    // A client loses its points; a missing property meets nothing.
    let mut client = msg.clone();
    client["payload"] = json!({"statut_compte": "client", "pays": "BE"});
    let r = execute(&code_in(PROSPECTION, "pb_score"), &client, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(r["payload"]["score_cible"], -95);
    // The criteria travel with the message.
    let keep = execute(
        &code_in(PROSPECTION, "pb_criteres"),
        &json!({"payload": [1, 2]}),
        2000,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert_eq!(keep["criteres"], json!([1, 2]));
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
        "payload": {"siren": "123456789", "contact_conformite_email": "Qualite@ACME-Formation.fr", "autre": "x@gmail.com"},
        "object": {"id": "o1", "title": "ACME Formation", "version": 2},
        "today": "2026-10-22"
    });
    let r = run("pw_preparer", msg);
    assert_eq!(r[0]["payload"], "ACME Formation");
    assert_eq!(r[0]["org"]["siren"], "123456789");
    assert_eq!(
        (
            r[1]["object_id"].clone(),
            r[1]["payload"]["site_recherche_le"].clone()
        ),
        (json!("o1"), json!("2026-10-22"))
    );
    assert_eq!(r[2]["payload"]["url"], "https://acme-formation.fr/");
    assert_eq!(r[2]["candidat"]["source"], "email");
    // A webmail names no website.
    let r = run(
        "pw_preparer",
        json!({"payload": {"mail": "jean@orange.fr"}, "object": {"id": "o", "title": "T", "version": 1}}),
    );
    assert!(r[2].is_null(), "{r}");

    let r = run(
        "pw_resultats",
        json!({"payload": {"query": "ACME", "provider": "serper",
        "results": [{"url": "https://acme.fr/", "rank": 1}, {"url": "https://b.fr/", "rank": 2}]}}),
    );
    assert_eq!(
        (
            r[0]["payload"]["url"].clone(),
            r[1]["candidat"]["rang"].clone()
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
            "candidat": {"source": source, "requete": "ACME", "rang": 2, "fournisseur": "serper"}
        });
        execute(&code_in(PRESENCE, "pw_score"), &msg, 2000)
            .unwrap()
            .return_json
    };
    let r = score(
        "ACME Formation SAS, SIREN 123 456 789, NDA 84 69 12345 69",
        "recherche",
    )
    .unwrap();
    assert_eq!(
        r[0]["payload"],
        json!({"url_racine": "https://www.acme.fr/", "domaine": "acme.fr"})
    );
    // Port 0: accepted by the org rule (D377); port 1 (proposed) empty.
    assert!(r[1].is_null(), "{r}");
    assert_eq!(
        r[0]["accept"],
        "presence_web.seuil_confirme >= 80 (score 100)"
    );
    assert_eq!(r[0]["lien"]["score"], 100);
    assert_eq!(r[0]["confidence"], 100);
    assert_eq!(
        (
            r[0]["lien"]["preuve_siren"].clone(),
            r[0]["lien"]["rang"].clone()
        ),
        (json!(true), json!(2))
    );
    // The name and the org's own e-mail domain: for a human to check.
    let r = score("ACME Formation, organisme à Lyon", "email").unwrap();
    assert!(r[0].is_null(), "{r}");
    assert_eq!(r[1]["lien"]["score"], json!(40));
    assert!(r[1].get("accept").is_none(), "a human decides: {r}");
    // The name alone: nothing written.
    assert!(score("ACME Formation", "recherche").is_none());
    // D368: the org lowered the confirming score to 40.
    let msg = json!({
        "payload": {"url": "https://acme.fr/", "title": "", "text": "", "legal_text": "ACME Formation, Lyon"},
        "org": {"id": "o1", "title": "ACME Formation"},
        "candidat": {"source": "email"}
    });
    let mut settings = serde_json::Map::new();
    settings.insert("presence_web.seuil_confirme".into(), json!(40));
    let r = pnex_node_starlark::executor::execute_with(
        &code_in(PRESENCE, "pw_score"),
        &msg,
        2000,
        &settings,
    )
    .unwrap()
    .return_json
    .unwrap();
    assert!(r[1].is_null() && r[0]["lien"]["score"] == 40, "{r}");
    assert_eq!(
        r[0]["accept"],
        "presence_web.seuil_confirme >= 40 (score 40)"
    );
}

#[test]
fn model_opinion_becomes_evidence_and_moves_the_confidence() {
    let opinion = |text: &str| {
        let msg = json!({
            "payload": {"text": text, "model": "m1"},
            "proposition": {"source_id": "o1", "target_id": "s1", "attributes": {}, "confidence": 70}
        });
        execute(&code_in(PRESENCE, "pw_avis"), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let r = opinion("OUI\nLes mentions citent le SIREN.");
    assert_eq!(
        r["payload"]["confidence"], 79,
        "never above the proposal band"
    );
    assert_eq!(
        r["payload"]["evidence"]["avis_llm"],
        "OUI\nLes mentions citent le SIREN."
    );
    assert_eq!(opinion("non, un annuaire")["payload"]["confidence"], 50);
    assert_eq!(opinion("PEUT-ETRE")["payload"]["confidence"], 70);
}

#[test]
fn signals_digest_lists_the_new_ones_newest_first() {
    let row = |title: &str, date: &str| {
        json!({"id": title, "title": title, "version": 1,
        "properties": {"date": date, "resume": "Modifications diverses"}})
    };
    let msg =
        json!({"payload": [row("ACME · Modif", "2026-10-01"), row("BETA · Vente", "2026-10-20")]});
    let r = execute(&code_in(PROSPECTION, "pb_digest"), &msg, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (
            r[0]["payload"]["trigger"].clone(),
            r[0]["payload"]["nombre"].clone()
        ),
        (json!(true), json!(2))
    );
    let text = r[0]["payload"]["texte"].as_str().unwrap();
    assert!(text.starts_with("- 2026-10-20 BETA · Vente"), "{text}");
    assert_eq!(text.lines().count(), 2);
    assert!(execute(
        &code_in(PROSPECTION, "pb_digest"),
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
        execute(&code_in(PROSPECTION_FR, f), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let r = run(
        "pl_preparer",
        json!({
        "payload": {"societe": "Mecachrome", "ville": "Amboise", "code_postal": "37530"},
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
    let r = run("pl_trouve", found.clone());
    assert_eq!(
        (r[0]["payload"]["siren"].clone(), r[0]["title"].clone()),
        (json!("429853351"), json!("MECACHROME FRANCE"))
    );
    assert_eq!(
        (
            r[1]["object_id"].clone(),
            r[1]["payload"]["rapprochement"].clone()
        ),
        (json!("c1"), json!("trouve"))
    );
    let r = run("pl_revoir", found);
    assert_eq!(r["payload"]["rapprochement"], "a_revoir");
    assert_eq!(
        r["payload"]["candidats"],
        "MECACHROME FRANCE (429853351, AMBOISE, 95)"
    );
    let r = run(
        "pl_revoir",
        json!({"payload": {"candidates": []}, "contact": {"id": "c2"}}),
    );
    assert_eq!(r["payload"]["rapprochement"], "introuvable");
}

#[test]
fn a_changed_page_becomes_a_signal_only_on_a_listed_topic() {
    let run = |f: &str, msg: serde_json::Value| {
        execute(&code_in(PROSPECTION, f), &msg, 2000)
            .unwrap()
            .return_json
            .unwrap()
    };
    let prep = run(
        "ps_preparer",
        json!({
        "payload": {"url": "https://acme.fr/actus", "texte": "ACME ouvre une usine à Lyon.", "compte": "org-1"},
        "object": {"id": "p1", "title": "Actualités", "version": 2}}),
    );
    assert_eq!(prep["payload"]["texte"], "ACME ouvre une usine à Lyon.");
    let page = prep["page"].clone();
    let r = run(
        "ps_signal",
        json!({"payload": {"text": "ouverture_site\n« ACME ouvre une usine à Lyon. »"},
        "page": page, "today": "2026-10-22"}),
    );
    assert_eq!(r[0]["payload"]["theme"], "ouverture_site");
    assert_eq!(
        r[0]["payload"]["cle"],
        "page:https://acme.fr/actus:2026-10-22"
    );
    assert_eq!(r[0]["payload"]["etat"], "a_valider");
    assert_eq!(
        r[0]["payload"]["extrait"],
        "« ACME ouvre une usine à Lyon. »"
    );
    assert_eq!(
        (
            r[1]["object_id"].clone(),
            r[1]["payload"]["modifiee"].clone()
        ),
        (json!("p1"), json!(false))
    );
    // Nothing announced, or a topic off the list: only marked read.
    for answer in ["aucun", "météo", ""] {
        let r = run(
            "ps_signal",
            json!({"payload": {"text": answer}, "page": page}),
        );
        assert!(r[0].is_null(), "{answer}: {r}");
        assert_eq!(r[1]["payload"]["modifiee"], false);
    }
    let link = run(
        "ps_lien",
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
    let mut r: Vec<String> = out["ecarts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["props"]["cle"]
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
        "RS7511",
        json!({"code": "RS7511"}),
        json!({
            "porte": [joined("w", "WEBMARKETING", json!({"siren": "790150577"}), json!({}))],
            "habilite_pour": [
                joined("ceased", "OF CESSE", json!({"siren": "111111111", "etat_administratif": "cesse", "qualiopi": true, "qualiopi_maj": "2026-04-11"}), json!({"declaree_fc": true, "perimetre": "les_deux"})),
                joined("eval", "OF EVAL", json!({"siren": "222222222", "qualiopi": false, "qualiopi_maj": "2026-04-11"}), json!({"declaree_fc": true, "perimetre": "evaluation"})),
                joined("old", "OF ANCIEN", json!({"siren": "333333333"}), json!({"perimetre": "les_deux", "etat_fc": "inactif", "date_fin": "2026-01-31", "source": "page_fc"})),
                joined("ok", "LA STRAT", json!({"siren": "830370342", "qualiopi": true, "qualiopi_maj": "2026-04-11"}), json!({"declaree_fc": true, "perimetre": "les_deux", "etat_fc": "actif"}))
            ],
            "porte_sur": [
                joined("o1", "SENZA · SEO", json!({"siret": "88024436300011", "id_action": "S_SEO", "vue_le": "2026-10-23"}), json!({})),
                joined("o2", "OF EVAL · SEO", json!({"siret": "22222222200010", "id_action": "E_SEO", "vue_le": "2026-10-23"}), json!({})),
                joined("o3", "OF ANCIEN · SEO", json!({"siret": "33333333300010", "id_action": "A_SEO", "vue_le": "2026-10-23"}), json!({})),
                joined("o4", "LA STRAT · SEO", json!({"siret": "83037034200024", "id_action": "P_SEO", "vue_le": "2026-10-23"}), json!({})),
                joined("o5", "WEBMARKETING · SEO", json!({"siret": "79015057700010", "id_action": "W_SEO", "vue_le": "2026-10-23"}), json!({}))
            ]
        }),
    );
    // A certification no certifier of the org holds is not watched.
    let other = row(
        "c2",
        "RS1",
        json!({"code": "RS1"}),
        json!({"porte_sur": [joined("o9", "X", json!({"siret": "99999999900010"}), json!({}))]}),
    );
    let msg = json!({"payload": [cert, other], "now": "2026-10-24T05:30:00+00:00", "today": "2026-10-24"});
    let out = execute(&code_of("qw_regles_reseau"), &msg, 5000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        rules_of(&out),
        vec!["ADM-01", "ADM-02", "CPF-01", "CPF-02", "CPF-03", "REG-01", "REG-02"],
        "{out}"
    );
    let e = out["ecarts"].as_array().unwrap();
    let cpf1 = e
        .iter()
        .find(|x| x["props"]["cle"].as_str().unwrap().starts_with("CPF-01"))
        .unwrap();
    assert_eq!(
        cpf1["props"]["cle"], "CPF-01:880244363:RS7511:S_SEO",
        "the certifier's own offer is not one"
    );
    assert_eq!(
        (
            cpf1["props"]["etat"].clone(),
            cpf1["props"]["gravite"].clone()
        ),
        (json!("propose"), json!("critique"))
    );
    assert_eq!(cpf1["liens"]["constate_dans"], "o1");
    assert!(
        e.iter()
            .all(|x| !x["props"]["cle"].as_str().unwrap().contains("830370342")),
        "LA STRAT is clean: {out}"
    );
}

/// The partner message of the website check: a partner reached from a
/// certification, its site and accreditation joined.
fn partner_msg() -> serde_json::Value {
    json!({
        "payload": {"siren": "830370342", "qualiopi": true, "qualiopi_maj": "2026-04-11", "qualiopi_bilans": false},
        "object": {"id": "ls", "type": "organization", "title": "LA STRAT", "version": 3},
        "links": {
            "a_pour_site": [{"direction": "out", "attributes": {}, "other": {"id": "site1", "title": "lastrat.fr", "properties": {"url_racine": "https://www.lastrat.fr/", "domaine": "lastrat.fr"}}}],
            "habilite_pour": [{"direction": "out", "attributes": {"etat_fc": "actif", "perimetre": "les_deux"}, "other": {"id": "c1", "title": "RS7511", "properties": {"code": "RS7511"}}}]
        },
        "reseau": {
            "RS7511": {"id": "c1", "intitule": "Développer son activité avec le référencement naturel", "statut": "actif", "certificateurs": ["WEBMARKETING & CO'M"]},
            "RS9999": {"id": "c9", "intitule": "Piloter un projet", "statut": "actif", "certificateurs": ["WEBMARKETING & CO'M"]}
        },
        "today": "2026-10-26", "now": "2026-10-26T02:00:00+00:00"
    })
}

#[test]
fn crawl_targets_carry_codes_titles_and_the_partner() {
    let out = execute(&code_of("qw_cibles"), &partner_msg(), 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(out["payload"]["url"], "https://www.lastrat.fr/");
    assert_eq!(out["payload"]["link_to"], "site1");
    let rules = out["payload"]["match"].to_string();
    assert!(
        rules.contains("RS\\\\s?7511")
            && rules.contains("référencement naturel")
            && rules.contains("WEBMARKETING"),
        "{rules}"
    );
    assert_eq!(
        out["partenaire"]["habilitations"]["RS7511"]["etat_fc"],
        "actif"
    );
    assert!(out["partenaire"]["habilitations"].get("RS9999").is_none());
    // No website: no target.
    let mut bare = partner_msg();
    bare["links"]["a_pour_site"] = json!([]);
    assert!(execute(&code_of("qw_cibles"), &bare, 2000)
        .unwrap()
        .return_json
        .is_none());
}

#[test]
fn website_rules_quote_the_page() {
    let target = execute(&code_of("qw_cibles"), &partner_msg(), 2000)
        .unwrap()
        .return_json
        .unwrap();
    let mut msg = partner_msg();
    msg["partenaire"] = target["partenaire"].clone();
    let page = |text: &str| {
        let mut m = msg.clone();
        m["payload"] = json!({"url": "https://www.lastrat.fr/seo", "title": "SEO", "text": text, "page_id": "p1", "object_id": "po1", "changed": true});
        execute(&code_of("qw_regles_web"), &m, 5000)
            .unwrap()
            .return_json
            .unwrap()
    };
    // A clean page: code, exact title, certifier, CPF while accredited.
    let clean = page("Formation Développer son activité avec le référencement naturel (RS7511), certificateur WEBMARKETING & CO'M. Éligible au CPF.");
    assert_eq!(rules_of(&clean), Vec::<String>::new(), "{clean}");
    // RS9999 (not accredited) offered with the CPF; a free gift; Qualiopi
    // on skills assessments not certified; RS7511 under a wrong title and
    // without its certifier.
    let bad = page("Préparez le RS 9999 Piloter un projet, éligible au CPF ! Tablette offerte pour toute inscription. \
        Organisme certifié Qualiopi pour vos bilans de compétences. Formation SEO RS7511 en 2 jours.");
    assert_eq!(
        rules_of(&bad),
        vec!["WEB-01", "WEB-03", "WEB-03", "WEB-04", "WEB-05", "WEB-06", "WEB-08"],
        "{bad}"
    );
    let e = bad["ecarts"].as_array().unwrap();
    for x in e {
        assert_eq!(x["props"]["etat"], "a_valider");
        let quote = x["props"]["extrait"].as_str().unwrap();
        assert!(
            !quote.is_empty() && msg_text_holds(&bad, quote),
            "word for word: {quote}"
        );
    }
    let web08 = e
        .iter()
        .find(|x| x["props"]["cle"].as_str().unwrap().starts_with("WEB-08"))
        .unwrap();
    assert_eq!(web08["props"]["cle"], "WEB-08:830370342:RS9999:p1");
    assert_eq!(
        (
            web08["liens"]["sur"].clone(),
            web08["liens"]["constate_dans"].clone()
        ),
        (json!("c9"), json!("po1"))
    );
    // The title without its code (WEB-02).
    let no_code = page("Notre parcours Développer son activité avec le référencement naturel, par WEBMARKETING & CO'M.");
    assert_eq!(rules_of(&no_code), vec!["WEB-02"], "{no_code}");
    // The site summary: nothing relevant (COL-01), a finding to propose.
    let mut m = msg.clone();
    m["payload"] = json!({"url": "https://www.lastrat.fr", "pages": 40, "relevant": 0, "truncated": false, "mission_id": "m1"});
    let col = execute(&code_of("qw_regles_web"), &m, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(rules_of(&col), vec!["COL-01"]);
    assert_eq!(col["ecarts"][0]["props"]["etat"], "propose");
}

/// Every excerpt is cut from the page text of the test (word for word).
fn msg_text_holds(_out: &serde_json::Value, quote: &str) -> bool {
    "Préparez le RS 9999 Piloter un projet, éligible au CPF ! Tablette offerte pour toute inscription. \
        Organisme certifié Qualiopi pour vos bilans de compétences. Formation SEO RS7511 en 2 jours."
        .contains(quote)
}

#[test]
fn a_finding_becomes_a_write_then_its_links() {
    let e = json!({"payload": {"props": {"cle": "WEB-08:1:RS1:p", "etat": "a_valider"}, "title": "WEB-08 · X", "liens": {"concerne": "o", "sur": "c", "constate_dans": ""}},
                   "object": {"id": "stale"}, "links": {"big": []}});
    let w = execute(&code_of("qw_ecart"), &e, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(
        (
            w["payload"]["cle"].clone(),
            w["title"].clone(),
            w["object"].clone()
        ),
        (json!("WEB-08:1:RS1:p"), json!("WEB-08 · X"), json!(null))
    );
    let written = json!({"object": {"id": "e1"}, "liens": w["liens"].clone()});
    let l = execute(&code_of("qw_liens"), &written, 2000)
        .unwrap()
        .return_json
        .unwrap();
    assert_eq!(l["concerne"], json!({"source_id": "e1", "target_id": "o"}));
    assert_eq!(l["sur"]["target_id"], "c");
    assert!(l.get("constate_dans").is_none());
}
