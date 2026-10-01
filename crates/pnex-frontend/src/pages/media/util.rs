use super::*;

/// Soft pill classes per kind (info card + preview overlay badge).
pub(super) fn kind_pill_classes(asset: &MediaAsset) -> &'static str {
    match asset.media_kind() {
        MediaKind::Panorama => "bg-blue-100 text-blue-700",
        MediaKind::Splat => "bg-purple-100 text-purple-700",
        // Floorplan (Studio): amber, same family as the old list icon.
        MediaKind::Floorplan => "bg-amber-100 text-amber-700",
        MediaKind::Photo => "bg-green-100 text-green-700",
        // ONNX vision model (D81).
        MediaKind::Model => "bg-fuchsia-100 text-fuchsia-700",
    }
}

/// Gradient classes per kind — decorative version thumbnails (no thumbnail
/// endpoint yet: one row = one full blob download, same V1 trade-off as
/// the list).
pub(super) fn kind_gradient(asset: &MediaAsset) -> &'static str {
    match asset.media_kind() {
        MediaKind::Panorama => "from-blue-500 to-indigo-600",
        MediaKind::Splat => "from-purple-500 to-fuchsia-600",
        MediaKind::Floorplan => "from-amber-500 to-orange-600",
        MediaKind::Photo => "from-green-500 to-emerald-600",
        MediaKind::Model => "from-fuchsia-500 to-pink-600",
    }
}

/// Short format label for the metadata strip: splat `metadata.format`
/// first (content_type is octet-stream there), else a compact content
/// type; panoramas get a "· 360°" suffix (GPano metadata carries no
/// pixel dimensions — the strip stays honest with the API contract).
pub(super) fn format_label(asset: &MediaAsset) -> String {
    if let Some(format) = asset
        .metadata
        .as_ref()
        .and_then(|m| m.get("format"))
        .and_then(|f| f.as_str())
    {
        return format.to_uppercase();
    }
    let short = match asset.content_type.as_deref().unwrap_or("") {
        "image/jpeg" => "JPEG",
        "image/png" => "PNG",
        "image/webp" => "WEBP",
        "image/heic" | "image/heif" => "HEIC",
        "model/gltf-binary" => "GLB",
        other => {
            return if other.is_empty() {
                "—".to_string()
            } else {
                other.to_string()
            }
        }
    };
    if asset.media_kind() == MediaKind::Panorama {
        format!("{short} · 360°")
    } else {
        short.to_string()
    }
}

pub(super) fn kind_label(asset: &MediaAsset) -> String {
    match asset.media_kind() {
        MediaKind::Photo => t!("media-kind-photo").to_string(),
        MediaKind::Panorama => t!("media-kind-panorama").to_string(),
        MediaKind::Splat => t!("media-kind-splat").to_string(),
        MediaKind::Floorplan => t!("media-kind-floorplan").to_string(),
        MediaKind::Model => t!("media-kind-model").to_string(),
    }
}

/// Download filename: sanitized asset name (safe chars, spaces → `_`) plus
/// extension inferred when missing — the version has no readable field from
/// the detail, derive from name + content_type (the name entered at upload
/// often carries the extension already).
pub(super) fn download_filename(asset: &MediaAsset) -> String {
    let base = asset
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .replace(' ', "_");
    let base = if base.is_empty() {
        format!("media-{}", asset.id)
    } else {
        base
    };
    match extension_for(asset) {
        Some(ext) if !base.to_lowercase().ends_with(&format!(".{ext}")) => {
            format!("{base}.{ext}")
        }
        _ => base,
    }
}

/// Inferred extension: `metadata.format` first (splats — content_type is
/// then `application/octet-stream`), else content_type.
pub(super) fn extension_for(asset: &MediaAsset) -> Option<&'static str> {
    if let Some(format) = asset
        .metadata
        .as_ref()
        .and_then(|m| m.get("format"))
        .and_then(|f| f.as_str())
    {
        match format {
            "splat" => return Some("splat"),
            "ply" => return Some("ply"),
            "ksplat" => return Some("ksplat"),
            "spz" => return Some("spz"),
            _ => {}
        }
    }
    let ext = match asset.content_type.as_deref().unwrap_or("") {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/heic" | "image/heif" => "heic",
        "model/gltf-binary" => "glb",
        _ => "",
    };
    if ext.is_empty() {
        None
    } else {
        Some(ext)
    }
}
