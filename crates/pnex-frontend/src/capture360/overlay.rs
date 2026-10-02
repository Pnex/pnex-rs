//! Overlay plein écran de capture guidée (module cfg Android).
//!
//! PAS-À-PAS STRICT (V3, 2026-09-10) — le déclenchement continu (V1) a
//! vécu : il tirait en marche arrière (le |Δyaw| comptait le retour
//! utilisateur — « photo déjà prise » pendant une correction) et
//! sous-capture l'anneau (trou azimutal ~33° sur la 1re HD). Nouveau
//! modèle, celui des apps 360 du commerce :
//!
//! 1. l'utilisateur vise son point de départ et appuie « Ancrer ici » ;
//! 2. chaque anneau a N cibles fixes (yaw = k·pas, pitch = consigne) — N
//!    et le pas viennent de `guidance::ring_plan(hfov portrait réel)` :
//!    recouvrement ≥ 20° et fermeture d'anneau par construction ;
//! 3. une frame ne part QUE sur la cible courante, quand alignement
//!    (yaw/pitch/roll) + stabilité (vitesse angulaire) + dwell 700 ms
//!    sont tenus EN CONTINU — tourner en arrière ne déclenche RIEN ;
//! 4. capture → cible suivante ; « Reprendre » retire la dernière frame
//!    et recule la cible (piloté par `UNDO_REQUEST`, single-writer).
//!
//! Rendu : preview + SVG guidage. Re-render dioxus UNIQUEMENT sur
//! changement discret (bannières, compteur) — les éléments continus (point
//! guideur, arc de dwell, flèche, degrés, stabilité, flash) sont pilotés
//! en DOM direct à 15 Hz (école `push_debug_dom` : le re-render 30 Hz
//! sature le GPU webview, MALI BAD ALLOC — constat device 2026-09-09).

use super::pipeline::CapturedFrame;
use super::state::{self, Phase};
use super::{fov, frames, guidance, sensors, wakelock};
use dioxus::prelude::*;
use dioxus_i18n::t;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Id DOM du `<video>` (aussi utilisé par `frames.rs`).
const VIDEO_ID: &str = "p360-video";

/// Rayon du réticule (unités viewBox 400×400).
const RETICLE_R: f32 = 38.0;
/// Distance max du point guideur du centre (unités viewBox).
const DOT_MAX_PX: f32 = 110.0;
/// Fenêtre de mesure de stabilité (ms) — vitesse angulaire max dedans.
const STABLE_WINDOW_MS: u64 = 450;
/// Ids DOM des éléments continus (SVG/HTML) pilotés en eval 15 Hz.
const ID_DOT: &str = "p360-dot";
const ID_ARROW: &str = "p360-arrow";
const ID_ARC: &str = "p360-arc";
const ID_DEG: &str = "p360-deg";
const ID_PITCH: &str = "p360-pitch";
const ID_STAB: &str = "p360-stab";
const ID_FLASH: &str = "p360-flash";

/// Snapshot de rendu copié depuis les atomics à chaque tick — DISCRET
/// uniquement (les valeurs qui bougent à 30 Hz passent par la couche DOM
/// éval, cf. docs module).
#[derive(Debug, Clone, Copy, PartialEq)]
struct UiState {
    /// Ancre posée ? (sinon : pastille « visez + Ancrer »).
    anchored: bool,
    phase: Phase,
    /// Anneau courant et pas courant (frames capturées dans l'anneau).
    ring: usize,
    step: usize,
    /// Plan courant (cibles par anneau) — cf. `guidance::ring_plan`.
    n_steps: usize,
    /// Consigne yaw discrète : −1 gauche, 0 aucune, +1 droite (affichée
    /// seulement si la bande de pitch est tenue et |erreur| > tolérance).
    turn_dir: i8,
    /// Bande de pitch de l'anneau incliné tenue (rings 1/2 ; vrai sinon).
    band_ok: bool,
    /// Afficher « Tenez la position » (dwell en cours ou instable).
    hold: bool,
    error_code: u8,
    progress: u8,
    sensor_ok: bool,
}

/// Composant plein écran. `on_uploaded` : callback après upload réussi ou
/// échec définitif — la page y recharge la liste.
#[component]
pub fn Take360Overlay(on_close: Callback<()>, on_uploaded: Callback<()>) -> Element {
    // Mémoire des frames — use_hook OBLIGATOIRE : un Arc::new « nu » est
    // recréé à CHAQUE render → n Arcs immortels dans les boucles driver →
    // fuite mémoire + N finish jobs (swap-thrash, stitch bloqué à 15 %,
    // constat device 2026-09-09).
    let frames_mem: Arc<Mutex<Vec<CapturedFrame>>> = use_hook(|| Arc::new(Mutex::new(Vec::new())));
    // Snapshot de rendu — mis à jour par la boucle tick, jamais depuis un
    // thread détaché.
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

    // Ouverture de session : permission, capteur, caméra, plan d'anneau.
    use_future(move || async move {
        super::filelog::log("overlay: montage, reset session");
        state::reset_session();
        state::PHASE.store(Phase::Searching as u8, Ordering::Relaxed);
        wakelock::keep_screen_on(true);
        if !crate::capture::camera_granted() {
            crate::capture::request_camera();
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
        // Le <video> est monté au premier rendu — retry court. À
        // l'ouverture, on pose le PLAN D'ANNEAU : hfov capteur (paysage) ×
        // w/h du track = hfov portrait réel → `guidance::ring_plan` (fermeture
        // d'anneau garantie — cf. guidance.rs).
        for _ in 0..20 {
            match frames::open_camera(VIDEO_ID).await {
                Ok((w, h)) => {
                    super::filelog::log("overlay: caméra ouverte");
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

    // Boucle pilote : tick 33 ms — pas-à-pas strict + snapshots UI + couche
    // DOM continue (15 Hz).
    let mut last_result_seq = use_signal(|| 0u64);
    use_effect(move || {
        // Une seule boucle driver par process (use_effect ré-exécuté →
        // 3 finish déclenchés — constat device 2026-09-09).
        if state::DRIVER_RUNNING.swap(true, Ordering::Relaxed) {
            return;
        }
        let frames_mem = frames_mem.clone();
        spawn(async move {
            let mut last_rendered: Option<UiState> = None;
            let mut last_debug_push: u64 = 0;
            // Dwell courant (instant de démarrage) — None = pas de dwell.
            let mut dwell_start: Option<u64> = None;
            // Fenêtre de poses pour la vitesse angulaire (~[STABLE_WINDOW_MS]).
            let mut samples: Vec<(u64, guidance::Pose)> = Vec::with_capacity(16);
            let mut last_dom_push: u64 = 0;
            loop {
                crate::util::sleep(Duration::from_millis(33)).await;
                let beat = state::DRIVER_BEAT.fetch_add(1, Ordering::Relaxed);
                let now = state::now_ms();
                let phase = state::phase();

                // Session terminée : préviens la page et stoppe la boucle.
                let seq = state::RESULT_SEQ.load(Ordering::Relaxed);
                if seq != last_result_seq() {
                    last_result_seq.set(seq);
                    on_uploaded.call(());
                    break;
                }
                if matches!(phase, Phase::Idle) {
                    continue;
                }

                // ── « Reprendre » : pop de la dernière frame + recul cible.
                // La boucle driver est single-writer de frames_mem (avec le
                // finish) — le bouton UI ne fait que poser la demande.
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
                            "undo : retour à la cible {}/{}",
                            step - 1,
                            state::N_STEPS.load(Ordering::Relaxed)
                        ));
                    }
                }

                // ── Pas-à-pas : cible courante → aligné+stable+dwell → fire.
                let pose = state::pose(now);
                if let Some(p) = pose {
                    samples.push((now, p));
                }
                // Prune des échantillons hors fenêtre (capteur 30 Hz, poll
                // 33 ms : ~14 échantillons dans la fenêtre).
                samples.retain(|(t, _)| now.saturating_sub(*t) <= STABLE_WINDOW_MS);
                let stable = guidance::max_rate_deg_s(&samples) <= guidance::MAX_RATE_DEG_S;

                let ring = state::RING.load(Ordering::Relaxed) as usize;
                let step = state::STEP.load(Ordering::Relaxed) as usize;
                let n_steps = state::N_STEPS.load(Ordering::Relaxed) as usize;
                let step_deg = state::STEP_DEG_MD.load(Ordering::Relaxed) as f32 / 1000.0;
                let (target_yaw, target_pitch) = guidance::target_for(ring, step, step_deg);
                // SANS ancre, RIEN ne se déclenche (la pose est déjà
                // relative au zéro de session, mais le point de départ
                // doit être POSÉ par l'utilisateur — « Ancrer ici »).
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
                                // Avance : pas suivant, anneau suivant ou fin.
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
                                        // Dernier anneau complet : stop des
                                        // ressources puis pipeline détaché
                                        // (l'overlay reste monté).
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
                // PREMIER tour : toujours setter (sans ça, ui reste Idle →
                // <video> jamais monté — constat device 2026-09-09).
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
                    push_overlay_dom(pose, target_yaw, target_pitch, dwell_start, now, stable);
                }

                // Progression de stitch animée (le rendu ne reporte pas de
                // % fin — on fait avancer lentement 30→80 pour montrer la vie).
                if matches!(phase, Phase::Stitching) {
                    let p = state::PROGRESS.load(Ordering::Relaxed);
                    if (30..80).contains(&p) {
                        state::PROGRESS.store(p + 1, Ordering::Relaxed);
                    }
                }

                // Debug en DOM à 10 Hz (temporaire — smoke test).
                if now.saturating_sub(last_debug_push) >= 100 {
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

    // Nettoyage au démontage (annulation, capteur, wakelock, caméra).
    use_drop(move || {
        state::CANCEL.store(true, Ordering::Relaxed);
        sensors::stop();
        wakelock::keep_screen_on(false);
        frames::close_camera();
    });

    // Annulation : stoppe tout et referme.
    let cancel = move |_| {
        state::CANCEL.store(true, Ordering::Relaxed);
        on_close.call(());
    };

    // Bouton « Reprendre » : demande de retrait de la dernière frame —
    // la boucle driver l'exécute (single-writer de frames_mem).
    let undo = move |_| {
        state::UNDO_REQUEST.store(true, Ordering::Relaxed);
    };

    // ───────────────────────── rendu ─────────────────────────
    let in_guidance = matches!(ui().phase, Phase::Searching | Phase::Capturing);
    let in_processing = matches!(ui().phase, Phase::Stitching | Phase::Uploading);
    let n_steps = ui().n_steps;
    // Avancement : pas de l'anneau courant + total session (discret — les
    // deux barres ne bougent qu'aux captures).
    let ring_pct = (ui().step * 100).checked_div(n_steps).unwrap_or(0);
    let overall_done = ui().ring * n_steps + ui().step;
    let overall_total = guidance::N_RINGS * n_steps;
    let overall_pct = overall_done.checked_div(overall_total).unwrap_or(0);

    rsx! {
        div { class: "fixed inset-0 z-[100] bg-black select-none",
            // Preview caméra + SVG guidage (masqués dès le stitching).
            if in_guidance {
                video {
                    key: "{VIDEO_ID}",
                    id: VIDEO_ID,
                    autoplay: true,
                    playsinline: true,
                    class: "absolute inset-0 h-full w-full object-cover",
                }
                // SVG guidage : arc de dwell (remplissage DOM), réticule,
                // point guideur, flèche, pastille stabilité — positions
                // pilotées en DOM direct (push_overlay_dom, 15 Hz).
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
                    // Point guideur : PRÉCÈDE — l'utilisateur va vers lui.
                    // (Signe vertical : SVG y descend, pitch monte → cy =
                    // 200 − dy, cf. push_overlay_dom.)
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
                    // Chevron directionnel (points DOM) — visible hors
                    // alignement yaw.
                    polygon {
                        id: ID_ARROW,
                        points: "",
                        fill: "#38bdf8",
                        "fill-opacity": "0.95",
                        style: "display:none",
                    }
                    // Pastille stabilité : verte immobile, orange en
                    // mouvement (couleur DOM).
                    circle {
                        id: ID_STAB,
                        cx: "200",
                        cy: "252",
                        r: "5",
                        fill: "#22c55e",
                    }
                }
                // Flash blanc de capture — opacité pilotée DOM (pas de
                // re-render dioxus).
                div {
                    id: ID_FLASH,
                    class: "absolute inset-0 bg-white pointer-events-none",
                    style: "opacity:0",
                }
            }

            // Pastille de consigne — t! à littéral, le match choisit.
            if in_guidance {
                div { class: "absolute top-8 inset-x-0 flex justify-center",
                    div { class: "px-4 py-2 rounded-full bg-black/60 text-white text-sm text-center",
                        if !ui().anchored {
                            {t!("media-take360-hint-anchor")}
                        } else if ui().ring >= 1 && !ui().band_ok {
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
                }
            }

            // Compteur de pas + progression (discret) + lectures continues
            // (degrés/pitch) en spans DOM.
            if in_guidance {
                div { class: "absolute bottom-24 inset-x-0 text-center text-white",
                    if ui().ring >= 1 && !ui().band_ok {
                        div { class: "text-sm text-white/80 mb-1",
                            span { id: ID_PITCH, "…" }
                        }
                    }
                    div { class: "text-2xl font-bold", "{ui().step} / {n_steps}" }
                    div { class: "mt-2 mx-auto h-1.5 w-48 bg-white/20 rounded-full overflow-hidden",
                        div { class: "h-full bg-blue-500", width: "{ring_pct}%" }
                    }
                    // Lecture continue : distance yaw + stabilité.
                    div { class: "mt-1.5 text-sm font-medium",
                        span { id: ID_DEG, "" }
                    }
                    div { class: "text-xs text-white/60 mt-1",
                        "Anneau {ui().ring + 1}/{guidance::N_RINGS} · {overall_pct} % du panorama"
                    }
                }
            }

            // Phases de traitement.
            if in_processing {
                div { class: "absolute inset-0 flex flex-col items-center justify-center gap-4 text-white",
                    span { class: "animate-spin rounded-full h-10 w-10 border-b-2 border-blue-400" }
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

            // Terminé.
            if ui().phase == Phase::Done {
                div { class: "absolute inset-0 flex flex-col items-center justify-center gap-4 text-white",
                    div { class: "text-4xl", "✓" }
                    div { class: "text-lg", {t!("media-take360-done")} }
                    button {
                        class: "px-6 py-2 bg-blue-600 text-white rounded-lg text-sm font-medium",
                        onclick: cancel,
                        {t!("media-take360-close")}
                    }
                }
            }

            // Erreurs : permission, caméra, capteur, stitch/upload.
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

            // [DEBUG] état interne — temporaire (smoke test device).
            div {
                id: "p360-debug",
                class: "absolute bottom-4 left-4 text-green-400 text-xs font-mono bg-black/70 px-2 py-1 rounded",
                "…"
            }

            // Boutons : « Reprendre » (retrait dernière frame), ancre
            // (origine/recalage), annuler.
            if in_guidance {
                if ui().anchored && ui().step > 0 {
                    button {
                        class: "absolute bottom-8 left-6 px-3 py-1.5 rounded-full bg-black/60 text-white text-sm",
                        onclick: undo,
                        {t!("media-take360-undo")}
                    }
                }
                button {
                    class: "absolute top-6 left-6 px-3 py-1.5 rounded-full bg-black/60 text-white text-sm",
                    onclick: move |_| state::reanchor(),
                    {t!("media-take360-anchor")}
                }
                button {
                    class: "absolute top-6 right-6 px-3 py-1.5 rounded-full bg-black/60 text-white text-sm",
                    onclick: cancel,
                    {t!("media-take360-cancel")}
                }
            }
        }
    }
}

/// Longueur de l'arc de dwell (circonférence du réticule).
const ARC_LEN: f32 = 2.0 * std::f32::consts::PI * RETICLE_R;

/// Couche DOM continue (15 Hz) : positions/couleurs des éléments de
/// guidage sans re-render dioxus. École `push_debug_dom` — le re-render
/// 30 Hz sature le GPU webview (MALI BAD ALLOC, constat device).
fn push_overlay_dom(
    pose: Option<guidance::Pose>,
    target_yaw: f32,
    target_pitch: f32,
    dwell_start: Option<u64>,
    now: u64,
    stable: bool,
) {
    // Pré-ancre : la cible serait relative au zéro de session (souvent
    // capturé EN MOUVEMENT — cf. state.rs) → point guideur au CENTRE et
    // pas de guidage de rotation : l'instruction est « visez votre point
    // de départ puis Ancrer ici », pas de chasse à une cible arbitraire.
    let anchored = state::anchored();
    // Valeurs par défaut : tout caché/neutre si pose absente.
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
    // Chevron : pointe à distance d (proportionnelle à l'erreur, bornée),
    // dans le sens de la correction — masqué pré-ancre (cf. dot).
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
    let stab_color = if stable { "#22c55e" } else { "#f59e0b" };

    // Flash : la JS garde sa propre séquence — un delta déclenche
    // l'animation (opacity 0.7 → fade 250 ms).
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
         const stab = document.getElementById('{id_stab}'); \
         if (stab) stab.setAttribute('fill', '{stab_color}'); \
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
        id_stab = ID_STAB,
        id_deg = ID_DEG,
        id_pitch = ID_PITCH,
        id_flash = ID_FLASH,
        dot_x = dot_x,
        dot_y = dot_y,
        arc_offset = arc_offset,
        arrow_show = arrow_show,
        arrow_pts = arrow_pts,
        stab_color = stab_color,
        deg_text = deg_text,
        pitch_text = pitch_text,
        capture_seq = capture_seq,
    ));
}

/// État brut dans le DOM à 10 Hz (debug temporaire — smoke test device).
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
