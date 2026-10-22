//! The Starlark rules shipped by the `quali_watch` pack (partner-watch.md
//! D314, D316) and the target score of `prospection_b2b`
//! (account-mapping.md D336) run as the flow runtime runs them.

use pnex_node_starlark::executor::execute;
use serde_json::json;

const PACK: &str = include_str!("../../pnex-backend/packs/quali_watch.yaml");
const PROSPECTION: &str = include_str!("../../pnex-backend/packs/prospection_b2b.yaml");
const PRESENCE: &str = include_str!("../../pnex-backend/packs/presence_web.yaml");

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
    assert_eq!(r[0]["lien"]["statut"], "confirme");
    assert_eq!(r[0]["lien"]["score"], 100);
    assert_eq!(
        (
            r[0]["lien"]["preuve_siren"].clone(),
            r[0]["lien"]["rang"].clone()
        ),
        (json!(true), json!(2))
    );
    // The name and the org's own e-mail domain: for a human to check.
    let r = score("ACME Formation, organisme à Lyon", "email").unwrap();
    assert_eq!(
        (
            r[0]["lien"]["statut"].clone(),
            r[0]["lien"]["score"].clone()
        ),
        (json!("propose"), json!(40))
    );
    // The name alone: nothing written.
    assert!(score("ACME Formation", "recherche").is_none());
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
