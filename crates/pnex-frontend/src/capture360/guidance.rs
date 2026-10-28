//! 360 capture guidance — pure math, no dependency, compiled on
//! every target (wasm included): testable on the host.
//!
//! V3 (2026-09-10): back to STRICT STEP-BY-STEP — continuous triggering
//! (V1, device pivot 2026-09-09) is over: it took frames
//! while moving backwards (|Δyaw| counted the user's return) and
//! under-captured the ring (step ~20° × 15 + portrait hfov 43.7° ≈ 348° <
//! 360° — azimuth gap observed on the 1st HD capture 2026-09-10).
//! New model, the one of commercial 360 apps:
//!
//! 1. the user anchors the starting point ("Anchor here");
//! 2. each ring has N fixed targets (yaw = k·step, pitch = ring
//!    setpoint) — N and the step are computed from the camera's real PORTRAIT
//!    hfov (`ring_plan`): neighbour overlap is guaranteed
//!    ≥ [`MIN_OVERLAP_DEG`] and the ring closes by construction;
//! 3. a frame fires ONLY on the current target, when the three
//!    conditions hold together for [`DWELL_MS`]:
//!    aligned (yaw/pitch/roll within tolerances) + steady (angular
//!    rate < [`MAX_RATE_DEG_S`]). Turning backwards, getting the tilt
//!    wrong or moving too fast: NOTHING fires;
//! 4. capture validated → next target. "Retake" triggers the
//!    previous target (the frame is removed from memory).
//!
//! Angle convention: the SAME as `pnex-stitcher::geom` (yaw = gaze
//! azimuth, pitch = elevation, roll around the optical axis). The Android
//! sensor → angles conversion is in `sensors.rs` via
//! `pnex_stitcher::geom` — a single source of truth, consistency is
//! locked by the stitcher crate tests.

// The runtime consumers are cfg android (overlay) — without allow, these
// items are "dead" on desktop (capture.rs style: documented allow,
// no ghost code in non-android native targets).
#![allow(dead_code)]

/// Number of rings in the protocol (level, up, down).
pub const N_RINGS: usize = 3;
/// 3-ring protocol: level (the anchor), tilted UP, tilted
/// DOWN — the union of the lat bands covers ~[-72°, +72°] of latitude
/// in real content (the bottom third of the equirect stays synthetic: see
/// `pnex-stitcher::render` poles).
pub const RING_TILT_DEG: f32 = 40.0;
/// Half-width of the pitch band of a tilted ring: target ±40°,
/// holdable band [28°, 52°] — at portrait vfov ~65°, every frame of
/// the up ring covers at least lat [-4°, +60°] → overlap ≥ 27°
/// with the level ring (covers up to ±32.5°).
pub const RING_PITCH_TOL_DEG: f32 = 12.0;
/// Target azimuth overlap between neighbour frames of a ring: the target
/// step = hfov − 20° (at portrait hfov 43.7° → N = 16, real step
/// 22.5°). The server Quality pipeline realigns the poses by features —
/// 20° of overlap gives ~10° of aiming error margin.
pub const MIN_OVERLAP_DEG: f32 = 20.0;
/// Azimuth alignment tolerance (degrees). Widened on device (observed
/// 2026-09-09: 2°/1°/4° impossible to hit hand-held — never a dwell);
/// the Quality server realigns by features (corrections up to ~10°
/// observed), aiming precision is no longer the limiting factor.
pub const TOL_YAW_DEG: f32 = 6.0;
/// Elevation tolerance — pitch is RELATIVE to the posture at session
/// start (pitch zero, see sensors.rs): you hold the phone
/// naturally, the ring follows your aiming height.
pub const TOL_PITCH_DEG: f32 = 8.0;
/// Roll tolerance (degrees) — wide: the server straightens by
/// features, only user comfort matters here.
pub const TOL_ROLL_DEG: f32 = 10.0;
/// Alignment hold time before triggering (ms) — held = aligned AND
/// steady continuously (any loss cancels and resets the counter).
pub const DWELL_MS: u64 = 700;
/// Max angular rate (°/s, absolute max over the 3 axes) to be
/// "steady" — hand tremor < 1°/s, micro-adjustment 5-15°/s:
/// beyond 3.5°/s you are in full motion. The frame fires with the pose
/// still: the recorded pose = the aimed pose.
pub const MAX_RATE_DEG_S: f32 = 3.5;
/// Guide dot displacement (px per degree of error).
pub const DOT_PX_PER_DEG: f32 = 2.0;
/// Requested rotation direction: the yaw pose is the ENU azimuth (grows when
/// turning LEFT — euler_from_mat, see `sensors.rs`) — for the
/// chevron/dot to point PHYSICALLY at the target, it must therefore be −1:
/// err_yaw > 0 (target at higher yaw) = target on the left → arrow "←".
/// Constant isolated for the device smoke-test flip.
pub const ROTATION_DIR: f32 = -1.0;

/// Phone pose (degrees), stitcher convention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

/// Plan of a ring: number of targets and angular step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingPlan {
    pub n_steps: usize,
    pub step_deg: f32,
}

/// Ring plan for a given portrait hfov: step = hfov −
/// [`MIN_OVERLAP_DEG`], N = ceil(360/step) clamped to [10, 20]. Ring
/// closure is guaranteed BY CONSTRUCTION: (N−1)·step + hfov = 360 +
/// overlap ≥ 360 as soon as hfov > step, i.e. hfov > 360/N — true for
/// every N ≤ 20 and hfov ≥ 30° (clamp bounds).
#[must_use]
pub fn ring_plan(hfov_deg: f32) -> RingPlan {
    let hfov = hfov_deg.clamp(30.0, 75.0);
    let step = (hfov - MIN_OVERLAP_DEG).max(12.0);
    let n = (360.0 / step).ceil() as usize;
    let n = n.clamp(10, 20);
    RingPlan {
        n_steps: n,
        step_deg: 360.0 / n as f32,
    }
}

/// Setpoint pitch of a ring: 0 (level), +40 (up), −40 (down).
#[must_use]
pub fn ring_pitch(ring: usize) -> f32 {
    match ring {
        1 => RING_TILT_DEG,
        2 => -RING_TILT_DEG,
        _ => 0.0,
    }
}

/// Current target (yaw, pitch): yaw relative to the anchor (k·step), pitch =
/// ring setpoint.
#[must_use]
pub fn target_for(ring: usize, step: usize, step_deg: f32) -> (f32, f32) {
    (step as f32 * step_deg, ring_pitch(ring))
}

/// Wrap into [−180, 180).
#[must_use]
pub fn wrap180(deg: f32) -> f32 {
    let mut d = deg % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    d
}

/// Signed errors (target − pose) in degrees, wrapped at 180:
/// (yaw, pitch, roll).
#[must_use]
pub fn signed_error(target_yaw: f32, target_pitch: f32, pose: Pose) -> (f32, f32, f32) {
    (
        wrap180(target_yaw - pose.yaw),
        target_pitch - pose.pitch,
        -pose.roll,
    )
}

/// Aligned on the current target (all three tolerances).
#[must_use]
pub fn aligned(err: (f32, f32, f32)) -> bool {
    err.0.abs() <= TOL_YAW_DEG && err.1.abs() <= TOL_PITCH_DEG && err.2.abs() <= TOL_ROLL_DEG
}

/// Guide dot displacement (px): it LEADS — the user moves TOWARDS
/// it. Direction: target at higher yaw = physically on the left (ENU azimuth)
/// → negative dx via [`ROTATION_DIR`]; target above → positive dy.
#[must_use]
pub fn dot_offset_px(err: (f32, f32, f32), max_px: f32) -> (f32, f32) {
    let dx = (err.0 * DOT_PX_PER_DEG * ROTATION_DIR).clamp(-max_px, max_px);
    let dy = (err.1 * DOT_PX_PER_DEG).clamp(-max_px, max_px);
    (dx, dy)
}

/// Advance after capture: next step, next ring, or done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    NextStep,
    NextRing,
    Finish,
}

/// Transition (ring, step) after a validated capture. Step-by-step
/// guarantees the order: triggering can neither skip nor go back
/// (the only way back = "Retake", driven by the
/// UI).
#[must_use]
pub fn advance(ring: usize, step: usize, n_steps: usize) -> Advance {
    if step + 1 < n_steps {
        Advance::NextStep
    } else if ring + 1 < N_RINGS {
        Advance::NextRing
    } else {
        Advance::Finish
    }
}

/// Max angular rate (°/s) over the sample window
/// `(t_ms, (yaw, pitch, roll))` — absolute max over the 3 axes between
/// consecutive pairs, yaw wrapped. Empty or single-sample window → 0
/// (steady): a sensor that is starting is not penalised.
#[must_use]
pub fn max_rate_deg_s(samples: &[(u64, Pose)]) -> f32 {
    let mut max = 0.0f32;
    for pair in samples.windows(2) {
        let (t0, a) = pair[0];
        let (t1, b) = pair[1];
        let dt_ms = t1.saturating_sub(t0);
        if dt_ms == 0 {
            continue;
        }
        let dy = wrap180(b.yaw - a.yaw).abs() * 1000.0 / dt_ms as f32;
        let dp = (b.pitch - a.pitch).abs() * 1000.0 / dt_ms as f32;
        let dr = (b.roll - a.roll).abs() * 1000.0 / dt_ms as f32;
        max = max.max(dy).max(dp).max(dr);
    }
    max
}

/// Decision of the guidance loop, pure and testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    /// Not (or no longer) aligned/steady: aim, show the direction.
    Wait,
    /// Just became aligned+steady: start the dwell now.
    StartDwell,
    /// Dwell in progress, conditions held — elapsed time (ms).
    Dwell(u64),
    /// Dwell expired with conditions held: FIRE.
    Fire,
    /// Conditions lost during the dwell: cancel and search again.
    CancelDwell,
}

/// Decides the next step: pure, no clock nor effect.
///
/// The dwell requires aligned AND steady CONTINUOUSLY: any loss (pose missing,
/// tolerance exceeded, motion) cancels. `dwell_started_ms` = start instant of
/// the current dwell (None = no dwell), `now_ms` = current
/// clock (ms, same origin).
#[must_use]
pub fn decide(
    target_yaw: f32,
    target_pitch: f32,
    pose: Option<Pose>,
    stable: bool,
    dwell_started_ms: Option<u64>,
    now_ms: u64,
) -> Decision {
    let aligned = pose
        .map(|p| aligned(signed_error(target_yaw, target_pitch, p)))
        .unwrap_or(false);
    if !aligned || !stable {
        return if dwell_started_ms.is_some() {
            Decision::CancelDwell
        } else {
            Decision::Wait
        };
    }
    match dwell_started_ms {
        None => Decision::StartDwell,
        Some(started) => {
            let elapsed = now_ms.saturating_sub(started);
            if elapsed >= DWELL_MS {
                Decision::Fire
            } else {
                Decision::Dwell(elapsed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(yaw: f32, pitch: f32, roll: f32) -> Option<Pose> {
        Some(Pose { yaw, pitch, roll })
    }

    /// Ring closure: for any plausible hfov, (N−1)·step + hfov
    /// covers 360° and neighbour overlap stays comfortable. This is the
    /// fix for the ~33° azimuth gap observed on the 1st HD (continuous V1:
    /// step ~20° × 15 frames).
    #[test]
    fn ring_plan_ferme_l_anneau_partout() {
        let mut hfov = 30.0f32;
        while hfov <= 75.0 {
            let plan = ring_plan(hfov);
            assert!((10..=20).contains(&plan.n_steps), "n={}", plan.n_steps);
            let coverage = (plan.n_steps as f32 - 1.0) * plan.step_deg + hfov;
            assert!(
                coverage >= 360.0,
                "hfov={hfov} n={} step={:.1} coverage={coverage:.1}",
                plan.n_steps,
                plan.step_deg
            );
            // Neighbour overlap ≥ 10° (the pairs of the server realignment).
            assert!(
                hfov - plan.step_deg >= 10.0,
                "hfov={hfov} overlap={:.1}",
                hfov - plan.step_deg
            );
            hfov += 0.5;
        }
    }

    /// Nominal case: portrait hfov 43.7° → 16 targets of 22.5° (like the
    /// original protocol, but closed).
    #[test]
    fn ring_plan_hfov_portrait() {
        let plan = ring_plan(43.7);
        assert_eq!(plan.n_steps, 16);
        assert!((plan.step_deg - 22.5).abs() < 0.01);
    }

    #[test]
    fn cibles_et_bandes_de_pitch() {
        // Ring 0 level: target pitch 0.
        assert_eq!(ring_pitch(0), 0.0);
        assert_eq!(ring_pitch(1), RING_TILT_DEG);
        assert_eq!(ring_pitch(2), -RING_TILT_DEG);
        let (ty, tp) = target_for(0, 3, 22.5);
        assert!((ty - 67.5).abs() < 1e-5 && tp.abs() < 1e-5);
        let (ty, tp) = target_for(1, 0, 22.5);
        assert!(ty.abs() < 1e-5 && (tp - RING_TILT_DEG).abs() < 1e-5);
    }

    #[test]
    fn erreur_et_alignement() {
        // On target: zero errors → aligned.
        let err = signed_error(22.5, RING_TILT_DEG, pose(22.5, RING_TILT_DEG, 0.0).unwrap());
        assert!(err.0.abs() < 1e-5 && err.1.abs() < 1e-5 && err.2.abs() < 1e-5);
        assert!(aligned(err));
        // Wrap at 180: target 170, pose −170 → error −20.
        let err = signed_error(170.0, 0.0, pose(-170.0, 0.0, 0.0).unwrap());
        assert!((err.0 + 20.0).abs() < 1e-4);
        // Out of tolerance (margin beyond the constants).
        assert!(!aligned((0.0, TOL_PITCH_DEG + 1.0, 0.0)));
        assert!(!aligned((0.0, 0.0, TOL_ROLL_DEG + 1.0)));
        assert!(!aligned((TOL_YAW_DEG + 0.5, 0.0, 0.0)));
    }

    /// Angular rate: still → 0, clear rotation → well beyond
    /// the steadiness threshold.
    #[test]
    fn vitesse_angulaire() {
        let statique = vec![
            (
                0,
                Pose {
                    yaw: 10.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
            (
                100,
                Pose {
                    yaw: 10.01,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
            (
                200,
                Pose {
                    yaw: 10.02,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
        ];
        assert!(max_rate_deg_s(&statique) < 0.5);
        let rotation = vec![
            (
                0,
                Pose {
                    yaw: 0.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
            (
                100,
                Pose {
                    yaw: 3.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
            (
                200,
                Pose {
                    yaw: 6.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
        ];
        assert!((max_rate_deg_s(&rotation) - 30.0).abs() < 0.1);
        // Window too short (0 or 1 sample) → 0 (steady).
        assert_eq!(max_rate_deg_s(&[]), 0.0);
        assert_eq!(
            max_rate_deg_s(&[(
                0,
                Pose {
                    yaw: 0.0,
                    pitch: 0.0,
                    roll: 0.0
                }
            )]),
            0.0
        );
        // Yaw wrap: 179 → −179 = +2° by the short path (20°/s),
        // NOT −358° (the wrap avoids the false rate spike).
        let saut = vec![
            (
                0,
                Pose {
                    yaw: 179.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
            (
                100,
                Pose {
                    yaw: -179.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            ),
        ];
        let rate = max_rate_deg_s(&saut);
        assert!((rate - 20.0).abs() < 0.1, "rate={rate}");
    }

    /// Transition table of `decide` with the steadiness gate.
    #[test]
    fn transitions_decide() {
        let (cible_y, cible_p) = (22.5, RING_TILT_DEG);
        let sur_cible = pose(cible_y, cible_p, 0.0);
        // Not aligned → Wait.
        assert_eq!(
            decide(cible_y, cible_p, pose(0.0, cible_p, 0.0), true, None, 0),
            Decision::Wait
        );
        // Unsteady on target → Wait (no dwell start).
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, false, None, 0),
            Decision::Wait
        );
        // Pose missing → Wait.
        assert_eq!(
            decide(cible_y, cible_p, None, true, None, 0),
            Decision::Wait
        );
        // Aligned + steady → StartDwell.
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, true, None, 1000),
            Decision::StartDwell
        );
        // Held → Dwell(elapsed), then Fire at the end.
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, true, Some(1000), 1500),
            Decision::Dwell(500)
        );
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, true, Some(1000), 1700),
            Decision::Fire
        );
        // Yaw drift during the dwell → CancelDwell.
        assert_eq!(
            decide(
                cible_y,
                cible_p,
                pose(cible_y + TOL_YAW_DEG + 2.0, cible_p, 0.0),
                true,
                Some(1000),
                1500
            ),
            Decision::CancelDwell
        );
        // Instability during the dwell → CancelDwell.
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, false, Some(1000), 1500),
            Decision::CancelDwell
        );
        // Pose lost during the dwell → CancelDwell.
        assert_eq!(
            decide(cible_y, cible_p, None, true, Some(0), 500),
            Decision::CancelDwell
        );
        // Roll out of tolerance → no dwell.
        assert_eq!(
            decide(
                cible_y,
                cible_p,
                pose(cible_y, cible_p, TOL_ROLL_DEG + 1.0),
                true,
                None,
                0
            ),
            Decision::Wait
        );
    }

    /// Advance transitions: step → ring → end, never skipping.
    #[test]
    fn avance_sans_sauter() {
        assert_eq!(advance(0, 3, 16), Advance::NextStep);
        // Last step of the ring (15/16) → next ring.
        assert_eq!(advance(0, 15, 16), Advance::NextRing);
        assert_eq!(advance(1, 15, 16), Advance::NextRing);
        assert_eq!(advance(2, 15, 16), Advance::Finish);
        // Last step of the last ring with a different n.
        assert_eq!(advance(2, 9, 10), Advance::Finish);
    }

    #[test]
    fn point_guideur_clamp() {
        // Direction: err_yaw > 0 (target at higher yaw = physically on the
        // LEFT, ENU azimuth) → NEGATIVE dx (dot on the left, see ROTATION_DIR).
        let (dx, dy) = dot_offset_px((50.0, -50.0, 0.0), 40.0);
        assert!((dx + 40.0).abs() < 1e-5);
        assert!((dy + 40.0).abs() < 1e-5);
        let (dx, _) = dot_offset_px((1.5, 0.0, 0.0), 40.0);
        assert!((dx + 3.0).abs() < 1e-5);
    }
}
