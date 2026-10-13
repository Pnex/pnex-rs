//! Pages written by flows (media-vision-studio.md D209): a `page_write`
//! node sends Markdown; the server creates a page or appends to one of the
//! org, then freezes a named version stamped with the flow and its AI
//! provenance (D210). Appends go through [`super::write_server`] (same
//! checks as an editor, live to open editors).

use chrono::{DateTime, Utc};
use pnex_core::pages::{
    PagePdfRequest, PagePdfResponse, PagePdfSource, PagePdfStatus, PageWriteMode, PageWriteRequest,
    PageWriteResponse, MAX_DOC_BYTES, MAX_FLOW_MARKDOWN, MAX_TITLE_CHARS, MAX_VERSION_LABEL_CHARS,
};
use sea_orm::sea_query::Condition;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use super::{create_version_by, load, seed, write_server, PageError, VersionAuthor};
use crate::models::_entities::{doc_version_exports, doc_versions, docs, media_assets};

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

/// PDF of a page version for a `page_pdf` node (D209, D210): the version
/// from the message, or the latest one allowed to leave not yet exported
/// by this node; an AI version waits for approval when required. The PDF
/// becomes a `document` of the media library; the export is recorded.
pub async fn export_pdf(
    ctx: &loco_rs::app::AppContext,
    req: &PagePdfRequest,
) -> Result<PagePdfResponse, FlowPageError> {
    let db = &ctx.db;
    let id = Uuid::parse_str(req.page_id.trim()).map_err(|_| FlowPageError::NotFound)?;
    let page = docs::Entity::find_by_id(id)
        .filter(docs::Column::OrgId.eq(req.org_id))
        .one(db)
        .await?
        .ok_or(FlowPageError::NotFound)?;
    let node_key = format!("{}:{}", req.flow_id, req.node_id);
    let answer = |status, version: Option<i64>| PagePdfResponse {
        status,
        page_id: id.to_string(),
        version,
        title: page.title.clone(),
        asset_id: None,
        filename: None,
    };
    let version = match req.source {
        PagePdfSource::Message => {
            let n = req.version.ok_or(FlowPageError::Invalid("version"))?;
            doc_versions::Entity::find()
                .filter(doc_versions::Column::DocId.eq(id))
                .filter(doc_versions::Column::VersionNumber.eq(n))
                .one(db)
                .await?
                .ok_or(FlowPageError::NotFound)?
        }
        PagePdfSource::LatestSendable => {
            let sendable = Condition::any()
                .add(doc_versions::Column::ApprovedAt.is_not_null())
                .add(doc_versions::Column::AiGenerated.eq(false));
            let Some(v) = doc_versions::Entity::find()
                .filter(doc_versions::Column::DocId.eq(id))
                .filter(sendable)
                .order_by_desc(doc_versions::Column::VersionNumber)
                .one(db)
                .await?
            else {
                return Ok(answer(PagePdfStatus::NothingNew, None));
            };
            let sent = doc_version_exports::Entity::find()
                .filter(doc_version_exports::Column::DocVersionId.eq(v.id))
                .filter(doc_version_exports::Column::NodeKey.eq(node_key.clone()))
                .one(db)
                .await?;
            if sent.is_some() {
                return Ok(answer(PagePdfStatus::NothingNew, Some(v.version_number)));
            }
            v
        }
    };
    if req.require_approval && version.ai_generated && version.approved_at.is_none() {
        return Ok(answer(
            PagePdfStatus::NeedsApproval,
            Some(version.version_number),
        ));
    }
    let rendered =
        super::pdf::render_version(db, req.org_id, id, version.version_number, &req.language)
            .await?
            .ok_or(FlowPageError::NotFound)?;
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(req.org_id),
        kind: Set("document".into()),
        name: Set(rendered.filename.clone()),
        description: Set(Some(format!(
            "PDF of version {} of the page \"{}\" (flow #{}).",
            version.version_number, page.title, req.flow_id
        ))),
        metadata: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await?;
    crate::controllers::media::write_version(
        ctx,
        req.org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: rendered.filename.clone(),
            content_type: "application/pdf".into(),
            note: None,
            bytes: axum::body::Bytes::from(rendered.bytes),
        },
        &settings,
    )
    .await
    .map_err(|e| PageError::Refused(format!("pdf not stored: {e}")))?;
    doc_version_exports::ActiveModel {
        org_id: Set(req.org_id),
        doc_version_id: Set(version.id),
        node_key: Set(node_key),
        asset_id: Set(Some(asset.id)),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(PagePdfResponse {
        asset_id: Some(asset.id.to_string()),
        filename: Some(rendered.filename),
        ..answer(PagePdfStatus::Ready, Some(version.version_number))
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
