//! Notification templates (D49–D54): the single write path of the HTTP
//! controller and of the assistant (ai-assistant.md §9.3, extension rule).
//!
//! A template is edited in place; deployed flows keep the snapshot taken
//! at deploy, so a change reaches them only when the user redeploys.

use std::collections::BTreeMap;

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::models::_entities::notify_templates;
use pnex_core::TemplateVar;

/// What a create or an update receives.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TemplateInput {
    pub name: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub vars: Vec<TemplateVar>,
}

#[derive(Debug)]
pub enum TemplateWriteError {
    /// Field error: `(field, machine token)`.
    Field(&'static str, String),
    /// The template does not render (minijinja syntax).
    Render(pnex_notify::NotifyError),
    /// Another template of the org has this name.
    NameTaken,
    Db,
}

fn validate(input: &TemplateInput) -> Result<String, TemplateWriteError> {
    use pnex_core::err_codes;
    let name = input.name.trim();
    if name.is_empty() {
        return Err(TemplateWriteError::Field(
            "name",
            err_codes::FIELD_REQUIRED.into(),
        ));
    }
    if name.chars().count() > 200 {
        return Err(TemplateWriteError::Field(
            "name",
            format!("{}:200", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    if input.body.trim().is_empty() {
        return Err(TemplateWriteError::Field(
            "body",
            err_codes::FIELD_REQUIRED.into(),
        ));
    }
    // Render against empty contexts: a broken template is refused at save
    // time (an error at send time is much harder to debug).
    if let Err(e) = pnex_notify::render(
        input.subject.as_deref(),
        &input.body,
        &Default::default(),
        serde_json::json!({}),
        serde_json::json!({}),
    ) {
        if matches!(e, pnex_notify::NotifyError::Render(_)) {
            return Err(TemplateWriteError::Render(e));
        }
    }
    Ok(name.to_string())
}

/// Variables detected in the subject and body merged with the declared
/// ones: a detected variable missing from the declaration is appended
/// (empty example), a declared example is kept. Declared order first, then
/// the detected ones — the order of the notify node's input rows.
fn merge_vars(input: &TemplateInput) -> Vec<TemplateVar> {
    let mut out = input.vars.clone();
    for name in pnex_notify::template_vars(input.subject.as_deref(), &input.body) {
        if !out.iter().any(|v| v.name == name) {
            out.push(TemplateVar {
                name,
                example: String::new(),
            });
        }
    }
    out
}

async fn ensure_name_free(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
) -> Result<(), TemplateWriteError> {
    let clash = notify_templates::Entity::find()
        .filter(notify_templates::Column::OrgId.eq(org_id))
        .filter(notify_templates::Column::Name.eq(name))
        .one(db)
        .await
        .map_err(|_| TemplateWriteError::Db)?
        .is_some();
    if clash {
        return Err(TemplateWriteError::NameTaken);
    }
    Ok(())
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    input: &TemplateInput,
) -> Result<notify_templates::Model, TemplateWriteError> {
    let name = validate(input)?;
    ensure_name_free(db, org_id, &name).await?;
    let vars = serde_json::to_value(merge_vars(input)).map_err(|_| TemplateWriteError::Db)?;
    notify_templates::ActiveModel {
        org_id: Set(org_id),
        name: Set(name),
        subject: Set(input.subject.clone()),
        body: Set(input.body.clone()),
        vars: Set(vars),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(|_| TemplateWriteError::Db)
}

pub async fn update(
    db: &DatabaseConnection,
    current: notify_templates::Model,
    input: &TemplateInput,
) -> Result<notify_templates::Model, TemplateWriteError> {
    let name = validate(input)?;
    if name != current.name {
        ensure_name_free(db, current.org_id, &name).await?;
    }
    let vars = serde_json::to_value(merge_vars(input)).map_err(|_| TemplateWriteError::Db)?;
    let mut active: notify_templates::ActiveModel = current.into();
    active.name = Set(name);
    active.subject = Set(input.subject.clone());
    active.body = Set(input.body.clone());
    active.vars = Set(vars);
    active.update(db).await.map_err(|_| TemplateWriteError::Db)
}

/// Renders a stored template (never sends): missing variables take their
/// declared non-empty example (an empty one would mask a payload key of
/// the same name, vars taking precedence in the context).
pub fn preview(
    m: &notify_templates::Model,
    mut vars: BTreeMap<String, String>,
    payload: serde_json::Value,
) -> Result<pnex_notify::Message, pnex_notify::NotifyError> {
    let declared: Vec<TemplateVar> = serde_json::from_value(m.vars.clone()).unwrap_or_default();
    for v in declared {
        if !v.example.is_empty() {
            vars.entry(v.name).or_insert(v.example);
        }
    }
    pnex_notify::render(
        m.subject.as_deref(),
        &m.body,
        &vars,
        payload,
        serde_json::json!({ "preview": true }),
    )
}
