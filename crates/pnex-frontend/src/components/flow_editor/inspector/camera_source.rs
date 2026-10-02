use super::helpers::*;
use super::*;

// ─────────────── camera-source (camera-video.md D78) ───────────────

/// Camera source inspector: camera picker (cameras of the org, from
/// `GET /api/v1/cameras`) + `max_fps` sampling (0 = every frame).
#[component]
pub(super) fn CameraSourceForm(
    mut cx: EditorCx,
    initial: CameraSourceConfig,
    can_write: bool,
) -> Element {
    let cameras =
        use_resource(move || async move { api::cameras::list().await.unwrap_or_default() });
    let device_slug = initial.device_id.clone();
    let mut fps_raw = use_signal(move || v_to_string(initial.max_fps));
    let mut fps_invalid = use_signal(|| false);

    // Known cameras + the configured slug when it is not (or no longer) a
    // camera of the org — kept selectable so the config never silently
    // changes under the user.
    let mut slugs: Vec<String> = cameras
        .value()
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.device_id)
        .collect();
    if !device_slug.is_empty() && !slugs.contains(&device_slug) {
        slugs.push(device_slug.clone());
    }
    let no_camera = cameras
        .value()
        .read()
        .as_ref()
        .is_some_and(|c| c.is_empty());

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-camera-source-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-camera-source-camera")}
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let slug = event.value();
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::CameraSource { config } = &mut node.kind {
                                    config.device_id = slug;
                                }
                            },
                        );
                    },
                    option { value: "", selected: device_slug.is_empty(),
                        {t!("flows-camera-source-none")}
                    }
                    for slug in slugs {
                        option {
                            key: "{slug}",
                            value: "{slug}",
                            selected: slug == device_slug,
                            "{slug}"
                        }
                    }
                }
            }
            if no_camera {
                p { class: "text-xs text-amber-700", {t!("flows-camera-source-no-camera")} }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-camera-max-fps")}
                }
                input {
                    class: if fps_invalid() { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                    r#type: "number",
                    min: "0",
                    max: "25",
                    step: "0.5",
                    value: "{fps_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        fps_raw.set(raw.clone());
                        let parsed = if raw.trim().is_empty() { Some(0.0) } else { parse_secs(&raw) };
                        let valid = parsed
                            .filter(|v| (0.0..=pnex_core::CAMERA_NODE_MAX_FPS).contains(v));
                        fps_invalid.set(valid.is_none());
                        if let Some(v) = valid {
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::CameraSource { config } = &mut node.kind {
                                        config.max_fps = v;
                                    }
                                },
                            );
                        }
                    },
                }
                span { class: "text-xs text-gray-400 mt-1 block", {t!("flows-camera-max-fps-hint")} }
            }
        }
    }
}
