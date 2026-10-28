//! Guided 360 panoramic capture (Take 360) — mobile.
//!
//! V3 model: 3-ring protocol (level, up, down), STRICT step-by-step
//! — N targets per ring computed from the real portrait hfov
//! (`guidance::ring_plan`), auto-shutter on the current target after an
//! aligned+stable dwell, on-device stitching via `pnex-stitcher` then
//! `kind=panorama` upload and HD server job through the media pipeline.
//!
//! Approaches followed:
//! - `capture.rs`: raw JNI, plain atomics + 500 ms page polling, `t!`
//!   labels resolved before detaching, stubs outside Android (keeps `task check`
//!   green on desktop/web);
//! - `media_viewer.rs`: JS injected via `document::eval` (getUserMedia
//!   preview + canvas grab → JPEG → chunked base64).
//!
//! Android submodules: `sensors` (rotation vector via ndk-sys),
//! `fov`/`wakelock` (JNI Camera2 / Window flag), `frames` (JS bridge),
//! `pipeline` (decode → stitch → upload, `spawn_forever`), `overlay`
//! (full-screen dioxus component).

// Desktop/web: the runtime consumers of guidance/state are cfg
// android (overlay, sensors, pipeline) — without allow, these items are
// "dead" on desktop (capture.rs approach: documented allow(dead_code),
// no phantom code in non-android native targets).
// filelog only makes sense outside wasm: JNI + ndk_context (deps absent
// from the web target). The wasm consumers (media_viewer wasm32) do not
// call it — only the native impl of media_viewer touches it.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod filelog;
// guidance carries its own inner #![allow(dead_code)] (alignment
// machine kept in reserve for v2) — an outer cfg_attr would be a duplicate
// (clippy 1.96 duplicated_attributes).
pub mod guidance;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod state;

#[cfg(target_os = "android")]
pub mod fov;
#[cfg(target_os = "android")]
pub mod frames;
#[cfg(target_os = "android")]
pub mod overlay;
#[cfg(target_os = "android")]
pub mod pipeline;
#[cfg(target_os = "android")]
pub mod sensors;
#[cfg(target_os = "android")]
pub mod wakelock;

/// Stub outside Android: same signature as the real component (the media page
/// compiles on every native target) — the button click has no
/// effect on desktop (see media page, `capture.rs` approach).
#[cfg(not(target_os = "android"))]
pub mod overlay {
    use dioxus::prelude::*;

    #[component]
    pub fn Take360Overlay(on_close: Callback<()>, on_uploaded: Callback<()>) -> Element {
        let _ = (on_close, on_uploaded);
        rsx! {}
    }
}
