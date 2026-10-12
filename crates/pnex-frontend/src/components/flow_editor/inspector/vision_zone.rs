use std::rc::Rc;

use super::helpers::*;
use super::*;

use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_LABEL};
use crate::pages::cameras::live::LiveView;
use pnex_core::vision::{
    VisionZone, VisionZoneConfig, ZoneAnchor, TRACK_LOST_MAX_SECS, ZONE_MAX_COUNT, ZONE_MAX_POINTS,
};

// ─────────────── vision-zone (media-vision-studio.md D193) ───────────────

/// Rounds a normalized coordinate to 3 decimals (tidy config, 0.1 % grid).
fn norm(v: f64) -> f32 {
    ((v.clamp(0.0, 1.0) * 1000.0).round() / 1000.0) as f32
}

/// `"a, b ,,c"` → `["a", "b", "c"]`.
fn split_labels(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// First free `zone-N` name.
fn next_zone_name(zones: &[VisionZone]) -> String {
    (1..)
        .map(|n| format!("zone-{n}"))
        .find(|n| !zones.iter().any(|z| &z.name == n))
        .unwrap_or_default()
}

/// SVG `points` attribute of a polygon in the 0–1 view box.
fn svg_points(points: &[[f32; 2]]) -> String {
    points
        .iter()
        .map(|[x, y]| format!("{x},{y}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn patch_zone_cfg(cx: &mut EditorCx, f: impl FnOnce(&mut VisionZoneConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::VisionZone { config } = &mut node.kind {
            f(config);
        }
    });
}

/// Vision zone inspector: camera backdrop (live view of a camera of the
/// org, editor only), polygons drawn by clicking over it, zone list, label
/// filter, anchor and minimum dwell.
#[component]
pub(super) fn VisionZoneForm(
    mut cx: EditorCx,
    initial: VisionZoneConfig,
    can_write: bool,
) -> Element {
    let cameras =
        use_resource(move || async move { api::cameras::list().await.unwrap_or_default() });
    let mut draft = use_signal(Vec::<[f32; 2]>::new);
    let mut draft_name = use_signal(String::new);
    let mut svg_el = use_signal(|| None::<Rc<MountedData>>);
    let mut labels_raw = use_signal(|| initial.labels.join(", "));
    let mut dwell_raw = use_signal(|| v_to_string(initial.min_dwell_secs));

    let cams = cameras.value().read().clone().unwrap_or_default();
    let preview = initial.preview_device;
    let (w, h) = cams
        .iter()
        .find(|c| Some(c.device) == preview)
        .map(|c| c.settings.framesize.dims())
        .unwrap_or((4, 3));
    let zones = initial.zones.clone();
    let draft_now = draft();
    let can_close = draft_now.len() >= 3 && zones.len() < ZONE_MAX_COUNT;
    let dwell_now = dwell_raw();
    let dwell_invalid = !dwell_now.trim().is_empty()
        && parse_secs(&dwell_now).is_none_or(|v| !(0.0..=TRACK_LOST_MAX_SECS).contains(&v));

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-zone-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-zone-preview")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let device = event.value().parse::<i64>().ok();
                        patch_zone_cfg(&mut cx, move |c| c.preview_device = device);
                    },
                    option { value: "", selected: preview.is_none(), {t!("flows-zone-preview-none")} }
                    for cam in cams.iter() {
                        option {
                            key: "{cam.device}",
                            value: "{cam.device}",
                            selected: Some(cam.device) == preview,
                            "{cam.device_id}"
                        }
                    }
                }
            }
            div {
                class: "relative w-full bg-gray-50 border border-dashed border-gray-300 rounded-lg overflow-hidden select-none",
                style: "aspect-ratio: {w} / {h};",
                if let Some(device) = preview {
                    div { class: "absolute inset-0",
                        LiveView {
                            key: "zone-live-{device}",
                            device,
                            fill: true,
                        }
                    }
                }
                svg {
                    class: if can_write { "absolute inset-0 w-full h-full cursor-crosshair" } else { "absolute inset-0 w-full h-full" },
                    view_box: "0 0 1 1",
                    preserve_aspect_ratio: "none",
                    onmounted: move |e| svg_el.set(Some(e.data())),
                    onclick: move |e: MouseEvent| async move {
                        if !can_write || draft.peek().len() >= ZONE_MAX_POINTS {
                            return;
                        }
                        let Some(el) = svg_el.cloned() else {
                            return;
                        };
                        let Ok(rect) = el.get_client_rect().await else {
                            return;
                        };
                        if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
                            return;
                        }
                        let c = e.client_coordinates();
                        let x = norm((c.x - rect.origin.x) / rect.size.width);
                        let y = norm((c.y - rect.origin.y) / rect.size.height);
                        draft.write().push([x, y]);
                    },
                    for z in zones.iter() {
                        polygon {
                            key: "{z.name}",
                            points: svg_points(&z.points),
                            fill: "rgba(217, 70, 239, 0.25)",
                            stroke: "#d946ef",
                            stroke_width: "2",
                            vector_effect: "non-scaling-stroke",
                            pointer_events: "none",
                        }
                    }
                    if !draft_now.is_empty() {
                        polyline {
                            points: svg_points(&draft_now),
                            fill: "none",
                            stroke: "#facc15",
                            stroke_width: "2",
                            stroke_dasharray: "4 3",
                            vector_effect: "non-scaling-stroke",
                            pointer_events: "none",
                        }
                    }
                }
                for z in zones.iter() {
                    if let Some([x, y]) = z.points.first() {
                        span {
                            key: "label-{z.name}",
                            class: "absolute px-1 rounded bg-fuchsia-600 text-white text-xs pointer-events-none",
                            style: "left: calc({x} * 100%); top: calc({y} * 100%);",
                            "{z.name}"
                        }
                    }
                }
                for (i, [x, y]) in draft_now.iter().enumerate() {
                    span {
                        key: "pt-{i}",
                        class: "absolute h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-yellow-400 border border-white pointer-events-none",
                        style: "left: calc({x} * 100%); top: calc({y} * 100%);",
                    }
                }
            }
            p { class: "text-xs text-gray-500", {t!("flows-zone-draw-hint", max : ZONE_MAX_POINTS)} }
            if can_write {
                div { class: "space-y-2",
                    input {
                        class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        r#type: "text",
                        maxlength: "64",
                        aria_label: t!("flows-zone-name").to_string(),
                        placeholder: next_zone_name(&zones),
                        value: "{draft_name}",
                        oninput: move |event| draft_name.set(event.value()),
                    }
                    div { class: "flex flex-wrap gap-2",
                        button {
                            r#type: "button",
                            class: BTN_PRIMARY_SM,
                            disabled: !can_close,
                            onclick: move |_| {
                                let points = draft.peek().clone();
                                let typed = draft_name.peek().trim().to_string();
                                patch_zone_cfg(
                                    &mut cx,
                                    move |c| {
                                        let name = if typed.is_empty() {
                                            next_zone_name(&c.zones)
                                        } else {
                                            typed
                                        };
                                        c.zones.push(VisionZone { name, points });
                                    },
                                );
                                draft.set(Vec::new());
                                draft_name.set(String::new());
                            },
                            {t!("flows-zone-close")}
                        }
                        button {
                            r#type: "button",
                            class: BTN_SECONDARY_SM,
                            disabled: draft_now.is_empty(),
                            onclick: move |_| {
                                draft.write().pop();
                            },
                            {t!("flows-zone-undo")}
                        }
                    }
                }
            }
            if zones.is_empty() {
                p { class: "text-xs text-amber-700", {t!("flows-zone-none")} }
            }
            ul { class: "divide-y divide-gray-100 text-sm",
                for z in zones.iter() {
                    li {
                        key: "row-{z.name}",
                        class: "flex items-center justify-between py-1",
                        span { class: "font-medium", "{z.name}" }
                        span { class: "text-xs text-gray-500 ml-2 flex-1",
                            {t!("flows-zone-points", count : z.points.len())}
                        }
                        if can_write {
                            button {
                                r#type: "button",
                                class: "text-xs text-red-600 hover:underline",
                                onclick: {
                                    let name = z.name.clone();
                                    move |_| {
                                        let name = name.clone();
                                        patch_zone_cfg(&mut cx, move |c| c.zones.retain(|z| z.name != name));
                                    }
                                },
                                {t!("flows-zone-delete")}
                            }
                        }
                    }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-zone-labels")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    r#type: "text",
                    placeholder: "person, car",
                    value: "{labels_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        labels_raw.set(raw.clone());
                        patch_zone_cfg(&mut cx, move |c| c.labels = split_labels(&raw));
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-zone-labels-hint")} }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-zone-anchor")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let anchor = if event.value() == "center" {
                            ZoneAnchor::Center
                        } else {
                            ZoneAnchor::Bottom
                        };
                        patch_zone_cfg(&mut cx, move |c| c.anchor = anchor);
                    },
                    option {
                        value: "bottom",
                        selected: initial.anchor == ZoneAnchor::Bottom,
                        {t!("flows-zone-anchor-bottom")}
                    }
                    option {
                        value: "center",
                        selected: initial.anchor == ZoneAnchor::Center,
                        {t!("flows-zone-anchor-center")}
                    }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-zone-min-dwell")} }
                input {
                    class: if dwell_invalid { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm" },
                    r#type: "number",
                    min: "0",
                    max: "{TRACK_LOST_MAX_SECS}",
                    step: "1",
                    value: "{dwell_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        dwell_raw.set(raw.clone());
                        let parsed = if raw.trim().is_empty() { Some(0.0) } else { parse_secs(&raw) };
                        if let Some(v) = parsed.filter(|v| (0.0..=TRACK_LOST_MAX_SECS).contains(v)) {
                            patch_zone_cfg(&mut cx, move |c| c.min_dwell_secs = v);
                        }
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-zone-min-dwell-hint")} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(split_labels(" person, ,car,"), vec!["person", "car"]);
        assert_eq!(norm(1.4), 1.0);
        assert_eq!(norm(0.12345), 0.123);
        let z = |n: &str| VisionZone {
            name: n.into(),
            points: vec![],
        };
        assert_eq!(next_zone_name(&[z("zone-1"), z("dock")]), "zone-2");
        assert_eq!(svg_points(&[[0.0, 0.5], [1.0, 0.25]]), "0,0.5 1,0.25");
    }
}
