//! Capture settings dialog (D76) and the display helpers of the camera
//! table.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::camera::{
    CameraSettings, CameraView, CaptureMode, FrameSize, FPS_MAX, FPS_MIN, QUALITY_MAX, QUALITY_MIN,
};

use crate::api;
use crate::api::error::ApiError;
use crate::components::crud::form::FormDialog;
use crate::state::toasts;

/// Age of the last frame, compact (`3 s`, `4 min`, `2 h`, `5 d`) — data,
/// no i18n (units are international symbols).
pub(super) fn age_label(last_frame_ms: i64, now_ms: i64) -> String {
    let secs = ((now_ms - last_frame_ms).max(0)) / 1000;
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        format!("{} h", secs / 3600)
    } else {
        format!("{} d", secs / 86_400)
    }
}

/// Framesize option label: `VGA · 640×480`.
pub(super) fn framesize_label(f: FrameSize) -> String {
    let (w, h) = f.dims();
    format!("{} · {w}×{h}", f.wire().to_uppercase())
}

/// Capture settings dialog (owner/admin — the table hides the button for
/// viewers, the server enforces) — every field sent at once by PATCH; 400
/// field tokens (`range:10..63`, `invalid`) rendered through i18n.
#[component]
pub(super) fn SettingsDialog(
    cam: CameraView,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let device = cam.device;
    let initial = cam.settings.clone();
    let mut framesize = use_signal(move || initial.framesize);
    let mut quality = use_signal(move || initial.quality.to_string());
    let mut fps = use_signal(move || initial.fps.to_string());
    let mut mode = use_signal(move || initial.capture_mode);
    let mut vflip = use_signal(move || initial.vflip);
    let mut hmirror = use_signal(move || initial.hmirror);
    let mut field_errors = use_signal(Vec::<(String, String)>::new);
    let mut save_error = use_signal(|| None::<ApiError>);
    let mut saving = use_signal(|| false);
    let disabled = saving();

    let error_of = move |field: &str| -> Option<String> {
        field_errors
            .read()
            .iter()
            .find(|(f, _)| f == field)
            .map(|(f, token)| crate::api::error_i18n::localize_field(f, token))
    };
    let quality_err = error_of("quality");
    let fps_err = error_of("fps");
    let framesize_err = error_of("framesize");
    let mode_err = error_of("capture_mode");

    let save = move |_: ()| {
        let saved_msg = t!("cameras-settings-saved").to_string();
        // Local parse first: a non-number never reaches the server.
        let mut local = Vec::new();
        let q = quality.peek().trim().parse::<u8>().ok();
        let f = fps.peek().trim().parse::<u8>().ok();
        if q.is_none() {
            local.push((
                "quality".to_string(),
                format!("range:{QUALITY_MIN}..{QUALITY_MAX}"),
            ));
        }
        if f.is_none() {
            local.push(("fps".to_string(), format!("range:{FPS_MIN}..{FPS_MAX}")));
        }
        if !local.is_empty() {
            field_errors.set(local);
            return;
        }
        let settings = CameraSettings {
            framesize: *framesize.peek(),
            quality: q.unwrap_or_default(),
            fps: f.unwrap_or_default(),
            capture_mode: *mode.peek(),
            vflip: *vflip.peek(),
            hmirror: *hmirror.peek(),
        };
        let patch = api::cameras::SettingsPatch::from_settings(&settings);
        spawn(async move {
            saving.set(true);
            field_errors.set(Vec::new());
            save_error.set(None);
            match api::cameras::patch_settings(device, &patch).await {
                Ok(_) => {
                    toasts::success(saved_msg);
                    on_saved.call(());
                }
                Err(err) => {
                    let fields = api::cameras::field_errors(&err);
                    if fields.is_empty() {
                        save_error.set(Some(err));
                    } else {
                        field_errors.set(fields);
                    }
                }
            }
            saving.set(false);
        });
    };

    rsx! {
        FormDialog {
            title: t!("cameras-settings-title", camera: cam.device_id.clone()).to_string(),
            submit_label: t!("cameras-settings-save").to_string(),
            on_close,
            on_submit: save,
            busy: saving(),
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("cameras-framesize")} }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white",
                    disabled,
                    onchange: move |event| {
                        if let Some(f) = FrameSize::from_wire(&event.value()) {
                            framesize.set(f);
                        }
                    },
                    for f in FrameSize::ALL {
                        option { key: "{f.wire()}", value: f.wire(), selected: framesize() == f, {framesize_label(f)} }
                    }
                }
                if let Some(e) = framesize_err {
                    span { class: "text-xs text-red-600", {e} }
                }
            }
            div { class: "grid grid-cols-2 gap-3",
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("cameras-quality")} }
                    input {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                        r#type: "number",
                        min: "{QUALITY_MIN}",
                        max: "{QUALITY_MAX}",
                        value: "{quality}",
                        disabled,
                        oninput: move |event| quality.set(event.value()),
                    }
                    span { class: "text-xs text-gray-400 block", {t!("cameras-quality-hint", min: QUALITY_MIN, max: QUALITY_MAX)} }
                    if let Some(e) = quality_err {
                        span { class: "text-xs text-red-600 block", {e} }
                    }
                }
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("cameras-fps")} }
                    input {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                        r#type: "number",
                        min: "{FPS_MIN}",
                        max: "{FPS_MAX}",
                        value: "{fps}",
                        disabled,
                        oninput: move |event| fps.set(event.value()),
                    }
                    span { class: "text-xs text-gray-400 block", {t!("cameras-fps-hint", min: FPS_MIN, max: FPS_MAX)} }
                    if let Some(e) = fps_err {
                        span { class: "text-xs text-red-600 block", {e} }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("cameras-capture-mode")} }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white",
                    disabled,
                    onchange: move |event| {
                        if let Some(m) = CaptureMode::from_wire(&event.value()) {
                            mode.set(m);
                        }
                    },
                    option { value: "on_demand", selected: mode() == CaptureMode::OnDemand, {t!("cameras-mode-on-demand")} }
                    option { value: "continuous", selected: mode() == CaptureMode::Continuous, {t!("cameras-mode-continuous")} }
                }
                span { class: "text-xs text-gray-500 mt-1 block",
                    if mode() == CaptureMode::Continuous {
                        {t!("cameras-mode-continuous-help")}
                    } else {
                        {t!("cameras-mode-on-demand-help")}
                    }
                }
                if let Some(e) = mode_err {
                    span { class: "text-xs text-red-600 block", {e} }
                }
            }
            div { class: "flex gap-4",
                label { class: "flex items-center gap-2 text-sm text-gray-700 select-none",
                    input {
                        class: "h-4 w-4 accent-blue-600",
                        r#type: "checkbox",
                        checked: vflip(),
                        disabled,
                        onchange: move |event| vflip.set(event.checked()),
                    }
                    {t!("cameras-vflip")}
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700 select-none",
                    input {
                        class: "h-4 w-4 accent-blue-600",
                        r#type: "checkbox",
                        checked: hmirror(),
                        disabled,
                        onchange: move |event| hmirror.set(event.checked()),
                    }
                    {t!("cameras-hmirror")}
                }
            }
            if let Some(err) = save_error() {
                div { class: "rounded-lg bg-red-50 border border-red-200 p-2 text-xs text-red-700",
                    {crate::api::error_i18n::localize(&err)}
                }
            }
        }
    }
}
