//! Pages of a case (cases.md K8): a page started from a template of an
//! installed pack joins the case as an undated piece; the page designated as the
//! conclusion (`conclusion_page`) gets a named version when the case
//! concludes, so the conclusion read later is the one concluded on.

use pnex_core::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::ontology::api::{CasePageInput, PageTemplate};
use pnex_core::pages::{DocSummary, MAX_TITLE_CHARS};
use pnex_core::resources::KIND_DOC;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use uuid::Uuid;

use super::super::{invalid, links, objects, packs, Actor, Result};
use super::{case_of, in_case};
use crate::models::_entities::docs;
use crate::services::pages;

/// Version label of the conclusion page at the conclusion.
const CONCLUSION_LABEL: &str = "Conclusion";

pub async fn templates(db: &DatabaseConnection, org_id: i64) -> Result<Vec<PageTemplate>> {
    packs::page_templates(db, org_id).await
}

pub async fn add_page(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CasePageInput,
) -> Result<DocSummary> {
    case_of(db, org_id, case_id).await?;
    let title = input.title.trim();
    if title.is_empty() {
        return Err(invalid("title", FIELD_REQUIRED));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(invalid(
            "title",
            format!("{FIELD_MAX_LENGTH}:{MAX_TITLE_CHARS}"),
        ));
    }
    let body = match input.template.as_deref().filter(|k| !k.is_empty()) {
        None => None,
        Some(key) => {
            let t = templates(db, org_id)
                .await?
                .into_iter()
                .find(|t| t.key == key)
                .ok_or_else(|| invalid("template", FIELD_INVALID))?;
            let lang = input.lang.as_deref().unwrap_or("en");
            Some(pnex_core::ontology::localized(&t.body, &t.bodies, lang).to_string())
        }
    };
    let row = docs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        title: Set(title.into()),
        snapshot: Set(body.map(|b| pages::seed::from_markdown(&b))),
        snapshot_seq: Set(0),
        created_by: Set(actor.user_id),
        ..Default::default()
    }
    .insert(db)
    .await?;
    // The identity trigger gave the page its object row.
    let page = objects::find_by_native(db, org_id, KIND_DOC, &row.id.to_string()).await?;
    links::create(db, org_id, actor, &in_case(case_id, page.id)).await?;
    Ok(DocSummary {
        id: row.id,
        title: row.title,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    })
}

/// Called once a case turned `concluded`: freezes its conclusion page
/// (`page` = the object id held by `conclusion_page`).
/// A failure is logged, never undoes the status change already saved.
pub async fn on_concluded(db: &DatabaseConnection, org_id: i64, actor: &Actor, page: &str) {
    let Ok(page) = Uuid::parse_str(page) else {
        return;
    };
    let doc = match objects::find(db, org_id, page).await {
        Ok(o) => Uuid::parse_str(&o.native_id).ok(),
        Err(_) => None,
    };
    let Some(doc) = doc else {
        return;
    };
    if let Err(e) = pages::create_version(db, doc, org_id, CONCLUSION_LABEL, actor.user_id).await {
        tracing::warn!(doc_id = %doc, error = %e, "cases: conclusion page not versioned");
    }
}

/// A page from a pack template, filled from the object `id` (account-mapping.md
/// D340): the template's placeholders read the object, its open links and
/// its hierarchies today; the page is then linked to the object by the
/// template's link. A snapshot of the day, edited freely afterwards.
/// Filled with what the author reads: `hide` masks the object, its links
/// and its hierarchies as on the author's own reads (D443, SEC-53).
pub async fn from_object(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    input: &pnex_core::ontology::api::ObjectPageInput,
    hide: super::super::redact::Redaction,
) -> Result<DocSummary> {
    use pnex_core::ontology::page_render::{hierarchies, render, RenderCtx};
    let found = objects::find(db, org_id, id).await?;
    if found.valid_to.is_some() {
        return Err(super::super::OntologyError::NotFound);
    }
    let schema = super::super::schema(db, org_id).await?;
    let mut obj = objects::view(&found, &schema);
    hide.object(&schema, &mut obj);
    let t = templates(db, org_id)
        .await?
        .into_iter()
        .find(|t| t.key == input.template && t.subject.as_deref() == Some(obj.type_key.as_str()))
        .ok_or_else(|| invalid("template", FIELD_INVALID))?;
    // Bounded: a language tag is a few letters (`fr-FR`).
    let lang: String = input
        .lang
        .as_deref()
        .unwrap_or("en")
        .chars()
        .take(16)
        .collect();
    let title = match input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(t) => t.to_string(),
        None => format!(
            "{} — {}",
            pnex_core::ontology::localized(&t.name, &t.names, &lang),
            obj.title
        ),
    };
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(invalid(
            "title",
            format!("{FIELD_MAX_LENGTH}:{MAX_TITLE_CHARS}"),
        ));
    }
    let mut open = links::for_object(db, org_id, id, None, false).await?;
    for l in &mut open {
        hide.link(&schema, l);
    }
    let template_body = pnex_core::ontology::localized(&t.body, &t.bodies, &lang);
    let mut trees = std::collections::HashMap::new();
    for k in hierarchies(template_body) {
        // A hierarchy the org lacks leaves its placeholder as written.
        // shortcut: the page keeps what its author reads; masking it again
        // for a later reader needs object visibility (iam.md I5).
        if let Ok(mut v) = super::super::hierarchy::tree(db, org_id, id, Some(&k), None, hide).await
        {
            hide.hierarchy(&schema, &mut v);
            if v.key == k {
                trees.insert(k, v);
            }
        }
    }
    let props = obj.properties.clone();
    let defs = super::super::types::get(db, org_id, &obj.type_key)
        .await
        .map(|t| t.def.properties)
        .unwrap_or_default();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let id_text = id.to_string();
    let body = render(
        template_body,
        &RenderCtx {
            id: &id_text,
            title: &obj.title,
            properties: &props,
            defs: &defs,
            lang: &lang,
            today: &today,
            links: &open,
            trees: &trees,
        },
    );
    let row = docs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        title: Set(title),
        snapshot: Set(Some(pages::seed::from_markdown(&body))),
        snapshot_seq: Set(0),
        created_by: Set(actor.user_id),
        ..Default::default()
    }
    .insert(db)
    .await?;
    if let Some(link_type) = &t.link {
        let page = objects::find_by_native(db, org_id, KIND_DOC, &row.id.to_string()).await?;
        let input = pnex_core::ontology::api::LinkInput {
            link_type: link_type.clone(),
            source_id: page.id.to_string(),
            target_id: id.to_string(),
            ..Default::default()
        };
        links::create(db, org_id, actor, &input).await?;
    }
    Ok(DocSummary {
        id: row.id,
        title: row.title,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    })
}
