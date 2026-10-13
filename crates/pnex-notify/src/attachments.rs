//! Files attached by a `pnex-notify` node (D209): the runtime reads them
//! from the backend (`/internal/flow/media/{id}`, flow service token and
//! fence), which only serves the PDFs exported by `page_pdf` nodes of the org.

use base64::Engine as _;
use serde::Deserialize;
use uuid::Uuid;

use crate::channels::CLIENT;
use crate::error::NotifyError;
use crate::secrets::FLOW_TOKEN_HEADER;
use crate::{Attachment, ATTACHMENT_MAX_BYTES};

/// Answer of the backend endpoint.
#[derive(Debug, Deserialize)]
pub struct MediaFile {
    pub filename: String,
    pub content_type: String,
    /// Standard base64.
    pub data: String,
}

impl MediaFile {
    pub fn decode(self) -> Result<Attachment, NotifyError> {
        let data = base64::engine::general_purpose::STANDARD
            .decode(self.data.as_bytes())
            .map_err(|e| NotifyError::Config(format!("attachment unreadable: {e}")))?;
        if data.len() > ATTACHMENT_MAX_BYTES {
            return Err(NotifyError::Config("attachment too large".into()));
        }
        Ok(Attachment {
            filename: self.filename,
            content_type: self.content_type,
            data,
        })
    }
}

/// The attachment `id` of org `org_id`.
pub async fn fetch(
    base_url: &str,
    token: &str,
    org_id: i64,
    id: Uuid,
) -> Result<Attachment, NotifyError> {
    let resp = CLIENT
        .get(format!("{}/{id}", base_url.trim_end_matches('/')))
        .query(&[("org_id", org_id)])
        .header(FLOW_TOKEN_HEADER, token)
        .header(
            pnex_core::FLOW_WORKER_HEADER,
            pnex_core::flow_worker_fence().unwrap_or_default(),
        )
        .send()
        .await
        .map_err(|e| NotifyError::Network(format!("media endpoint unreachable: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(NotifyError::Http {
            status: status.as_u16(),
        });
    }
    resp.json::<MediaFile>()
        .await
        .map_err(|e| NotifyError::Network(format!("media endpoint answer unreadable: {e}")))?
        .decode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_file_decodes() {
        let f = MediaFile {
            filename: "r.pdf".into(),
            content_type: "application/pdf".into(),
            data: base64::engine::general_purpose::STANDARD.encode(b"%PDF"),
        };
        assert_eq!(f.decode().unwrap().data, b"%PDF");
        let bad = MediaFile {
            filename: "r.pdf".into(),
            content_type: "application/pdf".into(),
            data: "***".into(),
        };
        assert!(bad.decode().is_err());
    }
}
