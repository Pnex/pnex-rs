//! Assemblage d'un anneau de photos guidées en équirectangulaire 2:1.
//!
//! Take 360 v1 : l'app mobile capture 16 frames en tournant sur place
//! (guidage capteur), en notant pour chacune la pose (yaw, pitch, roll)
//! au déclenchement. Cette crate rassemble les frames sur la sphère
//! **sans détection de points d'intérêt** — le placement vient uniquement
//! des poses (hypothèse : capteur fusionné, dérive acceptée en v1).
//!
//! ## Pipeline
//!
//! 1. préparation (focale rectiligne depuis la HFOV, matrice monde→caméra,
//!    bande de latitude couverte) — [`project`] ;
//! 2. rendu équirect par lignes, **sélection** pondérée par distance au bord
//!    (pas de moyennage → pas de fantômes) précédée d'une compensation de
//!    gain inter-frames, échantillonnage bilinéaire — [`render`] ;
//! 3. contrôle de couverture **dans la bande couverte** (un anneau
//!    horizontal ne couvre qu'une tranche de l'équirect) ;
//! 4. remplissage des pôles par étalement de la ligne couverte la plus
//!    proche (quick & dirty assumé) ;
//! 5. encodage JPEG + métadonnées GPano ([`gpano`]) — le sniff backend
//!    classe ensuite l'asset en `panorama` tout seul.
//!
//! Pure Rust, sans OpenCV, aucune dépendance native : compilable partout,
//! testable sur hôte avec une scène synthétique ([`tests`] en intégration).
//!
//! ## Mémoire / perf
//!
//! Les accumulateurs sont des lignes f32 (pas de tampon f32 pleine image).
//! Largeur de sortie : [`Params::out_width`] — 2048 par défaut (device) ;
//! 4096 reste possible mais se juge sur `stitch_ms` mesuré sur le téléphone
//! (cf. docs/architecture/media.md).

pub mod align;
pub mod geom;
pub mod gpano;
pub mod jpeg;
pub mod multiband;
#[cfg(feature = "pose-model")]
pub mod pose_model;
pub mod project;
pub mod render;
pub mod rotation;
pub mod seam;

use thiserror::Error;

pub use jpeg::{decode_jpeg_rgb, decode_jpeg_rgb_capped, encode_jpeg, JpegError};

/// HFOV de repli (degrés) si la requête Camera2 échoue — principal
/// grand-angle arrière des téléphones actuels.
pub const FALLBACK_HFOV_DEG: f32 = 65.0;

/// Largeur de sortie par défaut (device) — knob perf : 1024 = complétion
/// garantie sur un téléphone sous pression mémoire (constat 2026-09-09 :
/// 2048 + swap → stitch jamais terminé) ; monter vers [`SPEC_OUT_WIDTH`]
/// quand la perf le permet.
pub const DEVICE_OUT_WIDTH: u32 = 1024;

/// Qualité d'encodage du panorama final. 90 (au lieu de 85) : la sélection
/// nette du blend crée des coutures plus franches que le moyennage — la
/// marge JPEG supplémentaire évite le ringing au raccord, et la sortie
/// device gagne en fidélité pour ~+25 % de poids fichier.
pub const OUTPUT_JPEG_QUALITY: u8 = 90;

/// Une frame capturée : pixels RGB8 + pose au déclenchement.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Pixels RGB8, `width·height·3` octets.
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Yaw en degrés, relatif au repère de début de session.
    pub yaw_deg: f32,
    /// Élévation du regard en degrés (+ = vers le haut).
    pub pitch_deg: f32,
    /// Roulis en degrés autour de l'axe optique.
    pub roll_deg: f32,
}

/// Paramètres de stitching.
#[derive(Debug, Clone)]
pub struct Params {
    /// HFOV horizontale de l'objectif (degrés) — requête Camera2 côté
    /// appelant, [`FALLBACK_HFOV_DEG`] en repli.
    pub hfov_deg: f32,
    /// Largeur de sortie (hauteur = largeur/2).
    pub out_width: u32,
    /// Couverture minimale du **cœur de bande** (0-1) : fraction de pixels
    /// couverts parmi les lignes couvertes à ≥ 90 % (cf.
    /// [`render::band_coverage`]). Sous ce seuil l'anneau a un trou (frame
    /// manquante) et le résultat est refusé.
    pub min_band_coverage: f32,
    /// Pipeline de rendu : [`Pipeline::Legacy`] (sélection par centralité,
    /// défaut — le device Android garde exactement le comportement V1) ou
    /// [`Pipeline::Quality`] (alignement + coutures DP + multi-bandes,
    /// destiné au serveur).
    pub pipeline: Pipeline,
    /// Réglages fins du pipeline [`Pipeline::Quality`].
    pub quality: QualityParams,
}

/// Choix du pipeline de rendu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pipeline {
    /// Blend « sélection » V1 (interior^P), poses capteur brutes. Défaut :
    /// comportement historique octet pour octet.
    #[default]
    Legacy,
    /// Pipeline qualité : gains per-channel → alignement par les images →
    /// coutures DP → blending multi-bandes → fills.
    Quality,
}

/// Réglages du pipeline Quality (valeurs par défaut = tuning initial,
/// cf. docs/architecture/media.md).
#[derive(Debug, Clone)]
pub struct QualityParams {
    /// Raffinement de pose par les images (A/B tests : `false` = poses
    /// capteur brutes).
    pub align: bool,
    /// Largeur de travail des strips équirect gris pour l'alignement.
    pub align_work_width: u32,
    /// Fenêtre de recherche du décalage par paire (degrés).
    pub align_search_deg: f32,
    /// Seuil NCC minimal, paires même pitch.
    pub align_min_ncc_intra: f32,
    /// Seuil NCC minimal, paires inter-anneaux (contenu étiré ×1/cos φ →
    /// pic structurellement plus bas).
    pub align_min_ncc_inter: f32,
    /// Régularisation Tikhonov vers la pose capteur (1/rad²) — c'est la
    /// jauge du solve ; ≈ (σ_mesure/σ_pose)² avec σ_mesure ≈ 0,15° et
    /// σ_pose ≈ 1,5°.
    pub align_lambda_reg: f32,
    /// Itérations Gauss-Newton maximales.
    pub align_max_iters: u32,
    /// Niveaux de la pyramide de Laplace (1-8). DÉFAUT 3 (histoire
    /// 2026-09-11 : 2 → 1 → 3 — à 2, les bugs signe-Laplacien inversé +
    /// numérateur non pondéré par le masque faisaient du « blending » une
    /// double-exposition ; corrigés, 3 niveaux fondent large sans
    /// fantôme). 1 = chemin plat crossfade + fondu étroit (recours).
    pub multiband_levels: u32,
    /// Coutures DP à demi-résolution (2× plus rapide, lissage naturel).
    pub seam_half_res: bool,
    /// Canaux de la compensation de gain : 1 = luminance répliquée
    /// (Legacy), 3 = R/G/B indépendants (dérives de balance des blancs).
    pub gain_channels: u32,
    // ─── Raffinement de pose par modèle (`pose-model`, serveur) ───
    /// Plus grand côté de l'image de travail SuperPoint (multiple de 8 en
    /// pratique via `pose_model::work_size`). 896 ≈ 0,44 Mpx → extraction
    /// ~0,3 s/paire RPi.
    pub model_work_px: u32,
    /// Inliers RANSAC minimaux pour accepter une paire (une paire à 10
    /// inliers sur du texte fin peut être un mirage ; 15 est prudent sans
    /// être avare).
    pub model_min_inliers: u32,
    /// Tolérance RANSAC en pixels de l'image de travail (erreur angulaire
    /// équivalente via la focale de travail). 3 px ≈ 0,27° à 504×896.
    pub model_max_err_px: f32,
    /// Prior capteur yaw — FAIBLE : le magnétomètre intérieur est la
    /// principale source d'erreur, on laisse le solve bouger le yaw
    /// librement (jauge assurée par le graphe de paires).
    pub model_lambda_yaw: f64,
    /// Prior capteur pitch/roll — plus fort : la gravité (accéléromètre) est
    /// fiable, le solve ne doit les retoucher qu'à la marge.
    pub model_lambda_pitch_roll: f64,
}

impl Default for QualityParams {
    fn default() -> Self {
        QualityParams {
            align: true,
            align_work_width: 512,
            align_search_deg: 4.0,
            align_min_ncc_intra: 0.5,
            align_min_ncc_inter: 0.35,
            align_lambda_reg: 0.05,
            align_max_iters: 5,
            // 3 (histoire 2026-09-11 : 2 → 1 → 3). À 2, le blend
            // « adoucissait » en réalité via DEUX bugs (signe du Laplacien
            // inversé + numérateur non pondéré par le masque :
            // double-comptage du contenu plein dans le recouvrement) —
            // d'où halos blancs saturés, franges et diffusion en fantômes.
            // Corrigés (tests recouvrement_constant_sans_double_comptage +
            // reconstruction à octave fine), le multibande fait son travail
            // : fondu large sans fantôme. LEVELS=1 = chemin plat
            // crossfade + fondu étroit (knob de recours).
            // 5 since 2026-09-28: once each frame's image extends past its
            // weight (multiband.rs, `dilate_support`), wide levels no longer
            // halo — seams and per-frame exposure steps melt, no added
            // ghosting (real capture beb93c42; ~250 s vs ~140 s at 4096).
            multiband_levels: 5,
            seam_half_res: true,
            gain_channels: 3,
            model_work_px: 896,
            // 8 (était 15) : les zones sans texture (mur blanc, tableau,
            // plafond) laissaient leurs paires rejetées → aucune contrainte
            // d'image → poses capteur brutes → contenu DOUBLÉ aux coutures
            // (~30 px à 4096, constat réel 2026-09-11 capture 52fdd79b,
            // zone tableau blanc). Horn/RANSAC reste gardé par le
            // median_err ≤ model_max_err_px — 8 inliers suffit.
            model_min_inliers: 8,
            model_max_err_px: 3.0,
            model_lambda_yaw: 0.3,
            model_lambda_pitch_roll: 1.5,
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Params {
            hfov_deg: FALLBACK_HFOV_DEG,
            out_width: DEVICE_OUT_WIDTH,
            min_band_coverage: 0.98,
            pipeline: Pipeline::Legacy,
            quality: QualityParams::default(),
        }
    }
}

/// Résultat d'un stitching réussi.
#[derive(Debug, Clone)]
pub struct Stitched {
    /// JPEG complet (métadonnées GPano incluses).
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Couverture mesurée de la bande couverte (0-1).
    pub band_coverage: f32,
    /// Durée du stitching (rendu + encodage), millisecondes.
    pub stitch_ms: u32,
    /// Rapport d'alignement (pipeline Quality uniquement) : corrections de
    /// pose appliquées, résidus par paire — journalisation serveur.
    pub align_report: Option<align::AlignReport>,
}

/// Erreurs du stitching.
#[derive(Debug, Error)]
pub enum StitchError {
    /// Aucune frame fournie.
    #[error("aucune frame")]
    NoFrames,
    /// Frame incohérente (buffer ≠ w·h·3).
    #[error("frame {index} incohérente : {got} octets, attendu {expected}")]
    InvalidFrame {
        /// Index de la frame fautive.
        index: usize,
        /// Taille reçue.
        got: usize,
        /// Taille attendue (w·h·3).
        expected: usize,
    },
    /// Paramètre hors bornes.
    #[error("paramètre invalide : {0}")]
    Params(String),
    /// Anneau troué : couverture de bande insuffisante.
    #[error("couverture de bande insuffisante : {0:.0} %")]
    Coverage(f32),
    /// Échec d'encodage du panorama.
    #[error("encodage : {0}")]
    Encode(#[from] JpegError),
}

/// Référence optionnelle au modèle de pose — type fantôme pour partager
/// `stitch_inner` entre les builds device (pas de modèle) et serveur
/// (feature `pose-model`).
#[cfg(feature = "pose-model")]
pub type ModelRef<'a> = pose_model::PoseModel;
/// Variante device : aucun modèle, l'alias existe pour la signature seule.
#[cfg(not(feature = "pose-model"))]
pub type ModelRef<'a> = ();

/// Assemble les frames en un JPEG équirectangulaire 2:1 (GPano inclus).
///
/// # Erreurs
/// - [`StitchError::NoFrames`] / [`StitchError::InvalidFrame`] : entrées ;
/// - [`StitchError::Params`] : hfov ou largeur absurdes ;
/// - [`StitchError::Coverage`] : trou dans l'anneau (voir
///   [`Params::min_band_coverage`]) ;
/// - [`StitchError::Encode`] : échec JPEG.
pub fn stitch(frames: &[Frame], params: &Params) -> Result<Stitched, StitchError> {
    stitch_inner(frames, params, None)
}

/// Comme [`stitch`], avec raffinement de pose par modèle IA (SuperPoint +
/// LightGlue) — feature `pose-model`, serveur uniquement. `None` = chemin
/// NCC historique.
///
/// # Erreurs
/// Identiques à [`stitch`].
#[cfg(feature = "pose-model")]
pub fn stitch_with_model(
    frames: &[Frame],
    params: &Params,
    model: Option<&pose_model::PoseModel>,
) -> Result<Stitched, StitchError> {
    stitch_inner(frames, params, model)
}

/// Cœur du stitching — `model` = raffinement de pose par les images
/// (rotation 3 DOF) quand fourni, sinon raffinement NCC translation 2 DOF.
fn stitch_inner(
    frames: &[Frame],
    params: &Params,
    #[allow(unused_variables)] model: Option<&ModelRef<'_>>,
) -> Result<Stitched, StitchError> {
    let start = std::time::Instant::now();

    if frames.is_empty() {
        return Err(StitchError::NoFrames);
    }
    if !(1.0..=179.0).contains(&params.hfov_deg) {
        return Err(StitchError::Params(format!("hfov {}", params.hfov_deg)));
    }
    let out_width = params.out_width;
    if out_width < 64 || !out_width.is_multiple_of(2) || out_width > 8192 {
        return Err(StitchError::Params(format!("out_width {out_width}")));
    }
    let out_height = out_width / 2;

    // Préparation : validation + focale + matrice + bande de latitude.
    let mut prepared: Vec<project::PreparedFrame> = Vec::with_capacity(frames.len());
    for (k, f) in frames.iter().enumerate() {
        let expected = f.width as usize * f.height as usize * 3;
        if f.rgb.len() != expected || f.width == 0 || f.height == 0 {
            return Err(StitchError::InvalidFrame {
                index: k,
                got: f.rgb.len(),
                expected,
            });
        }
        let f_px = project::f_px_from_hfov(params.hfov_deg, f.width);
        let (lat_min, lat_max) = project::lat_band(f.height, f_px, f.pitch_deg);
        prepared.push(project::PreparedFrame {
            yaw_deg: f.yaw_deg,
            pitch_deg: f.pitch_deg,
            roll_deg: f.roll_deg,
            inv: geom::euler_to_mat(f.yaw_deg, f.pitch_deg, f.roll_deg),
            width: f.width,
            height: f.height,
            f_px,
            lat_min,
            lat_max,
            // Fondu : ~10 % du plus petit côté, borné pour rester utile.
            feather_px: (f.width.min(f.height) as f32 * 0.10).max(1.0),
            gain: [1.0; 3],
            local_gain: Vec::new(),
            warp: None,
            rgb: f.rgb.clone(),
        });
    }

    let mut align_report: Option<align::AlignReport> = None;
    let (mut rgb, covered, mut mask);
    if params.pipeline == Pipeline::Quality {
        let qp = &params.quality;
        // 1) gains per-channel grossiers (poses brutes).
        let gains = render::estimate_gains(&prepared, qp.gain_channels);
        for (pf, g) in prepared.iter_mut().zip(gains) {
            pf.gain = g;
        }
        // 2) alignement par les images. Avec modèle (serveur) : rotations
        //    relatives 3 DOF (SuperPoint+LightGlue → Horn/RANSAC → solve
        //    géodésique SO(3)) ; la pose capteur n'est qu'un prior. Sinon
        //    (device / modèle absent / paires trop rares) : NCC translation
        //    2 DOF historique. Re-dérive inv/bande lat via refresh_pose.
        // Poses capteur (pour le repli du chemin modèle — cfg pose-model).
        #[cfg_attr(not(feature = "pose-model"), allow(unused_variables))]
        let sensor_poses: Vec<[f32; 3]> = prepared
            .iter()
            .map(|f| [f.yaw_deg, f.pitch_deg, f.roll_deg])
            .collect();
        let report = match model {
            #[cfg(feature = "pose-model")]
            Some(m) => {
                let rep = align::refine_poses_model(&mut prepared, qp, m);
                // Repli NCC si le modèle n'a pas ancré assez de paires
                // (scène sans texture, modèle défaillant) : on repart des
                // poses capteur, jamais d'un semi-état.
                let gate = (prepared.len() / 3).max(2);
                if rep.converged && rep.accepted.len() >= gate {
                    rep
                } else {
                    for (f, p) in prepared.iter_mut().zip(sensor_poses) {
                        [f.yaw_deg, f.pitch_deg, f.roll_deg] = p;
                        f.refresh_pose();
                    }
                    let mut ncc = align::refine_poses(&mut prepared, qp);
                    ncc.fallback = true;
                    ncc
                }
            }
            _ => align::refine_poses(&mut prepared, qp),
        };
        align_report = Some(report);
        // 2ter) Warp local (mesh) : colle les recouvrements au niveau
        // local (~0,1-0,3° de résidu que l'align global 3 DOF ne voit
        // pas) — franges cuivre sur les edges bois et objets « déchirés »
        // disparaissent. HISTORIQUE 2026-09-11 : désactivé le soir car
        // « hyper contrasté » — mais le vrai coupable était le
        // double-comptage du multibande (bugs signe + pondération,
        // corrigés ce même soir) qui amplifiait tout ; réactivé après
        // validation pleine frame warp × pyramide corrigée. LOCALWARP=0 :
        // knob A/B.
        if std::env::var("LOCALWARP").ok().as_deref() != Some("0") {
            align::estimate_local_warp(&mut prepared);
        }
        // 3) gains finaux sur poses corrigées.
        let gains = render::estimate_gains(&prepared, qp.gain_channels);
        for (pf, g) in prepared.iter_mut().zip(gains) {
            pf.gain = g;
        }
        // 3bis) gains locaux basse fréquence (aplani les gradients
        // d'illumination — sinon jointures en rectangles visibles).
        // LOCALGAIN=0 : knob A/B (capture 3 anneaux, constat 2026-09-10).
        if std::env::var("LOCALGAIN").ok().as_deref() != Some("0") {
            render::estimate_local_gains(&mut prepared);
        }
        // 4) coutures DP par paire → carte de propriété.
        let owner = seam::find_seams(&prepared, out_width, qp);
        // 5) blending multi-bandes (masques binaires), 6) fills APRÈS.
        let coverage = render::band_coverage(out_width, &owner.covered_per_row());
        if coverage < params.min_band_coverage {
            return Err(StitchError::Coverage(coverage * 100.0));
        }
        let (blended, covered_mask) = multiband::blend(&prepared, &owner, qp.multiband_levels);
        rgb = blended;
        covered = owner.covered_per_row();
        mask = covered_mask;
    } else {
        // Pipeline Legacy : gains luminance répliqués + rendu sélection.
        let gains = render::estimate_gains(&prepared, 1);
        for (pf, g) in prepared.iter_mut().zip(gains) {
            pf.gain = g;
        }
        let (rendered, counts, mask_px) = render::render(&prepared, out_width);
        rgb = rendered;
        covered = counts;
        mask = mask_px;
    }

    // Handedness d'affichage : le mapping interne (u croît avec la lon ENU)
    // rend une image en miroir à l'affichage « depuis l'intérieur » — le
    // miroir de sortie (rgb + masque, `covered` est invariant) rétablit le
    // standard équirect : u croît en tournant à DROITE. Une seule inversion
    // dans la chaîne, à la frontière — le pipeline interne (align NCC +
    // sonde, solve, seams, multiband) garde sa convention historique.
    // Constat device 2026-09-11 : « propre mais en miroir » gauche-droite.
    render::mirror_rows(&mut rgb, out_width);
    render::mirror_mask(&mut mask, out_width);

    // Contrôle de couverture de bande (AVANT tout remplissage : la métrique
    // doit refléter la capture réelle, pas l'habillage).
    let coverage = render::band_coverage(out_width, &covered);
    if coverage < params.min_band_coverage {
        return Err(StitchError::Coverage(coverage * 100.0));
    }

    // Trous intra-bande (retraits de coins), puis pôles par étalement.
    render::fill_band_holes(&mut rgb, out_width, &mask);
    render::fill_poles(&mut rgb, out_width, &covered);

    // Encodage + GPano.
    let plain = encode_jpeg(&rgb, out_width, out_height, OUTPUT_JPEG_QUALITY)?;
    let jpeg = gpano::write_gpano_xmp(&plain, out_width, out_height);

    Ok(Stitched {
        jpeg,
        width: out_width,
        height: out_height,
        band_coverage: coverage,
        stitch_ms: start.elapsed().as_millis() as u32,
        align_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une frame unique face au rayon lon 0 : rendu sans erreur, couverture
    /// ≈ hfov/360 (bande pleine en latitude, pas en longitude — une seule
    /// frame ne fait pas un anneau), dimensions 2:1, GPano en tête.
    #[test]
    fn frame_unique() {
        let (w, h) = (64u32, 48u32);
        let frame = Frame {
            rgb: vec![128; (w * h * 3) as usize],
            width: w,
            height: h,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        let params = Params {
            min_band_coverage: 0.0,
            ..Params::default()
        };
        let out = stitch(&[frame], &params).unwrap();
        assert_eq!(out.width, DEVICE_OUT_WIDTH);
        assert_eq!(out.height, DEVICE_OUT_WIDTH / 2);
        // Une frame seule : couverture de bande ≈ hfov/360 (bande pleine
        // en latitude, trouée en longitude — ce n'est pas un anneau).
        assert!(
            (out.band_coverage - 65.0 / 360.0).abs() < 0.02,
            "couverture = {}",
            out.band_coverage
        );
        assert!(out.stitch_ms < 60_000);
        // Sniff-like : les littéraux GPano sont bien dans les 128 premiers Ko.
        let head = &out.jpeg[..out.jpeg.len().min(128 * 1024)];
        assert!(head.windows(6).any(|w| w == b"GPano:"));
        assert!(head.windows(15).any(|w| w == b"equirectangular"));
    }

    /// SONDE handedness : le contenu à gauche de l'image d'une frame
    /// (lon > yaw de la frame, cf. euler_to_mat : image right = lon plus
    /// PETITE) doit sortir à GAUCHE du centre dans l'équirect finale —
    /// standard : u croît en tournant à droite. Verrouille le miroir de
    /// sortie de [`stitch`] (constat device 2026-09-11 : miroir G-D).
    #[test]
    fn rendu_handedness() {
        let (w, h) = (512u32, 384u32);
        // Moitié gauche d'image blanche (lon > 30), droite noire (lon < 30).
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        for j in 0..h {
            for i in 0..w {
                let v = if (i as f32) < w as f32 / 2.0 { 240 } else { 10 };
                let o = ((j * w + i) * 3) as usize;
                rgb[o] = v;
                rgb[o + 1] = v;
                rgb[o + 2] = v;
            }
        }
        let frame = Frame {
            rgb,
            width: w,
            height: h,
            yaw_deg: 30.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        let params = Params {
            out_width: 360,
            hfov_deg: 40.0,
            min_band_coverage: 0.0,
            ..Params::default()
        };
        let out = stitch(&[frame], &params).unwrap();
        let (px, dec_w, dec_h) = jpeg::decode_jpeg_rgb(&out.jpeg).expect("decode");
        assert_eq!((dec_w, dec_h), (360, 180));
        let mid = 90usize; // lat ≈ 0
        let at = |col: usize| px[(mid * 360 + col) * 3];
        // lon 30 → colonne interne 210 → miroir : 359−210 = 149. La zone
        // blanche (lon > 30) finit À GAUCHE de 149, la noire à droite.
        let blanc = at(145);
        let noir = at(155);
        assert!(
            blanc > 200 && noir < 60,
            "handedness : blanc(col145)={blanc} noir(col155)={noir}"
        );
    }

    /// Paramètres absurdes → erreurs nommées.
    #[test]
    fn parametres_invalides() {
        let frame = Frame {
            rgb: vec![0; 64 * 48 * 3],
            width: 64,
            height: 48,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        assert!(matches!(
            stitch(&[], &Params::default()),
            Err(StitchError::NoFrames)
        ));
        let bad = Params {
            hfov_deg: 200.0,
            ..Params::default()
        };
        assert!(matches!(
            stitch(std::slice::from_ref(&frame), &bad),
            Err(StitchError::Params(_))
        ));
        let bad = Params {
            out_width: 1001, // impair → invalide
            ..Params::default()
        };
        assert!(matches!(
            stitch(&[frame], &bad),
            Err(StitchError::Params(_))
        ));
    }

    /// Frame incohérente → `InvalidFrame` avec l'index.
    #[test]
    fn frame_incoherente() {
        let frame = Frame {
            rgb: vec![0; 10],
            width: 64,
            height: 48,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        assert!(matches!(
            stitch(&[frame], &Params::default()),
            Err(StitchError::InvalidFrame { index: 0, .. })
        ));
    }
}
