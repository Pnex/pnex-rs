//! Pipeline de fin de session Take 360 — decode → stitch → upload.
//!
//! Tourne en `spawn_forever` (école `capture.rs::spawn_watcher`) : le
//! sous-arbre de l'overlay peut être démonté pendant le traitement, un
//! `spawn` normal serait annulé. Communication par atomics uniquement
//! (PHASE/PROGRESS/RESULT), libellés `t!` résolus côté page.

use super::guidance::Pose;
use super::state::{self, Phase};
use crate::api;
use crate::api::media::{MediaKind, UploadParams};
use pnex_stitcher::{decode_jpeg_rgb_capped, stitch, Frame, Params};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Timeout de décodage d'une frame — le décodeur `image`/zune-jpeg peut
/// partir en boucle infinie sur certains JPEG caméra (constat device
/// 2026-09-09 : stitch figé à 15 %, état 2 = décode). Timeout → frame
/// lâchée, le stitching continue avec les autres.
const DECODE_TIMEOUT: Duration = Duration::from_secs(3);

/// Une frame capturée : JPEG + pose au déclenchement.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub jpeg: Vec<u8>,
    pub pose: Pose,
}

/// Lance le pipeline détaché : decode 16 JPEG → stitch (out 2048, GPano
/// inclus) → upload `kind=panorama`.
///
/// `hfov_deg` : HFOV Camera2 (repli 65°) captée à l'ouverture de session.
/// Le résultat est publié via `RESULT_SEQ`/`RESULT_CODE` (1 = OK, 2 =
/// stitch KO, 3 = upload KO) + PHASE Done/Error — la page media sonde ces
/// atomics à 500 ms pour le toast et le reload.
pub fn start_finish_job(frames: Arc<Mutex<Vec<CapturedFrame>>>, hfov_deg: f32) {
    // Un seul finish par session (constat device : 3 threads stitch).
    if state::FINISH_STARTED
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    state::PHASE.store(Phase::Stitching as u8, Ordering::Relaxed);
    state::STITCH_STATE.store(1, Ordering::Relaxed);
    super::filelog::log("finish: start");
    // Le stitching (CPU, secondes) tourne dans un thread OS dédié ; le
    // résultat traverse un canal mpsc, sondé par la tâche async (école
    // capture.rs : polling 200 ms — pas d'executor bloqué).
    // Les frames sont prises UNE seule fois ici : copie profonde pour le
    // thread de stitch (~3 Mo de JPEG), l'original part au job serveur
    // (Take 360 V2). Un `mem::take` partagé par mutex ferait la course —
    // constat 2026-09-10 : frames_total=0 envoyé au serveur.
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
        if spawned.is_ok() { "ok" } else { "ÉCHEC" }
    ));

    dioxus::dioxus_core::spawn_forever(async move {
        // Attente non bloquante du résultat de stitch (avec le hfov ajusté
        // à l'orientation réelle des frames — cf. stitch_frames).
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

        // Upload (kind=panorama forcé — le sniff GPano classerait pareil).
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

                // Take 360 V2 : les frames+poses repartent au serveur pour
                // l'assemblage HD (pipeline Quality, 4096) en nouvelle
                // version de l'asset. Non bloquant : l'aperçu 1024 est déjà
                // en place, un échec de job est journalisé seulement.
                match submit_server_job(&asset, taken, hfov_frame).await {
                    Ok(job_id) => {
                        super::filelog::log(&format!("job serveur HD : {job_id}"));
                        spawn_job_poller(asset.id.clone(), job_id.clone());
                    }
                    Err(e) => {
                        super::filelog::log(&format!("job serveur HD : ÉCHEC création {e}"));
                    }
                }
                1
            }
            Err(e) => {
                super::filelog::log(&format!("upload: ERREUR {e}"));
                state::PHASE.store(Phase::Error as u8, Ordering::Relaxed);
                state::ERROR_CODE.store(5, Ordering::Relaxed);
                3
            }
        };
        state::RESULT_CODE.store(code, Ordering::Relaxed);
        state::RESULT_SEQ.fetch_add(1, Ordering::Relaxed);
    });
}

/// Soumet le job d'assemblage serveur : création (poses+hfov) puis
/// téléversement séquentiel des frames (octet-stream, retry 401 intégré).
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

/// Sonde l'état du job serveur (2 s, borné à 15 min) pour la filelog —
/// l'asset média se recharge dans l'UI médias (nouvelle version servie par
/// le backend à l'occasion d'un pull de la page).
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
                Err(e) => super::filelog::log(&format!("job serveur HD : poll {e}")),
            }
        }
    });
}

/// Décodage avec timeout : le décodeur tourne dans un thread jetable, le
/// canal impose la borne temporelle. `None` = échec ou blocage.
fn decode_with_timeout(jpeg: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let jpeg = jpeg.to_vec();
    std::thread::Builder::new()
        .name("pnex-decode".into())
        .spawn(move || {
            // Plafond 540 : l'aperçu 1024 n'a besoin que de ~14 px/°, et 45
            // frames pleine résolution satureraient la RAM du téléphone.
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

/// Decode + stitch (thread bloquant) → code d'erreur via Result.
/// Ajuste la HFOV (mesurée sur le grand côté capteur, paysage) à
/// l'orientation réelle des frames : une image portrait (720×1280, la
/// vidéo getUserMedia pivotée) montre le PETIT côté capteur sur son axe
/// large → même focale, tan(fov/2) proportionnel au côté. Sans cela le
/// f_px est faux de ~35 % (71° appliqué au lieu de ~52°) et le placement
/// des frames part en biais (constat 2026-09-10, filelog : 720×1280).
/// Publié : l'overlay l'utilise aussi pour poser le plan d'anneau (hfov
/// portrait du track caméra → `guidance::ring_plan`).
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
    super::filelog::log(&format!("decode: {} frames à décoder", taken.len()));
    let mut decoded: Vec<Frame> = Vec::with_capacity(taken.len());
    let total = taken.len().max(1);
    for (k, captured) in taken.iter().enumerate() {
        // Decoding dominates on device (seconds for 48 frames): advance the
        // bar 15 → 30 % frame by frame instead of sitting on 15 %.
        state::PROGRESS.store((15 + k * 15 / total) as u8, Ordering::Relaxed);
        // Index de la frame en décodage — visible dans la ligne debug :
        // un stitch figé montre exactement quelle frame pend.
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
        super::filelog::log("decode: 0 frame décodable → NoFrames");
        return (Err(pnex_stitcher::StitchError::NoFrames), hfov_deg);
    }
    // HFOV ajustée à l'orientation de la 1re frame décodée (toutes
    // identiques — même canvas).
    let (f0w, f0h) = (decoded[0].width, decoded[0].height);
    let hfov_frame = hfov_for_orientation(hfov_deg, f0w, f0h);
    super::filelog::log(&format!(
        "hfov: capteur {hfov_deg:.1}° → frame {f0w}x{f0h} = {hfov_frame:.1}°",
    ));

    // Toutes les frames ont le même (w, h) — le stitcher les recalcule
    // individuellement, rien à faire ici.
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
        // Seuil device 0,75 : les bords de bande (rangées hautes près du
        // pôle) sont partiellement couverts par nature — la métrique chute
        // avec l'anneau incliné (79 % constaté) sans trou réel ; un trou
        // azimuthal (frame manquante) coûte plusieurs % et reste détecté.
        min_band_coverage: 0.75,
        // Aperçu device : pipeline V1 inchangé (sélection + pôles habillés).
        // Le pipeline Quality (alignement, coutures DP, multi-bandes) tourne
        // côté serveur, cf. worker StitchPanoramaWorker.
        ..Params::default()
    };
    let out = stitch(&decoded, &params);
    match &out {
        Ok(_) => {
            state::STITCH_STATE.store(4, Ordering::Relaxed);
            state::PROGRESS.store(85, Ordering::Relaxed);
            super::filelog::log("render: OK");
        }
        Err(e) => super::filelog::log(&format!("render: ERREUR {e}")),
    }
    (out, hfov_frame)
}
