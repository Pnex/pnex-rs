//! Pages of a case (cases.md K8): a page started from a template of an
//! installed pack joins the case as a note; the page designated as the
//! conclusion (`conclusion_page`) gets a named version when the case
//! concludes, so the conclusion read later is the one concluded on.

use pnex_core::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::ontology::api::{CasePageInput, PageTemplate};
use pnex_core::ontology::KIND_CASE;
use pnex_core::pages::{DocSummary, MAX_TITLE_CHARS};
use pnex_core::resources::KIND_DOC;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use uuid::Uuid;

use super::{cases, invalid, links, objects, packs, Actor, OntologyError, Result};
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
    let case = objects::find(db, org_id, case_id).await?;
    if case.type_key != KIND_CASE || case.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
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
        Some(key) => Some(
            templates(db, org_id)
                .await?
                .into_iter()
                .find(|t| t.key == key)
                .ok_or_else(|| invalid("template", FIELD_INVALID))?
                .body,
        ),
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
    links::create(
        db,
        org_id,
        actor,
        &cases::member(&case.id.to_string(), &page.id.to_string(), "note"),
    )
    .await?;
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
