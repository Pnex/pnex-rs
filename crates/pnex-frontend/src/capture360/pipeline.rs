//! Take 360 end-of-session pipeline — decode → stitch → upload.
//!
//! Runs in `spawn_forever` (`capture.rs::spawn_watcher` approach): the
//! overlay subtree may be unmounted during processing, a normal
//! `spawn` would be cancelled. Communication through atomics only
//! (PHASE/PROGRESS/RESULT), `t!` labels resolved on the page side.

use super::guidance::Pose;
use super::state::{self, Phase};
use crate::api;
use crate::api::media::{MediaKind, UploadParams};
use pnex_stitcher::{decode_jpeg_rgb_capped, stitch, Frame, Params};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Decode timeout of one frame — the `image`/zune-jpeg decoder can
/// loop forever on some camera JPEGs (observed on device
/// 2026-09-09: stitch frozen at 15 %, state 2 = decode). Timeout → frame
/// dropped, stitching goes on with the others.
const DECODE_TIMEOUT: Duration = Duration::from_secs(3);

/// A captured frame: JPEG + pose at trigger time.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub jpeg: Vec<u8>,
    pub pose: Pose,
}

/// Starts the detached pipeline: decode 16 JPEGs → stitch (out 2048, GPano
/// included) → `kind=panorama` upload.
///
/// `hfov_deg`: Camera2 HFOV (fallback 65°) read when the session opened.
/// The result is published through `RESULT_SEQ`/`RESULT_CODE` (1 = OK, 2 =
/// stitch KO, 3 = upload KO) + PHASE Done/Error — the media page polls these
/// atomics every 500 ms for the toast and the reload.
pub fn start_finish_job(frames: Arc<Mutex<Vec<CapturedFrame>>>, hfov_deg: f32) {
    // A single finish per session (observed on device: 3 stitch threads).
    if state::FINISH_STARTED
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    state::PHASE.store(Phase::Stitching as u8, Ordering::Relaxed);
    state::STITCH_STATE.store(1, Ordering::Relaxed);
    super::filelog::log("finish: start");
    // Stitching (CPU, seconds) runs in a dedicated OS thread; the
    // result goes through an mpsc channel, polled by the async task (capture.rs
    // approach: 200 ms polling — no blocked executor).
    // The frames are taken ONLY once here: deep copy for the
    // stitch thread (~3 MB of JPEG), the original goes to the server job
    // (Take 360 V2). A `mem::take` shared through a mutex would race —
    // observed 2026-09-10: frames_total=0 sent to the server.
    let taken: Vec<CapturedFrame> = {
        let mut guard = match frames.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        std::mem::take(&mut *guard)
    };
    let (tx, rx) = std::sync::mpsc::channel::<(
        Result<pnex_stitcher::Stitched, pnex_stitcher::StitchError>,
        f32,
    )>();
    let hfov = hfov_deg;
    let frames_for_stitch = taken.clone();
    let spawned = std::thread::Builder::new()
        .name("pnex-stitch".into())
        .spawn(move || {
            super::filelog::log("stitch thread: entered");
            let (out, hfov_frame) = stitch_frames(frames_for_stitch, hfov);
            super::filelog::log(&format!(
                "stitch thread: done → {} (hfov frame {:.1}°)",
                match &out {
                    Ok(s) => format!(
                        "OK {}x{} {} ms, {} o jpeg",
                        s.width,
                        s.height,
                        s.stitch_ms,
                        s.jpeg.len()
                    ),
                    Err(e) => format!("ERR {e}"),
                },
                hfov_frame
            ));
            let _ = tx.send((out, hfov_frame));
        });
    super::filelog::log(&format!(
        "stitch thread spawn: {}",
        if spawned.is_ok() { "ok" } else { "FAILED" }
    ));

    dioxus::dioxus_core::spawn_forever(async move {
        // Non-blocking wait for the stitch result (with the hfov adjusted
        // to the real orientation of the frames — see stitch_frames).
        let (stitched, s_hfov) = loop {
            match rx.try_recv() {
                Ok((result, hfov_frame)) => break (result, hfov_frame),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    crate::util::sleep(std::time::Duration::from_millis(200)).await;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
                    state::ERROR_CODE.store(4, Ordering::Relaxed);
                    state::RESULT_CODE.store(2, Ordering::Relaxed);
                    state::RESULT_SEQ.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        };
        let (stitched, hfov_frame) = match stitched {
            Ok(s) => (s, s_hfov),
            Err(e) => {
                log::warn!("take360 : stitch : {e}");
                state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
                state::ERROR_CODE.store(4, Ordering::Relaxed);
                state::RESULT_CODE.store(2, Ordering::Relaxed);
                state::RESULT_SEQ.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };

        // Upload (kind=panorama forced — the GPano sniff would classify the same).
        state::PHASE.store(Phase::Uploading as u8, Ordering::Relaxed);
        state::PROGRESS.store(90, Ordering::Relaxed);
        let name = format!(
            "Panorama 360 {}",
            chrono::Utc::now().format("%d/%m/%Y %H:%M")
        );
        let params = UploadParams {
            name: Some(name),
            filename: Some(format!(
                "pano-{}.jpg",
                chrono::Utc::now().timestamp_millis()
            )),
            kind: Some(MediaKind::Panorama),
            content_type: Some("image/jpeg".to_string()),
            note: Some("guided 360 capture".to_string()),
        };
        let code = match api::media::upload(&params, stitched.jpeg).await {
            Ok(asset) => {
                super::filelog::log(&format!("upload: OK id={}", asset.id));
                state::PHASE.store(Phase::Done as u8, Ordering::Relaxed);
                state::PROGRESS.store(100, Ordering::Relaxed);

                // Take 360 V2: the frames+poses go back to the server for
                // HD assembly (Quality pipeline, 4096) as a new
                // version of the asset. Non-blocking: the 1024 preview is already
                // in place, a job failure is only logged.
                match submit_server_job(&asset, taken, hfov_frame).await {
                    Ok(job_id) => {
                        super::filelog::log(&format!("server HD job: {job_id}"));
                        spawn_job_poller(asset.id.clone(), job_id.clone());
                    }
                    Err(e) => {
                        super::filelog::log(&format!("HD server job: creation FAILED {e}"));
                    }
                }
                1
            }
            Err(e) => {
                super::filelog::log(&format!("upload: ERROR {e}"));
                state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
                state::ERROR_CODE.store(5, Ordering::Relaxed);
                3
            }
        };
        state::RESULT_CODE.store(code, Ordering::Relaxed);
        state::RESULT_SEQ.fetch_add(1, Ordering::Relaxed);
    });
}

/// Submits the server assembly job: creation (poses+hfov) then
/// sequential upload of the frames (octet-stream, built-in 401 retry).
async fn submit_server_job(
    asset: &crate::api::media::MediaAsset,
    frames: Vec<CapturedFrame>,
    hfov_deg: f32,
) -> Result<String, String> {
    let req = api::stitch_jobs::CreateJob {
        asset_id: asset.id.clone(),
        frames_total: frames.len(),
        hfov_deg,
        poses: frames
            .iter()
            .map(|f| api::stitch_jobs::JobPose {
                yaw_deg: f.pose.yaw,
                pitch_deg: f.pose.pitch,
                roll_deg: f.pose.roll,
            })
            .collect(),
    };
    let job = api::stitch_jobs::create(&req)
        .await
        .map_err(|e| format!("create : {e}"))?;
    for (k, f) in frames.into_iter().enumerate() {
        api::stitch_jobs::upload_frame(&job.id, k, f.jpeg)
            .await
            .map_err(|e| format!("frame {k} : {e}"))?;
    }
    Ok(job.id)
}

/// Polls the server job state (2 s, bounded to 15 min) for the filelog —
/// the media asset reloads in the media UI (new version served by
/// the backend on a page pull).
fn spawn_job_poller(asset_id: String, job_id: String) {
    dioxus::dioxus_core::spawn_forever(async move {
        for _ in 0..450 {
            crate::util::sleep(std::time::Duration::from_secs(2)).await;
            match api::stitch_jobs::get(&job_id).await {
                Ok(job) => match job.state.as_str() {
                    "succeeded" => {
                        super::filelog::log(&format!(
                            "HD server job {job_id}: OK — HD version attached to asset {asset_id}"
                        ));
                        return;
                    }
                    "failed" => {
                        super::filelog::log(&format!(
                            "HD server job {job_id}: FAILED — {}",
                            job.error.unwrap_or_default()
                        ));
                        return;
                    }
                    _ => {}
                },
                Err(e) => super::filelog::log(&format!("server HD job: poll {e}")),
            }
        }
    });
}

/// Decode with timeout: the decoder runs in a throwaway thread, the
/// channel enforces the time bound. `None` = failure or hang.
fn decode_with_timeout(jpeg: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let jpeg = jpeg.to_vec();
    std::thread::Builder::new()
        .name("pnex-decode".into())
        .spawn(move || {
            // Cap 540: the 1024 preview only needs ~14 px/°, and 45
            // full-resolution frames would saturate the phone's RAM.
            let (rgb, w, h) = decode_jpeg_rgb_capped(&jpeg, 540);
            let _ = tx.send(if rgb.is_empty() {
                None
            } else {
                Some((rgb, w, h))
            });
        })
        .ok()?;
    match rx.recv_timeout(DECODE_TIMEOUT) {
        Ok(result) => result,
        Err(_) => None,
    }
}

/// Decode + stitch (blocking thread) → error code via Result.
/// Adjusts the HFOV (measured on the long sensor side, landscape) to
/// the real orientation of the frames: a portrait image (720×1280, the
/// rotated getUserMedia video) shows the SHORT sensor side on its wide
/// axis → same focal length, tan(fov/2) proportional to the side. Without it
/// f_px is off by ~35 % (71° applied instead of ~52°) and frame
/// placement drifts (observed 2026-09-10, filelog: 720×1280).
/// Public: the overlay also uses it to set the ring plan (portrait hfov
/// of the camera track → `guidance::ring_plan`).
pub fn hfov_for_orientation(hfov_landscape_deg: f32, w: u32, h: u32) -> f32 {
    if w < h {
        2.0 * ((hfov_landscape_deg.to_radians() / 2.0).tan() * (w as f32 / h as f32))
            .atan()
            .to_degrees()
    } else {
        hfov_landscape_deg
    }
}

fn stitch_frames(
    taken: Vec<CapturedFrame>,
    hfov_deg: f32,
) -> (
    Result<pnex_stitcher::Stitched, pnex_stitcher::StitchError>,
    f32,
) {
    state::STITCH_STATE.store(2, Ordering::Relaxed);
    state::PROGRESS.store(15, Ordering::Relaxed);
    super::filelog::log(&format!("decode: {} frames to decode", taken.len()));
    let mut decoded: Vec<Frame> = Vec::with_capacity(taken.len());
    let total = taken.len().max(1);
    for (k, captured) in taken.iter().enumerate() {
        // Decoding dominates on device (seconds for 48 frames): advance the
        // bar 15 → 30 % frame by frame instead of sitting on 15 %.
        state::PROGRESS.store((15 + k * 15 / total) as u8, Ordering::Relaxed);
        // Index of the frame being decoded — visible in the debug line:
        // a frozen stitch shows exactly which frame hangs.
        state::STITCH_IDX.store(k as u8, Ordering::Relaxed);
        let t0 = std::time::Instant::now();
        match decode_with_timeout(&captured.jpeg) {
            Some((rgb, w, h)) => {
                super::filelog::log(&format!(
                    "decode[{k}]: ok {w}x{h} (capped 540) in {} ms",
                    t0.elapsed().as_millis()
                ));
                decoded.push(Frame {
                    rgb,
                    width: w,
                    height: h,
                    yaw_deg: captured.pose.yaw,
                    pitch_deg: captured.pose.pitch,
                    roll_deg: captured.pose.roll,
                });
            }
            None => {
                super::filelog::log(&format!(
                    "decode[{k}]: FAILED/timeout ({} ms, {} B jpeg)",
                    t0.elapsed().as_millis(),
                    captured.jpeg.len()
                ));
            }
        }
    }
    if decoded.is_empty() {
        state::STITCH_STATE.store(5, Ordering::Relaxed);
        super::filelog::log("decode: 0 decodable frame → NoFrames");
        return (Err(pnex_stitcher::StitchError::NoFrames), hfov_deg);
    }
    // HFOV adjusted to the orientation of the 1st decoded frame (all
    // identical — same canvas).
    let (f0w, f0h) = (decoded[0].width, decoded[0].height);
    let hfov_frame = hfov_for_orientation(hfov_deg, f0w, f0h);
    super::filelog::log(&format!(
        "hfov: capteur {hfov_deg:.1}° → frame {f0w}x{f0h} = {hfov_frame:.1}°",
    ));

    // All frames have the same (w, h) — the stitcher recomputes them
    // individually, nothing to do here.
    state::STITCH_STATE.store(3, Ordering::Relaxed);
    state::PROGRESS.store(30, Ordering::Relaxed);
    super::filelog::log(&format!(
        "render: {} frames, hfov={hfov_deg:.1}, out={} px",
        decoded.len(),
        pnex_stitcher::DEVICE_OUT_WIDTH
    ));
    let params = Params {
        hfov_deg: hfov_frame,
        out_width: pnex_stitcher::DEVICE_OUT_WIDTH,
        // Device threshold 0.75: band edges (top rows near the
        // pole) are partially covered by nature — the metric drops
        // with the tilted ring (79 % observed) without a real hole; an
        // azimuthal hole (missing frame) costs several % and is still detected.
        min_band_coverage: 0.75,
        // Device preview: V1 pipeline unchanged (selection + dressed poles).
        // The Quality pipeline (alignment, DP seams, multi-band) runs
        // server side, see worker StitchPanoramaWorker.
        ..Params::default()
    };
    let out = stitch(&decoded, &params);
    match &out {
        Ok(_) => {
            state::STITCH_STATE.store(4, Ordering::Relaxed);
            state::PROGRESS.store(85, Ordering::Relaxed);
            super::filelog::log("render: OK");
        }
        Err(e) => super::filelog::log(&format!("render: ERROR {e}")),
    }
    (out, hfov_frame)
}
