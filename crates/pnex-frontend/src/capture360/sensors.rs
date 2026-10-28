//! Android orientation sensor — raw `ndk-sys` FFI (the `ndk` 0.9 crate
//! exposes no sensor wrapper, see exploration).
//!
//! Dedicated thread: ALooper prepared on this thread, NDK SensorManager
//! event queue (no Java classes). Rotation vector
//! (TYPE_ROTATION_VECTOR = 11, geomagnetic fallback = 20) at 30 Hz, converted
//! to (yaw, pitch, roll) in the `pnex-stitcher` convention:
//!
//! 1. `R = quat_to_mat(q)`: device→world (X east, Y north, Z sky);
//! 2. back camera: optical axis = −Z device, image right = +X device, image
//!    down = −Y device → `M = S·Rᵀ` with `S = diag(1, −1, −1)`: rows
//!    of M = (R col1, −R col2, −R col3);
//! 3. `euler_from_mat(M)` → (yaw, pitch, roll), yaw relative to the session
//!    zero (mean of the first samples).
//!
//! The remaining signs (requested rotation direction, guide dot) are
//! isolated in `guidance.rs` (ROTATION_DIR) — calibrated at the device
//! smoke test. If the ring comes out reversed or flipped, flip the
//! constants THERE.

use super::state;
use crate::capture::{attach_env, package_name};
use pnex_stitcher::geom::{euler_from_mat, quat_to_mat, Mat3};
use std::sync::atomic::{AtomicBool, Ordering};

/// Android sensor types (ASensorEvent.type_).
const TYPE_ROTATION_VECTOR: i32 = 11;
const TYPE_GEOMAGNETIC_ROTATION_VECTOR: i32 = 20;

/// Requested period: 30 Hz.
const PERIOD_US: i32 = 33_333;
/// Number of samples for the session zero.
const ZERO_SAMPLES: usize = 5;

/// True while the sensor thread is expected to be alive.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts the sensor thread. `false` if already running.
pub fn start() -> bool {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return false;
    }
    state::CANCEL.store(false, Ordering::Relaxed);
    state::POSE_TICK.store(0, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("pnex-pose-sensor".into())
        .spawn(sensor_loop)
        .is_ok()
}

/// Stops the sensor thread (stop request + releases the liveness flag).
pub fn stop() {
    state::CANCEL.store(true, Ordering::Relaxed);
    RUNNING.store(false, Ordering::Relaxed);
}

/// Rows of `M = S·Rᵀ`: (R col1, −R col2, −R col3) — see module docs.
fn back_camera_matrix(r: Mat3) -> Mat3 {
    Mat3([
        r.0[0], r.0[3], r.0[6], // image right = +X device
        -r.0[1], -r.0[4], -r.0[7], // image down = screen −Y device
        -r.0[2], -r.0[5], -r.0[8], // axe optique = −Z device
    ])
}

/// Thread body: ALooper loop + sensor event queue.
///
/// Logs to logcat via the global panic hook (PNEX-PANIC); if the
/// sensor is missing, sets `ERROR_CODE = 3` and exits.
fn sensor_loop() {
    // 1) Package name (JNI, capture.rs pattern) for the NDK manager.
    let pkg = attach_env().and_then(|env| unsafe {
        let ctx = ndk_context::try_android_context()?;
        package_name(env, ctx.context() as jni::sys::jobject)
    });
    let Some(pkg) = pkg else {
        return sensor_missing();
    };
    let Ok(pkg_c) = std::ffi::CString::new(pkg) else {
        return sensor_missing();
    };

    // 2) Manager + sensor (rotation vector, geomagnetic fallback).
    // SAFETY: ndk-sys FFI — signatures checked against ndk-sys 0.6; the
    // manager is a thread-safe singleton on the NDK side.
    let manager = unsafe { ndk_sys::ASensorManager_getInstanceForPackage(pkg_c.as_ptr()) };
    if manager.is_null() {
        return sensor_missing();
    }
    let sensor = unsafe { ndk_sys::ASensorManager_getDefaultSensor(manager, TYPE_ROTATION_VECTOR) };
    let sensor = if sensor.is_null() {
        unsafe {
            ndk_sys::ASensorManager_getDefaultSensor(manager, TYPE_GEOMAGNETIC_ROTATION_VECTOR)
        }
    } else {
        sensor
    };
    if sensor.is_null() {
        return sensor_missing();
    }

    // 3) ALooper on THIS thread + event queue (no callback:
    //    ALLOW_NON_CALLBACKS + pollAll).
    let looper =
        unsafe { ndk_sys::ALooper_prepare(ndk_sys::ALOOPER_PREPARE_ALLOW_NON_CALLBACKS as i32) };
    if looper.is_null() {
        return sensor_missing();
    }
    // Ident LOOPER_ID_USER (3) — the standard NDK pattern without callback:
    // pollAll returns the queue's ident when events arrive.
    // (ALOOPER_POLL_CALLBACK = -2 is reserved for queues WITH a callback —
    // with it, pollAll never returns the events: pose=false
    // observed on device 2026-09-09.)
    const LOOPER_ID_USER: i32 = 3;
    let queue = unsafe {
        ndk_sys::ASensorManager_createEventQueue(
            manager,
            looper,
            LOOPER_ID_USER,
            None,
            std::ptr::null_mut(),
        )
    };
    if queue.is_null() {
        return sensor_missing();
    }
    unsafe {
        ndk_sys::ASensorEventQueue_enableSensor(queue, sensor);
        ndk_sys::ASensorEventQueue_setEventRate(queue, sensor, PERIOD_US);
    }

    // 4) Loop: drain events → conversion → atomics. Exits on
    //    CANCEL (overlay unmount / end of ring).
    let mut zero_samples: Vec<f32> = Vec::new();
    let mut zero_yaw: Option<f32> = None;
    let mut zero_pitch: Option<f32> = None;
    let mut last_log_ms: u64 = 0;

    while !state::CANCEL.load(Ordering::Relaxed) {
        // 100 ms: wake-up bound to notice CANCEL (timeout = −3).
        let mut fd: i32 = 0;
        let mut events: i32 = 0;
        let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
        let ident = unsafe {
            ndk_sys::ALooper_pollAll(
                100,
                std::ptr::from_mut(&mut fd),
                std::ptr::from_mut(&mut events),
                std::ptr::from_mut(&mut data),
            )
        };
        let _ = (fd, events);
        match ident {
            // LOOPER_ID_USER = 3: sensor events ready.
            3 => {
                drain(
                    queue,
                    &mut zero_samples,
                    &mut zero_yaw,
                    &mut zero_pitch,
                    &mut last_log_ms,
                );
            }
            // ALOOPER_POLL_TIMEOUT = −3, error = −1: recheck CANCEL.
            _ => {}
        }
    }

    // 5) Cleanup.
    unsafe {
        ndk_sys::ASensorEventQueue_disableSensor(queue, sensor);
        ndk_sys::ASensorManager_destroyEventQueue(manager, queue);
    }
}

/// Drains the queue: quaternion conversion → relative pose → atomics.
fn drain(
    queue: *mut ndk_sys::ASensorEventQueue,
    zero_samples: &mut Vec<f32>,
    zero_yaw: &mut Option<f32>,
    zero_pitch: &mut Option<f32>,
    last_log_ms: &mut u64,
) {
    let mut events = [0u8; std::mem::size_of::<ndk_sys::ASensorEvent>() * 16];
    loop {
        let n = unsafe {
            ndk_sys::ASensorEventQueue_getEvents(
                queue,
                events.as_mut_ptr().cast::<ndk_sys::ASensorEvent>(),
                16,
            )
        };
        if n <= 0 {
            return;
        }
        let slice = unsafe {
            std::slice::from_raw_parts(events.as_ptr().cast::<ndk_sys::ASensorEvent>(), n as usize)
        };
        for ev in slice {
            if ev.type_ != TYPE_ROTATION_VECTOR && ev.type_ != TYPE_GEOMAGNETIC_ROTATION_VECTOR {
                continue;
            }
            // SAFETY: bindgen union — `data` for the rotation types.
            let q = unsafe { ev.__bindgen_anon_1.__bindgen_anon_1.data };
            let (yaw_abs_deg, pitch, roll) =
                euler_from_mat(back_camera_matrix(quat_to_mat([q[0], q[1], q[2], q[3]])));
            // Session zero: mean of the first N (yaw, pitch) — the
            // starting posture becomes the origin (yaw AND pitch).
            let yaw_rel = match zero_yaw {
                Some(z) => super::guidance::wrap180(yaw_abs_deg - *z),
                None => {
                    zero_samples.push(yaw_abs_deg);
                    if zero_samples.len() >= ZERO_SAMPLES {
                        let mean = zero_samples.iter().sum::<f32>() / zero_samples.len() as f32;
                        *zero_yaw = Some(super::guidance::wrap180(mean));
                        *zero_pitch = Some(pitch);
                    }
                    0.0
                }
            };
            let pitch_rel = match zero_pitch {
                Some(z) => pitch - *z,
                None => 0.0,
            };
            state::POSE_YAW.store((yaw_rel * 1000.0) as i32, Ordering::Relaxed);
            state::POSE_PITCH.store((pitch_rel * 1000.0) as i32, Ordering::Relaxed);
            state::POSE_ROLL.store((roll * 1000.0) as i32, Ordering::Relaxed);
            state::POSE_TICK.store(state::now_ms(), Ordering::Relaxed);
            // Sign smoke test (1 Hz): relative yaw must grow in the
            // direction the user turns, pitch ≈ gaze elevation,
            // roll ≈ 0 with the phone upright.
            let now = state::now_ms();
            if now.saturating_sub(*last_log_ms) >= 1000 {
                *last_log_ms = now;
                log::info!(
                    "pnex-pose yaw={yaw_rel:+.1} pitch={pitch:+.1} roll={roll:+.1} zero={}",
                    zero_yaw.is_some()
                );
            }
        }
    }
}

/// Missing sensor: dedicated error and thread exit.
fn sensor_missing() {
    state::ERROR_CODE.store(3, Ordering::Relaxed);
    state::POSE_TICK.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::Mat3;

    /// Rows of M = S·Rᵀ for R = identity: forward = −Z world, right =
    /// +X world (portrait back-camera wiring, see module docs).
    #[test]
    fn back_camera_matrix_convention() {
        let r = Mat3::IDENTITY;
        let m = back_camera_matrix(r);
        assert_eq!(m.0[6], 0.0);
        assert_eq!(m.0[7], 0.0);
        assert_eq!(m.0[8], -1.0);
        assert_eq!(m.0[0], 1.0);
    }
}
