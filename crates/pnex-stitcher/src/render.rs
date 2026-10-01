//! Rendu équirectangulaire : warp sphérique + **sélection** (pas moyennage) +
//! remplissage des pôles, précédé d'une **compensation de gain** inter-frames.
//!
//! Pour chaque pixel de sortie `(lon, lat)` → rayon unitaire monde → pour
//! chaque frame éligible (pré-filtre par bande de latitude) → projection
//! rectiligne → échantillonnage bilinéaire → pondération par distance au
//! bord élevée à [`SELECT_POWER`]. Une puissance élevée fait dominer la frame
//! qui voit le pixel le plus au centre (couture nette), tout en gardant un
//! fondu étroit là où deux frames se valent. Les accumulateurs sont f32 par
//! ligne de sortie (pas d'état inter-lignes) — mémoire bornée.
//!
//! RENDU SÉRIAL, volontairement SANS rayon : le pool global rayon meurt en
//! deadlock sur Android device (workers futex-blocked à l'init, constat
//! 2026-09-09 — 4 threads `pnex-stitch` à 0 % CPU). À 1024 px de sortie le
//! sérial passe en ~1-2 s sur ARM moderne.
//!
//! Repère : cf. [`crate::geom`].

use crate::project::{interior_weight, project_uvc, PreparedFrame};

/// Exposant de sélection : `poids = interior^P`. Plus il est grand, plus la
/// frame la plus centrale l'emporte franchement (couture nette, fondu
/// étroit) ; plus il est petit, plus le recouvrement se moyenne (fantôme).
pub const SELECT_POWER: f32 = 24.0;

/// Near-saturated sample (any channel ≥ 250): a clipped value carries no
/// exposure ratio — a blown-out window reads 255 in every frame whatever
/// its exposure, so such pairs pulled the gains towards 1 and left whole
/// frames visibly darker or tinted (real capture beb93c42, 2026-09-28).
fn is_clipped(col: [f32; 3]) -> bool {
    col.iter().any(|&v| v >= 250.0)
}

/// Estime un gain d'exposition multiplicatif par frame et par canal en
/// égalisant la moyenne dans les recouvrements (équivalent Rust pur du
/// `GainCompensator` d'OpenCV). `channels = 1` égalise la luminance
/// (Rec.601) et réplique le gain sur les 3 canaux (comportement Legacy) ;
/// `channels = 3` résout R, G et B indépendamment (corrige les dérives de
/// balance des blancs entre frames).
///
/// Grille grossière de rayons ; pour chaque rayon vu par ≥ 2 frames (loin
/// des bords), on accumule la valeur par paire. On résout ensuite des
/// log-gains par relaxation (Gauss-Seidel), ancrés à moyenne nulle, puis
/// `gain = exp(G)` borné à `[0.5, 2.0]`. Aucune dépendance d'algèbre.
#[must_use]
pub fn estimate_gains(frames: &[PreparedFrame], channels: u32) -> Vec<[f32; 3]> {
    let n = frames.len();
    let mut gains = vec![[1.0f32; 3]; n];
    if n < 2 {
        return gains;
    }
    let per_channel: bool = channels >= 3;
    // Pour chaque canal traité : la valeur comparée (lum ou canal brut).
    let channel_of = |col: [f32; 3], c: usize| -> f64 {
        if per_channel {
            col[c] as f64
        } else {
            0.299 * col[0] as f64 + 0.587 * col[1] as f64 + 0.114 * col[2] as f64
        }
    };
    // Le résultat du canal logique `c` se réplique sur ces canaux RGB.
    let mapped = |c: usize| -> usize {
        if per_channel {
            c
        } else {
            0
        }
    };

    for c in 0..if per_channel { 3 } else { 1 } {
        // Accumulateurs par paire (a < b) : (somme_a, somme_b, compte).
        let mut pair: std::collections::HashMap<(usize, usize), (f64, f64, u32)> =
            std::collections::HashMap::new();

        // Grille grossière sur la sphère (3° × 3°).
        let mut lat = -87.0f32;
        while lat <= 87.0 {
            let mut lon = -180.0f32;
            while lon < 180.0 {
                // Frames voyant ce rayon suffisamment au centre (évite vignettage/bords).
                let mut seen: Vec<(usize, f64)> = Vec::new();
                for (k, f) in frames.iter().enumerate() {
                    if lat < f.lat_min || lat > f.lat_max {
                        continue;
                    }
                    if let Some((u, v, _)) = project_uvc(f, crate::project::sample_ray(f, lon, lat))
                    {
                        if interior_weight(f, u, v) > 0.25 {
                            let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
                            let val = channel_of(col, c);
                            if val > 1.0 && !is_clipped(col) {
                                seen.push((k, val));
                            }
                        }
                    }
                }
                for i in 0..seen.len() {
                    for j in (i + 1)..seen.len() {
                        let (a, la) = seen[i];
                        let (b, lb) = seen[j];
                        let key = if a < b { (a, b) } else { (b, a) };
                        let (sa, sb) = if a < b { (la, lb) } else { (lb, la) };
                        let e = pair.entry(key).or_insert((0.0, 0.0, 0));
                        e.0 += sa;
                        e.1 += sb;
                        e.2 += 1;
                    }
                }
                lon += 3.0;
            }
            lat += 3.0;
        }

        // Arêtes du graphe : delta_{a→b} = ln(mean_b/mean_a), poids = compte.
        // On veut G_a + ln(mean_a) ≈ G_b + ln(mean_b) (valeurs égalisées).
        let mut adj: Vec<Vec<(usize, f32, f32)>> = vec![Vec::new(); n];
        for (&(a, b), &(sa, sb, cnt)) in pair.iter() {
            if cnt < 12 {
                continue; // recouvrement trop maigre → bruit
            }
            let (ma, mb) = (sa / cnt as f64, sb / cnt as f64);
            if ma <= 0.0 || mb <= 0.0 {
                continue;
            }
            let w = cnt as f32;
            let d_ab = (mb / ma).ln() as f32;
            adj[a].push((b, w, d_ab));
            adj[b].push((a, w, -d_ab));
        }

        // Relaxation de Gauss-Seidel EN PLACE (l'itération Jacobi oscille
        // en permanence sur les graphes bipartis — anneau pair de frames —
        // et ne converge jamais : gains restés neutres en V1, constat
        // 2026-09-10). Ancrage moyenne nulle à chaque balayage.
        let mut g = vec![0.0f32; n];
        for _ in 0..80 {
            for k in 0..n {
                let mut num = 0.0f32;
                let mut den = 0.0f32;
                for &(nb, w, d) in &adj[k] {
                    num += w * (g[nb] + d);
                    den += w;
                }
                if den > 0.0 {
                    g[k] = num / den;
                }
            }
            let mean = g.iter().sum::<f32>() / n as f32;
            for x in g.iter_mut() {
                *x -= mean;
            }
        }

        let dst = mapped(c);
        for k in 0..n {
            let gain = g[k].exp().clamp(0.5, 2.0);
            if per_channel {
                gains[k][dst] = gain;
            } else {
                gains[k] = [gain; 3];
            }
        }
    }

    // Bornage couleur : quand une frame voit une zone peu colorée (plafond
    // blanc), l'égalisation par canal se DÉCOUPLE et la correction R/G/B
    // part dans les coins → image délavée (constat capture réelle
    // 2026-09-10). La déviation de chaque canal est ramenée à ±15 % du gain
    // de luminance (moyenne géométrique des canaux) — la correction de
    // balance des blancs reste possible, la dérive non.
    if per_channel {
        for g3 in gains.iter_mut() {
            let lum = (g3[0] * g3[1] * g3[2]).cbrt();
            let (lo, hi) = (lum * 0.85, lum * 1.15);
            g3[0] = g3[0].clamp(lo, hi);
            g3[1] = g3[1].clamp(lo, hi);
            g3[2] = g3[2].clamp(lo, hi);
        }
    }
    gains
}

/// Estime le gain **local** basse fréquence par frame (grille 3°) : pour
/// chaque cellule vue par ≥ 2 frames, chaque frame est ramenée à la moyenne
/// des frames couvrant la cellule — aplanit les gradients d'illumination
/// intra-scène (fenêtre lumineuse vs mur sombre) responsables des jointures
/// en rectangles. Deux lissages 3×3 sur la grille, clamp [0.7, 1.4].
pub fn estimate_local_gains(frames: &mut [PreparedFrame]) {
    let n = frames.len();
    if n < 2 {
        return;
    }
    for f in frames.iter_mut() {
        f.local_gain = vec![1.0; crate::project::LOCAL_GRID_W * crate::project::LOCAL_GRID_H];
    }
    // Luminance gainée accumulée par cellule et par frame.
    let cells = crate::project::LOCAL_GRID_W * crate::project::LOCAL_GRID_H;
    let mut sums = vec![vec![0.0f64; n]; cells];
    let mut counts = vec![vec![0u32; n]; cells];
    let mut covered_by = vec![vec![false; n]; cells];

    let mut lat = -88.5f32;
    while lat <= 88.5 {
        let mut lon = -178.5f32;
        while lon < 180.0 {
            let cell = crate::project::local_gain_cell(lon, lat);
            for (k, f) in frames.iter().enumerate() {
                if lat < f.lat_min || lat > f.lat_max {
                    continue;
                }
                if let Some((u, v, _)) = project_uvc(f, crate::project::sample_ray(f, lon, lat)) {
                    if interior_weight(f, u, v) > 0.25 {
                        let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
                        if is_clipped(col) {
                            continue;
                        }
                        let g = f.gain;
                        let lum = (0.299 * (col[0] * g[0]).min(255.0)
                            + 0.587 * (col[1] * g[1]).min(255.0)
                            + 0.114 * (col[2] * g[2]).min(255.0))
                            as f64;
                        if lum > 1.0 {
                            sums[cell][k] += lum;
                            counts[cell][k] += 1;
                            covered_by[cell][k] = true;
                        }
                    }
                }
            }
            lon += 3.0;
        }
        lat += 3.0;
    }

    // Correction par frame et cellule : ramener chaque frame à la moyenne
    // géométrique des frames couvrant la cellule (≥ 2 frames).
    let mut raw = vec![vec![1.0f32; n]; cells];
    for cell in 0..cells {
        let mut n_cov = 0usize;
        let mut log_mean = 0.0f64;
        for k in 0..n {
            if covered_by[cell][k] && counts[cell][k] > 0 {
                log_mean += (sums[cell][k] / counts[cell][k] as f64).ln();
                n_cov += 1;
            }
        }
        if n_cov < 2 {
            continue;
        }
        log_mean /= n_cov as f64;
        for k in 0..n {
            if covered_by[cell][k] {
                let own = (sums[cell][k] / counts[cell][k] as f64).ln();
                raw[cell][k] = (log_mean - own).exp().clamp(0.7, 1.4) as f32;
            }
        }
    }

    // Lissage : 2 passes de boîte 3×3 sur la grille (les cellules non
    // couvertes gardent 1.0 dans la moyenne).
    for (k, frame) in frames.iter_mut().enumerate() {
        let (gw, gh) = (crate::project::LOCAL_GRID_W, crate::project::LOCAL_GRID_H);
        let mut cur = raw_cell_grid(&raw, k, gw, gh);
        for _ in 0..2 {
            let mut next = cur.clone();
            for j in 0..gh {
                for i in 0..gw {
                    let mut acc = 0.0f32;
                    let mut cnt = 0u32;
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let y = (j as i32 + dy).clamp(0, gh as i32 - 1) as usize;
                            let x = (i as i32 + dx).rem_euclid(gw as i32) as usize;
                            acc += cur[y * gw + x];
                            cnt += 1;
                        }
                    }
                    next[j * gw + i] = acc / cnt as f32;
                }
            }
            cur = next;
        }
        frame.local_gain = cur;
    }
}

/// Extrait la grille plate d'une frame depuis les valeurs brutes par cellule.
fn raw_cell_grid(raw: &[Vec<f32>], k: usize, gw: usize, gh: usize) -> Vec<f32> {
    let mut out = vec![1.0f32; gw * gh];
    for (cell, grid) in out.iter_mut().enumerate() {
        *grid = raw[cell][k];
    }
    let _ = (gw, gh);
    out
}

/// Rendu équirectangulaire 2:1 de largeur `out_width`.
///
/// Retourne le buffer RGB8, le nombre de pixels couverts par ligne et le
/// masque pixel de couverture. Les frames peuvent être vides (rendu noir,
/// couverture nulle) — `stitch` valide en amont.
#[must_use]
pub fn render(frames: &[PreparedFrame], out_width: u32) -> (Vec<u8>, Vec<u32>, Vec<bool>) {
    let out_h = (out_width / 2).max(1);
    let mut rgb = vec![0u8; (out_width * out_h * 3) as usize];
    let mut covered = vec![0u32; out_h as usize];
    let mut mask = vec![false; (out_width * out_h) as usize];

    // Pré-filtre par ligne : indices de frames éligibles, calculés une fois.
    let eligible: Vec<Vec<usize>> = (0..out_h)
        .map(|j| {
            let lat = lat_of(j, out_width, out_h);
            frames
                .iter()
                .enumerate()
                .filter(|(_, f)| lat >= f.lat_min && lat <= f.lat_max)
                .map(|(k, _)| k)
                .collect()
        })
        .collect();

    let row_len = (out_width * 3) as usize;
    // Rendu sérial ligne par ligne (cf. docs module : rayon interdit sur
    // device). La boucle retourne (compte couvert, masque pixel) par ligne.
    let per_row: Vec<(u32, Vec<bool>)> = rgb
        .chunks_mut(row_len)
        .enumerate()
        .map(|(j, row)| {
            let lat = lat_of(j as u32, out_width, out_h);
            let elig = &eligible[j];
            let mut sum = vec![0.0_f32; (out_width * 3) as usize];
            let mut wsum = vec![0.0_f32; out_width as usize];
            let mut row_mask = vec![false; out_width as usize];

            for &k in elig {
                let f = &frames[k];
                for i in 0..out_width as usize {
                    let lon = lon_of(i as u32, out_width);
                    let Some((u, v, _)) = project_uvc(f, crate::project::sample_ray(f, lon, lat))
                    else {
                        continue;
                    };
                    // Sélection : distance au bord ^ P. La frame qui voit le
                    // pixel le plus au centre domine ; fondu étroit à la couture.
                    let iw = interior_weight(f, u, v);
                    if iw <= 0.0 {
                        continue;
                    }
                    let w = iw.powf(SELECT_POWER);
                    if w <= 0.0 {
                        continue;
                    }
                    let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
                    let g = f.gain;
                    sum[i * 3] += w * (col[0] * g[0]).min(255.0);
                    sum[i * 3 + 1] += w * (col[1] * g[1]).min(255.0);
                    sum[i * 3 + 2] += w * (col[2] * g[2]).min(255.0);
                    wsum[i] += w;
                }
            }

            let mut n_couverts = 0u32;
            for (i, px) in row.chunks_exact_mut(3).enumerate() {
                if wsum[i] > 0.0 {
                    px[0] = (sum[i * 3] / wsum[i]).round().clamp(0.0, 255.0) as u8;
                    px[1] = (sum[i * 3 + 1] / wsum[i]).round().clamp(0.0, 255.0) as u8;
                    px[2] = (sum[i * 3 + 2] / wsum[i]).round().clamp(0.0, 255.0) as u8;
                    row_mask[i] = true;
                    n_couverts += 1;
                }
            }
            (n_couverts, row_mask)
        })
        .collect();

    for (j, (count, row_mask)) in per_row.into_iter().enumerate() {
        covered[j] = count;
        let base = j * out_width as usize;
        mask[base..base + out_width as usize].copy_from_slice(&row_mask);
    }

    (rgb, covered, mask)
}

/// Habille le haut et le bas hors de la **bande solide** (les latitudes non
/// couvertes par l'anneau de capture) proprement, au lieu d'étaler
/// verticalement le bord de bande (qui donnait des traînées et un raccord
/// ondulé « cassé »).
///
/// Une capture mono-anneau ne couvre qu'une tranche de l'équirect : on ne
/// peut pas inventer les calottes. On les rend donc *régulières* — couleur du
/// bord lissée horizontalement (fin des traînées), fondu doux depuis la bande
/// (fin du bord dur ondulé), convergence vers la teinte moyenne du bord vers
/// le pôle (ciel/sol uni). Résultat : bande de contenu nette + calottes
/// unies, plutôt qu'un letterbox déchiré.
pub fn fill_poles(rgb: &mut [u8], out_width: u32, covered: &[u32]) {
    let out_h = covered.len();
    let w = out_width as usize;
    if out_h == 0 || w == 0 {
        return;
    }
    let row_len = w * 3;

    // Bande « solide » : lignes largement couvertes (seuil dégressif si la
    // couverture est faible). Tout ce qui est au-dessus/dessous est habillé.
    let first_where = |t: f32| {
        covered
            .iter()
            .position(|&c| c as f32 >= t * out_width as f32)
    };
    let last_where = |t: f32| {
        covered
            .iter()
            .rposition(|&c| c as f32 >= t * out_width as f32)
    };
    let (top, bot) = [0.9f32, 0.5, 0.01]
        .iter()
        .find_map(|&t| Some((first_where(t)?, last_where(t)?)))
        .unwrap_or((0, out_h - 1));

    // Copie lissée horizontalement (blur à recouvrement circulaire — la
    // longitude boucle) de la ligne de bord : tue les traînées verticales.
    let smoothed = |rgb: &[u8], j: usize| -> Vec<f32> {
        let r = (w / 32).max(1) as i32;
        let base = j * row_len;
        let mut out = vec![0.0f32; row_len];
        for i in 0..w {
            let mut acc = [0.0f32; 3];
            for d in -r..=r {
                let x = ((i as i32 + d).rem_euclid(w as i32)) as usize;
                for k in 0..3 {
                    acc[k] += rgb[base + x * 3 + k] as f32;
                }
            }
            let n = (2 * r + 1) as f32;
            for k in 0..3 {
                out[i * 3 + k] = acc[k] / n;
            }
        }
        out
    };
    let mean_of = |row: &[f32]| -> [f32; 3] {
        let mut m = [0.0f32; 3];
        for i in 0..w {
            for k in 0..3 {
                m[k] += row[i * 3 + k];
            }
        }
        [m[0] / w as f32, m[1] / w as f32, m[2] / w as f32]
    };

    // Habillage supérieur : de la bande (bord lissé) vers le pôle (teinte unie).
    if top > 0 {
        let edge = smoothed(rgb, top);
        let mean = mean_of(&edge);
        for j in 0..top {
            let a = j as f32 / top as f32; // 0 au pôle, 1 au bord de bande
            let dst = j * row_len;
            for i in 0..w {
                for k in 0..3 {
                    let v = mean[k] * (1.0 - a) + edge[i * 3 + k] * a;
                    rgb[dst + i * 3 + k] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    // Habillage inférieur : symétrique.
    if bot < out_h - 1 {
        let edge = smoothed(rgb, bot);
        let mean = mean_of(&edge);
        let denom = (out_h - 1 - bot) as f32;
        for j in (bot + 1)..out_h {
            let a = (out_h - 1 - j) as f32 / denom; // 1 sous la bande, 0 au pôle
            let dst = j * row_len;
            for i in 0..w {
                for k in 0..3 {
                    let v = mean[k] * (1.0 - a) + edge[i * 3 + k] * a;
                    rgb[dst + i * 3 + k] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Couverture de la **bande couverte** : fraction de pixels réellement
/// couverts parmi les lignes contenant au moins un pixel couvert, mesurée
/// AVANT remplissage (les trous intra-bande — retrait des coins en
/// rectiligne, frame manquante — abaissent la métrique, c'est voulu).
///
/// Un anneau horizontal ne couvre qu'une tranche de l'équirect : le seuil
/// se juge DANS la bande, jamais sur la sphère entière (les pôles sont
/// remplis ensuite par étalement).
#[must_use]
pub fn band_coverage(out_width: u32, covered: &[u32]) -> f32 {
    let total: u64 = covered.iter().filter(|&&c| c > 0).map(|&c| c as u64).sum();
    let rows = covered.iter().filter(|&&c| c > 0).count() as u64;
    if rows == 0 {
        return 0.0;
    }
    total as f32 / (rows * out_width as u64) as f32
}

/// Comble les trous **intra-bande** par **extension verticale** : tout pixel
/// non couvert reçoit la fusion des voisins couverts les plus proches
/// au-dessus et en dessous (même colonne), pondérée par l'inverse des
/// distances. Les structures verticales (murs, encadrements, plafond→mur)
/// restent crédibles là où l'ancien remplissage horizontal (moyenne
/// gauche/droite) étirait les bords en traînées « échelle » — constat
/// capture réelle 3 anneaux 2026-09-10 (trou azimutal équateur ~33° :
/// l'anneau n'a pas refermé, 15 frames × ~20° + hfov 43,7 < 360°).
pub fn fill_band_holes(rgb: &mut [u8], out_width: u32, mask: &[bool]) {
    let w = out_width as usize;
    let h = mask.len() / w;
    if h == 0 || w == 0 {
        return;
    }
    let row_len = w * 3;

    // Extension verticale colonne par colonne (O(h) mémoire) : dist_up /
    // dist_dn = distances aux voisins couverts au-dessus/en dessous
    // (-1 = aucun).
    for i in 0..w {
        let mut dist_up = vec![-1i32; h];
        let mut dist_dn = vec![-1i32; h];
        let mut last: i32 = -1;
        for j in 0..h {
            if mask[j * w + i] {
                last = j as i32;
                continue;
            }
            dist_up[j] = if last >= 0 { j as i32 - last } else { -1 };
        }
        let mut last: i32 = -1;
        for j in (0..h).rev() {
            if mask[j * w + i] {
                last = j as i32;
                continue;
            }
            dist_dn[j] = if last >= 0 { last - j as i32 } else { -1 };
        }
        // Fusion pondérée 1/distance (le voisin le plus proche domine).
        for j in 0..h {
            let (du, dd) = (dist_up[j], dist_dn[j]);
            if du < 0 && dd < 0 {
                continue;
            }
            let wa = if du >= 0 { 1.0 / du as f32 } else { 0.0 };
            let wb = if dd >= 0 { 1.0 / dd as f32 } else { 0.0 };
            let s = wa + wb;
            let dst = j * row_len + i * 3;
            let mut v = [0.0f32; 3];
            if du >= 0 {
                let sa = (j - du as usize) * row_len + i * 3;
                for c in 0..3 {
                    v[c] += wa * rgb[sa + c] as f32;
                }
            }
            if dd >= 0 {
                let sb = (j + dd as usize) * row_len + i * 3;
                for c in 0..3 {
                    v[c] += wb * rgb[sb + c] as f32;
                }
            }
            for c in 0..3 {
                rgb[dst + c] = (v[c] / s).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    // Lissage horizontal des pixels remplis (rayon 2) : l'extension colonne
    // par colonne crée du fin striage vertical (voisins étendus issus de
    // lignes de bord au contenu légèrement différent). Rayon court — on
    // garde la transition vers le contenu réel aux bords du trou.
    let r = 2usize;
    let out = rgb.to_vec();
    for j in 0..h {
        let base = j * row_len;
        for i in 0..w {
            if mask[j * w + i] {
                continue;
            }
            for c in 0..3 {
                let mut acc = 0.0f32;
                let mut cnt = 0u32;
                for d in -(r as i64)..=(r as i64) {
                    let x = (i as i64 + d).rem_euclid(w as i64) as usize;
                    if mask[j * w + x] {
                        continue;
                    }
                    acc += out[base + x * 3 + c] as f32;
                    cnt += 1;
                }
                if cnt > 0 {
                    rgb[base + i * 3 + c] = (acc / cnt as f32).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

// ─────────────────────────── helpers sphère ───────────────────────────

/// Miroir horizontal du buffer équirect (rgb + masque) — le handedness
/// d'affichage standard (u croît en tournant à DROITE, sens horaire vu du
/// ciel) s'obtient en inversant le mapping interne (u croît avec la lon
/// ENU). Une seule inversion dans toute la chaîne, à la frontière de
/// rendu : tout le pipeline interne (align NCC + sonde, solve, seams,
/// multiband, gains) conserve la convention historique verrouillée par
/// les sondes. Constat device 2026-09-11 : sans ce miroir, le panoramique
/// sort « propre mais en miroir » gauche-droite.
pub(crate) fn mirror_rows(rgb: &mut [u8], out_width: u32) {
    let w = out_width as usize;
    let h = rgb.len() / (w * 3);
    for j in 0..h {
        let base = j * w * 3;
        for i in 0..w / 2 {
            for c in 0..3 {
                rgb.swap(base + i * 3 + c, base + (w - 1 - i) * 3 + c);
            }
        }
    }
}

/// Miroir horizontal du masque de couverture pixel — doit accompagner
/// [`mirror_rows`] : `fill_band_holes` consomme rgb et masque ensemble.
pub(crate) fn mirror_mask(mask: &mut [bool], out_width: u32) {
    let w = out_width as usize;
    let h = mask.len() / w;
    for j in 0..h {
        let base = j * w;
        for i in 0..w / 2 {
            mask.swap(base + i, base + (w - 1 - i));
        }
    }
}

/// Latitude (degrés) du centre de la ligne de sortie `j`.
pub(crate) fn lat_of(j: u32, _out_width: u32, out_h: u32) -> f32 {
    90.0 - (j as f32 + 0.5) * 180.0 / out_h as f32
}

/// Longitude (degrés) du centre du pixel de la colonne `i`.
///
/// Convention INTERNE (align + sonde : u croît avec la longitude ENU).
/// Le handedness d'affichage standard (u croît en tournant à DROITE,
/// sens horaire vu du ciel) est appliqué au retour de [`render`] par
/// miroir des rangées — cf. `mirror_rows` + sonde `rendu_handedness`.
/// Sans ce miroir de sortie, le rendu était en MIROIR gauche-droite :
/// auto-cohérent (coutures propres) mais inversé vs. la scène réelle,
/// constat device 2026-09-11.
pub(crate) fn lon_of(i: u32, out_width: u32) -> f32 {
    -180.0 + (i as f32 + 0.5) * 360.0 / out_width as f32
}

/// Rayon unitaire monde depuis (lon, lat) en degrés.
pub(crate) fn ray_of(lon_deg: f32, lat_deg: f32) -> [f32; 3] {
    let (lo, la) = (lon_deg.to_radians(), lat_deg.to_radians());
    let (sl, cl) = lo.sin_cos();
    let (sp, cp) = la.sin_cos();
    [cp * cl, cp * sl, sp]
}

/// Échantillonnage bilinéaire RGB8 avec clamp aux bords.
pub(crate) fn sample_bilinear(buf: &[u8], w: u32, h: u32, u: f32, v: f32) -> [f32; 3] {
    let x = u.clamp(0.0, (w - 1) as f32);
    let y = v.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let at = |xx: u32, yy: u32| -> [f32; 3] {
        let o = ((yy * w + xx) * 3) as usize;
        [buf[o] as f32, buf[o + 1] as f32, buf[o + 2] as f32]
    };
    let c00 = at(x0, y0);
    let c10 = at(x1, y0);
    let c01 = at(x0, y1);
    let c11 = at(x1, y1);

    let mut out = [0.0_f32; 3];
    for k in 0..3 {
        let top = c00[k] * (1.0 - fx) + c10[k] * fx;
        let bot = c01[k] * (1.0 - fx) + c11[k] * fx;
        out[k] = top * (1.0 - fy) + bot * fy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Conventions sphère : ligne 0 = pôle nord, u croissant = tourner à
    /// DROITE (lon décroissant de 180 → −180) — handedness standard des
    /// équirects (cf. doc lon_of).
    /// Conventions sphère (interne) : ligne 0 = pôle nord, lon croissant
    /// de gauche à droite sur la grille (−180 → +180). Le handedness
    /// d'affichage est appliqué à la frontière par `mirror_rows` (cf.
    /// sonde `rendu_handedness`).
    #[test]
    fn conventions_sphere() {
        assert!((lat_of(0, 2048, 1024) - 89.9).abs() < 0.1);
        assert!((lat_of(1023, 2048, 1024) + 89.9).abs() < 0.1);
        assert!((lon_of(0, 2048) + 180.0).abs() < 0.1);
        assert!((lon_of(2047, 2048) - 180.0).abs() < 0.1);
        assert!((lon_of(1023, 2048)).abs() < 0.2);
        let r = ray_of(0.0, 90.0);
        assert!((r[2] - 1.0).abs() < 1e-6);
    }
    /// Gains per-channel : une frame ×1,4 doit être ramenée ≈ ×1/1,4 par
    /// l'égalisation des recouvrements (canaux indépendants).
    #[test]
    fn gains_per_channel_egalise() {
        use crate::geom::euler_to_mat;
        use crate::project::{f_px_from_hfov, lat_band, PreparedFrame};
        fn frame(k: usize, expo: f32) -> PreparedFrame {
            let (w, h, hfov) = (128u32, 96u32, 65.0_f32);
            let f_px = f_px_from_hfov(hfov, w);
            let yaw = k as f32 * 40.0;
            let m = euler_to_mat(yaw, 0.0, 0.0);
            let mut rgb = vec![0u8; (w * h * 3) as usize];
            for j in 0..h {
                for i in 0..w {
                    let a = (i as f32 + 0.5 - w as f32 / 2.0) / f_px;
                    let b = (j as f32 + 0.5 - h as f32 / 2.0) / f_px;
                    let world = m.transpose().mul_vec([a, b, 1.0]);
                    let lon = world[1].atan2(world[0]).to_degrees();
                    let n =
                        (world[0] * world[0] + world[1] * world[1] + world[2] * world[2]).sqrt();
                    let lat = (world[2] / n).asin().to_degrees();
                    let (lo, la) = (lon.to_radians(), lat.to_radians());
                    let mut v = 0.0f32;
                    for &(cy, cx, amp, p, q) in &[
                        (3, 2, 0.30, 0.7, 1.3),
                        (7, 5, 0.25, 2.1, 0.4),
                        (13, 9, 0.22, 4.7, 2.9),
                        (23, 17, 0.15, 1.8, 5.2),
                    ] {
                        v += amp * ((cy as f32 * lo + p).sin() * (cx as f32 * la + q).sin());
                    }
                    let val = (128.0 + 55.0 * v.clamp(-0.92, 0.92)) * expo;
                    let o = ((j * w + i) * 3) as usize;
                    rgb[o] = val.round().clamp(0.0, 255.0) as u8;
                    rgb[o + 1] = val.round().clamp(0.0, 255.0) as u8;
                    rgb[o + 2] = val.round().clamp(0.0, 255.0) as u8;
                }
            }
            PreparedFrame {
                yaw_deg: yaw,
                pitch_deg: 0.0,
                roll_deg: 0.0,
                inv: m,
                width: w,
                height: h,
                f_px,
                lat_min: lat_band(h, f_px, 0.0).0,
                lat_max: lat_band(h, f_px, 0.0).1,
                feather_px: 1.0,
                gain: [1.0; 3],
                local_gain: Vec::new(),
                warp: None,
                rgb,
            }
        }
        let frames = vec![frame(0, 1.0), frame(1, 1.4)];
        let gains = estimate_gains(&frames, 3);
        // La jauge (ancrage log-moyen) répartit la correction entre les deux
        // frames ; c'est le RATIO qui doit annuler l'écart d'exposition.
        let ratio = gains[1][0] / gains[0][0];
        eprintln!("DIAG gains = {gains:?}");
        assert!(
            (ratio - 1.0 / 1.4).abs() <= 0.03,
            "ratio de gains = {ratio:.3}, attendu ≈ {:+.3}",
            1.0 / 1.4
        );
    }

    /// Couverture de bande : pleine, partiellement trouée, vide.
    #[test]
    fn couverture_bande() {
        let w = 100u32;
        assert!((band_coverage(w, &[0, 10, 100, 100, 100, 10, 0]) - 0.64).abs() < 1e-6);
        assert!((band_coverage(w, &[100, 100, 92, 100]) - 0.98).abs() < 1e-6);
        assert!(band_coverage(w, &[0, 0]).abs() < 1e-6);
    }
}
