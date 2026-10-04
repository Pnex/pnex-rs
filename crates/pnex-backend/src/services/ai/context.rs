//! Prompt système de l'assistant — rôle + garde-fous, règles du moteur de
//! flow, contexte vivant de l'org (devices, pins, télémétrie, flows) et
//! contexte de page. Reconstruit à chaque tour de chat : pas de RAG, la
//! « connaissance » de l'agent est le bundle fraîche + les outils.
//! The knowledge card of the current page is injected (D142).

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;

use crate::models::_entities::{
    device_capability_instances, device_registries, flow_versions, flows,
};
use crate::services::openobserve::client::Client;

/// Contexte de page envoyé par le front (route courante + entité ouverte).
#[derive(Debug, Default, Deserialize)]
pub struct PageContext {
    pub page: Option<String>,
    pub flow_id: Option<i64>,
    pub device_id: Option<i64>,
}

/// Caps défensifs du bundle (budget de prompt borné).
const DEVICES_IN_PROMPT: usize = 50;
const PINS_PER_DEVICE: usize = 8;
const SERIES_IN_PROMPT: usize = 30;
const FLOWS_IN_PROMPT: usize = 20;

/// Prompt système complet. `o2: None` (O2 non configuré) → section
/// télémétrie dégradée annoncée au modèle.
pub async fn build_system_prompt(
    db: &DatabaseConnection,
    o2: Option<&Client>,
    org_id: i64,
    org_name: &str,
    language: &str,
    page: Option<&PageContext>,
) -> String {
    let mut p = String::with_capacity(8_000);

    // ── 1. Rôle & garde-fous ──
    p.push_str(
        r#"Tu es l'assistant PNEX, intégré à la plateforme IoT PNEX (devices ESP, télémétrie OpenObserve, flows ETL type Node-RED).
GARDE-FOUS ABSOLUS :
- Tu peux créer et modifier des brouillons (drafts) de flows via les outils create_flow/update_flow.
- A deployed (running) flow cannot be edited: update_flow refuses it (ai-flow-running). Tell the user to stop the flow in the flow editor, then to ask again; never suggest a workaround such as copying the flow to bypass the rule.
- You never act on the physical world: no device command, no control or shared-memory write. Only a flow the user deploys acts on devices.
- Tu ne peux JAMAIS déployer un flow (le déploiement est versionné et reste un geste humain dans l'éditeur), ni supprimer quoi que ce soit (devices, flows, versions), ni pousser la moindre commande device (set_mode/write/subscribe). Ces capacités n'existent pas dans tes outils : ne les promets jamais, n'improvise jamais ces actions, et si l'utilisateur les demande, explique que c'est réservé à l'interface PNEX.
- Les outils d'écriture exigent le rôle owner, admin ou member ; sinon l'outil échoue et tu l'expliques.
- Ne déclare jamais "c'est fait" sans un outil ok:true qui le prouve ; cite les ids internes (flow #id) quand tu les connais.

RÈGLES FLOW :
"#,
    );

    // ── 2. Engine rules: generated from the core node docs (D142) ──
    for (key, rule) in pnex_core::FLOW_AUTHORING_RULES {
        p.push_str(&format!("- {key}: {rule}\n"));
    }
    p.push_str(
        "- Always validate BEFORE writing: validate_flow_graph + validate_calc_expression, fix, then create_flow/update_flow.\n\
         - Node kinds, config fields and ports: call describe_node_types (never guess a kind or a field).\n\n",
    );

    // ── 3. Contexte org ──
    p.push_str(&format!("CONTEXTE ORG « {org_name} » :\n"));
    let devices = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .unwrap_or_default();
    let devices: Vec<_> = devices.into_iter().take(DEVICES_IN_PROMPT).collect();
    let devices_line = devices
        .iter()
        .map(|d| {
            format!(
                "#{} {} ({})",
                d.id,
                d.device_id,
                if d.active { "actif" } else { "inactif" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    p.push_str(&format!(
        "Devices (id interne, slug, état) : {}\n",
        if devices.is_empty() {
            "aucun".to_string()
        } else {
            devices_line
        }
    ));
    // Pins par device (bundle compact, cap par device).
    for d in &devices {
        let pins = device_capability_instances::Entity::find()
            .filter(device_capability_instances::Column::DeviceRegistryId.eq(d.id))
            .order_by_asc(device_capability_instances::Column::Gpio)
            .all(db)
            .await
            .unwrap_or_default();
        let pins_line: String = pins
            .iter()
            .take(PINS_PER_DEVICE)
            .map(|pin| format!("{}(gpio{})", pin.label, pin.gpio))
            .collect::<Vec<_>>()
            .join(", ");
        if !pins_line.is_empty() {
            p.push_str(&format!(
                "  - #{} {} : pins {}\n",
                d.id, d.device_id, pins_line
            ));
        }
    }
    // Télémétrie (catalogue O2 — dégradé silencieux si O2 absent).
    let catalog = crate::services::visualization::series_catalog(db, o2, org_id).await;
    if catalog.available {
        let series_line: String = catalog
            .series
            .iter()
            .take(SERIES_IN_PROMPT)
            .map(|s| format!("{}×{}", s.metric, s.device_id))
            .collect::<Vec<_>>()
            .join(", ");
        p.push_str(&format!(
            "Séries télémétrie OpenObserve (métrique×device) : {}{}\n",
            if series_line.is_empty() {
                "(vide)"
            } else {
                &series_line
            },
            if catalog.series.len() > SERIES_IN_PROMPT {
                " …"
            } else {
                ""
            }
        ));
    } else {
        p.push_str("Télémétrie OpenObserve : indisponible (O2 non configuré ou injoignable) — l'outil query_telemetry le confirmera.\n");
    }
    // Flows existants (nom | statut | dernière version).
    let flow_rows = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .order_by_desc(flows::Column::Id)
        .find_with_related(flow_versions::Entity)
        .all(db)
        .await
        .unwrap_or_default();
    let flows_line: String = flow_rows
        .iter()
        .take(FLOWS_IN_PROMPT)
        .map(|(f, versions)| {
            let latest = versions.iter().map(|v| v.version_number).max().unwrap_or(0);
            format!("#{} {} [{}] v{}", f.id, f.name, f.status, latest)
        })
        .collect::<Vec<_>>()
        .join(", ");
    p.push_str(&format!(
        "Flows : {}\n\n",
        if flows_line.is_empty() {
            "aucun".to_string()
        } else {
            flows_line
        }
    ));
    // ── 4. Page courante ──
    if let Some(page) = page {
        if let Some(flow_id) = page.flow_id {
            if let Ok(Some(row)) = flows::Entity::find_by_id(flow_id)
                .filter(flows::Column::OrgId.eq(org_id))
                .one(db)
                .await
            {
                p.push_str(&format!(
                    "L'utilisateur regarde le flow #{} « {} » (statut {}). Détail : outil get_flow.\n",
                    row.id, row.name, row.status
                ));
            }
        }
        if let Some(device_id) = page.device_id {
            p.push_str(&format!(
                "L'utilisateur regarde le device #{} — détail : outil get_device_pins.\n",
                device_id
            ));
        }
        // Knowledge card of the page the user is on (D142).
        if let Some(card) = page
            .page
            .as_deref()
            .and_then(super::knowledge::card_for_page)
        {
            p.push_str(&format!(
                "CURRENT PAGE: {} — {} (full card: read_knowledge \"{}\").\n",
                card.title, card.summary, card.id
            ));
        }
    }
    p.push_str(
        "KNOWLEDGE: for how-to, where-is or why-does-it-fail questions, call search_knowledge then read_knowledge, and diagnose_device / diagnose_flow for the facts of a given device or flow. Describe gestures in the PneX UI only: never a command line, an API call or a config file.\n\n",
    );

    // ── 5. Language ──
    p.push_str(lang_instruction(language));
    p
}

/// Language instruction closing the system prompt. `language` is a full
/// BCP-47 tag ("fr-FR"/"en-US" from the front): match on the primary subtag
/// only. The tag is the DEFAULT; the reply mirrors the language of the
/// user's last message.
fn lang_instruction(language: &str) -> &'static str {
    match language.split('-').next().unwrap_or("fr") {
        "en" => "LANGUAGE: Reply in the same language as the user's most recent message (an English question gets an English answer, a French question gets a French answer). If the language is ambiguous, reply in English.",
        _ => "LANGUE : réponds dans la langue du dernier message de l'utilisateur (une question en anglais reçoit une réponse en anglais, une question en français une réponse en français). Si la langue est ambiguë, réponds en français.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_context_parse() {
        let pc: PageContext =
            serde_json::from_str(r#"{"page": "/flows", "flow_id": 3}"#).expect("parsed");
        assert_eq!(pc.flow_id, Some(3));
        assert_eq!(pc.device_id, None);
    }

    #[test]
    fn lang_instruction_matches_primary_subtag() {
        // Full tags from the front must match, not just bare "en"/"fr".
        assert!(lang_instruction("en-US").starts_with("LANGUAGE"));
        assert!(lang_instruction("en").starts_with("LANGUAGE"));
        assert!(lang_instruction("fr-FR").starts_with("LANGUE"));
        assert!(lang_instruction("fr").starts_with("LANGUE"));
        // Unknown tags fall back to the French default.
        assert!(lang_instruction("de-DE").starts_with("LANGUE"));
        assert!(lang_instruction("").starts_with("LANGUE"));
    }
}
