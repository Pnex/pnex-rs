//! The Starlark rules shipped by the `quali_watch` pack (partner-watch.md
//! D314, D316) run as the flow runtime runs them.

use pnex_node_starlark::executor::execute;
use serde_json::json;

const PACK: &str = include_str!("../../pnex-backend/packs/quali_watch.yaml");

/// The `code: |` block of the pack function `name` (6-space indent).
fn code_of(name: &str) -> String {
    let start = PACK
        .find(&format!("- name: {name}\n"))
        .unwrap_or_else(|| panic!("function {name}"));
    let rest = &PACK[start..];
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
