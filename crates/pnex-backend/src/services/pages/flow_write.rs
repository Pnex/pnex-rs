//! Pages written by flows (media-vision-studio.md D209): a `page_write`
//! node sends Markdown; the server creates a page or appends to one of the
//! org, then freezes a named version stamped with the flow and its AI
//! provenance (D210). Appends go through [`super::write_server`] (same
//! checks as an editor, live to open editors).

use chrono::{DateTime, Utc};
use pnex_core::pages::{
    PageWriteMode, PageWriteRequest, PageWriteResponse, MAX_DOC_BYTES, MAX_FLOW_MARKDOWN,
    MAX_TITLE_CHARS, MAX_VERSION_LABEL_CHARS,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use super::{create_version_by, load, seed, write_server, PageError, VersionAuthor};
use crate::models::_entities::docs;

/// Why a flow write was refused (`code` for the node status).
#[derive(Debug, thiserror::Error)]
pub enum FlowPageError {
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("page not found in the organization")]
    NotFound,
    #[error(transparent)]
    Page(#[from] PageError),
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

impl FlowPageError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "page-write-invalid",
            Self::NotFound => "page-not-found",
            Self::Page(PageError::Refused(_)) => "page-write-refused",
            Self::Page(_) | Self::Db(_) => "internal",
        }
    }
}

/// `{date}`, `{time}`, `{week}` of a title or version label (UTC).
pub fn render(template: &str, now: DateTime<Utc>) -> String {
    template
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{time}", &now.format("%H:%M").to_string())
        .replace("{week}", &now.format("%G-W%V").to_string())
}

/// Writes `req.markdown` and freezes a named version.
pub async fn write(
    db: &DatabaseConnection,
    valkey_url: Option<String>,
    req: &PageWriteRequest,
) -> Result<PageWriteResponse, FlowPageError> {
    if req.markdown.trim().is_empty() || req.markdown.chars().count() > MAX_FLOW_MARKDOWN {
        return Err(FlowPageError::Invalid("markdown"));
    }
    let now = Utc::now();
    let label: String = render(req.version_label.trim(), now)
        .chars()
        .take(MAX_VERSION_LABEL_CHARS)
        .collect();
    if label.is_empty() {
        return Err(FlowPageError::Invalid("version_label"));
    }
    let (doc_id, title) = match req.mode {
        PageWriteMode::NewPage => {
            let title: String = render(req.title.trim(), now)
                .chars()
                .take(MAX_TITLE_CHARS)
                .collect();
            if title.is_empty() {
                return Err(FlowPageError::Invalid("title"));
            }
            let state = seed::from_markdown(&req.markdown);
            if state.len() > MAX_DOC_BYTES {
                return Err(PageError::Refused("page too large".into()).into());
            }
            let row = docs::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(req.org_id),
                title: Set(title.clone()),
                snapshot: Set(Some(state)),
                snapshot_seq: Set(0),
                created_by: Set(None),
                ..Default::default()
            }
            .insert(db)
            .await?;
            (row.id, title)
        }
        PageWriteMode::Append => {
            let id = Uuid::parse_str(req.page_id.trim()).map_err(|_| FlowPageError::NotFound)?;
            let row = docs::Entity::find_by_id(id)
                .filter(docs::Column::OrgId.eq(req.org_id))
                .one(db)
                .await?
                .ok_or(FlowPageError::NotFound)?;
            let doc = load(db, id).await?;
            let update = seed::append_update(&doc, &req.markdown);
            write_server(db, valkey_url, id, update).await?;
            (id, row.title)
        }
    };
    let version = create_version_by(
        db,
        doc_id,
        req.org_id,
        &label,
        VersionAuthor {
            user_id: None,
            flow_id: (req.flow_id > 0).then_some(req.flow_id),
            ai_generated: req.ai_generated,
        },
    )
    .await?;
    Ok(PageWriteResponse {
        page_id: doc_id.to_string(),
        title,
        version: version.version_number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_render_in_utc() {
        let t = DateTime::parse_from_rfc3339("2026-10-12T07:05:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            render("Report {date} {time} ({week})", t),
            "Report 2026-10-12 07:05 (2026-W42)"
        );
    }
}
