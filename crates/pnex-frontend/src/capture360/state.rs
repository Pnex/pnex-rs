//! État de session Take 360 — atomics purs, zéro lock, zéro signal dioxus.
//!
//! École `capture.rs` : les threads détachés (capteur, pipeline) et la
//! boucle de guidage ne peuvent pas toucher aux signaux dioxus (contexte
//! absent → panic → mutex runtime empoisonné). Toute la communication
//! passe par ces atomics, sondés par l'overlay (tick 33 ms) et par la page
//! media (polling 500 ms pour le toast final).

use super::guidance::Pose;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};

/// Phase de la session.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Aucune session.
    Idle = 0,
    /// Session ouverte : capteur + caméra en cours d'ouverture.
    Starting = 1,
    /// Recherche d'alignement sur la cible courante.
    Searching = 2,
    /// Dwell en cours (alignement tenu).
    Dwelling = 3,
    /// Grab de la frame en vol.
    Capturing = 4,
    /// Stitching on-device.
    Stitching = 5,
    /// Upload en vol.
    Uploading = 6,
    /// Panorama uploadé.
    Done = 7,
    /// Erreur (cf. [`ERROR_CODE`]).
    Error = 8,
}

/// Phase courante.
pub static PHASE: AtomicU8 = AtomicU8::new(Phase::Idle as u8);
/// Frames capturées (0..=16).
pub static STEP: AtomicU8 = AtomicU8::new(0);
/// Progression stitching/upload (0-100).
pub static PROGRESS: AtomicU8 = AtomicU8::new(0);
/// Code d'erreur : 0 aucune, 1 permission, 2 caméra, 3 capteur, 4 stitch,
/// 5 upload.
pub static ERROR_CODE: AtomicU8 = AtomicU8::new(0);
/// Compteur de sessions terminées + code résultat (toast page media).
pub static RESULT_SEQ: AtomicU64 = AtomicU64::new(0);
pub static RESULT_CODE: AtomicU8 = AtomicU8::new(0);

/// Poses en millidegrés (i32 : ±180 000 suffit), relatifs au zéro de
/// session. Écrites par le thread capteur à 30 Hz.
pub static POSE_YAW: AtomicI32 = AtomicI32::new(0);
pub static POSE_PITCH: AtomicI32 = AtomicI32::new(0);
pub static POSE_ROLL: AtomicI32 = AtomicI32::new(0);
/// Horodatage (ms unix) du dernier échantillon capteur — fraîcheur.
pub static POSE_TICK: AtomicU64 = AtomicU64::new(0);
/// Fraîcheur max d'une pose (ms) au-delà de laquelle `pose()` renvoie None.
pub const POSE_MAX_AGE_MS: u64 = 250;

/// Yaw de début de session (millidegrés) — posé par le thread capteur.
pub static ZERO_YAW: AtomicI32 = AtomicI32::new(0);
/// Pitch de début de session (millidegrés) — idem : la cible de visée est
/// la posture naturelle de départ.
pub static ZERO_PITCH: AtomicI32 = AtomicI32::new(0);
/// Ancre utilisateur (millidegrés, poses relatives zéro de session) :
/// « Ancrer ici » redéfinit l'origine du ring sur la pose courante —
/// utilise au départ ou à tout moment (dérive capteur).
pub static ANCHOR_YAW: AtomicI32 = AtomicI32::new(0);
pub static ANCHOR_PITCH: AtomicI32 = AtomicI32::new(0);
/// Ancre posée ? Jusque-là : pas de cible, l'utilisateur vise son point de
/// départ et appuie sur « Ancrer ici » (le zéro auto capturé au démarrage
/// tombe sur la posture en MOUVEMENT — constat device 2026-09-09).
pub static ANCHOR_VALID: AtomicBool = AtomicBool::new(false);
/// Yaw ancré de la dernière frame capturée (millidegrés) — debug + reco
/// « Reprendre ».
pub static LAST_FRAME_YAW: AtomicI32 = AtomicI32::new(0);
/// HFOV PORTRAIT de la caméra arrière (millidegrés) — posée par l'overlay
/// à l'ouverture de session (hfov capteur paysage × w/h du track).
/// 0 = inconnue → repli [`DEFAULT_PORTRAIT_HFOV_MD`]. Le plan d'anneau
/// (nombre de cibles, pas) en découle : fermeture garantie.
pub static PORTRAIT_HFOV_MD: AtomicI32 = AtomicI32::new(0);
/// HFOV portrait par défaut (millidegrés) — la mesure Camera2 échoue
/// rarement, mais le plan d'anneau doit être posé même sans.
pub const DEFAULT_PORTRAIT_HFOV_MD: i32 = 43_700;
/// Nombre de cibles par anneau (plan courant, cf. `guidance::ring_plan`)
/// et pas angulaire (millidegrés) — posés à l'ouverture de session.
pub static N_STEPS: AtomicU8 = AtomicU8::new(16);
pub static STEP_DEG_MD: AtomicI32 = AtomicI32::new(22_500);
/// Séquence de capture : incrémentée à CHAQUE frame réussie — la couche
/// DOM (eval 15 Hz) y lit le déclenchement pour le flash visuel sans
/// re-render dioxus.
pub static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(0);
/// Demande « Reprendre » (retirer la dernière frame, reculer la cible) —
/// posée par le bouton UI, traitée par la boucle driver (single-writer).
pub static UNDO_REQUEST: AtomicBool = AtomicBool::new(false);
/// Horodatage de la dernière capture (ms unix) — anti-rafales.
pub static LAST_CAPTURE_MS: AtomicU64 = AtomicU64::new(0);
/// Dernière erreur de grab (0 = aucune, 1 = eval, 2 = json, 3 = pas de
/// payload, 4 = préfixe data:, 5 = base64, 6 = vidéo absente/vide) —
/// affichée dans la ligne debug (le logger ne sort pas, constat device).
pub static LAST_GRAB_ERR: AtomicU8 = AtomicU8::new(0);

/// Anneau de capture courant (Take 360 V2) : 0 = tour à plat, 1 = anneau
/// incliné vers le haut (pôle nord réel). La fin d'un anneau n'entraîne le
/// finish qu'après l'anneau 1 (cf. overlay — le stitcher accepte les poses
/// arbitraires, les bandes lat suffisent).
pub static RING: AtomicU8 = AtomicU8::new(0);
/// État du stitch (debug) : 0 idle, 1 decode, 2 render, 3 encode, 4 prêt,
/// 5 erreur.
pub static STITCH_STATE: AtomicU8 = AtomicU8::new(0);
/// Frame en cours de décodage (0-15) — debug.
pub static STITCH_IDX: AtomicU8 = AtomicU8::new(0);
/// Battement de cœur de la boucle driver (avance à chaque tick 33 ms) —
/// si figé, la boucle/executor est morte ; si il avance, elle vit.
pub static DRIVER_BEAT: AtomicU64 = AtomicU64::new(0);
/// Garde : un seul finish par session (la boucle driver a pu tripler —
/// 3 threads stitch bloqués constatés device 2026-09-09).
pub static FINISH_STARTED: AtomicBool = AtomicBool::new(false);
/// Garde : une seule boucle driver par process (use_effect ré-exécuté).
pub static DRIVER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Ancre posée ?
#[must_use]
pub fn anchored() -> bool {
    ANCHOR_VALID.load(Ordering::Relaxed)
}

/// Abandon demandé par l'UI (annulation ou démontage de l'overlay).
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// Horloge murale en ms unix (même origine que [`POSE_TICK`]).
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

/// Pose courante si fraîche (None si capteur muet > [`POSE_MAX_AGE_MS`] ou
/// jamais vu).
///
/// Cohérence : POSE_TICK est relu après les trois angles — s'il a avancé
/// pendant la lecture, l'échantillon est mélangé → None (l'overlay
/// réessayera au tick suivant, 33 ms).
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
    // Ancre utilisateur : soustraite à la lecture — les cibles (k·22,5°)
    // deviennent relatives au point ancré, à tout instant.
    let anchor_yaw = ANCHOR_YAW.load(Ordering::Relaxed) as f32 / 1000.0;
    let anchor_pitch = ANCHOR_PITCH.load(Ordering::Relaxed) as f32 / 1000.0;
    Some(Pose {
        yaw: crate::capture360::guidance::wrap180(yaw as f32 / 1000.0 - anchor_yaw),
        pitch: pitch as f32 / 1000.0 - anchor_pitch,
        roll: roll as f32 / 1000.0,
    })
}

/// « Ancrer ici » : l'origine du ring devient la pose courante (le pitch
/// garde son roll, seul yaw/pitch sont reancrés). Utilisable à tout
/// moment — la géométrie de l'anneau reste cohérente (tout est relatif).
pub fn reanchor() {
    let yaw = POSE_YAW.load(Ordering::Relaxed);
    let pitch = POSE_PITCH.load(Ordering::Relaxed);
    ANCHOR_YAW.store(yaw, Ordering::Relaxed);
    ANCHOR_PITCH.store(pitch, Ordering::Relaxed);
    ANCHOR_VALID.store(true, Ordering::Relaxed);
}

/// Remise à zéro de l'état de session (à l'ouverture de l'overlay).
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
