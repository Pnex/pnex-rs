//! Guidage de capture 360 — math pure, aucune dépendance, compilé sur
//! toutes les cibles (wasm inclus) : testable sur hôte.
//!
//! V3 (2026-09-10) : retour au PAS-À-PAS STRICT — le déclenchement
//! continu (V1, pivot device 2026-09-09) a vécu : il prenait des frames
//! en marche arrière (|Δyaw| comptait le retour utilisateur) et
//! sous-capture l'anneau (pas ~20° × 15 + hfov portrait 43,7° ≈ 348° <
//! 360° — trou azimutal constaté sur la 1re capture HD 2026-09-10).
//! Nouveau modèle, celui des apps 360 du commerce :
//!
//! 1. l'utilisateur ancre son point de départ (« Ancrer ici ») ;
//! 2. chaque anneau a N cibles fixes (yaw = k·pas, pitch = consigne
//!    d'anneau) — N et le pas sont calculés sur la hfov PORTRAIT réelle
//!    de la caméra (`ring_plan`) : le recouvrement voisin est garanti
//!    ≥ [`MIN_OVERLAP_DEG`] et l'anneau se referme par construction ;
//! 3. une frame ne part QUE sur la cible courante, quand les trois
//!    conditions sont tenues ensemble pendant [`DWELL_MS`] :
//!    aligné (yaw/pitch/roll dans les tolérances) + stable (vitesse
//!    angulaire < [`MAX_RATE_DEG_S`]). Tourner en arrière, se tromper
//!    d'inclinaison ou passer trop vite : RIEN ne se déclenche ;
//! 4. capture validée → cible suivante. « Reprendre » déclenche la
//!    cible précédente (la frame est retirée de la mémoire).
//!
//! Convention d'angles : la MÊME que `pnex-stitcher::geom` (yaw = azimuth
//! du regard, pitch = élévation, roll autour de l'axe optique). Le capteur
//! Android → angles se fait dans `sensors.rs` via
//! `pnex_stitcher::geom` — une seule source de vérité, la cohérence est
//! verrouillée par les tests de la crate stitcher.

// Les consommateurs runtime sont cfg android (overlay) — sans allow, ces
// items sont « morts » côté desktop (école capture.rs : allow documenté,
// pas de code fantôme dans les cibles natives non-android).
#![allow(dead_code)]

/// Nombre d'anneaux du protocole (à plat, haut, bas).
pub const N_RINGS: usize = 3;
/// Protocole 3 anneaux : à plat (l'ancre), incliné vers le HAUT, incliné
/// vers le BAS — l'union des bandes lat couvre ~[-72°, +72°] de latitude
/// en contenu réel (le tiers bas de l'équirect reste synthétique : cf.
/// `pnex-stitcher::render` pôles).
pub const RING_TILT_DEG: f32 = 40.0;
/// Demi-largeur de la bande de pitch d'un anneau incliné : cible ±40°,
/// bande tenable [28°, 52°] — à vfov portrait ~65°, toute frame de
/// l'anneau haut couvre au moins lat [-4°, +60°] → recouvrement ≥ 27°
/// avec l'anneau à plat (couvre jusqu'à ±32,5°).
pub const RING_PITCH_TOL_DEG: f32 = 12.0;
/// Recouvrement azimutal visé entre frames voisines d'un anneau : le pas
/// de cible = hfov − 20° (à hfov portrait 43,7° → N = 16, pas réel
/// 22,5°). Le pipeline Quality serveur réaligne les poses par features —
/// 20° de recouvrement donne ~10° de marge d'erreur de visée.
pub const MIN_OVERLAP_DEG: f32 = 20.0;
/// Tolérance d'alignement en azimut (degrés). Élargie device (constat
/// 2026-09-09 : 2°/1°/4° introuvable à la main tenue — jamais de dwell) ;
/// le serveur Quality réaligne par features (corrections jusqu'à ~10°
/// constatées), la précision de visée n'est plus le facteur limitant.
pub const TOL_YAW_DEG: f32 = 6.0;
/// Tolérance d'élévation — le pitch est RELATIF à la posture de début de
/// session (zéro de pitch, cf. sensors.rs) : tu tiens le téléphone
/// naturellement, l'anneau suit ta hauteur de visée.
pub const TOL_PITCH_DEG: f32 = 8.0;
/// Tolérance de roulis (degrés) — large : le serveur redresse par
/// features, seul le confort utilisateur compte ici.
pub const TOL_ROLL_DEG: f32 = 10.0;
/// Durée d'alignement tenu avant déclenchement (ms) — tenue = aligné ET
/// stable en continu (toute perte annule et remet le compteur à zéro).
pub const DWELL_MS: u64 = 700;
/// Vitesse angulaire max (°/s, max absolu sur les 3 axes) pour être
/// « stable » — trémulation main < 1°/s, micro-ajustement 5-15°/s :
/// au-delà de 3,5°/s on est en plein mouvement. La frame part pose
/// immobile : la pose enregistrée = la pose visée.
pub const MAX_RATE_DEG_S: f32 = 3.5;
/// Déplacement du point guideur (px par degré d'erreur).
pub const DOT_PX_PER_DEG: f32 = 2.0;
/// Sens de rotation demandé : la pose yaw est l'azimut ENU (croît en
/// tournant à GAUCHE — euler_from_mat, cf. `sensors.rs`) — pour que le
/// chevron/point pointent PHYSIQUEMENT vers la cible, il faut donc −1 :
/// err_yaw > 0 (cible à yaw plus haut) = cible à gauche → flèche « ← ».
/// Constante isolée pour le retournement smoke-test device.
pub const ROTATION_DIR: f32 = -1.0;

/// Pose du téléphone (degrés), convention stitcher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

/// Plan d'un anneau : nombre de cibles et pas angulaire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingPlan {
    pub n_steps: usize,
    pub step_deg: f32,
}

/// Plan d'anneau pour une hfov portrait donnée : pas = hfov −
/// [`MIN_OVERLAP_DEG`], N = ceil(360/pas) borné [10, 20]. La fermeture
/// d'anneau est garantie PAR CONSTRUCTION : (N−1)·pas + hfov = 360 +
/// recouvrement ≥ 360 dès que hfov > pas, i.e. hfov > 360/N — vrai pour
/// tout N ≤ 20 et hfov ≥ 30° (bornes du clamp).
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

/// Pitch de consigne d'un anneau : 0 (à plat), +40 (haut), −40 (bas).
#[must_use]
pub fn ring_pitch(ring: usize) -> f32 {
    match ring {
        1 => RING_TILT_DEG,
        2 => -RING_TILT_DEG,
        _ => 0.0,
    }
}

/// Cible courante (yaw, pitch) : yaw relatif à l'ancre (k·pas), pitch =
/// consigne d'anneau.
#[must_use]
pub fn target_for(ring: usize, step: usize, step_deg: f32) -> (f32, f32) {
    (step as f32 * step_deg, ring_pitch(ring))
}

/// Repli dans [−180, 180).
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

/// Erreurs signées (cible − pose) en degrés, enveloppées à 180 :
/// (yaw, pitch, roll).
#[must_use]
pub fn signed_error(target_yaw: f32, target_pitch: f32, pose: Pose) -> (f32, f32, f32) {
    (
        wrap180(target_yaw - pose.yaw),
        target_pitch - pose.pitch,
        -pose.roll,
    )
}

/// Aligné sur la cible courante (les trois tolérances).
#[must_use]
pub fn aligned(err: (f32, f32, f32)) -> bool {
    err.0.abs() <= TOL_YAW_DEG && err.1.abs() <= TOL_PITCH_DEG && err.2.abs() <= TOL_ROLL_DEG
}

/// Déplacement du point guideur (px) : il PRÉCÈDE — l'utilisateur va VERS
/// lui. Sens : cible à yaw plus haut = physiquement à gauche (azimut ENU)
/// → dx négatif via [`ROTATION_DIR`] ; cible au-dessus → dy positif.
#[must_use]
pub fn dot_offset_px(err: (f32, f32, f32), max_px: f32) -> (f32, f32) {
    let dx = (err.0 * DOT_PX_PER_DEG * ROTATION_DIR).clamp(-max_px, max_px);
    let dy = (err.1 * DOT_PX_PER_DEG).clamp(-max_px, max_px);
    (dx, dy)
}

/// Avance après capture : pas suivant, anneau suivant, ou terminé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    NextStep,
    NextRing,
    Finish,
}

/// Transition (anneau, pas) après une capture validée. Le pas-à-pas
/// garantit l'ordre : on ne peut ni sauter ni revenir en arrière via le
/// déclenchement (le seul retour en arrière = « Reprendre », piloté par
/// l'UI).
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

/// Vitesse angulaire max (°/s) sur la fenêtre d'échantillons
/// `(t_ms, (yaw, pitch, roll))` — max absolu sur les 3 axes entre paires
/// consécutives, yaw enveloppé. Fenêtre vide ou d'échantillon unique → 0
/// (stable) : on ne pénalise pas un capteur qui démarre.
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

/// Décision de la boucle de guidage, pure et testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    /// Pas (ou plus) aligné/stable : viser, montrer la direction.
    Wait,
    /// Vient d'être aligné+stable : démarrer le dwell maintenant.
    StartDwell,
    /// Dwell en cours, conditions tenues — durée écoulée (ms).
    Dwell(u64),
    /// Dwell expiré avec conditions tenues : DÉCLENCHER.
    Fire,
    /// Conditions perdues pendant le dwell : annuler et re-chercher.
    CancelDwell,
}

/// Décide de l'étape suivante : pure, pas d'horloge ni d'effet.
///
/// Le dwell exige aligné ET stable EN CONTINU : toute perte (pose absente,
/// tolérance dépassée, mouvement) annule. `dwell_started_ms` = instant de
/// démarrage du dwell courant (None = pas de dwell), `now_ms` = horloge
/// courante (ms, même origine).
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

    /// Fermeture d'anneau : pour toute hfov plausible, (N−1)·pas + hfov
    /// couvre 360° et le recouvrement voisin reste confortable. C'est le
    /// correctif du trou azimutal ~33° constaté sur la 1re HD (V1
    /// continu : pas ~20° × 15 frames).
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
            // Recouvrement voisin ≥ 10° (les paires du réalignement serveur).
            assert!(
                hfov - plan.step_deg >= 10.0,
                "hfov={hfov} overlap={:.1}",
                hfov - plan.step_deg
            );
            hfov += 0.5;
        }
    }

    /// Cas nominal : hfov portrait 43,7° → 16 cibles de 22,5° (comme le
    /// protocole d'origine, mais refermé).
    #[test]
    fn ring_plan_hfov_portrait() {
        let plan = ring_plan(43.7);
        assert_eq!(plan.n_steps, 16);
        assert!((plan.step_deg - 22.5).abs() < 0.01);
    }

    #[test]
    fn cibles_et_bandes_de_pitch() {
        // Anneau 0 à plat : pitch cible 0.
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
        // Sur la cible : erreurs nulles → aligné.
        let err = signed_error(22.5, RING_TILT_DEG, pose(22.5, RING_TILT_DEG, 0.0).unwrap());
        assert!(err.0.abs() < 1e-5 && err.1.abs() < 1e-5 && err.2.abs() < 1e-5);
        assert!(aligned(err));
        // Enveloppe à 180 : cible 170, pose −170 → erreur −20.
        let err = signed_error(170.0, 0.0, pose(-170.0, 0.0, 0.0).unwrap());
        assert!((err.0 + 20.0).abs() < 1e-4);
        // Hors tolérances (marge au-delà des constantes).
        assert!(!aligned((0.0, TOL_PITCH_DEG + 1.0, 0.0)));
        assert!(!aligned((0.0, 0.0, TOL_ROLL_DEG + 1.0)));
        assert!(!aligned((TOL_YAW_DEG + 0.5, 0.0, 0.0)));
    }

    /// Vitesse angulaire : immobile → 0, rotation franche → bien au-delà
    /// du seuil de stabilité.
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
        // Fenêtre trop courte (0 ou 1 échantillon) → 0 (stable).
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
        // Enveloppe yaw : 179 → −179 = +2° par le chemin court (20°/s),
        // PAS −358° (le wrap évite la fausse pointe de vitesse).
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

    /// Table de transitions de `decide` avec le gate de stabilité.
    #[test]
    fn transitions_decide() {
        let (cible_y, cible_p) = (22.5, RING_TILT_DEG);
        let sur_cible = pose(cible_y, cible_p, 0.0);
        // Pas aligné → Wait.
        assert_eq!(
            decide(cible_y, cible_p, pose(0.0, cible_p, 0.0), true, None, 0),
            Decision::Wait
        );
        // Instable sur la cible → Wait (pas de départ de dwell).
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, false, None, 0),
            Decision::Wait
        );
        // Pose absente → Wait.
        assert_eq!(
            decide(cible_y, cible_p, None, true, None, 0),
            Decision::Wait
        );
        // Aligné + stable → StartDwell.
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, true, None, 1000),
            Decision::StartDwell
        );
        // Tenue → Dwell(écoulé), puis Fire au terme.
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, true, Some(1000), 1500),
            Decision::Dwell(500)
        );
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, true, Some(1000), 1700),
            Decision::Fire
        );
        // Dérive yaw pendant le dwell → CancelDwell.
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
        // Instabilité pendant le dwell → CancelDwell.
        assert_eq!(
            decide(cible_y, cible_p, sur_cible, false, Some(1000), 1500),
            Decision::CancelDwell
        );
        // Pose perdue pendant le dwell → CancelDwell.
        assert_eq!(
            decide(cible_y, cible_p, None, true, Some(0), 500),
            Decision::CancelDwell
        );
        // Roll hors tolérance → pas de dwell.
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

    /// Transitions d'avancement : pas → anneau → fin, sans jamais sauter.
    #[test]
    fn avance_sans_sauter() {
        assert_eq!(advance(0, 3, 16), Advance::NextStep);
        // Dernier pas de l'anneau (15/16) → anneau suivant.
        assert_eq!(advance(0, 15, 16), Advance::NextRing);
        assert_eq!(advance(1, 15, 16), Advance::NextRing);
        assert_eq!(advance(2, 15, 16), Advance::Finish);
        // Dernier pas du dernier anneau avec n différent.
        assert_eq!(advance(2, 9, 10), Advance::Finish);
    }

    #[test]
    fn point_guideur_clamp() {
        // Sens : err_yaw > 0 (cible à yaw plus haut = physiquement à
        // GAUCHE, azimut ENU) → dx NÉGATIF (dot à gauche, cf. ROTATION_DIR).
        let (dx, dy) = dot_offset_px((50.0, -50.0, 0.0), 40.0);
        assert!((dx + 40.0).abs() < 1e-5);
        assert!((dy + 40.0).abs() < 1e-5);
        let (dx, _) = dot_offset_px((1.5, 0.0, 0.0), 40.0);
        assert!((dx + 3.0).abs() < 1e-5);
    }
}
