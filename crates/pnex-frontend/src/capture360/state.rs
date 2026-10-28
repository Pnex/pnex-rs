//! Take 360 session state — pure atomics, zero lock, zero dioxus signal.
//!
//! `capture.rs` school: detached threads (sensor, pipeline) and the
//! guidance loop cannot touch dioxus signals (missing context → panic →
//! poisoned runtime mutex). All communication goes through these atomics,
//! polled by the overlay (33 ms tick) and by the media page (500 ms
//! polling for the final toast).

use super::guidance::Pose;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};

/// Session phase.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No session.
    Idle = 0,
    /// Session open: sensor + camera being opened.
    Starting = 1,
    /// Searching for alignment on the current target.
    Searching = 2,
    /// Dwell in progress (alignment held).
    Dwelling = 3,
    /// Frame grab in flight.
    Capturing = 4,
    /// Stitching on-device.
    Stitching = 5,
    /// Upload in flight.
    Uploading = 6,
    /// Panorama uploaded.
    Done = 7,
    /// Error (see [`ERROR_CODE`]).
    Error = 8,
}

/// Phase courante.
pub static PHASE: AtomicU8 = AtomicU8::new(Phase::Idle as u8);
/// Captured frames (0..=16).
pub static STEP: AtomicU8 = AtomicU8::new(0);
/// Progression stitching/upload (0-100).
pub static PROGRESS: AtomicU8 = AtomicU8::new(0);
/// Error code: 0 none, 1 permission, 2 camera, 3 sensor, 4 stitch,
/// 5 upload.
pub static ERROR_CODE: AtomicU8 = AtomicU8::new(0);
/// Counter of finished sessions + result code (media page toast).
pub static RESULT_SEQ: AtomicU64 = AtomicU64::new(0);
pub static RESULT_CODE: AtomicU8 = AtomicU8::new(0);

/// Poses in millidegrees (i32: ±180 000 is enough), relative to the
/// session zero. Written by the sensor thread at 30 Hz.
pub static POSE_YAW: AtomicI32 = AtomicI32::new(0);
pub static POSE_PITCH: AtomicI32 = AtomicI32::new(0);
pub static POSE_ROLL: AtomicI32 = AtomicI32::new(0);
/// Timestamp (unix ms) of the last sensor sample — freshness.
pub static POSE_TICK: AtomicU64 = AtomicU64::new(0);
/// Max age of a pose (ms) beyond which `pose()` returns None.
pub const POSE_MAX_AGE_MS: u64 = 250;

/// Session start yaw (millidegrees) — set by the sensor thread.
pub static ZERO_YAW: AtomicI32 = AtomicI32::new(0);
/// Session start pitch (millidegrees) — likewise: the aiming target is
/// the natural starting posture.
pub static ZERO_PITCH: AtomicI32 = AtomicI32::new(0);
/// User anchor (millidegrees, poses relative to session zero):
/// "Anchor here" redefines the ring origin on the current pose —
/// usable at start or at any time (sensor drift).
pub static ANCHOR_YAW: AtomicI32 = AtomicI32::new(0);
pub static ANCHOR_PITCH: AtomicI32 = AtomicI32::new(0);
/// Anchor set? Until then: no target, the user aims at their starting
/// point and presses "Anchor here" (the auto zero captured at startup
/// lands on the posture IN MOTION — device finding 2026-09-09).
pub static ANCHOR_VALID: AtomicBool = AtomicBool::new(false);
/// Anchored yaw of the last captured frame (millidegrees) — debug + "Retake"
/// recovery.
pub static LAST_FRAME_YAW: AtomicI32 = AtomicI32::new(0);
/// PORTRAIT HFOV of the rear camera (millidegrees) — set by the overlay
/// at session open (landscape sensor hfov × track w/h).
/// 0 = unknown → fallback [`DEFAULT_PORTRAIT_HFOV_MD`]. The ring plan
/// (number of targets, step) derives from it: closure guaranteed.
pub static PORTRAIT_HFOV_MD: AtomicI32 = AtomicI32::new(0);
/// Default portrait HFOV (millidegrees) — the Camera2 measurement rarely
/// fails, but the ring plan must be set even without it.
pub const DEFAULT_PORTRAIT_HFOV_MD: i32 = 43_700;
/// Number of targets per ring (current plan, see `guidance::ring_plan`)
/// and angular step (millidegrees) — set at session open.
pub static N_STEPS: AtomicU8 = AtomicU8::new(16);
pub static STEP_DEG_MD: AtomicI32 = AtomicI32::new(22_500);
/// Capture sequence: incremented on EACH successful frame — the DOM
/// layer (15 Hz eval) reads the trigger from it for the visual flash
/// without a dioxus re-render.
pub static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(0);
/// "Retake" request (remove the last frame, step the target back) —
/// set by the UI button, handled by the driver loop (single-writer).
pub static UNDO_REQUEST: AtomicBool = AtomicBool::new(false);
/// Timestamp of the last capture (unix ms) — anti-burst.
pub static LAST_CAPTURE_MS: AtomicU64 = AtomicU64::new(0);
/// Last grab error (0 = none, 1 = eval, 2 = json, 3 = no payload,
/// 4 = data: prefix, 5 = base64, 6 = video missing/empty) — shown
/// in the debug line (the logger does not output, device finding).
pub static LAST_GRAB_ERR: AtomicU8 = AtomicU8::new(0);

/// Current capture ring (Take 360 V2): 0 = flat turn, 1 = ring tilted
/// upward (real north pole). The end of a ring triggers the finish only
/// after ring 1 (see overlay — the stitcher accepts arbitrary poses,
/// the lat bands are enough).
pub static RING: AtomicU8 = AtomicU8::new(0);
/// Stitch state (debug): 0 idle, 1 decode, 2 render, 3 encode, 4 ready,
/// 5 error.
pub static STITCH_STATE: AtomicU8 = AtomicU8::new(0);
/// Frame being decoded (0-15) — debug.
pub static STITCH_IDX: AtomicU8 = AtomicU8::new(0);
/// Driver loop heartbeat (advances on each 33 ms tick) — if frozen, the
/// loop/executor is dead; if it advances, it is alive.
pub static DRIVER_BEAT: AtomicU64 = AtomicU64::new(0);
/// Guard: a single finish per session (the driver loop could triple —
/// 3 blocked stitch threads seen on device 2026-09-09).
pub static FINISH_STARTED: AtomicBool = AtomicBool::new(false);
/// Guard: a single driver loop per process (use_effect re-run).
pub static DRIVER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Anchor set?
#[must_use]
pub fn anchored() -> bool {
    ANCHOR_VALID.load(Ordering::Relaxed)
}

/// Abort requested by the UI (cancellation or overlay unmount).
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// Wall clock in unix ms (same origin as [`POSE_TICK`]).
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Phase courante.
#[must_use]
pub fn phase() -> Phase {
    match PHASE.load(Ordering::Relaxed) {
        1 => Phase::Starting,
        2 => Phase::Searching,
        3 => Phase::Dwelling,
        4 => Phase::Capturing,
        5 => Phase::Stitching,
        6 => Phase::Uploading,
        7 => Phase::Done,
        8 => Phase::Error,
        _ => Phase::Idle,
    }
}

/// Current pose if fresh (None if the sensor is silent > [`POSE_MAX_AGE_MS`]
/// or never seen).
///
/// Consistency: POSE_TICK is re-read after the three angles — if it moved
/// during the read, the sample is mixed → None (the overlay will retry
/// on the next tick, 33 ms).
#[must_use]
pub fn pose(now: u64) -> Option<Pose> {
    let tick = POSE_TICK.load(Ordering::Relaxed);
    if tick == 0 || now.saturating_sub(tick) > POSE_MAX_AGE_MS {
        return None;
    }
    let t1 = POSE_TICK.load(Ordering::Relaxed);
    let yaw = POSE_YAW.load(Ordering::Relaxed);
    let pitch = POSE_PITCH.load(Ordering::Relaxed);
    let roll = POSE_ROLL.load(Ordering::Relaxed);
    let t2 = POSE_TICK.load(Ordering::Relaxed);
    if t1 != t2 {
        return None;
    }
    // User anchor: subtracted on read — the targets (k·22.5°) become
    // relative to the anchored point, at any time.
    let anchor_yaw = ANCHOR_YAW.load(Ordering::Relaxed) as f32 / 1000.0;
    let anchor_pitch = ANCHOR_PITCH.load(Ordering::Relaxed) as f32 / 1000.0;
    Some(Pose {
        yaw: crate::capture360::guidance::wrap180(yaw as f32 / 1000.0 - anchor_yaw),
        pitch: pitch as f32 / 1000.0 - anchor_pitch,
        roll: roll as f32 / 1000.0,
    })
}

/// "Anchor here": the ring origin becomes the current pose (pitch keeps
/// its roll, only yaw/pitch are re-anchored). Usable at any time — the
/// ring geometry stays consistent (everything is relative).
pub fn reanchor() {
    let yaw = POSE_YAW.load(Ordering::Relaxed);
    let pitch = POSE_PITCH.load(Ordering::Relaxed);
    ANCHOR_YAW.store(yaw, Ordering::Relaxed);
    ANCHOR_PITCH.store(pitch, Ordering::Relaxed);
    ANCHOR_VALID.store(true, Ordering::Relaxed);
}

/// Reset of the session state (when the overlay opens).
pub fn reset_session() {
    PHASE.store(Phase::Idle as u8, Ordering::Relaxed);
    STEP.store(0, Ordering::Relaxed);
    PROGRESS.store(0, Ordering::Relaxed);
    ERROR_CODE.store(0, Ordering::Relaxed);
    CANCEL.store(false, Ordering::Relaxed);
    POSE_YAW.store(0, Ordering::Relaxed);
    POSE_PITCH.store(0, Ordering::Relaxed);
    POSE_ROLL.store(0, Ordering::Relaxed);
    POSE_TICK.store(0, Ordering::Relaxed);
    ZERO_YAW.store(0, Ordering::Relaxed);
    ZERO_PITCH.store(0, Ordering::Relaxed);
    ANCHOR_YAW.store(0, Ordering::Relaxed);
    ANCHOR_PITCH.store(0, Ordering::Relaxed);
    ANCHOR_VALID.store(false, Ordering::Relaxed);
    LAST_FRAME_YAW.store(0, Ordering::Relaxed);
    LAST_CAPTURE_MS.store(0, Ordering::Relaxed);
    LAST_GRAB_ERR.store(0, Ordering::Relaxed);
    PORTRAIT_HFOV_MD.store(0, Ordering::Relaxed);
    N_STEPS.store(16, Ordering::Relaxed);
    STEP_DEG_MD.store(22_500, Ordering::Relaxed);
    CAPTURE_SEQ.store(0, Ordering::Relaxed);
    UNDO_REQUEST.store(false, Ordering::Relaxed);
    RING.store(0, Ordering::Relaxed);
    STITCH_STATE.store(0, Ordering::Relaxed);
    STITCH_IDX.store(0, Ordering::Relaxed);
    FINISH_STARTED.store(false, Ordering::Relaxed);
    DRIVER_RUNNING.store(false, Ordering::Relaxed);
    DRIVER_BEAT.store(0, Ordering::Relaxed);
}
