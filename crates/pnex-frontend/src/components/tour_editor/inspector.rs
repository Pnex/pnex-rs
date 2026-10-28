//! Tour editor inspector — one form per selection (scene,
//! floor, link). Remounted by `key` at each selection change: the
//! fields start again from the document value (flow_editor style).
//!
//! Closure idiom: they only capture `cx` (Copy, `patch_selected`
//! style) — the id of the edited element is derived from the selection at
//! gesture time, never captured (otherwise FnOnce → rejected by Callbacks).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::TourDoc;

use super::{state, MediaPicker, Select, TourEditorCx};
use crate::api::media::MediaKind;
use crate::components::ui::{BTN_DANGER, BTN_SECONDARY_SM};
use crate::components::ui::{FIELD_LABEL, INPUT};

/// Patches the selected scene **via the reducer** (the id is derived from the
/// selection at the gesture — FnMut-safe closures, `patch_selected` style).
fn patch_scene(cx: &TourEditorCx, f: impl FnOnce(&mut TourDoc, &str) + 'static) {
    let Some(Select::Scene(id)) = cx.selected.cloned() else {
        return;
    };
    cx.update_doc(move |doc| f(doc, &id));
}

/// Patches the selected floor via the reducer.
fn patch_floor(cx: &TourEditorCx, f: impl FnOnce(&mut TourDoc, &str) + 'static) {
    let Some(Select::Floor(id)) = cx.selected.cloned() else {
        return;
    };
    cx.update_doc(move |doc| f(doc, &id));
}

/// Inspector body — mounted only on selection (principle 4);
/// the panel, header and close are the shell's business.
#[component]
pub(crate) fn InspectorBody(cx: TourEditorCx, can_write: bool) -> Element {
    let doc = cx.doc.cloned();
    match cx.selected.cloned() {
        Some(Select::Scene(id)) => {
            let Some(scene) = doc.scenes.iter().find(|s| s.id == id).cloned() else {
                return rsx! {};
            };
            rsx! {
                SceneForm {
                    key: "{id}",
                    cx,
                    can_write,
                    scene,
                }
            }
        }
        Some(Select::Floor(id)) => {
            let Some(floor) = doc.floors.iter().find(|f| f.id == id).cloned() else {
                return rsx! {};
            };
            rsx! {
                FloorForm {
                    key: "{id}",
                    cx,
                    can_write,
                    floor,
                }
            }
        }
        Some(Select::Link(id)) => {
            let Some(link) = doc.links.iter().find(|l| l.id == id).cloned() else {
                return rsx! {};
            };
            rsx! {
                LinkForm {
                    key: "{id}",
                    cx,
                    can_write,
                    link,
                }
            }
        }
        None => rsx! {},
    }
}

/// Scene: panorama asset, label, initial view, start, deletion.
#[component]
fn SceneForm(cx: TourEditorCx, can_write: bool, scene: pnex_core::TourScene) -> Element {
    let mut picker_open = use_signal(|| false);
    let mut label = use_signal(|| scene.label.clone());
    let mut yaw = use_signal(|| scene.initial_yaw.to_string());
    let mut pitch = use_signal(|| scene.initial_pitch.to_string());
    let mut fov = use_signal(|| scene.initial_fov.to_string());
    let is_start = cx.doc.peek().start_scene.as_deref() == Some(scene.id.as_str());

    rsx! {
        div { class: "space-y-3",
            // Referenced panorama asset.
            div {
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("studio-scene-media")}
                }
                if can_write {
                    button {
                        class: "w-full px-3 py-2 text-left text-sm border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors truncate",
                        onclick: move |_| picker_open.set(true),
                        {scene.media_asset_id.clone()}
                    }
                } else {
                    p { class: "text-sm text-gray-600 truncate", {scene.media_asset_id.clone()} }
                }
            }

            label { class: "block",
                span { class: FIELD_LABEL, {t!("studio-scene-label")} }
                input {
                    class: "{INPUT} w-full",
                    r#type: "text",
                    value: "{label}",
                    disabled: !can_write,
                    oninput: move |event| {
                        label.set(event.value());
                        let value = event.value();
                        patch_scene(&cx, move |doc, id| state::set_scene_label(doc, id, value));
                    },
                }
            }

            div { class: "grid grid-cols-3 gap-2",
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("studio-scene-yaw")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        step: "1",
                        value: "{yaw}",
                        disabled: !can_write,
                        oninput: move |event| {
                            yaw.set(event.value());
                            if let (Ok(y), Ok(p), Ok(f)) = (
                                yaw().parse::<f64>(),
                                pitch().parse::<f64>(),
                                fov().parse::<f64>(),
                            ) {
                                patch_scene(&cx, move |doc, id| state::set_scene_view(doc, id, y, p, f));
                            }
                        },
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("studio-scene-pitch")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        step: "1",
                        value: "{pitch}",
                        disabled: !can_write,
                        oninput: move |event| {
                            pitch.set(event.value());
                            if let (Ok(y), Ok(p), Ok(f)) = (
                                yaw().parse::<f64>(),
                                pitch().parse::<f64>(),
                                fov().parse::<f64>(),
                            ) {
                                patch_scene(&cx, move |doc, id| state::set_scene_view(doc, id, y, p, f));
                            }
                        },
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("studio-scene-fov")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        step: "1",
                        value: "{fov}",
                        disabled: !can_write,
                        oninput: move |event| {
                            fov.set(event.value());
                            if let (Ok(y), Ok(p), Ok(f)) = (
                                yaw().parse::<f64>(),
                                pitch().parse::<f64>(),
                                fov().parse::<f64>(),
                            ) {
                                patch_scene(&cx, move |doc, id| state::set_scene_view(doc, id, y, p, f));
                            }
                        },
                    }
                }
            }

            // Scene links: outgoing hotspots (round-trip pair
            // created automatically — auto-link, inspector or link mode).
            SceneLinks { cx, can_write, scene_id: scene.id.clone() }

            if can_write {
                button {
                    class: if is_start { "w-full px-3 py-2 text-sm text-emerald-700 bg-emerald-50 border border-emerald-300 rounded-lg" } else { "w-full px-3 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors" },
                    disabled: is_start,
                    onclick: move |_| {
                        let Some(Select::Scene(id)) = cx.selected.cloned() else {
                            return;
                        };
                        cx.update_doc(move |doc| state::set_start_scene(doc, Some(id)));
                    },
                    {t!("studio-scene-start")}
                }
                button {
                    class: "{BTN_DANGER} w-full",
                    onclick: move |_| {
                        let Some(Select::Scene(id)) = cx.selected.cloned() else {
                            return;
                        };
                        cx.update_doc(move |doc| state::remove_scene(doc, &id));
                        cx.selected.set(None);
                    },
                    {t!("common-delete")}
                }
            }

            if picker_open() {
                MediaPicker {
                    kind: MediaKind::Panorama,
                    on_picked: move |picked: (String, String)| {
                        let (asset_id, _name) = picked;
                        picker_open.set(false);
                        patch_scene(&cx, move |doc, id| state::set_scene_media(doc, id, asset_id));
                    },
                    on_close: move |_| picker_open.set(false),
                }
            }
        }
    }
}

/// Links of a scene: list of outgoing hotspots (derived kind, target
/// label, deletion) + add towards a chosen scene — creation goes
/// through `state::connect_scenes` (round-trip pair, auto angles). Same
/// Tailwind style as the rest of the inspector (violet chips = stair).
#[component]
fn SceneLinks(cx: TourEditorCx, can_write: bool, scene_id: String) -> Element {
    let doc = cx.doc.cloned();

    // Outgoing: (link id, stair?, target label).
    let mut rows: Vec<(String, bool, String)> = Vec::new();
    for link in &doc.links {
        if link.from != scene_id {
            continue;
        }
        let Some(target) = doc.scenes.iter().find(|s| s.id == link.to) else {
            continue;
        };
        rows.push((
            link.id.clone(),
            link.kind == pnex_core::TOUR_LINK_STAIR,
            if target.label.is_empty() {
                target.id.clone()
            } else {
                target.label.clone()
            },
        ));
    }
    // Possible targets: the other scenes, labelled with their floor.
    let mut candidates: Vec<(String, String)> = Vec::new();
    for s in &doc.scenes {
        if s.id == scene_id {
            continue;
        }
        let name = if s.label.is_empty() {
            s.id.clone()
        } else {
            s.label.clone()
        };
        let floor = doc
            .floors
            .iter()
            .find(|f| f.id == s.floor_id)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        candidates.push((
            s.id.clone(),
            if floor.is_empty() {
                name
            } else {
                format!("{name} · {floor}")
            },
        ));
    }
    let mut target = use_signal(|| {
        candidates
            .first()
            .map(|(id, _)| id.clone())
            .unwrap_or_default()
    });

    rsx! {
        div { class: "space-y-2",
            span { class: "text-xs font-medium text-gray-500 block", {t!("studio-scene-links")} }
            if rows.is_empty() {
                p { class: "text-xs text-gray-500", {t!("studio-scene-links-none")} }
            }
            for (link_id, is_stair, label) in rows {
                div {
                    key: "{link_id}",
                    class: "flex items-center gap-2 rounded-lg border border-gray-200 px-2 py-1.5",
                    span { class: if is_stair { "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-violet-50 text-violet-700 border border-violet-200" } else { "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-signal-50 text-signal-700 border border-signal-200" },
                        {
                            if is_stair { t!("studio-link-kind-stair") } else { t!("studio-link-kind-walk") }
                        }
                    }
                    span { class: "text-xs text-gray-700 truncate flex-1", {label} }
                    if can_write {
                        button {
                            class: "text-xs text-red-600 hover:text-red-800 shrink-0",
                            onclick: move |_| {
                                let id = link_id.clone();
                                cx.update_doc(move |doc| state::remove_link(doc, &id));
                            },
                            "✕"
                        }
                    }
                }
            }
            if can_write && !candidates.is_empty() {
                div { class: "flex gap-2",
                    select {
                        class: "{INPUT} flex-1 min-w-0",
                        value: "{target}",
                        onchange: move |event| target.set(event.value()),
                        for (id, label) in candidates.clone() {
                            option { key: "{id}", value: "{id}", {label} }
                        }
                    }
                    button {
                        class: "px-3 py-2 text-sm text-signal-700 bg-signal-50 border border-signal-200 rounded-lg hover:bg-signal-100 transition-colors shrink-0 whitespace-nowrap",
                        onclick: move |_| {
                            let to = target.cloned();
                            if to.is_empty() {
                                return;
                            }
                            let from = scene_id.clone();
                            cx.update_doc(move |doc| {
                                state::connect_scenes(doc, &from, &to);
                            });
                        },
                        {t!("studio-scene-links-add")}
                    }
                }
            }
        }
    }
}

/// Floor: name, level, plan (choice + dimensions + scale), deletion.
#[component]
fn FloorForm(cx: TourEditorCx, can_write: bool, floor: pnex_core::TourFloor) -> Element {
    let mut picker_open = use_signal(|| false);
    let mut name = use_signal(|| floor.name.clone());
    let mut level = use_signal(|| floor.level.to_string());
    let mut north = use_signal(|| floor.north_deg.to_string());

    rsx! {
        div { class: "space-y-3",
            label { class: "block",
                span { class: FIELD_LABEL, {t!("studio-floor-name")} }
                input {
                    class: "{INPUT} w-full",
                    r#type: "text",
                    value: "{name}",
                    disabled: !can_write,
                    oninput: move |event| {
                        name.set(event.value());
                        let value = event.value();
                        patch_floor(
                            &cx,
                            move |doc, id| {
                                if let Some(f) = doc.floors.iter_mut().find(|f| f.id == id) {
                                    f.name = value;
                                }
                            },
                        );
                    },
                }
            }
            div { class: "grid grid-cols-2 gap-2",
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("studio-floor-level")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        step: "1",
                        value: "{level}",
                        disabled: !can_write,
                        oninput: move |event| {
                            level.set(event.value());
                            let value = event.value().parse::<i32>().unwrap_or(0);
                            patch_floor(
                                &cx,
                                move |doc, id| {
                                    if let Some(f) = doc.floors.iter_mut().find(|f| f.id == id) {
                                        f.level = value;
                                    }
                                },
                            );
                        },
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("studio-floor-north")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        step: "1",
                        value: "{north}",
                        disabled: !can_write,
                        oninput: move |event| {
                            north.set(event.value());
                            let value = event.value().parse::<f64>().unwrap_or(0.0);
                            patch_floor(
                                &cx,
                                move |doc, id| {
                                    if let Some(f) = doc.floors.iter_mut().find(|f| f.id == id) {
                                        f.north_deg = value;
                                    }
                                },
                            );
                        },
                    }
                }
            }

            // Floor plan: asset + dimensions + scale.
            {
                let plan = floor.plan.clone();
                rsx! {
                    div {
                        span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("studio-floor-plan")} }
                        if let Some(plan) = plan {
                            div { class: "space-y-2",
                                p { class: "text-xs text-gray-600 break-all", {plan.media_asset_id.clone()} }
                                div { class: "grid grid-cols-2 gap-2",
                                    label { class: "block",
                                        span { class: "text-xs text-gray-500 block", {t!("studio-plan-width")} }
                                        input {
                                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                                            r#type: "number",
                                            value: "{plan.width.unwrap_or(0.0)}",
                                            disabled: !can_write,
                                            oninput: move |event| {
                                                let value = event.value().parse::<f64>().ok().filter(|v| *v > 0.0);
                                                patch_floor(
                                                    &cx,
                                                    move |doc, id| {
                                                        state::patch_plan(doc, id, |plan| plan.width = value);
                                                    },
                                                );
                                            },
                                        }
                                    }
                                    label { class: "block",
                                        span { class: "text-xs text-gray-500 block", {t!("studio-plan-height")} }
                                        input {
                                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                                            r#type: "number",
                                            value: "{plan.height.unwrap_or(0.0)}",
                                            disabled: !can_write,
                                            oninput: move |event| {
                                                let value = event.value().parse::<f64>().ok().filter(|v| *v > 0.0);
                                                patch_floor(
                                                    &cx,
                                                    move |doc, id| {
                                                        state::patch_plan(doc, id, |plan| plan.height = value);
                                                    },
                                                );
                                            },
                                        }
                                    }
                                }
                                label { class: "block",
                                    span { class: "text-xs text-gray-500 block", {t!("studio-plan-scale")} }
                                    input {
                                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                                        r#type: "number",
                                        step: "0.01",
                                        value: "{plan.scale_m_per_px.unwrap_or(0.0)}",
                                        disabled: !can_write,
                                        oninput: move |event| {
                                            let value = event.value().parse::<f64>().ok().filter(|v| *v > 0.0);
                                            patch_floor(
                                                &cx,
                                                move |doc, id| {
                                                    state::patch_plan(doc, id, |plan| plan.scale_m_per_px = value);
                                                },
                                            );
                                        },
                                    }
                                }
                                if can_write {
                                    button {
                                        class: "{BTN_SECONDARY_SM} w-full",
                                        onclick: move |_| {
                                            patch_floor(&cx, move |doc, id| state::set_floor_plan(doc, id, None));
                                        },
                                        {t!("studio-plan-clear")}
                                    }
                                }
                            }
                        } else if can_write {
                            button {
                                class: "w-full px-3 py-2 text-sm text-signal-700 bg-signal-50 border border-signal-200 rounded-lg hover:bg-signal-100 transition-colors",
                                onclick: move |_| picker_open.set(true),
                                {t!("studio-floor-plan-pick")}
                            }
                        } else {
                            p { class: "text-sm text-gray-500", {t!("studio-floor-plan-none")} }
                        }
                    }
                }
            }

            if can_write {
                button {
                    class: "{BTN_DANGER} w-full",
                    onclick: move |_| {
                        let Some(Select::Floor(id)) = cx.selected.cloned() else {
                            return;
                        };
                        let occupied = {
                            let doc = cx.doc.peek();
                            doc.scenes.iter().any(|s| s.floor_id == id)
                        };
                        if occupied {
                            crate::state::toasts::info(t!("studio-floor-refused").to_string());
                        } else {
                            cx.update_doc(move |doc| {
                                state::remove_floor(doc, &id);
                            });
                            cx.selected.set(None);
                        }
                    },
                    {t!("common-delete")}
                }
            }

            if picker_open() {
                MediaPicker {
                    kind: MediaKind::Floorplan,
                    on_picked: move |picked: (String, String)| {
                        let (asset_id, _name) = picked;
                        picker_open.set(false);
                        patch_floor(
                            &cx,
                            move |doc, id| {
                                state::set_floor_plan(
                                    doc,
                                    id,
                                    Some(pnex_core::FloorPlan {
                                        media_asset_id: asset_id,
                                        width: None,
                                        height: None,
                                        scale_m_per_px: None,
                                    }),
                                );
                            },
                        );
                    },
                    on_close: move |_| picker_open.set(false),
                }
            }
        }
    }
}

/// Link: label + derived kind (read-only) + deletion.
#[component]
fn LinkForm(cx: TourEditorCx, can_write: bool, link: pnex_core::TourLink) -> Element {
    let mut label = use_signal(|| link.label.clone().unwrap_or_default());

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500",
                {t!("studio-link-endpoints", from : link.from.clone(), to : link.to.clone())}
            }
            p { class: "text-xs",
                span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-violet-50 text-violet-700 border border-violet-200",
                    {
                        if link.kind == pnex_core::TOUR_LINK_STAIR {
                            t!("studio-link-kind-stair")
                        } else {
                            t!("studio-link-kind-walk")
                        }
                    }
                }
                span { class: "ml-2 text-gray-500", {t!("studio-link-kind-derived")} }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("studio-link-label")} }
                input {
                    class: "{INPUT} w-full",
                    r#type: "text",
                    value: "{label}",
                    disabled: !can_write,
                    oninput: move |event| {
                        label.set(event.value());
                        let value = event.value();
                        let Some(Select::Link(id)) = cx.selected.cloned() else {
                            return;
                        };
                        cx.update_doc(move |doc| state::set_link_label(doc, &id, Some(value)));
                    },
                }
            }
            if can_write {
                button {
                    class: "{BTN_DANGER} w-full",
                    onclick: move |_| {
                        let Some(Select::Link(id)) = cx.selected.cloned() else {
                            return;
                        };
                        cx.update_doc(move |doc| state::remove_link(doc, &id));
                        cx.selected.set(None);
                    },
                    {t!("common-delete")}
                }
            }
        }
    }
}
