//! 3D model viewer (assisted-creation.md §8, D231): orbit view of a GLB,
//! its animations, its skeleton, bone rotation and named poses. The WebGL
//! side is `js/mesh.js` (`window.pnexViewers.mesh`), reached through
//! `media_viewer::mesh_call`.

use dioxus::prelude::*;
use dioxus_i18n::t;
use serde_json::{json, Value};

use crate::components::gerbe::Gerbe;
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::media_viewer::mesh_call;

/// A pose saved on an object: its name and bone rotations.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedPose {
    pub name: String,
    pub pose: Value,
}

/// A pose to save: name, bone rotations, PNG data URL of the view.
#[derive(Clone, Debug, PartialEq)]
pub struct NewPose {
    pub name: String,
    pub pose: Value,
    pub png: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct Bone {
    name: String,
    x: i64,
    y: i64,
    z: i64,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct Info {
    ready: bool,
    error: bool,
    bones: Vec<Bone>,
    clips: Vec<String>,
}

fn parse_info(v: &Value) -> Info {
    let n = |b: &Value, k: &str| b.get(k).and_then(Value::as_i64).unwrap_or(0);
    Info {
        ready: v["ready"].as_bool().unwrap_or(false),
        error: v["error"].as_bool().unwrap_or(false),
        bones: v["bones"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| Bone {
                name: b["name"].as_str().unwrap_or_default().to_string(),
                x: n(b, "x"),
                y: n(b, "y"),
                z: n(b, "z"),
            })
            .collect(),
        clips: v["clips"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().map(str::to_string))
            .collect(),
    }
}

/// Bone name without the rig prefix most exporters add ("mixamorig:").
fn bone_label(name: &str) -> &str {
    let rest = name.rsplit_once(':').map_or(name, |(_, r)| r);
    rest.strip_prefix("mixamorig").unwrap_or(rest)
}

/// Current angles of `bone`, read back from the viewer (after a rotation,
/// a reset or a pose, the first load's angles are stale).
async fn angles(host: &str, bone: &str) -> (i64, i64, i64) {
    mesh_call(host, "info", Value::Null)
        .await
        .map(|v| parse_info(&v))
        .and_then(|i| i.bones.into_iter().find(|b| b.name == bone))
        .map_or((0, 0, 0), |b| (b.x, b.y, b.z))
}

#[component]
pub fn MeshViewer(
    host_id: String,
    url: String,
    #[props(default)] poses: Vec<SavedPose>,
    on_save_pose: Option<EventHandler<NewPose>>,
) -> Element {
    let mut info = use_signal(Info::default);
    let mut bone = use_signal(String::new);
    let mut rot = use_signal(|| (0i64, 0i64, 0i64));
    let mut skeleton = use_signal(|| false);
    let mut pose_name = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let host = host_id.clone();

    // Mount, then poll until the model is loaded (or failed).
    use_effect(use_reactive!(|url| {
        let host = host.clone();
        spawn(async move {
            info.set(Info::default());
            if !crate::media_viewer::mount("mesh", &host, &url).await {
                info.set(Info {
                    error: true,
                    ..Info::default()
                });
                return;
            }
            for _ in 0..150 {
                let i = mesh_call(&host, "info", Value::Null)
                    .await
                    .map(|v| parse_info(&v))
                    .unwrap_or_default();
                if i.ready || i.error {
                    if let Some(b) = i.bones.first() {
                        bone.set(b.name.clone());
                        rot.set((b.x, b.y, b.z));
                    }
                    info.set(i);
                    return;
                }
                crate::util::sleep(std::time::Duration::from_millis(200)).await;
            }
            info.set(Info {
                error: true,
                ..Info::default()
            });
        });
    }));
    let drop_host = host_id.clone();
    use_drop(move || crate::media_viewer::unmount(&drop_host));

    let i = info();
    let h = host_id.clone();
    let send = move |x: i64, y: i64, z: i64| {
        rot.set((x, y, z));
        let h = h.clone();
        let b = bone();
        spawn(async move {
            let _ = mesh_call(&h, "rotate", json!({ "bone": b, "x": x, "y": y, "z": z })).await;
        });
    };
    let (x, y, z) = rot();

    rsx! {
        div { class: "space-y-3",
            div { class: "relative rounded-lg bg-gray-900 overflow-hidden",
                div {
                    id: "{host_id}",
                    class: "w-full h-[420px]",
                    style: "height: 420px",
                    "data-testid": "mesh-viewer",
                }
                if i.error {
                    div { class: "absolute inset-0 flex items-center justify-center",
                        span { class: "text-sm text-gray-300", {t!("media-preview-unavailable")} }
                    }
                } else if !i.ready {
                    div { class: "absolute inset-0 flex items-center justify-center pointer-events-none",
                        Gerbe { size: 36, inherit: true }
                    }
                }
            }
            if i.ready {
                div { class: "flex flex-wrap items-end gap-3",
                    if !i.clips.is_empty() {
                        label { class: "text-sm",
                            span { class: FIELD_LABEL, {t!("mesh-animation")} }
                            select {
                                class: INPUT,
                                "data-testid": "mesh-clip",
                                onchange: {
                                    let h = host_id.clone();
                                    move |e: FormEvent| {
                                        let h = h.clone();
                                        spawn(async move {
                                            let _ = mesh_call(&h, "play", json!(e.value())).await;
                                        });
                                    }
                                },
                                option { value: "", {t!("mesh-animation-none")} }
                                for c in i.clips.clone() {
                                    option { key: "{c}", value: "{c}", "{c}" }
                                }
                            }
                        }
                    }
                    if !i.bones.is_empty() {
                        label { class: "inline-flex items-center gap-2 text-sm text-ink min-h-10",
                            input {
                                r#type: "checkbox",
                                checked: skeleton(),
                                onchange: {
                                    let h = host_id.clone();
                                    move |e: FormEvent| {
                                        let on = e.checked();
                                        skeleton.set(on);
                                        let h = h.clone();
                                        spawn(async move {
                                            let _ = mesh_call(&h, "skeleton", json!(on)).await;
                                        });
                                    }
                                },
                            }
                            {t!("mesh-skeleton")}
                        }
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: {
                                let h = host_id.clone();
                                move |_| {
                                    let h = h.clone();
                                    spawn(async move {
                                        let _ = mesh_call(&h, "applyPose", json!({})).await;
                                        rot.set(angles(&h, &bone()).await);
                                    });
                                }
                            },
                            {t!("mesh-reset")}
                        }
                    }
                }
                if i.bones.is_empty() {
                    p { class: "text-sm text-slate", {t!("mesh-no-skeleton")} }
                } else {
                    div { class: "grid gap-3 sm:grid-cols-[minmax(0,14rem)_1fr]",
                        label { class: "text-sm",
                            span { class: FIELD_LABEL, {t!("mesh-bone")} }
                            select {
                                class: INPUT,
                                "data-testid": "mesh-bone",
                                value: "{bone}",
                                onchange: {
                                    let h = host_id.clone();
                                    move |e: FormEvent| {
                                        let name = e.value();
                                        bone.set(name.clone());
                                        let h = h.clone();
                                        spawn(async move {
                                            rot.set(angles(&h, &name).await);
                                        });
                                    }
                                },
                                for b in i.bones.clone() {
                                    option { key: "{b.name}", value: "{b.name}", {bone_label(&b.name)} }
                                }
                            }
                        }
                        div { class: "space-y-1",
                            for (axis, val) in [("X", x), ("Y", y), ("Z", z)] {
                                label {
                                    key: "{axis}",
                                    class: "flex items-center gap-2 text-sm text-ink",
                                    span { class: "w-4 font-medium", "{axis}" }
                                    input {
                                        r#type: "range",
                                        class: "flex-1 accent-signal",
                                        min: "-180",
                                        max: "180",
                                        value: "{val}",
                                        aria_label: t!("mesh-rotation", axis : axis),
                                        oninput: {
                                            let mut send = send.clone();
                                            move |e: FormEvent| {
                                                let v = e.value().parse::<i64>().unwrap_or(0);
                                                let (x, y, z) = rot();
                                                match axis {
                                                    "X" => send(v, y, z),
                                                    "Y" => send(x, v, z),
                                                    _ => send(x, y, v),
                                                }
                                            }
                                        },
                                    }
                                    span { class: "w-12 text-right tabular-nums text-slate",
                                        "{val}°"
                                    }
                                }
                            }
                        }
                    }
                    if !poses.is_empty() {
                        div { class: "flex flex-wrap items-center gap-2",
                            span { class: "text-sm text-slate", {t!("mesh-poses")} }
                            for p in poses.clone() {
                                button {
                                    key: "{p.name}",
                                    class: BTN_SECONDARY_SM,
                                    r#type: "button",
                                    onclick: {
                                        let h = host_id.clone();
                                        move |_| {
                                            let h = h.clone();
                                            let pose = p.pose.clone();
                                            spawn(async move {
                                                let _ = mesh_call(&h, "applyPose", pose).await;
                                                rot.set(angles(&h, &bone()).await);
                                            });
                                        }
                                    },
                                    "{p.name}"
                                }
                            }
                        }
                    }
                    if let Some(save) = on_save_pose {
                        form {
                            class: "flex flex-wrap items-end gap-2",
                            onsubmit: {
                                let h = host_id.clone();
                                move |e: FormEvent| {
                                    e.prevent_default();
                                    let name = pose_name().trim().to_string();
                                    if name.is_empty() || saving() {
                                        return;
                                    }
                                    saving.set(true);
                                    let h = h.clone();
                                    spawn(async move {
                                        let pose = mesh_call(&h, "pose", Value::Null).await;
                                        let png = mesh_call(&h, "snapshot", Value::Null).await;
                                        if let (Some(pose), Some(Value::String(png))) = (pose, png) {
                                            save.call(NewPose { name, pose, png });
                                            pose_name.set(String::new());
                                        }
                                        saving.set(false);
                                    });
                                }
                            },
                            label { class: "text-sm flex-1 min-w-[12rem]",
                                span { class: FIELD_LABEL, {t!("mesh-pose-name")} }
                                input {
                                    class: INPUT,
                                    "data-testid": "mesh-pose-name",
                                    maxlength: "255",
                                    value: "{pose_name}",
                                    placeholder: t!("mesh-pose-placeholder"),
                                    oninput: move |e| pose_name.set(e.value()),
                                }
                            }
                            button {
                                class: BTN_PRIMARY_SM,
                                r#type: "submit",
                                disabled: saving() || pose_name().trim().is_empty(),
                                {t!("mesh-pose-save")}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_reads_bones_and_clips() {
        let v = json!({
            "ready": true,
            "bones": [{"name": "hip", "x": 10, "y": -5, "z": 0}],
            "clips": ["walk"]
        });
        let i = parse_info(&v);
        assert!(i.ready && !i.error);
        assert_eq!(i.bones[0].name, "hip");
        assert_eq!((i.bones[0].x, i.bones[0].y), (10, -5));
        assert_eq!(i.clips, ["walk"]);
        assert_eq!(parse_info(&Value::Null), Info::default());
    }

    #[test]
    fn bone_labels_drop_the_rig_prefix() {
        assert_eq!(bone_label("mixamorig:RightArm"), "RightArm");
        assert_eq!(bone_label("mixamorigHips"), "Hips");
        assert_eq!(bone_label("spine"), "spine");
    }
}
