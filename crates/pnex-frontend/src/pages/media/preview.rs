use super::*;

/// Preview per kind — photo `<img>` via blob URL; panorama/splat via
/// `media_viewer` (host div + mount, retry included); `.ksplat`/`.spz` →
/// badge. Mount: `onmounted` + spawn of the mount, `use_drop` on unmount
/// (tron.rs/login.rs pattern). `version_override` (read-only view of a
/// non-current version) switches the blob to `/versions/{n}/content` —
/// remounted via the parent key. Kind/version pills overlaid top-left
/// (pointer-events-none: the 360 view stays navigable).
#[component]
pub(super) fn MediaPreview(
    asset: MediaAsset,
    version_override: Option<i64>,
    show_hd_overlay: bool,
) -> Element {
    let mut blob_url = use_signal(|| None::<String>);
    let mut preview_done = use_signal(|| false);
    let mut mount_ok = use_signal(|| true);
    let asset_id = asset.id.clone();
    let host_id = format!("media-preview-{}", asset.id.replace('-', ""));
    let host_id_for_drop = host_id.clone();
    let host_id_for_mount = host_id.clone();
    let is_photo = asset.media_kind() == MediaKind::Photo;
    let is_panorama = asset.media_kind() == MediaKind::Panorama;
    let is_splat = asset.media_kind() == MediaKind::Splat;
    let is_splat_viewable = is_splat && splat_viewable(&asset);

    // ─── Annotations (D58): flat markers on flat images ───
    // Media read model (items of published layers) — fetch gated on flat
    // images: the media page's panorama overlay is a D60(4) open door, not
    // built in V1.
    let mut annot_selected = use_signal(|| None::<pnex_core::ResolvedAnnotationItem>);
    let asset_id_for_annot = asset_id.clone();
    let annotations = use_resource(move || {
        let photo = is_photo;
        let id = asset_id_for_annot.clone();
        async move {
            if !photo {
                return None;
            }
            crate::api::annotation_layers::media_annotations(&id)
                .await
                .ok()
        }
    });
    let annot_items: Vec<pnex_core::ResolvedAnnotationItem> = annotations
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref())
        .map(|a| a.items.clone())
        .unwrap_or_default();
    // Precomputed flat markers (no `let` in the rsx body — tour_viewer
    // school): (id, x, y, kind, item).
    let flat_markers: Vec<(String, f64, f64, String, pnex_core::ResolvedAnnotationItem)> =
        annot_items
            .iter()
            .filter_map(|it| match &it.geometry {
                pnex_core::AnnotationGeometry::Flat { x, y } => {
                    Some((it.id.clone(), *x, *y, it.kind.clone(), it.clone()))
                }
                _ => None,
            })
            .collect();

    // Mime cloned BEFORE the effects: a `move` capturing
    // `asset.content_type` = partial move of `asset`, clashing with the
    // asset.* methods of the rsx (E0382 — owned-props school).
    let mime_for_blob = asset.content_type.clone();
    // Blob URL of the displayed version (current or override) (photos; and
    // panos/splats on web — the JS viewer loads the blob URL, not the
    // authenticated API).
    use_effect(move || {
        let path = match version_override {
            Some(n) => api::media::version_content_path(&asset_id, n),
            None => api::media::content_path(&asset_id),
        };
        let mime = mime_for_blob.clone();
        spawn(async move {
            blob_url.set(None);
            mount_ok.set(true);
            preview_done.set(false);
            let url = media_blob_url(&path, mime.as_deref()).await;
            blob_url.set(url);
            preview_done.set(true);
        });
    });

    // Viewer mount as soon as the blob URL is ready (retry included in
    // media_viewer::mount) — driven by the effect, not onmounted: the blob
    // URL only arrives after the first render.
    use_effect(move || {
        let url = blob_url();
        let host_id = host_id_for_mount.clone();
        spawn(async move {
            let Some(url) = url else {
                return;
            };
            let kind = if is_panorama {
                "panorama"
            } else if is_splat_viewable {
                "splat"
            } else {
                return;
            };
            let ok = crate::media_viewer::mount(kind, &host_id, &url).await;
            mount_ok.set(ok);
        });
    });

    // Clean viewer unmount (canvas + RAF loop).
    use_drop(move || {
        crate::media_viewer::unmount(&host_id_for_drop);
    });

    // Version pill: "v{n} · current" or "v{n}" (amber) when previewing.
    let version_pill_text: String = match version_override {
        Some(n) => format!("v{n}"),
        None => format!(
            "v{} · {}",
            asset.current_version_number.unwrap_or(0),
            t!("media-version-current")
        ),
    };
    let version_pill_classes: &str = if version_override.is_some() {
        "bg-amber-500/90"
    } else {
        "bg-emerald-500/90"
    };

    rsx! {
        // `relative`: the container anchors the HD overlay (absolute
        // inset-0).
        div { class: "relative bg-gray-900 rounded-lg overflow-hidden flex items-center justify-center min-h-[300px]",
            // Kind + version pills (top-RIGHT overlay, non-interactive —
            // never above the 360 drags; top-left is owned by pannellum's
            // native zoom/fullscreen controls, seen overlapping on screen).
            // Dark backdrop + white text ONLY: mixing the soft info-card
            // classes here produced light-text-on-light-bg (unreadable).
            div { class: "pointer-events-none absolute top-3 right-3 z-10 flex flex-wrap items-center gap-1.5",
                span {
                    class: "inline-flex items-center gap-1 rounded-full bg-black/60 px-2.5 py-1 text-xs font-medium text-white",
                    match asset.media_kind() {
                        MediaKind::Panorama => rsx! { icons::Image { class: "h-3 w-3" } },
                        MediaKind::Splat => rsx! { icons::Cube { class: "h-3 w-3" } },
                        MediaKind::Floorplan => rsx! { icons::Map { class: "h-3 w-3" } },
                        MediaKind::Photo => rsx! { icons::Image { class: "h-3 w-3" } },
                        MediaKind::Model => rsx! { icons::Eye { class: "h-3 w-3" } },
                    }
                    {kind_label(&asset)}
                }
                span { class: "rounded-full px-2.5 py-1 text-xs font-medium text-white {version_pill_classes}", "{version_pill_text}" }
            }
            if is_photo {
                match blob_url() {
                    Some(url) => rsx! {
                        // `relative w-full` container: the img fills the
                        // preview WIDTH (small images are upscaled too — a
                        // tiny demo PNG must not stay thumbnail-sized).
                        // `h-auto` keeps the element box at the drawn
                        // content box (no letterbox), so the %-of-container
                        // annotation markers stay aligned with the displayed
                        // image (tour_viewer mini-map school); `max-h` only
                        // clamps unusually tall images.
                        div { class: "relative w-full",
                            img { src: "{url}", class: "mx-auto h-auto w-full max-h-[560px] object-contain", alt: "{asset.name}" }
                            for (m_id, m_x, m_y, m_kind, m_item) in flat_markers {
                                button {
                                    key: "annot-{m_id}",
                                    class: "absolute h-5 w-5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-md hover:scale-110 transition-transform {crate::components::annotation_editor::popover::annot_dot_color(&m_kind)}",
                                    style: "left: calc({m_x} * 100%); top: calc({m_y} * 100%);",
                                    title: "{m_item.label}",
                                    onclick: move |_| annot_selected.set(Some(m_item.clone())),
                                }
                            }
                        }
                        if let Some(item) = annot_selected.cloned() {
                            div { key: "annot-pop-{item.id}",
                                crate::components::annotation_editor::popover::AnnotationPopover {
                                    item,
                                    on_close: move |_| annot_selected.set(None),
                                }
                            }
                        }
                    },
                    None if preview_done() => rsx! {
                        div { class: "p-6 text-center",
                            icons::Image { class: "h-10 w-10 text-gray-500 mx-auto mb-2" }
                            span { class: "text-sm text-gray-300", {t!("media-preview-unavailable")} }
                        }
                    },
                    None => rsx! { span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-white" } },
                }
            } else if is_panorama || is_splat_viewable {
                // Height in INLINE style: pannellum sets `.pnlm-container`
                // (height: 100%, loaded after Tailwind) on the host itself
                // → height:100% of an unsized parent = 0 px (invisible
                // canvas, black box — seen 2026-09-10). Inline keeps
                // precedence whatever the stylesheet order.
                div { id: "{host_id}", class: "w-full h-[420px]", style: "height: 420px" }
                if is_panorama && show_hd_overlay {
                    // Take 360 V2: the displayed preview is the phone's
                    // fast render (v1) — the real 360° HD (v2) is being
                    // stitched server-side. COMPACT bottom banner with
                    // `pointer-events-none`: the 360 view stays fully
                    // navigable underneath (seen on device 2026-09-10: a
                    // fullscreen overlay blocked drag/zoom). The page
                    // switches to HD by itself when the job succeeds.
                    div { class: "absolute inset-x-0 bottom-0 z-10 pointer-events-none flex flex-col items-center gap-1 p-3",
                        div { class: "flex items-center gap-2 bg-black/70 rounded-full px-4 py-2",
                            span { class: "animate-spin inline-block rounded-full h-3.5 w-3.5 border-b-2 border-white flex-shrink-0" }
                            span { class: "text-xs font-semibold text-white", {t!("media-stitch-hd-title")} }
                        }
                        div { class: "text-[11px] text-gray-300 bg-black/50 rounded-lg px-3 py-1 max-w-[92%] text-center",
                            {t!("media-stitch-hd-text")}
                        }
                    }
                }
                if !mount_ok() {
                    // Native fallback: the equirect as a flat image
                    // (immediate validation of the capture on the phone);
                    // the interactive 360 view remains on the web UI.
                    if is_panorama {
                        match blob_url() {
                            Some(url) => rsx! {
                                div { class: "absolute inset-0 flex flex-col items-center justify-center p-2",
                                    img { src: "{url}", class: "max-h-[360px] w-auto object-contain", alt: "{asset.name}" }
                                    span { class: "text-xs text-gray-400 mt-1", {t!("media-preview-pano-flat")} }
                                }
                            },
                            None => rsx! {
                                div { class: "absolute inset-0 flex items-center justify-center",
                                    span { class: "text-sm text-gray-300", {t!("media-preview-unavailable")} }
                                }
                            },
                        }
                    } else {
                        div { class: "absolute inset-0 flex items-center justify-center",
                            span { class: "text-sm text-gray-300", {t!("media-preview-unavailable")} }
                        }
                    }
                }
            } else {
                div { class: "p-6 text-center",
                    icons::Cube { class: "h-10 w-10 text-gray-500 mx-auto mb-2" }
                    span { class: "text-sm text-gray-300", {t!("media-preview-unavailable")} }
                }
            }
        }
    }
}

/// Splats with a V1 preview: .splat/.ply (gsplat.js); .ksplat/.spz without
/// preview (badge — Spark.js documented as plan B).
fn splat_viewable(asset: &MediaAsset) -> bool {
    asset
        .metadata
        .as_ref()
        .and_then(|m| m.get("format"))
        .and_then(|f| f.as_str())
        .is_none_or(|format| format == "splat" || format == "ply")
}
