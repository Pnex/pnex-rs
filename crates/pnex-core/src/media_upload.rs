//! A media file sent in several requests (data-pipelines.md §9.9): a proxy limits each
//! request (Cloudflare: 100 MB). Parts wait on S3, then a server joins them into one
//! version. Pure, wasm-safe.

use serde::{Deserialize, Serialize};

/// Largest part, under the request limit of proxies. Same bound as dataset parts.
pub const PART_MAX_BYTES: u64 = crate::datasets::UPLOAD_PART_MAX_BYTES;
pub const PARTS_MAX: i32 = crate::datasets::UPLOAD_PARTS_MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum MediaUploadState {
    /// Receiving parts.
    Open,
    /// Complete, waiting for a server.
    Ready,
    /// A server is joining the parts.
    Joining,
    Done,
    Failed,
}

impl MediaUploadState {
    pub fn ended(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// A media file sent in parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaUpload {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// The asset it adds a version to; once done, the asset it created.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub asset_id: Option<String>,
    pub filename: String,
    pub state: MediaUploadState,
    /// Parts received.
    pub parts: i32,
    /// Bytes received.
    pub bytes: i64,
    /// The version number it became, once done.
    pub version: Option<i64>,
    /// Machine code of the failure.
    pub code: Option<String>,
    /// Arguments of the failure.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub args: Option<serde_json::Value>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

/// Opens an upload: a new asset, or a new version of `asset_id`. Same fields as the
/// query of the single-request upload.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaUploadInput {
    pub filename: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub asset_id: Option<String>,
    /// Name of a new asset. Default: the file name.
    pub name: Option<String>,
    /// Kind of a new asset. Default: found from the content.
    pub kind: Option<String>,
    pub content_type: Option<String>,
    /// Note of the version.
    pub note: Option<String>,
}

/// Closes an upload: parts 1 to `parts` make the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaUploadComplete {
    pub parts: i32,
}
