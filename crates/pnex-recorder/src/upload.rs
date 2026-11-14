//! A recording sent to the org's media library (`POST /api/v1/media`), with
//! the caller's own HTTP client, token and org: no session of its own.

use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum UploadError {
    /// The WAV could not be read or encoded.
    Unreadable(String),
    Unreachable,
    /// A success that is not a PNeX media asset.
    NotPnex,
    /// HTTP status and `ErrorDetail.error` code when the server sent one.
    Server(u16, Option<String>),
}

/// Stores one Ogg/Opus part; returns its asset id.
pub async fn upload_part(
    http: &reqwest::Client,
    base_url: &str,
    bearer: &str,
    org: i64,
    bytes: Vec<u8>,
    filename: &str,
    title: &str,
) -> Result<String, UploadError> {
    let res = http
        .post(format!("{}/api/v1/media", base_url.trim_end_matches('/')))
        .query(&[("name", title), ("filename", filename), ("kind", "audio")])
        .bearer_auth(bearer)
        .header("X-Org-Id", org.to_string())
        .header("Content-Type", "audio/ogg")
        .timeout(std::time::Duration::from_secs(600))
        .body(bytes)
        .send()
        .await
        .map_err(|_| UploadError::Unreachable)?;
    let status = res.status().as_u16();
    if !res.status().is_success() {
        let body: Value = res.json().await.unwrap_or(Value::Null);
        return Err(UploadError::Server(
            status,
            body["error"].as_str().map(str::to_owned),
        ));
    }
    let asset: Value = res.json().await.map_err(|_| UploadError::NotPnex)?;
    asset["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or(UploadError::NotPnex)
}

/// Encodes a finished recording (the WAV of [`crate::capture`]) and stores
/// it, in parts of at most [`crate::encode::PART_SECS`]; the asset ids in
/// order. The WAV is left in place: the caller deletes it once satisfied.
pub async fn upload(
    http: &reqwest::Client,
    base_url: &str,
    bearer: &str,
    org: i64,
    wav: &Path,
    title: &str,
) -> Result<Vec<String>, UploadError> {
    let path = wav.to_path_buf();
    let parts = tokio::task::spawn_blocking(move || {
        crate::encode::ogg_parts(&path, crate::encode::PART_SECS)
    })
    .await
    .map_err(|e| UploadError::Unreadable(e.to_string()))?
    .map_err(UploadError::Unreadable)?;
    let stem = wav.file_stem().and_then(|f| f.to_str()).unwrap_or("call");
    let count = parts.len();
    let mut assets = Vec::with_capacity(count);
    for (i, part) in parts.into_iter().enumerate() {
        let (name, filename) = if count == 1 {
            (title.to_string(), format!("{stem}.ogg"))
        } else {
            (
                format!("{title} ({}/{count})", i + 1),
                format!("{stem}_{}.ogg", i + 1),
            )
        };
        assets.push(upload_part(http, base_url, bearer, org, part, &filename, &name).await?);
    }
    Ok(assets)
}
