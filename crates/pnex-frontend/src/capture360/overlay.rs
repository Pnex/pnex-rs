//! Full-screen guided capture overlay (Android cfg module).
//!
//! STRICT STEP-BY-STEP (V3, 2026-09-10) — continuous triggering (V1) is
//! gone: it fired when moving backwards (|Δyaw| counted the user's
//! return — "photo already taken" during a correction) and
//! under-captured the ring (~33° azimuth hole on the 1st HD). New
//! model, the one of commercial 360 apps:
//!
//! 1. the user aims at their starting point and presses "Anchor here";
//! 2. each ring has N fixed targets (yaw = k·step, pitch = setpoint) — N
//!    and the step come from `guidance::ring_plan(real portrait hfov)`:
//!    overlap ≥ 20° and ring closure by construction;
//! 3. a frame fires ONLY on the current target, when alignment
//!    (yaw/pitch/roll) + stability (angular speed) + 700 ms dwell
//!    are held CONTINUOUSLY — turning backwards triggers NOTHING;
//! 4. capture → next target; "Retake" removes the last frame
//!    and steps the target back (driven by `UNDO_REQUEST`, single-writer).
//!
//! Rendering: preview + guidance SVG. Dioxus re-render ONLY on a
//! discrete change (banners, counter) — the continuous elements (guide
//! dot, dwell arc, arrow, degrees, stability, flash) are driven
//! through direct DOM at 15 Hz (`push_debug_dom` approach: the 30 Hz re-render
//! saturates the webview GPU, MALI BAD ALLOC — observed on device 2026-09-09).

use super::pipeline::CapturedFrame;
use super::state::{self, Phase};
use super::{fov, frames, guidance, sensors, wakelock};
use crate::components::gerbe::Gerbe;
use crate::components::ui::BTN_PRIMARY;
use dioxus::prelude::*;
use dioxus_i18n::t;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// DOM id of the `<video>` (also used by `frames.rs`).
const VIDEO_ID: &str = "p360-video";

/// Reticle radius (viewBox 400×400 units).
const RETICLE_R: f32 = 38.0;
/// Max distance of the guide dot from the centre (viewBox units).
const DOT_MAX_PX: f32 = 110.0;
/// Stability measurement window (ms) — max angular speed within it.
const STABLE_WINDOW_MS: u64 = 450;
/// DOM ids of the continuous elements (SVG/HTML) driven by eval at 15 Hz.
const ID_DOT: &str = "p360-dot";
const ID_ARROW: &str = "p360-arrow";
const ID_ARC: &str = "p360-arc";
const ID_DEG: &str = "p360-deg";
/// 1×1 transparent GIF used as the <video> poster.
const VIDEO_POSTER: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
const ID_PITCH: &str = "p360-pitch";
const ID_FLASH: &str = "p360-flash";

/// Render snapshot copied from the atomics on each tick — DISCRETE
/// only (values moving at 30 Hz go through the eval DOM layer,
/// see module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
struct UiState {
    /// Anchor set? (otherwise: "aim + Anchor" pill).
    anchored: bool,
    phase: Phase,
    /// Current ring and current step (frames captured in the ring).
    ring: usize,
    step: usize,
    /// Current plan (targets per ring) — see `guidance::ring_plan`.
    n_steps: usize,
    /// Discrete yaw hint: −1 left, 0 none, +1 right (shown
    /// only if the pitch band is held and |error| > tolerance).
    turn_dir: i8,
    /// Pitch band of the tilted ring held (rings 1/2; true otherwise).
    band_ok: bool,
    /// Show "Hold the position" (dwell in progress or unstable).
    hold: bool,
    error_code: u8,
    progress: u8,
    sensor_ok: bool,
}

/// Full-screen component. `on_uploaded`: callback after a successful upload or
/// a final failure — the page reloads the list there.
#[component]
pub fn Take360Overlay(on_close: Callback<()>, on_uploaded: Callback<()>) -> Element {
    // Frame memory — use_hook MANDATORY: a "bare" Arc::new is
    // recreated on EVERY render → n immortal Arcs in the driver loops →
    // memory leak + N finish jobs (swap-thrash, stitch stuck at 15 %,
    // observed on device 2026-09-09).
    let frames_mem: Arc<Mutex<Vec<CapturedFrame>>> = use_hook(|| Arc::new(Mutex::new(Vec::new())));
    // Render snapshot — updated by the tick loop, never from a
    // detached thread.
    let mut ui = use_signal(|| UiState {
        anchored: false,
        phase: Phase::Idle,
        ring: 0,
        step: 0,
        n_steps: 16,
        turn_dir: 0,
        band_ok: true,
        hold: false,
        error_code: 0,
        progress: 0,
        sensor_ok: true,
    });

    // Session opening: permission, sensor, camera, ring plan.
    use_future(move || async move {
        super::filelog::log("overlay: montage, reset session");
        state::reset_session();
        state::PHASE.store(Phase::Searching as u8, Ordering::Relaxed);
        wakelock::keep_screen_on(true);
        if !crate::capture::camera_granted() && !wait_camera_permission().await {
            state::ERROR_CODE.store(1, Ordering::Relaxed);
            state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
            return;
        }
        let sensor_started = sensors::start();
        super::filelog::log(&format!("overlay: sensors start → {sensor_started}"));
        if !sensor_started {
            state::ERROR_CODE.store(3, Ordering::Relaxed);
            state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
            return;
        }
        // The <video> is mounted on the first render — short retry. On
        // opening, we set the RING PLAN: sensor hfov (landscape) ×
        // track w/h = real portrait hfov → `guidance::ring_plan` (ring
        // closure guaranteed — see guidance.rs).
        for _ in 0..20 {
            match frames::open_camera(VIDEO_ID).await {
                Ok((w, h)) => {
                    super::filelog::log("overlay: camera opened");
                    let hfov_land =
                        fov::back_camera_hfov_deg().unwrap_or(pnex_stitcher::FALLBACK_HFOV_DEG);
                    let hfov_portrait = if w > 0 && h > 0 {
                        super::pipeline::hfov_for_orientation(hfov_land, w, h)
                    } else {
                        state::DEFAULT_PORTRAIT_HFOV_MD as f32 / 1000.0
                    };
                    super::filelog::log(&format!(
                        "overlay: hfov capteur {hfov_land:.1}° → portrait {hfov_portrait:.1}° ({w}×{h})"
                    ));
                    state::PORTRAIT_HFOV_MD
                        .store((hfov_portrait * 1000.0) as i32, Ordering::Relaxed);
                    let plan = guidance::ring_plan(hfov_portrait);
                    state::N_STEPS.store(plan.n_steps as u8, Ordering::Relaxed);
                    state::STEP_DEG_MD.store((plan.step_deg * 1000.0) as i32, Ordering::Relaxed);
                    super::filelog::log(&format!(
                        "overlay: plan d'anneau — {} cibles de {:.1}°",
                        plan.n_steps, plan.step_deg
                    ));
                    return;
                }
                Err(frames::FrameError::Permission) => {
                    state::ERROR_CODE.store(1, Ordering::Relaxed);
                    state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
                    return;
                }
                Err(frames::FrameError::NoVideo) => {
                    state::ERROR_CODE.store(2, Ordering::Relaxed);
                    state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
                    return;
                }
                Err(frames::FrameError::Unknown(_)) => {
                    crate::util::sleep(Duration::from_millis(150)).await;
                }
            }
        }
        state::ERROR_CODE.store(2, Ordering::Relaxed);
        state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
    });

    // Driver loop: 33 ms tick — strict step-by-step + UI snapshots + continuous
    // DOM layer (15 Hz).
    let mut last_result_seq = use_signal(|| 0u64);
    use_effect(move || {
        // A single driver loop per process (use_effect re-run →
        // 3 finishes triggered — observed on device 2026-09-09).
        if state::DRIVER_RUNNING.swap(true, Ordering::Relaxed) {
            return;
        }
        let frames_mem = frames_mem.clone();
        spawn(async move {
            let mut last_rendered: Option<UiState> = None;
            let mut last_debug_push: u64 = 0;
            // Current dwell (start instant) — None = no dwell.
            let mut dwell_start: Option<u64> = None;
            // Pose window for the angular speed (~[STABLE_WINDOW_MS]).
            let mut samples: Vec<(u64, guidance::Pose)> = Vec::with_capacity(16);
            let mut last_dom_push: u64 = 0;
            loop {
                crate::util::sleep(Duration::from_millis(33)).await;
                let beat = state::DRIVER_BEAT.fetch_add(1, Ordering::Relaxed);
                let now = state::now_ms();
                let phase = state::phase();

                // Session over: notify the page and stop the loop.
                let seq = state::RESULT_SEQ.load(Ordering::Relaxed);
                if seq != last_result_seq() {
                    last_result_seq.set(seq);
                    on_uploaded.call(());
                    break;
                }
                if matches!(phase, Phase::Idle) {
                    continue;
                }

                // ── "Retake": pop the last frame + step the target back.
                // The driver loop is the single writer of frames_mem (with
                // finish) — the UI button only posts the request.
                if state::UNDO_REQUEST.swap(false, Ordering::Relaxed) {
                    let step = state::STEP.load(Ordering::Relaxed) as usize;
                    if step > 0 && matches!(phase, Phase::Searching) {
                        if let Ok(mut mem) = frames_mem.lock() {
                            mem.pop();
                        }
                        state::STEP.store(step as u8 - 1, Ordering::Relaxed);
                        let last_yaw = frames_mem
                            .lock()
                            .ok()
                            .and_then(|m| m.last().map(|f| (f.pose.yaw * 1000.0) as i32))
                            .unwrap_or(0);
                        state::LAST_FRAME_YAW.store(last_yaw, Ordering::Relaxed);
                        dwell_start = None;
                        super::filelog::log(&format!(
                            "undo: back to target {}/{}",
                            step - 1,
                            state::N_STEPS.load(Ordering::Relaxed)
                        ));
                    }
                }

                // ── Step-by-step: current target → aligned+stable+dwell → fire.
                let pose = state::pose(now);
                if let Some(p) = pose {
                    samples.push((now, p));
                }
                // Prune samples outside the window (30 Hz sensor, 33 ms
                // poll: ~14 samples in the window).
                samples.retain(|(t, _)| now.saturating_sub(*t) <= STABLE_WINDOW_MS);
                let stable = guidance::max_rate_deg_s(&samples) <= guidance::MAX_RATE_DEG_S;

                let ring = state::RING.load(Ordering::Relaxed) as usize;
                let step = state::STEP.load(Ordering::Relaxed) as usize;
                let n_steps = state::N_STEPS.load(Ordering::Relaxed) as usize;
                let step_deg = state::STEP_DEG_MD.load(Ordering::Relaxed) as f32 / 1000.0;
                let (target_yaw, target_pitch) = guidance::target_for(ring, step, step_deg);
                // WITHOUT an anchor, NOTHING fires (the pose is already
                // relative to the session zero, but the starting point
                // must be SET by the user — "Anchor here").
                let decision = if state::anchored() {
                    guidance::decide(target_yaw, target_pitch, pose, stable, dwell_start, now)
                } else {
                    dwell_start = None;
                    guidance::Decision::Wait
                };

                match decision {
                    guidance::Decision::Wait | guidance::Decision::CancelDwell => {
                        dwell_start = None;
                    }
                    guidance::Decision::StartDwell => {
                        dwell_start = Some(now);
                    }
                    guidance::Decision::Dwell(_) => {}
                    guidance::Decision::Fire => {
                        dwell_start = None;
                        let Some(p) = pose else { continue };
                        state::PHASE.store(Phase::Capturing as u8, Ordering::Relaxed);
                        super::filelog::log(&format!(
                            "grab[{ring}/{step}] : tir (pose y={:.1} p={:.1} r={:.1})",
                            p.yaw, p.pitch, p.roll
                        ));
                        match frames::grab_frame(VIDEO_ID).await {
                            Ok(jpeg) => {
                                super::filelog::log(&format!(
                                    "grab[{ring}/{step}] : ok {} o",
                                    jpeg.len()
                                ));
                                if let Ok(mut mem) = frames_mem.lock() {
                                    mem.push(CapturedFrame { jpeg, pose: p });
                                }
                                state::CAPTURE_SEQ.fetch_add(1, Ordering::Relaxed);
                                state::LAST_FRAME_YAW
                                    .store((p.yaw * 1000.0) as i32, Ordering::Relaxed);
                                state::LAST_CAPTURE_MS.store(now, Ordering::Relaxed);
                                // Advance: next step, next ring or end.
                                match guidance::advance(ring, step, n_steps) {
                                    guidance::Advance::NextStep => {
                                        state::STEP.store(step as u8 + 1, Ordering::Relaxed);
                                    }
                                    guidance::Advance::NextRing => {
                                        state::RING.store(ring as u8 + 1, Ordering::Relaxed);
                                        state::STEP.store(0, Ordering::Relaxed);
                                        super::filelog::log(&format!(
                                            "ring: {ring} complet → anneau {}",
                                            ring + 1
                                        ));
                                    }
                                    guidance::Advance::Finish => {
                                        // Last ring complete: stop the
                                        // resources then detached pipeline
                                        // (the overlay stays mounted).
                                        sensors::stop();
                                        wakelock::keep_screen_on(false);
                                        frames::close_camera();
                                        let hfov = fov::back_camera_hfov_deg()
                                            .unwrap_or(pnex_stitcher::FALLBACK_HFOV_DEG);
                                        super::pipeline::start_finish_job(frames_mem.clone(), hfov);
                                        continue;
                                    }
                                }
                            }
                            Err(e) => {
                                let detail = match &e {
                                    frames::FrameError::Unknown(s) => s.clone(),
                                    other => format!("{other:?}"),
                                };
                                log::warn!("take360 : grab : {detail}");
                            }
                        }
                        state::PHASE.store(Phase::Searching as u8, Ordering::Relaxed);
                    }
                }

                // ── Snapshot UI discret ────────────────────────────────
                let err = pose
                    .map(|p| guidance::signed_error(target_yaw, target_pitch, p))
                    .unwrap_or((999.0, 999.0, 999.0));
                let band_ok = err.1.abs() <= guidance::RING_PITCH_TOL_DEG;
                let turn_dir = if band_ok && err.0.abs() > guidance::TOL_YAW_DEG {
                    if err.0 > 0.0 {
                        1
                    } else {
                        -1
                    }
                } else {
                    0
                };
                let hold = band_ok && turn_dir == 0 && (!stable || dwell_start.is_some());
                let snapshot = UiState {
                    anchored: state::anchored(),
                    phase,
                    ring,
                    step,
                    n_steps,
                    turn_dir,
                    band_ok,
                    hold,
                    error_code: state::ERROR_CODE.load(Ordering::Relaxed),
                    progress: state::PROGRESS.load(Ordering::Relaxed),
                    sensor_ok: state::ERROR_CODE.load(Ordering::Relaxed) != 3,
                };
                // FIRST round: always set (without it, ui stays Idle →
                // <video> never mounted — observed on device 2026-09-09).
                let changed = match last_rendered {
                    None => true,
                    Some(prev) => prev != snapshot,
                };
                if changed {
                    ui.set(snapshot);
                    last_rendered = Some(snapshot);
                }

                // ── Couche DOM continue (15 Hz) ────────────────────────
                if now.saturating_sub(last_dom_push) >= 66 {
                    last_dom_push = now;
                    push_overlay_dom(pose, target_yaw, target_pitch, dwell_start, now);
                }

                // Animated stitch progress (rendering does not report a fine
                // % — we slowly advance 30→80 to show it is alive).
                if matches!(phase, Phase::Stitching) {
                    let p = state::PROGRESS.load(Ordering::Relaxed);
                    if (30..80).contains(&p) {
                        state::PROGRESS.store(p + 1, Ordering::Relaxed);
                    }
                }

                // Debug in the DOM at 10 Hz (temporary — smoke test).
                // Diagnostics builds only (feature `diag`).
                if cfg!(feature = "diag") && now.saturating_sub(last_debug_push) >= 100 {
                    last_debug_push = now;
                    push_debug_dom(
                        beat as u32,
                        phase as u8,
                        &format!("{step}/{n_steps}"),
                        state::LAST_GRAB_ERR.load(Ordering::Relaxed),
                        state::STITCH_STATE.load(Ordering::Relaxed),
                        state::STITCH_IDX.load(Ordering::Relaxed),
                        state::RESULT_CODE.load(Ordering::Relaxed),
                    );
                }

                if state::CANCEL.load(Ordering::Relaxed) {
                    return;
                }
            }
        });
    });

    // Cleanup on unmount (cancellation, sensor, wakelock, camera).
    use_drop(move || {
        state::CANCEL.store(true, Ordering::Relaxed);
        sensors::stop();
        wakelock::keep_screen_on(false);
        frames::close_camera();
    });

    // Cancel: stop everything and close.
    let cancel = move |_| {
        state::CANCEL.store(true, Ordering::Relaxed);
        on_close.call(());
    };

    // "Retake" button: request to remove the last frame —
    // the driver loop executes it (single writer of frames_mem).
    let undo = move |_| {
        state::UNDO_REQUEST.store(true, Ordering::Relaxed);
    };

    // ───────────────────────── render ─────────────────────────
    let in_guidance = matches!(ui().phase, Phase::Searching | Phase::Capturing);
    let in_processing = matches!(ui().phase, Phase::Stitching | Phase::Uploading);
    let n_steps = ui().n_steps;
    // Progress: step of the current ring + session total (discrete — the
    // two bars only move on captures).
    let ring_pct = (ui().step * 100).checked_div(n_steps).unwrap_or(0);
    let overall_done = ui().ring * n_steps + ui().step;
    let overall_total = guidance::N_RINGS * n_steps;
    let overall_pct = overall_done.checked_div(overall_total).unwrap_or(0);
    let progress_label = t!(
        "media-take360-progress",
        ring: ui().ring + 1,
        rings: guidance::N_RINGS,
        pct: overall_pct
    )
    .to_string();

    rsx! {
        div { class: "fixed inset-0 z-[100] bg-black select-none",
            // Camera preview + guidance SVG (hidden once stitching starts).
            if in_guidance {
                video {
                    key: "{VIDEO_ID}",
                    id: VIDEO_ID,
                    autoplay: true,
                    playsinline: true,
                    // Transparent poster: hides the Android WebView default
                    // grey "play" placeholder shown until the stream starts.
                    poster: VIDEO_POSTER,
                    class: "absolute inset-0 h-full w-full object-cover",
                }
                // Guidance SVG: dwell arc (DOM fill), reticle,
                // guide dot, arrow, stability pill — positions
                // driven through direct DOM (push_overlay_dom, 15 Hz).
                svg {
                    class: "absolute inset-0 h-full w-full pointer-events-none",
                    view_box: "0 0 400 400",
                    circle {
                        id: ID_ARC,
                        cx: "200",
                        cy: "200",
                        r: "{RETICLE_R}",
                        fill: "none",
                        stroke: "#38bdf8",
                        "stroke-width": "5",
                        "stroke-linecap": "round",
                        "stroke-dasharray": "{ARC_LEN:.1}",
                        "stroke-dashoffset": "{ARC_LEN:.1}",
                        transform: "rotate(-90 200 200)",
                    }
                    circle {
                        cx: "200",
                        cy: "200",
                        r: "38",
                        fill: "none",
                        stroke: "white",
                        "stroke-width": "2",
                    }
                    // Guide dot: LEADS — the user moves towards it.
                    // (Vertical sign: SVG y goes down, pitch goes up → cy =
                    // 200 − dy, see push_overlay_dom.)
                    circle {
                        id: ID_DOT,
                        cx: "200",
                        cy: "200",
                        r: "7",
                        fill: "#38bdf8",
                        "fill-opacity": "0.9",
                        stroke: "white",
                        "stroke-width": "1.5",
                    }
                    // Directional chevron (DOM points) — visible when out of
                    // yaw alignment.
                    polygon {
                        id: ID_ARROW,
                        points: "",
                        fill: "#38bdf8",
                        "fill-opacity": "0.95",
                        style: "display:none",
                    }
                }
                // White capture flash — opacity driven through the DOM (no
                // dioxus re-render).
                div {
                    id: ID_FLASH,
                    class: "absolute inset-0 bg-white pointer-events-none",
                    style: "opacity:0",
                }
            }

            // Hint pill — t! with a literal, the match picks.
            if in_guidance && ui().anchored {
                div { class: "absolute top-20 inset-x-0 flex flex-col items-center gap-2 px-6",
                    div { class: "px-4 py-2 rounded-full bg-black/60 text-white text-base font-medium text-center",
                        if ui().ring >= 1 && !ui().band_ok {
                            if ui().ring == 1 {
                                {t!("media-take360-hint-tilt")}
                            } else {
                                {t!("media-take360-hint-tiltdown")}
                            }
                        } else if ui().turn_dir == 1 {
                            {t!("media-take360-hint-right")}
                        } else if ui().turn_dir == -1 {
                            {t!("media-take360-hint-left")}
                        } else if ui().hold {
                            {t!("media-take360-hint-hold")}
                        } else {
                            {t!("media-take360-hint-keep")}
                        }
                    }
                    div { class: "px-3 py-1 rounded-full bg-black/40 text-white/80 text-xs text-center",
                        {t!("media-take360-hint-auto")}
                    }
                }
            }

            // Before the start: what the capture expects, step by step, and
            // one primary action (anchors the ring on the current heading).
            if in_guidance && !ui().anchored {
                div { class: "absolute inset-x-4 bottom-8 rounded-2xl bg-black/75 p-5 text-white",
                    div { class: "text-lg font-semibold mb-3", {t!("media-take360-intro-title")} }
                    ol { class: "space-y-2 text-sm text-white/90 list-decimal pl-5 mb-5",
                        li { {t!("media-take360-intro-step1")} }
                        li { {t!("media-take360-intro-step2")} }
                        li { {t!("media-take360-intro-step3")} }
                    }
                    button {
                        class: "{BTN_PRIMARY} w-full",
                        onclick: move |_| state::reanchor(),
                        {t!("media-take360-start")}
                    }
                }
            }

            // Step counter + progress (discrete) + continuous readings
            // (degrees/pitch) in DOM spans.
            if in_guidance && ui().anchored {
                div { class: "absolute bottom-24 inset-x-0 text-center text-white",
                    if ui().ring >= 1 && !ui().band_ok {
                        div { class: "text-sm text-white/80 mb-1",
                            span { id: ID_PITCH, "…" }
                        }
                    }
                    div { class: "text-2xl font-bold", "{ui().step} / {n_steps}" }
                    div { class: "mt-2 mx-auto h-1.5 w-48 bg-white/20 rounded-full overflow-hidden",
                        div {
                            class: "h-full bg-signal-500",
                            width: "{ring_pct}%",
                        }
                    }
                    // Continuous reading: yaw distance + stability.
                    div { class: "mt-1.5 text-sm font-medium",
                        span { id: ID_DEG, "" }
                    }
                    div { class: "text-xs text-white/60 mt-1", "{progress_label}" }
                }
            }

            // Processing phases.
            if in_processing {
                div { class: "absolute inset-0 flex flex-col items-center justify-center gap-4 text-white",
                    Gerbe { size: 44 }
                    div { class: "text-lg",
                        if ui().phase == Phase::Stitching {
                            {t!("media-take360-stitching")}
                        } else {
                            {t!("media-take360-uploading")}
                        }
                    }
                    div { class: "text-sm text-white/70", "{ui().progress} %" }
                }
            }

            // Done.
            if ui().phase == Phase::Done {
                div { class: "absolute inset-0 flex flex-col items-center justify-center gap-4 text-white",
                    div { class: "text-4xl", "✓" }
                    div { class: "text-lg", {t!("media-take360-done")} }
                    button { class: BTN_PRIMARY, onclick: cancel, {t!("media-take360-close")} }
                }
            }

            // Errors: permission, camera, sensor, stitch/upload.
            if ui().phase == Phase::Error {
                div { class: "absolute inset-0 flex flex-col items-center justify-center gap-4 text-white px-8 text-center",
                    div { class: "text-lg",
                        match ui().error_code {
                            1 => t!("media-take360-permission"),
                            2 => t!("media-take360-nocamera"),
                            3 => t!("media-take360-no-sensor"),
                            _ => t!("media-take360-failed"),
                        }
                    }
                    button {
                        class: "px-6 py-2 bg-gray-700 text-white rounded-lg text-sm font-medium",
                        onclick: cancel,
                        {t!("media-take360-close")}
                    }
                }
            }

            // Raw internal state line: diagnostics builds only (feature `diag`).
            if cfg!(feature = "diag") {
                div {
                    id: "p360-debug",
                    class: "absolute bottom-4 left-4 text-green-400 text-xs font-mono bg-black/70 px-2 py-1 rounded",
                    "…"
                }
            }

            // Buttons: "Retake" (remove last frame), anchor
            // (origin/re-zero), cancel.
            if in_guidance {
                if ui().anchored && ui().step > 0 {
                    button {
                        class: "absolute bottom-8 left-6 px-3 py-1.5 rounded-full bg-black/60 text-white text-sm",
                        onclick: undo,
                        {t!("media-take360-undo")}
                    }
                }
                // Moving the start point only makes sense before the first
                // photo: afterwards it would shift the ring off the frames.
                if ui().anchored && ui().ring == 0 && ui().step == 0 {
                    button {
                        class: "absolute top-6 left-6 px-3 py-1.5 rounded-full bg-black/60 text-white text-sm",
                        onclick: move |_| state::reanchor(),
                        {t!("media-take360-restart-here")}
                    }
                }
                button {
                    class: "absolute top-5 right-5 h-10 w-10 flex items-center justify-center rounded-full bg-black/60 text-white",
                    title: t!("media-take360-cancel"),
                    aria_label: t!("media-take360-cancel"),
                    onclick: cancel,
                    crate::components::icons::X { class: "h-6 w-6" }
                }
            }
        }
    }
}

/// Asks for the CAMERA permission and waits for the system dialog's answer
/// (polled: the request is fire-and-forget on the JNI side). Without this the
/// first Take 360 ended on the error screen even when the user granted the
/// permission right away. Gives up after ~60 s or on cancel.
async fn wait_camera_permission() -> bool {
    super::filelog::log("overlay: CAMERA not granted, requesting");
    crate::capture::request_camera();
    for _ in 0..200 {
        crate::util::sleep(Duration::from_millis(300)).await;
        if state::CANCEL.load(Ordering::Relaxed) {
            return false;
        }
        if crate::capture::camera_granted() {
            super::filelog::log("overlay: CAMERA granted");
            return true;
        }
    }
    super::filelog::log("overlay: CAMERA still not granted, giving up");
    false
}

/// Length of the dwell arc (reticle circumference).
const ARC_LEN: f32 = 2.0 * std::f32::consts::PI * RETICLE_R;

/// Continuous DOM layer (15 Hz): positions/colours of the guidance
/// elements without a dioxus re-render. `push_debug_dom` approach — the 30 Hz
/// re-render saturates the webview GPU (MALI BAD ALLOC, observed on device).
fn push_overlay_dom(
    pose: Option<guidance::Pose>,
    target_yaw: f32,
    target_pitch: f32,
    dwell_start: Option<u64>,
    now: u64,
) {
    // Pre-anchor: the target would be relative to the session zero (often
    // captured WHILE MOVING — see state.rs) → guide dot in the CENTRE and
    // no rotation guidance: the instruction is "aim at your starting point
    // then Anchor here", no chasing an arbitrary target.
    let anchored = state::anchored();
    // Defaults: everything hidden/neutral if there is no pose.
    let (dx, dy) = pose
        .filter(|_| anchored)
        .map(|p| {
            guidance::dot_offset_px(
                guidance::signed_error(target_yaw, target_pitch, p),
                DOT_MAX_PX,
            )
        })
        .unwrap_or((0.0, 0.0));
    let (dot_x, dot_y) = (200.0 + dx, 200.0 - dy);
    let err_yaw = pose
        .map(|p| guidance::signed_error(target_yaw, target_pitch, p).0)
        .unwrap_or(0.0);
    let in_tol = err_yaw.abs() <= guidance::TOL_YAW_DEG;
    let band_ok = pose
        .map(|p| (target_pitch - p.pitch).abs() <= guidance::RING_PITCH_TOL_DEG)
        .unwrap_or(false);
    // Chevron: points at distance d (proportional to the error, bounded),
    // in the direction of the correction — hidden pre-anchor (see dot).
    let (arrow_pts, arrow_show) = if anchored && !in_tol && band_ok {
        let dir = err_yaw.signum() * guidance::ROTATION_DIR;
        let d = (err_yaw.abs() * 1.6).clamp(52.0, 150.0);
        let tip = 200.0 + dir * d;
        let back = 200.0 + dir * (d - 14.0);
        (format!("{tip:.0},190 {back:.0},200 {tip:.0},210"), true)
    } else {
        (String::new(), false)
    };
    let frac = dwell_start
        .map(|s| (now.saturating_sub(s)) as f32 / guidance::DWELL_MS as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let arc_offset = ARC_LEN * (1.0 - frac);
    let deg_text = if anchored && !in_tol && band_ok {
        let arrow_char = if err_yaw * guidance::ROTATION_DIR > 0.0 {
            "→"
        } else {
            "←"
        };
        format!("{arrow_char} {:.0}°", err_yaw.abs())
    } else {
        String::new()
    };
    let pitch_text = pose
        .map(|p| format!("{:+.0}°", p.pitch))
        .unwrap_or_default();

    // Flash: the JS keeps its own sequence — a delta triggers
    // the animation (opacity 0.7 → 250 ms fade).
    let capture_seq = state::CAPTURE_SEQ.load(Ordering::Relaxed);
    let _ = dioxus::document::eval(&format!(
        "(() => {{ \
         const dot = document.getElementById('{id_dot}'); \
         if (dot) {{ dot.setAttribute('cx', '{dot_x:.1}'); dot.setAttribute('cy', '{dot_y:.1}'); }} \
         const arc = document.getElementById('{id_arc}'); \
         if (arc) arc.setAttribute('stroke-dashoffset', '{arc_offset:.1}'); \
         const arrow = document.getElementById('{id_arrow}'); \
         if (arrow) {{ \
            if ({arrow_show}) {{ arrow.setAttribute('points', '{arrow_pts}'); arrow.style.display = ''; }} \
            else arrow.style.display = 'none'; \
         }} \
         const deg = document.getElementById('{id_deg}'); \
         if (deg) deg.textContent = '{deg_text}'; \
         const pit = document.getElementById('{id_pitch}'); \
         if (pit) pit.textContent = '{pitch_text}'; \
         if (window.__p360LastSeq === undefined) window.__p360LastSeq = {capture_seq}; \
         if (window.__p360LastSeq !== {capture_seq}) {{ \
            window.__p360LastSeq = {capture_seq}; \
            const f = document.getElementById('{id_flash}'); \
            if (f) {{ \
                f.style.transition = 'none'; f.style.opacity = '0.7'; \
                requestAnimationFrame(() => {{ \
                    f.style.transition = 'opacity 250ms'; f.style.opacity = '0'; \
                }}); \
            }} \
         }} \
         }})()",
        id_dot = ID_DOT,
        id_arc = ID_ARC,
        id_arrow = ID_ARROW,
        id_deg = ID_DEG,
        id_pitch = ID_PITCH,
        id_flash = ID_FLASH,
        dot_x = dot_x,
        dot_y = dot_y,
        arc_offset = arc_offset,
        arrow_show = arrow_show,
        arrow_pts = arrow_pts,
        deg_text = deg_text,
        pitch_text = pitch_text,
        capture_seq = capture_seq,
    ));
}

/// Raw state in the DOM at 10 Hz (temporary debug — device smoke test).
fn push_debug_dom(
    beat: u32,
    phase: u8,
    steps: &str,
    grab_err: u8,
    stitch_state: u8,
    stitch_idx: u8,
    result_code: u8,
) {
    let pose_alive = state::POSE_TICK.load(Ordering::Relaxed) > 0;
    let yaw = state::POSE_YAW.load(Ordering::Relaxed) as f32 / 1000.0;
    let pitch = state::POSE_PITCH.load(Ordering::Relaxed) as f32 / 1000.0;
    let roll = state::POSE_ROLL.load(Ordering::Relaxed) as f32 / 1000.0;
    let anchored = state::anchored();
    let _ = dioxus::document::eval(&format!(
        "(() => {{ \
         const dbg = document.getElementById('p360-debug'); \
         if (dbg) {{ \
            const v = document.getElementById('p360-video'); \
            dbg.textContent = 'beat=' + {beat} + ' ph=' + {phase} + ' step=' + '{steps}' \
                + ' anchored=' + {anchored} \
                + ' grabErr=' + {grab_err} + ' stitch=' + {stitch_state} + '/' + {stitch_idx} \
                + ' result=' + {result_code} \
                + ' stream=' + (window.__p360Ready === true) \
                + ' video=' + (v ? v.readyState : -1) \
                + ' pose=' + {pose_alive} + ' y/p/r=' + {yaw} + '/' + {pitch} + '/' + {roll}; \
         }} \
         }})()",
        beat = beat,
        phase = phase,
        steps = steps,
        grab_err = grab_err,
        stitch_state = stitch_state,
        stitch_idx = stitch_idx,
        result_code = result_code,
        anchored = anchored,
        pose_alive = pose_alive,
        yaw = format!("{yaw:+.1}"),
        pitch = format!("{pitch:+.1}"),
        roll = format!("{roll:+.1}"),
    ));
}
