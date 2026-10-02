//! Cameras page (camera-video.md D80): the org's cameras as a CRUD table
//! (crud socle) — live state, capture summary — with three row actions in
//! modals: live view (`/ws/camera/live`, open only while the modal is: a
//! viewer keeps an `on_demand` camera awake), continuous recording (one
//! day timeline chaining the MJPEG-AVI segments written by flows, D78/D79,
//! with range export and day delete) and capture settings (D76,
//! owner/admin only — the server enforces, the UI hides).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::camera::{CameraView, CaptureMode};

use crate::api;
use crate::components::crud::filters::RefreshButton;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::modal::Modal;
use crate::state::{org, session};
use crate::util::sleep;

mod flash;
mod live;
mod player;
mod recordings;
mod settings;

use flash::FlashToggle;
use live::LiveView;
use recordings::RecordingsDialog;
use settings::{age_label, framesize_label, SettingsDialog};

/// User role in the current org (media/devices school).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

/// Live-state refresh period of the camera list (streaming badge, viewers,
/// last frame age) — the frames themselves never poll (WebSocket).
const LIST_REFRESH_SECS: u64 = 10;

/// Row action buttons (full literals for the Tailwind scan).
const ROW_BTN: &str = "inline-flex items-center px-3 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50";
const ROW_BTN_PRIMARY: &str = "inline-flex items-center px-3 py-1.5 text-sm text-white bg-blue-600 rounded-lg hover:bg-blue-700";

#[component]
pub fn Cameras() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut live_for = use_signal(|| None::<CameraView>);
    let mut recordings_for = use_signal(|| None::<CameraView>);
    let mut settings_for = use_signal(|| None::<CameraView>);
    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    // Re-fetched on `reload` and on org switch (both read inside).
    let cameras = use_resource(move || async move {
        let _ = reload();
        let _ = org::ORG.read();
        api::cameras::list().await
    });

    // Periodic live-state refresh; the modals hold their own copy of the
    // row, so an open live view or settings form survives it.
    use_future(move || async move {
        loop {
            sleep(std::time::Duration::from_secs(LIST_REFRESH_SECS)).await;
            reload.with_mut(|r| *r += 1);
        }
    });

    let (list_state, is_empty, rows) = match &*cameras.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(list)) => (Some(Ok(())), list.is_empty(), list.clone()),
        Some(Err(err)) => (Some(Err(err.clone())), false, Vec::new()),
    };
    let now_ms = chrono::Utc::now().timestamp_millis();

    let columns = vec![
        Column::new(t!("cameras-col-camera").to_string(), |c: &CameraView| {
            rsx! { "{c.device_id}" }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("cameras-col-status").to_string(), |c: &CameraView| {
            rsx! {
                if c.streaming {
                    span { class: "px-2 py-0.5 rounded-full text-xs bg-green-100 text-green-700", {t!("cameras-streaming")} }
                } else {
                    span { class: "px-2 py-0.5 rounded-full text-xs bg-gray-100 text-gray-600", {t!("cameras-idle")} }
                }
            }
        }),
        Column::new(t!("cameras-col-capture").to_string(), |c: &CameraView| {
            let mode = match c.settings.capture_mode {
                CaptureMode::OnDemand => t!("cameras-mode-on-demand"),
                CaptureMode::Continuous => t!("cameras-mode-continuous"),
            };
            rsx! {
                span { class: "text-sm text-gray-600",
                    {framesize_label(c.settings.framesize)}
                    " · {c.settings.fps} fps · "
                    {mode}
                }
            }
        }),
        Column::new(
            t!("cameras-col-last-frame").to_string(),
            move |c: &CameraView| {
                let label = match c.last_frame_ms {
                    Some(ms) => t!("cameras-last-frame", age: age_label(ms, now_ms)).to_string(),
                    None => t!("cameras-no-frame-yet").to_string(),
                };
                let viewers = t!("cameras-viewers", count: c.viewers).to_string();
                rsx! {
                    div { class: "text-sm text-gray-600", "{label}" }
                    div { class: "text-xs text-gray-400", "{viewers}" }
                }
            },
        ),
        Column::new(String::new(), move |c: &CameraView| {
            // One clone per button: each `move` closure consumes its own.
            let cam_live = c.clone();
            let cam_rec = c.clone();
            let cam_settings = c.clone();
            rsx! {
                div { class: "flex justify-end gap-2",
                    button {
                        class: ROW_BTN_PRIMARY,
                        r#type: "button",
                        onclick: move |_| live_for.set(Some(cam_live.clone())),
                        icons::Video { class: "h-4 w-4 mr-1" }
                        {t!("cameras-live-start")}
                    }
                    button {
                        class: ROW_BTN,
                        r#type: "button",
                        onclick: move |_| recordings_for.set(Some(cam_rec.clone())),
                        {t!("cameras-recordings")}
                    }
                    if can_write {
                        button {
                            class: ROW_BTN,
                            r#type: "button",
                            title: t!("cameras-settings"),
                            onclick: move |_| settings_for.set(Some(cam_settings.clone())),
                            icons::Wrench { class: "h-4 w-4" }
                        }
                    }
                }
            }
        }),
    ];

    rsx! {
        ListLayout {
            title: t!("cameras-title").to_string(),
            subtitle: Some(t!("cameras-subtitle").to_string()),
            can_write,
            actions: rsx! {
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            },
            ListStates {
                state: list_state,
                is_empty,
                empty_message: t!("cameras-empty-title").to_string(),
                empty_icon: rsx! {
                    icons::Video { class: "h-8 w-8 text-gray-400" }
                },
                empty_detail: rsx! {
                    p { class: "text-gray-600 mt-2 max-w-xl mx-auto", {t!("cameras-empty-message")} }
                },
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|c: &CameraView| c.device.to_string()),
                }
            }
        }
        if let Some(cam) = live_for() {
            Modal {
                title: t!("cameras-live-title", camera : cam.device_id.clone()).to_string(),
                max_width: "max-w-4xl".to_string(),
                on_close: move |_| live_for.set(None),
                LiveView { key: "live-{cam.device}", device: cam.device }
                // Flash LED switch (O15) — pin writes are owner/admin only.
                if can_write {
                    FlashToggle { key: "flash-{cam.device}", device: cam.device }
                }
            }
        }
        if let Some(cam) = recordings_for() {
            RecordingsDialog {
                key: "rec-{cam.device}",
                cam: cam.clone(),
                can_write,
                on_close: move |_| recordings_for.set(None),
            }
        }
        if let Some(cam) = settings_for() {
            SettingsDialog {
                key: "settings-{cam.device}",
                cam: cam.clone(),
                on_close: move |_| settings_for.set(None),
                on_saved: move |_| {
                    settings_for.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
    }
}
