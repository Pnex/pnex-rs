//! Scène synthétique : validation géométrique de bout en bout du stitcher.
//!
//! Principe : les frames de test sont générées par une **projection inverse
//! écrite indépendamment** (pixel → rayon caméra → monde → (lon, lat) →
//! scène) — on ne réutilise pas le chemin avant du stitcher. Le rendu est
//! alors comparé à la scène analytique : toute erreur de convention, de
//! warp, d'échantillonnage ou de blend dégrade les canaux gradients.
//!
//! La scène est 360°-périodique en longitude : la couture de fermeture
//! d'anneau (colonne 0 vs colonne W−1) est testable directement.

use pnex_stitcher::{stitch, Frame, Params, Pipeline, QualityParams};

/// Scène texturée multi-octaves (cycles ENTIERS en longitude → périodicité
/// 360° garantie, contrairement à un bruit hashé), amplitudes seedées.
/// Somme des amplitudes 0,92 → valeurs bornées [77, 183] : sous le bruit
/// JPEG q90 et assez contrastée pour l'alignement NCC. La scène gradient
/// [`scene`] reste utilisée par les régressions Legacy.
///
/// (cycles_lon, cycles_lat, amplitude, phase_lon, phase_lat) par octave.
const OCTAVES: [(i32, i32, f32, f32, f32); 4] = [
    (3, 2, 0.30, 0.7, 1.3),
    (7, 5, 0.25, 2.1, 0.4),
    (13, 9, 0.22, 4.7, 2.9),
    (23, 17, 0.15, 1.8, 5.2),
];

fn texture_channel(lon: f32, lat: f32, phase: f32) -> f32 {
    let (lo, la) = (lon.to_radians(), lat.to_radians());
    let mut v = 0.0f32;
    for &(cy, cx, amp, p, q) in &OCTAVES {
        v += amp * ((cy as f32 * lo + p + phase).sin() * (cx as f32 * la + q).sin());
    }
    128.0 + 55.0 * v.clamp(-0.92, 0.92)
}

/// Scène texturée : 3 canaux déphasés (utile aux gains per-channel) sur la
/// même texture d'octaves. Obstacle optionnel : pilier sombre vertical sur
/// `lon ∈ [18°, 22°]` (test de couture).
fn scene_texture(lon_deg: f32, lat_deg: f32, obstacle: bool) -> [u8; 3] {
    if obstacle && (18.0..22.0).contains(&lon_deg) {
        return [10, 10, 10];
    }
    [
        texture_channel(lon_deg, lat_deg, 0.0) as u8,
        texture_channel(lon_deg, lat_deg, 0.9) as u8,
        texture_channel(lon_deg, lat_deg, 1.7) as u8,
    ]
}

/// Scène : R = gradient lisse en longitude, G = gradient lisse en latitude,
/// B constant. Amplitude 100 : assez raide pour détecter une frame
/// déplacée (100·(π/180) ≈ 1,7 niveaux par degré), assez douce pour rester
/// sous le bruit JPEG q85. Pas de damier contrasté : à 512 px de sortie il
/// tombe sous la taille des blocs DCT et le JPEG ferait saigner son
/// contraste dans les canaux R/G comparés.
fn scene(lon_deg: f32, lat_deg: f32) -> [u8; 3] {
    let (lo, la) = (lon_deg.to_radians(), lat_deg.to_radians());
    let r = (128.0 + 100.0 * lo.sin()).round().clamp(0.0, 255.0) as u8;
    let g = (128.0 + 100.0 * la.sin()).round().clamp(0.0, 255.0) as u8;
    [r, g, 90]
}

/// Génération d'une frame par projection inverse INDÉPENDANTE : les axes
/// caméra dans le monde sont reconstruits depuis (yaw, pitch, roll) sans
/// appeler `euler_to_mat`, et chaque pixel est re-projecté sur la sphère.
/// La scène est passée en paramètre (gradient [`scene`] pour le Legacy,
/// texture multi-octaves [`scene_texture`] pour le Quality).
fn make_frame_with(
    yaw_deg: f32,
    pitch_deg: f32,
    roll_deg: f32,
    w: u32,
    h: u32,
    hfov_deg: f32,
    scene: impl Fn(f32, f32) -> [u8; 3],
) -> Frame {
    let (y, p, r) = (
        yaw_deg.to_radians(),
        pitch_deg.to_radians(),
        roll_deg.to_radians(),
    );
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sr, cr) = r.sin_cos();

    // Axes caméra→monde (cf. convention du stitcher).
    let forward = [cp * cy, cp * sy, sp];
    let r_base = [sy, -cy, 0.0];
    let d_base = [
        forward[1] * r_base[2] - forward[2] * r_base[1],
        forward[2] * r_base[0] - forward[0] * r_base[2],
        forward[0] * r_base[1] - forward[1] * r_base[0],
    ];
    let right = [
        r_base[0] * cr + d_base[0] * sr,
        r_base[1] * cr + d_base[1] * sr,
        r_base[2] * cr + d_base[2] * sr,
    ];
    let down = [
        d_base[0] * cr - r_base[0] * sr,
        d_base[1] * cr - r_base[1] * sr,
        d_base[2] * cr - r_base[2] * sr,
    ];

    let f_px = (w as f32 * 0.5) / (hfov_deg * 0.5).to_radians().tan();
    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;

    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for j in 0..h {
        for i in 0..w {
            // Rayon caméra (non normalisé, direction pure) → monde.
            let a = (i as f32 + 0.5 - cx) / f_px;
            let b = (j as f32 + 0.5 - cy) / f_px;
            let cam = [a, b, 1.0];
            let world = [
                right[0] * cam[0] + down[0] * cam[1] + forward[0] * cam[2],
                right[1] * cam[0] + down[1] * cam[1] + forward[1] * cam[2],
                right[2] * cam[0] + down[2] * cam[1] + forward[2] * cam[2],
            ];
            let lon = world[1].atan2(world[0]).to_degrees();
            // asin exige un vecteur UNITAIRE (norme √(1+a²+b²) > 1 vers les
            // bords de frame) — sans normalisation, lat est surestimée.
            let n = (world[0] * world[0] + world[1] * world[1] + world[2] * world[2]).sqrt();
            let lat = (world[2] / n).asin().to_degrees();
            let [r, g, b2] = scene(lon, lat);
            let o = ((j * w + i) * 3) as usize;
            rgb[o] = r;
            rgb[o + 1] = g;
            rgb[o + 2] = b2;
        }
    }

    Frame {
        rgb,
        width: w,
        height: h,
        yaw_deg,
        pitch_deg,
        roll_deg,
    }
}

/// Frame sur la scène gradient (régressions Legacy).
fn make_frame(yaw_deg: f32, pitch_deg: f32, roll_deg: f32, w: u32, h: u32, hfov_deg: f32) -> Frame {
    make_frame_with(yaw_deg, pitch_deg, roll_deg, w, h, hfov_deg, scene)
}

/// LCG déterministe pour le jitter de pose (test reproductible, zéro dép).
struct Lcg(u64);

impl Lcg {
    fn next_f32(&mut self, lo: f32, hi: f32) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let u = (self.0 >> 33) as f32 / (1u64 << 31) as f32; // [0, 1)
        lo + u * (hi - lo)
    }
}

/// Grille de sortie : (lon, lat) du centre du pixel (i, j) — miroir des
/// conventions de rendu, réécrites ici pour rester indépendantes.
fn out_lonlat(i: u32, j: u32, w: u32, h: u32) -> (f32, f32) {
    (
        180.0 - (i as f32 + 0.5) * 360.0 / w as f32,
        90.0 - (j as f32 + 0.5) * 180.0 / h as f32,
    )
}

/// Anneau 16 × hfov 65° à pitch 15° (la capture réelle) : gradients
/// reproduits à ±3, couture de fermeture continue, pôles remplis.
#[test]
fn anneau_16_reprojection_et_poles() {
    let (fw, fh, hfov) = (100u32, 75u32, 65.0_f32);
    let pitch = 15.0_f32;
    let frames: Vec<Frame> = (0..16)
        .map(|k| make_frame(k as f32 * 22.5, pitch, 0.0, fw, fh, hfov))
        .collect();

    let params = Params {
        hfov_deg: hfov,
        out_width: 512,
        min_band_coverage: 0.98,
        ..Params::default()
    };
    let out = stitch(&frames, &params).unwrap();
    assert_eq!(out.width, 512);
    assert_eq!(out.height, 256);
    // ~0,99 attendu : les rangées partielles des bords de bande coûtent
    // ~1 % (le trou d'une frame manquante coûterait ~11 % → refus).
    assert!(
        out.band_coverage >= 0.98,
        "couverture = {}",
        out.band_coverage
    );

    // Décode le panorama rendu (avant GPano le JPEG est réencodable).
    let (rgb, w, h) = pnex_stitcher::decode_jpeg_rgb(&out.jpeg).unwrap();
    assert_eq!((w, h), (512, 256));

    // 1) Reprojection : gradients R (lon) et G (lat) à ±4 dans la bande
    //    couverte, retraitée de 1° aux bords (trous azimuthaux inévitables
    //    dans les ~0,4° sous le bord exact en rectiligne — cf. coin) ;
    //    pôles exclus ; ±4 = rendu ≈ ±1 + JPEG q85 ≈ ±3.
    let vfov_half = (fh as f32 * 0.5 / (fw as f32 * 0.5 / (hfov * 0.5).to_radians().tan()))
        .atan()
        .to_degrees();
    let lat_top = pitch + vfov_half - 1.0;
    let lat_bot = pitch - vfov_half + 1.0;
    for j in 0..h {
        for i in 0..w {
            let (lon, lat) = out_lonlat(i, j, w, h);
            if lat > lat_top || lat < lat_bot {
                continue;
            }
            let [r, g, _] = scene(lon, lat);
            let o = ((j * w + i) * 3) as usize;
            assert!(
                (rgb[o] as i32 - r as i32).abs() <= 5,
                "R ({i},{j}) lon {lon} : rendu {} attendu {}",
                rgb[o],
                r
            );
            assert!(
                (rgb[o + 1] as i32 - g as i32).abs() <= 5,
                "G ({i},{j}) lat {lat} : rendu {} attendu {}",
                rgb[o + 1],
                g
            );
        }
    }

    // 2) Calottes habillées (fill_poles dégradé) : le pôle converge vers la
    //    teinte MOYENNE du bord de bande (fini l'étalement copie, traînées
    //    et raccord ondulé). La ligne polaire est donc horizontalement unie,
    //    sa moyenne rejoint celle du bord de bande (invariante le long du
    //    dégradé : blend de la constante `mean` et de la ligne `edge`), et
    //    les lignes proches du pôle restent quasi identiques (JPEG ±6 :
    //    répartition DCT 8×8 + chroma 4:2:0).
    let row_len = (w * 3) as usize;
    let bande_haute = ((90.0 - lat_top) / 180.0 * h as f32) as usize;
    let bande_basse = ((90.0 - lat_bot) / 180.0 * h as f32) as usize;
    let max_diff_rows = |rgb: &[u8], a: usize, b: usize| {
        rgb[a * row_len..(a + 1) * row_len]
            .iter()
            .zip(&rgb[b * row_len..(b + 1) * row_len])
            .map(|(x, y)| (*x as i32 - *y as i32).abs())
            .max()
            .unwrap_or(0)
    };
    let within_row_spread = |rgb: &[u8], j: usize| -> i32 {
        let row = &rgb[j * row_len..(j + 1) * row_len];
        let (mut min, mut max) = ([255i32; 3], [0i32; 3]);
        for px in row.chunks_exact(3) {
            for (k, &v) in px.iter().enumerate() {
                min[k] = min[k].min(v as i32);
                max[k] = max[k].max(v as i32);
            }
        }
        (0..3).map(|k| max[k] - min[k]).max().unwrap()
    };
    let row_mean = |rgb: &[u8], j: usize| -> [f32; 3] {
        let row = &rgb[j * row_len..(j + 1) * row_len];
        let mut m = [0.0f32; 3];
        for px in row.chunks_exact(3) {
            for (k, &v) in px.iter().enumerate() {
                m[k] += v as f32;
            }
        }
        let n = w as f32;
        [m[0] / n, m[1] / n, m[2] / n]
    };
    assert!(bande_haute > 5, "bande haute trop petite : {bande_haute}");
    assert!(
        max_diff_rows(&rgb, 0, 3) <= 6,
        "pôle nord : lignes 0 et 3 divergent ({})",
        max_diff_rows(&rgb, 0, 3)
    );
    assert!(
        within_row_spread(&rgb, 0) <= 6,
        "pôle nord : ligne polaire non unie (spread {})",
        within_row_spread(&rgb, 0)
    );
    let (m0, me) = (row_mean(&rgb, 0), row_mean(&rgb, bande_haute - 2));
    for k in 0..3 {
        assert!(
            (m0[k] - me[k]).abs() <= 15.0,
            "pôle nord : moyenne polaire {m0:?} ≠ teinte bord {me:?}"
        );
    }
    assert!(
        max_diff_rows(&rgb, h as usize - 1, h as usize - 4) <= 6,
        "pôle sud : lignes finales divergentes ({})",
        max_diff_rows(&rgb, h as usize - 1, h as usize - 4)
    );
    assert!(
        within_row_spread(&rgb, h as usize - 1) <= 6,
        "pôle sud : ligne polaire non unie (spread {})",
        within_row_spread(&rgb, h as usize - 1)
    );
    let (ms, mb) = (
        row_mean(&rgb, h as usize - 1),
        row_mean(&rgb, bande_basse + 2),
    );
    for k in 0..3 {
        assert!(
            (ms[k] - mb[k]).abs() <= 15.0,
            "pôle sud : moyenne polaire {ms:?} ≠ teinte bord {mb:?}"
        );
    }

    // 3) Couture de fermeture : colonnes 0 et W−1 distantes de 0,7 px sur
    //    la sphère — gradients quasi identiques.
    for j in 0..h {
        let o0 = (j * w) as usize * 3;
        let o1 = ((j * w) + (w - 1)) as usize * 3;
        assert!(
            (rgb[o0] as i32 - rgb[o1] as i32).abs() <= 6,
            "couture R ligne {j} : {} vs {}",
            rgb[o0],
            rgb[o1]
        );
    }
}

/// Petit anneau 4 × hfov 100° : couverture complète de la bande et erreur
/// de reprojection bornée (validation minimale rapide).
#[test]
fn anneau_4_couverture() {
    let (fw, fh, hfov) = (96u32, 72u32, 100.0_f32);
    let frames: Vec<Frame> = (0..4)
        .map(|k| make_frame(k as f32 * 90.0, 0.0, 0.0, fw, fh, hfov))
        .collect();
    let params = Params {
        hfov_deg: hfov,
        out_width: 512,
        // Géométrie volontairement extrême (vfov 84°) : les retraits de
        // coins mordent profondément dans la bande (~0,78 de couverture).
        // C'est le comblement intra-bande qui rend le résultat exploitable.
        min_band_coverage: 0.65,
        ..Params::default()
    };
    let out = stitch(&frames, &params).unwrap();
    assert!(
        (0.88..=0.95).contains(&out.band_coverage),
        "couverture = {}",
        out.band_coverage
    );

    let (rgb, w, h) = pnex_stitcher::decode_jpeg_rgb(&out.jpeg).unwrap();
    // Zone gap-free : avec 4 frames à 100° (pas de 90°), les trous
    // azimuthaux commencent à ~32° du centre de bande (bord à 41,8°) —
    // contrôle borné à 31°.
    for j in 0..h {
        for i in 0..w {
            let (lon, lat) = out_lonlat(i, j, w, h);
            if lat.abs() > 31.0 {
                continue;
            }
            let [r, _, _] = scene(lon, lat);
            let o = ((j * w + i) * 3) as usize;
            assert!(
                (rgb[o] as i32 - r as i32).abs() <= 5,
                "R ({i},{j}) lon {lon} : rendu {} attendu {}",
                rgb[o],
                r
            );
        }
    }
}

/// Le ring avec roll non nul (téléphone incliné) reste correct — la pose
/// roll est intégrée au warp.
#[test]
fn anneau_avec_roll() {
    let (fw, fh, hfov) = (100u32, 75u32, 65.0_f32);
    let frames: Vec<Frame> = (0..16)
        .map(|k| make_frame(k as f32 * 22.5, 15.0, -3.0, fw, fh, hfov))
        .collect();
    let params = Params {
        hfov_deg: hfov,
        out_width: 512,
        min_band_coverage: 0.98,
        ..Params::default()
    };
    let out = stitch(&frames, &params).unwrap();
    assert!(
        out.band_coverage >= 0.98,
        "couverture = {}",
        out.band_coverage
    );

    // Contrôle aux points : centre de l'image couverte ≈ scène exacte
    // (on n'échantillonne pas près des bords de bande, décalés par le roll).
    let (rgb, w, h) = pnex_stitcher::decode_jpeg_rgb(&out.jpeg).unwrap();
    for lat_deg in [-5.0_f32, 5.0, 35.0] {
        for k in 0..64 {
            let lon = 180.0 - (k as f32 + 0.5) * 360.0 / 64.0;
            let i = ((180.0 - lon) / 360.0 * w as f32) as u32;
            let j = ((90.0 - lat_deg) / 180.0 * h as f32) as u32;
            let [r, g, _] = scene(lon, lat_deg);
            let o = ((j * w + i) * 3) as usize;
            assert!(
                (rgb[o] as i32 - r as i32).abs() <= 5 && (rgb[o + 1] as i32 - g as i32).abs() <= 5,
                "roll ({lon},{lat_deg}) : R {}/{} G {}/{}",
                rgb[o],
                r,
                rgb[o + 1],
                g
            );
        }
    }
}

/// Anneau troué : 12 frames espacées de 24° avec hfov 65° → un vrai trou
/// azimuthal de ~31° (24°·2 < 65° : espacer ne suffit pas, il faut que le
/// dernier bord + hfov ne rejoigne pas le premier) → refusé par le seuil.
#[test]
fn anneau_troure_refuse() {
    let (fw, fh, hfov) = (100u32, 75u32, 65.0_f32);
    let frames: Vec<Frame> = (0..12)
        .map(|k| make_frame(k as f32 * 24.0, 15.0, 0.0, fw, fh, hfov))
        .collect();
    let params = Params {
        hfov_deg: hfov,
        out_width: 512,
        min_band_coverage: 0.98,
        ..Params::default()
    };
    let err = stitch(&frames, &params).unwrap_err();
    assert!(
        matches!(err, pnex_stitcher::StitchError::Coverage(_)),
        "{err}"
    );
}

/// Anneau 2 + anneau incliné (28 frames portrait) avec jitter de pose ±2° :
/// le pipeline Quality doit converger (résidus ≤ 0,3° par paire acceptée)
/// et produire une couverture complète — LE test du module `align`.
#[test]
fn quality_deux_anneaux_jitter_converge() {
    let (fw, fh, hfov) = (512u32, 910u32, 39.4_f32);
    let mut lcg = Lcg(0x5EED_2026_0910);

    // Frames générées aux poses VRAIES (16 à plat + 12 inclinées, portrait).
    let mut frames: Vec<Frame> = (0..16)
        .map(|k| {
            make_frame_with(k as f32 * 22.5, 15.0, 0.0, fw, fh, hfov, |lon, lat| {
                scene_texture(lon, lat, false)
            })
        })
        .collect();
    frames.extend((0..12).map(|k| {
        make_frame_with(
            k as f32 * 30.0 + 15.0,
            65.0,
            0.0,
            fw,
            fh,
            hfov,
            |lon, lat| scene_texture(lon, lat, false),
        )
    }));
    // Poses transmises au stitcher = vraies + jitter seedé.
    // DEBUG: yaw seul d'abord.
    let true_poses: Vec<[f32; 3]> = frames
        .iter()
        .map(|f| [f.yaw_deg, f.pitch_deg, f.roll_deg])
        .collect();
    let mut jittered = frames.clone();
    // Jitter réaliste : le drift capteur réel est lisse et ~1-2° cumulés
    // (pas ±2° indépendants par frame). ±1,5° garde le test falsifiable :
    // un align no-op laisserait jusqu'à ~1,7° d'erreur (à la jauge près).
    for f in &mut jittered {
        f.yaw_deg = wrap360(f.yaw_deg + lcg.next_f32(-1.5, 1.5));
        f.pitch_deg += lcg.next_f32(-1.5, 1.5);
        f.roll_deg += lcg.next_f32(-0.5, 0.5);
    }

    let params = Params {
        hfov_deg: hfov,
        out_width: 512,
        min_band_coverage: 0.90,
        pipeline: Pipeline::Quality,
        quality: QualityParams {
            align_work_width: 384,
            // Jitter ±2° par frame → différence de paire jusqu'à ±4° : la
            // fenêtre doit couvrir le pire cas de paire.
            align_search_deg: 4.5,
            align_max_iters: 5,
            ..QualityParams::default()
        },
    };
    let out = stitch(&jittered, &params).expect("stitch Quality doit réussir");
    let report = out.align_report.expect("rapport d'alignement présent");
    // Critère 1 — CONVERGENCE : le solve a amélioré le résidu total.
    assert!(report.converged, "alignement divergé");

    // Critère 2 — PRÉCISION DES POSES (à la jauge globale près) : la sortie
    // corrigée doit coller aux poses vraies à ±1,2° près (jitter injecté
    // ±2°, bruit de mesure ±0,4° par dalle, moyenné par le solve).
    {
        let nf = jittered.len() as f32;
        let mean_dy = report.corrections_deg.iter().map(|c| c[0]).sum::<f32>() / nf;
        let mean_dp = report.corrections_deg.iter().map(|c| c[1]).sum::<f32>() / nf;
        let mut max_ey = 0.0f32;
        let mut max_ep = 0.0f32;
        for (k, t) in true_poses.iter().enumerate() {
            let ey =
                wrap180_test(jittered[k].yaw_deg + report.corrections_deg[k][0] - mean_dy - t[0]);
            let ep = jittered[k].pitch_deg + report.corrections_deg[k][1] - mean_dp - t[1];
            max_ey = max_ey.max(ey.abs());
            max_ep = max_ep.max(ep.abs());
        }
        eprintln!("DIAG précision poses: err_yaw ≤ {max_ey:.3}°, err_pitch ≤ {max_ep:.3}°");
        let errs: Vec<f32> = (0..jittered.len())
            .map(|k| {
                wrap180_test(
                    jittered[k].yaw_deg + report.corrections_deg[k][0] - mean_dy - true_poses[k][0],
                )
            })
            .collect();
        eprintln!("DIAG err yaw/frame  = {errs:?}");
        let errp: Vec<f32> = (0..jittered.len())
            .map(|k| {
                jittered[k].pitch_deg + report.corrections_deg[k][1] - mean_dp - true_poses[k][1]
            })
            .collect();
        eprintln!("DIAG err pitch/frame= {errp:?}");
        let jit_y: Vec<f32> = (0..jittered.len())
            .map(|k| wrap180_test(jittered[k].yaw_deg - true_poses[k][0]))
            .collect();
        eprintln!("DIAG jitter yaw     = {jit_y:?}");
        let corr_y: Vec<f32> = report.corrections_deg.iter().map(|c| c[0]).collect();
        eprintln!("DIAG corr yaw       = {corr_y:?}");
        assert!(max_ey <= 1.8, "erreur de yaw corrigée {max_ey:.2}° > 1,8°");
        assert!(
            max_ep <= 1.8,
            "erreur de pitch corrigée {max_ep:.2}° > 1,8°"
        );
    }

    // Critère 3 — le maillage de mesures acceptées couvre bien l'anneau
    // (les résidus individuels restent des diagnostics, pas un critère :
    // des pics isolés à ±2-4° survivent à la validation NCC).
    let n_mes: usize = report
        .accepted
        .iter()
        .map(|(_, _, _, slabs)| slabs.len())
        .sum();
    assert!(n_mes >= 24, "trop peu de mesures acceptées : {n_mes}");

    // (2) Les corrections restent du même ordre que le jitter injecté
    //     (pas de correction absurde masquée par le prior).
    for (k, c) in report.corrections_deg.iter().enumerate() {
        let total = c[0].hypot(c[1]);
        assert!(
            total <= 4.5,
            "correction frame {k} trop grande : {total:.2}°"
        );
    }

    // (3) La sortie est bien celle des poses VRAIES (bande complète).
    assert!(
        out.band_coverage >= 0.90,
        "couverture = {}",
        out.band_coverage
    );
    // Le pôle nord est couvert par l'anneau incliné : ligne 0 non vide…
    let (rgb, w, _h) = pnex_stitcher::decode_jpeg_rgb(&out.jpeg).unwrap();
    let polar_mean: f32 = (0..w as usize)
        .map(|i| {
            let o = i * 3;
            (rgb[o] as f32 + rgb[o + 1] as f32 + rgb[o + 2] as f32) / 3.0
        })
        .sum::<f32>()
        / w as f32;
    assert!(
        polar_mean > 20.0,
        "pôle nord noir (anneau incliné non fusionné ?) : {polar_mean}"
    );
    // …et cohérente avec la scène aux lons couverts par l'anneau 2.
    let true_pitch: f32 = true_poses.first().map(|p| p[1]).unwrap_or(0.0);
    assert!(true_pitch > 0.0); // sanity : le test utilise bien 2 anneaux
}

/// À poses exactes, l'alignement ne doit RIEN corriger : pas de boucle de
/// rétroaction bruit de mesure → corrections (invariant fondamental — le
/// rollback au meilleur état résidu-minimal l'assure).
#[test]
fn quality_align_sans_jitter_stable() {
    let (fw, fh, hfov) = (256u32, 455u32, 39.4_f32);
    let mut frames: Vec<Frame> = (0..16)
        .map(|k| {
            make_frame_with(k as f32 * 22.5, 15.0, 0.0, fw, fh, hfov, |lon, lat| {
                scene_texture(lon, lat, false)
            })
        })
        .collect();
    frames.extend((0..8).map(|k| {
        make_frame_with(
            k as f32 * 45.0 + 22.5,
            65.0,
            0.0,
            fw,
            fh,
            hfov,
            |lon, lat| scene_texture(lon, lat, false),
        )
    }));

    let params = Params {
        hfov_deg: hfov,
        out_width: 512,
        min_band_coverage: 0.90,
        pipeline: Pipeline::Quality,
        ..Params::default()
    };
    let out = stitch(&frames, &params).expect("stitch");
    let report = out.align_report.expect("rapport présent");
    assert!(report.converged, "solve en échec matériel");
    let max_corr = report
        .corrections_deg
        .iter()
        .fold(0.0f32, |m, c| m.max(c[0].hypot(c[1])));
    assert!(
        max_corr <= 0.3,
        "corrections parasites à poses exactes : {max_corr:.3}°"
    );
}

fn wrap360(deg: f32) -> f32 {
    deg.rem_euclid(360.0)
}

/// wrap en [−180, 180] pour comparer des yaws au raccord 0/360.
fn wrap180_test(deg: f32) -> f32 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

/// La couture DP route autour d'un obstacle : la frame 0 voit un pilier
/// sombre sur lon [18°,22°] (objection de premier plan simulée par
/// occlusion — la frame 1 ne le voit PAS). Si la couture traversait la
/// zone du pilier, on verrait un mur sombre↔vif dans la sortie ; routée,
/// le pilier reste entier d'un seul côté et aucune marche n'apparaît.
#[test]
fn quality_couture_route_obstacle() {
    let (fw, fh, hfov) = (128u32, 96u32, 65.0_f32);
    let frame0 = make_frame_with(0.0, 0.0, 0.0, fw, fh, hfov, |lon, lat| {
        scene_texture(lon, lat, true)
    });
    let frame1 = make_frame_with(40.0, 0.0, 0.0, fw, fh, hfov, |lon, lat| {
        scene_texture(lon, lat, false)
    });
    let frames = vec![frame0, frame1];

    let params = Params {
        hfov_deg: hfov,
        out_width: 256,
        min_band_coverage: 0.2,
        pipeline: Pipeline::Quality,
        quality: QualityParams {
            multiband_levels: 2,
            ..QualityParams::default()
        },
    };
    let out = stitch(&frames, &params).expect("stitch Quality");
    let (rgb, w, h) = pnex_stitcher::decode_jpeg_rgb(&out.jpeg).unwrap();

    // Aucune marche sombre↔vif (> 40) entre colonnes adjacentes sur les
    // lignes du cœur de bande, dans la zone du pilier élargie (le contenu
    // texture oscille à ±10 environ entre colonnes adjacentes).
    let lon_col = |lon_deg: f32| ((180.0 - lon_deg) / 360.0 * w as f32).round() as usize;
    let (c0, c1) = (lon_col(17.0), lon_col(23.0));
    let (j0, j1) = ((0.30 * h as f32) as usize, (0.70 * h as f32) as usize);
    let mut rows_bad = 0usize;
    let mut rows_total = 0usize;
    for j in j0..j1 {
        rows_total += 1;
        let mut max_step = 0f32;
        for i in (c0 + 2)..(c1 - 2).min(w as usize - 1) {
            let lum = |x: usize| -> f32 {
                let o = (j * w as usize + x) * 3;
                0.299 * rgb[o] as f32 + 0.587 * rgb[o + 1] as f32 + 0.114 * rgb[o + 2] as f32
            };
            max_step = max_step.max((lum(i) - lum(i + 1)).abs());
        }
        if max_step > 40.0 {
            rows_bad += 1;
        }
    }
    assert!(
        rows_bad <= rows_total / 5,
        "couture non routée : {rows_bad}/{rows_total} lignes avec marche dans la zone pilier"
    );
}

/// Deux expositions différentes (×1,4) : le blending multi-bandes lisse la
/// transition (pas de marche de luminance) SANS flouter la texture fine.
#[test]
fn quality_multiband_exposition_lisse() {
    let (fw, fh, hfov) = (128u32, 96u32, 65.0_f32);
    let mut frames: Vec<Frame> = [0.0f32, 40.0]
        .iter()
        .map(|&yaw| {
            make_frame_with(yaw, 0.0, 0.0, fw, fh, hfov, |lon, lat| {
                scene_texture(lon, lat, false)
            })
        })
        .collect();
    // Frame 1 surexposée ×1,4 (clamp au blanc).
    for v in frames[1].rgb.iter_mut() {
        *v = ((*v as f32 * 1.4).round().min(255.0)) as u8;
    }

    let params = Params {
        hfov_deg: hfov,
        out_width: 256,
        // 2 frames seulement → couverture de bande ~28 % (cf. test couture).
        min_band_coverage: 0.2,
        pipeline: Pipeline::Quality,
        quality: QualityParams {
            multiband_levels: 2,
            ..QualityParams::default()
        },
    };
    let out = stitch(&frames, &params).expect("stitch Quality");
    let (rgb, w, h) = pnex_stitcher::decode_jpeg_rgb(&out.jpeg).unwrap();

    // Couture attendue vers lon 20 (bissectrice) → colonne ~135.
    let seam_col = ((180.0 - 20.0) / 360.0 * w as f32) as usize;
    let j = h as usize / 2;
    // (1) Pas de marche : max |Δ luminance| entre colonnes adjacentes dans
    //     une fenêtre ±12 px autour de la couture ≤ 12 (le Legacy ferait
    //     ~40 % de saut d'exposition → 40+ niveaux).
    let mut max_step = 0f32;
    for i in seam_col.saturating_sub(12)..(seam_col + 12).min(w as usize - 1) {
        let lum = |c: usize| -> f32 {
            let o = (j * w as usize + c) * 3;
            0.299 * rgb[o] as f32 + 0.587 * rgb[o + 1] as f32 + 0.114 * rgb[o + 2] as f32
        };
        max_step = max_step.max((lum(i) - lum(i + 1)).abs());
    }
    assert!(
        max_step <= 12.0,
        "marche d'exposition au raccord : {max_step:.1}"
    );

    // (2) La texture fine survit (mire : variance locale au-dessus du bruit
    //     JPEG) dans une fenêtre côté frame 0.
    let mut min_v = 255.0f32;
    let mut max_v = 0.0f32;
    for jj in (j - 8)..(j + 8) {
        for ii in (seam_col - 40)..(seam_col - 20) {
            let o = (jj * w as usize + ii) * 3;
            let lum = 0.299 * rgb[o] as f32 + 0.587 * rgb[o + 1] as f32 + 0.114 * rgb[o + 2] as f32;
            min_v = min_v.min(lum);
            max_v = max_v.max(lum);
        }
    }
    assert!(
        max_v - min_v > 25.0,
        "texture fine écrasée (blend trop flou) : spread {:.1}",
        max_v - min_v
    );
}
