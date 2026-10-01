//! Capteur d'orientation Android — FFI brut `ndk-sys` (la crate `ndk` 0.9
//! n'expose pas de wrapper sensor, cf. exploration).
//!
//! Thread dédié : ALooper préparé sur ce thread, file d'événements du
//! SensorManager NDK (pas de classes Java). Rotation vector
//! (TYPE_ROTATION_VECTOR = 11, fallback geomagnetic = 20) à 30 Hz, converti
//! en (yaw, pitch, roll) convention `pnex-stitcher` :
//!
//! 1. `R = quat_to_mat(q)` : device→monde (X est, Y nord, Z ciel) ;
//! 2. caméra arrière : optique = −Z device, droite image = +X device, bas
//!    image = −Y device → `M = S·Rᵀ` avec `S = diag(1, −1, −1)` : rangées
//!    de M = (R col1, −R col2, −R col3) ;
//! 3. `euler_from_mat(M)` → (yaw, pitch, roll), yaw relatif au zéro de
//!    session (moyenne des premiers échantillons).
//!
//! Les signes restants (sens de rotation demandé, point guideur) sont
//! isolés dans `guidance.rs` (ROTATION_DIR) — calibrés au smoke test
//! device. Si l'anneau sort inversé ou retourné, retourner les constantes
//! ICI.

use super::state;
use crate::capture::{attach_env, package_name};
use pnex_stitcher::geom::{euler_from_mat, quat_to_mat, Mat3};
use std::sync::atomic::{AtomicBool, Ordering};

/// Types de capteurs Android (ASensorEvent.type_).
const TYPE_ROTATION_VECTOR: i32 = 11;
const TYPE_GEOMAGNETIC_ROTATION_VECTOR: i32 = 20;

/// Période demandée : 30 Hz.
const PERIOD_US: i32 = 33_333;
/// Nombre d'échantillons pour le zéro de session.
const ZERO_SAMPLES: usize = 5;

/// Vrai tant que le thread capteur est attendu en vie.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Démarre le thread capteur. `false` si déjà lancé.
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

/// Arrête le thread capteur (demande d'arrêt + libère le flag de vie).
pub fn stop() {
    state::CANCEL.store(true, Ordering::Relaxed);
    RUNNING.store(false, Ordering::Relaxed);
}

/// Rangées de `M = S·Rᵀ` : (R col1, −R col2, −R col3) — cf. docs module.
fn back_camera_matrix(r: Mat3) -> Mat3 {
    Mat3([
        r.0[0], r.0[3], r.0[6], // droite image = +X device
        -r.0[1], -r.0[4], -r.0[7], // bas image = écran −Y device
        -r.0[2], -r.0[5], -r.0[8], // axe optique = −Z device
    ])
}

/// Corps du thread : boucle ALooper + file d'événements capteur.
///
/// Journalise en logcat via le hook de panic global (PNEX-PANIC) ; en cas
/// d'absence du capteur, pose `ERROR_CODE = 3` et sort.
fn sensor_loop() {
    // 1) Nom de package (JNI, école capture.rs) pour le manager NDK.
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

    // 2) Manager + capteur (rotation vector, fallback geomagnetic).
    // SAFETY : FFI ndk-sys — signatures vérifiées dans ndk-sys 0.6 ; le
    // manager est un singleton thread-safe côté NDK.
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

    // 3) ALooper sur CE thread + file d'événements (pas de callback :
    //    ALLOW_NON_CALLBACKS + pollAll).
    let looper =
        unsafe { ndk_sys::ALooper_prepare(ndk_sys::ALOOPER_PREPARE_ALLOW_NON_CALLBACKS as i32) };
    if looper.is_null() {
        return sensor_missing();
    }
    // Ident LOOPER_ID_USER (3) — le pattern NDK standard sans callback :
    // pollAll remontera l'ident de la queue quand des événements arrivent.
    // (ALOOPER_POLL_CALLBACK = -2 est réservé aux queues AVEC callback —
    // avec lui, pollAll ne remonte jamais les événements : pose=false
    // constaté device 2026-09-09.)
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

    // 4) Boucle : drain des événements → conversion → atomics. Sortie sur
    //    CANCEL (démontage overlay / fin d'anneau).
    let mut zero_samples: Vec<f32> = Vec::new();
    let mut zero_yaw: Option<f32> = None;
    let mut zero_pitch: Option<f32> = None;
    let mut last_log_ms: u64 = 0;

    while !state::CANCEL.load(Ordering::Relaxed) {
        // 100 ms : borne de réveil pour voir CANCEL (timeout = −3).
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
            // LOOPER_ID_USER = 3 : événements capteur prêts.
            3 => {
                drain(
                    queue,
                    &mut zero_samples,
                    &mut zero_yaw,
                    &mut zero_pitch,
                    &mut last_log_ms,
                );
            }
            // ALOOPER_POLL_TIMEOUT = −3, erreur = −1 : recheck CANCEL.
            _ => {}
        }
    }

    // 5) Nettoyage.
    unsafe {
        ndk_sys::ASensorEventQueue_disableSensor(queue, sensor);
        ndk_sys::ASensorManager_destroyEventQueue(manager, queue);
    }
}

/// Vide la file : conversion quaternion → pose relative → atomics.
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
            // SAFETY : union bindgen — `data` pour les types rotation.
            let q = unsafe { ev.__bindgen_anon_1.__bindgen_anon_1.data };
            let (yaw_abs_deg, pitch, roll) =
                euler_from_mat(back_camera_matrix(quat_to_mat([q[0], q[1], q[2], q[3]])));
            // Zéro de session : moyenne des N premiers (yaw, pitch) — la
            // posture de départ devient l'origine (yaw ET pitch).
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
            // Smoke test des signes (1 Hz) : yaw relatif doit croître dans
            // le sens où l'utilisateur tourne, pitch ≈ élévation du regard,
            // roll ≈ 0 téléphone droit.
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

/// Capteur absent : erreur dédiée et fin du thread.
fn sensor_missing() {
    state::ERROR_CODE.store(3, Ordering::Relaxed);
    state::POSE_TICK.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::Mat3;

    /// Rangées de M = S·Rᵀ pour R = identité : avant = −Z monde, droite =
    /// +X monde (câblage caméra arrière portrait, cf. docs module).
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
