//! QR tags (parts-docs.md D962–D963).

use pnex_core::qr::{QrLinkInput, QrSheetInput, QrState, QrTag};
use pnex_core::Paginated;
use reqwest::Method;

use super::client;
use super::error::ApiError;
use crate::api::media::urlencode;

/// The tags of one object, of the org in one state, or among `tokens`.
pub async fn list(
    object_id: Option<&str>,
    state: Option<QrState>,
    tokens: &[String],
    limit: i64,
    offset: i64,
) -> Result<Paginated<QrTag>, ApiError> {
    let mut path = format!("/api/v1/qr-tags?limit={limit}&offset={offset}");
    if !tokens.is_empty() {
        path.push_str(&format!("&tokens={}", urlencode(&tokens.join(","))));
    }
    if let Some(o) = object_id {
        path.push_str(&format!("&object_id={}", urlencode(o)));
    }
    if let Some(s) = state {
        path.push_str(&format!("&state={}", s.as_str()));
    }
    client::request(Method::GET, &path, None).await
}

pub async fn sheet(count: u32) -> Result<Vec<QrTag>, ApiError> {
    let body = serde_json::to_value(QrSheetInput { count }).ok();
    let s: pnex_core::qr::QrSheet =
        client::request(Method::POST, "/api/v1/qr-tags/sheets", body).await?;
    Ok(s.tags)
}

/// The live tag of an object, made now when it has none.
pub async fn for_object(id: &str) -> Result<QrTag, ApiError> {
    client::request(
        Method::POST,
        &format!("/api/v1/qr-tags/objects/{}", urlencode(id)),
        None,
    )
    .await
}

pub async fn get(token: &str) -> Result<QrTag, ApiError> {
    client::request(
        Method::GET,
        &format!("/api/v1/qr-tags/{}", urlencode(token)),
        None,
    )
    .await
}

pub async fn link(token: &str, object_id: &str) -> Result<QrTag, ApiError> {
    let body = serde_json::to_value(QrLinkInput {
        object_id: object_id.to_string(),
    })
    .ok();
    client::request(
        Method::POST,
        &format!("/api/v1/qr-tags/{}/link", urlencode(token)),
        body,
    )
    .await
}

pub async fn revoke(token: &str) -> Result<QrTag, ApiError> {
    client::request(
        Method::POST,
        &format!("/api/v1/qr-tags/{}/revoke", urlencode(token)),
        None,
    )
    .await
}

/// The address printed in a tag's QR: the server the app talks to.
pub fn address(token: &str) -> String {
    pnex_core::qr::address(&super::config::api_base(), token)
}

/// SVG of a QR code holding `text`, scaled by its container.
pub fn svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .build();
    Some(
        svg.replacen(" width=\"", " data-w=\"", 1)
            .replacen(" height=\"", " data-h=\"", 1)
            .replacen("<svg", "<svg style=\"width:100%;height:100%\"", 1),
    )
}
