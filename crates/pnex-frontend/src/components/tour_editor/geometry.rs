//! Géométrie du canevas de l'éditeur de tour : fonctions **pures** (testées)
//! et la seule touche DOM, la mesure de l'origine du canevas `canvas_rect`
//! (web-sys). École `flow_editor/geometry.rs` : l'SVG n'a **pas** de
//! `view_box` — 1 unité utilisateur = 1 px CSS, donc
//! `plan = (client − origine − pan) / zoom`.
//!
//! L'unité du plan est le **px natif de l'image du plan** ; les scènes sont
//! stockées dans cette unité (stable par rapport au zoom/pan, portable au
//! viewer). Le hit-test des scènes vit dans les pins eux-mêmes (composant
//! `ScenePin`) — pas de fonction de hit-test ici.

/// Rayon d'un pin de scène (px plan, avant zoom).
pub const SCENE_RADIUS: f64 = 16.0;
/// Bornes du zoom.
pub const ZOOM_MIN: f64 = 0.05;
pub const ZOOM_MAX: f64 = 4.0;
/// Marge du fit initial.
pub const FIT_PADDING: f64 = 24.0;
/// Dimensions de repli d'un plan sans dimensions connues (px natifs).
pub const PLAN_FALLBACK: (f64, f64) = (2000.0, 1000.0);

/// Transform initial « plan ajusté à la vue » : zoom = min des ratios
/// bordés, pan = centrage. Retourne (pan, zoom).
pub fn fit_transform(
    plan_w: f64,
    plan_h: f64,
    view_w: f64,
    view_h: f64,
    padding: f64,
) -> ((f64, f64), f64) {
    if plan_w <= 0.0 || plan_h <= 0.0 || view_w <= 0.0 || view_h <= 0.0 {
        return ((0.0, 0.0), 1.0);
    }
    let zoom = ((view_w - 2.0 * padding) / plan_w)
        .min((view_h - 2.0 * padding) / plan_h)
        .clamp(ZOOM_MIN, ZOOM_MAX);
    let pan = (
        (view_w - plan_w * zoom) / 2.0,
        (view_h - plan_h * zoom) / 2.0,
    );
    (pan, zoom)
}

/// Convertit une position client (px écran) en position plan.
pub fn to_plan(
    client: (f64, f64),
    rect: (f64, f64, f64, f64),
    pan: (f64, f64),
    zoom: f64,
) -> (f64, f64) {
    (
        (client.0 - rect.0 - pan.0) / zoom,
        (client.1 - rect.1 - pan.1) / zoom,
    )
}

/// Nouveau zoom appliqué vers un point fixe de l'écran (le curseur) :
/// `pan₂ = s − k·(s − pan₁)` avec `k = zoom₂ / zoom₁` (école flow_editor).
pub fn zoom_pan_towards(
    pan: (f64, f64),
    zoom: f64,
    new_zoom: f64,
    screen: (f64, f64),
) -> (f64, f64) {
    let k = new_zoom / zoom;
    (
        screen.0 - k * (screen.0 - pan.0),
        screen.1 - k * (screen.1 - pan.1),
    )
}

/// Dimensions de rendu du plan (px natifs) — repli si inconnues.
pub fn plan_size(plan_w: Option<f64>, plan_h: Option<f64>) -> (f64, f64) {
    let w = plan_w.filter(|w| *w > 0.0).unwrap_or(PLAN_FALLBACK.0);
    let h = plan_h.filter(|h| *h > 0.0).unwrap_or(PLAN_FALLBACK.1);
    (w, h)
}

#[cfg(target_arch = "wasm32")]
pub fn canvas_rect() -> Option<(f64, f64, f64, f64)> {
    let document = web_sys::window()?.document()?;
    let element = document.get_element_by_id("tour-canvas")?;
    let rect = element.get_bounding_client_rect();
    Some((rect.left(), rect.top(), rect.width(), rect.height()))
}

/// Cible native : pas de canvas (l'éditeur ne tourne qu'en CSR web pour
/// l'instant) — les gestes ne démarrent jamais sans rect.
#[cfg(not(target_arch = "wasm32"))]
pub fn canvas_rect() -> Option<(f64, f64, f64, f64)> {
    None
}

/// Spacing between two candidate spots of a new scene (px plan).
const SCENE_SPACING: f64 = 3.0 * SCENE_RADIUS;

/// Position of a new scene on a floor: the plan centre, else the first free
/// spot of square rings around it (grid of [`SCENE_SPACING`]), clamped to
/// the plan — new scenes never stack on `taken` ones (same floor only).
pub fn new_scene_position(plan: (f64, f64), taken: &[(f64, f64)]) -> (f64, f64) {
    let (w, h) = plan;
    let centre = (w / 2.0, h / 2.0);
    let margin = SCENE_RADIUS.min(w / 2.0).min(h / 2.0);
    let clamp = |(x, y): (f64, f64)| (x.clamp(margin, w - margin), y.clamp(margin, h - margin));
    let free = |p: (f64, f64)| {
        taken
            .iter()
            .all(|t| (t.0 - p.0).hypot(t.1 - p.1) >= 2.0 * SCENE_RADIUS)
    };
    for ring in 0..64_i32 {
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                if dx.abs() != ring && dy.abs() != ring {
                    continue;
                }
                let candidate = clamp((
                    centre.0 + f64::from(dx) * SCENE_SPACING,
                    centre.1 + f64::from(dy) * SCENE_SPACING,
                ));
                if free(candidate) {
                    return candidate;
                }
            }
        }
    }
    centre
}

/// Natural size (px) of an image (blob URL of a floor plan) once loaded;
/// `None` when it fails to load or has no intrinsic size.
#[cfg(target_arch = "wasm32")]
pub async fn image_natural_size(url: &str) -> Option<(f64, f64)> {
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::{closure::Closure, JsCast};

    let img = web_sys::HtmlImageElement::new().ok()?;
    let (tx, rx) = futures::channel::oneshot::channel::<bool>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let tx_err = tx.clone();
    let on_load: Closure<dyn FnMut()> = Closure::once(move || {
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(true);
        }
    });
    let on_error: Closure<dyn FnMut()> = Closure::once(move || {
        if let Some(tx) = tx_err.borrow_mut().take() {
            let _ = tx.send(false);
        }
    });
    img.set_onload(Some(on_load.as_ref().unchecked_ref()));
    img.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    img.set_src(url);
    let loaded = rx.await.unwrap_or(false);
    img.set_onload(None);
    img.set_onerror(None);
    drop((on_load, on_error));
    let (w, h) = (img.natural_width(), img.natural_height());
    (loaded && w > 0 && h > 0).then(|| (f64::from(w), f64::from(h)))
}

/// Native target: the editor only runs in web CSR, nothing to measure.
#[cfg(not(target_arch = "wasm32"))]
pub async fn image_natural_size(_url: &str) -> Option<(f64, f64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_scene_starts_at_plan_centre() {
        assert_eq!(new_scene_position((1000.0, 500.0), &[]), (500.0, 250.0));
    }

    #[test]
    fn new_scenes_never_stack() {
        let mut taken = Vec::new();
        for _ in 0..30 {
            let p = new_scene_position((800.0, 600.0), &taken);
            assert!(p.0 >= 0.0 && p.0 <= 800.0 && p.1 >= 0.0 && p.1 <= 600.0);
            assert!(taken
                .iter()
                .all(|t: &(f64, f64)| (t.0 - p.0).hypot(t.1 - p.1) >= 2.0 * SCENE_RADIUS));
            taken.push(p);
        }
    }

    #[test]
    fn fit_centre_le_plan() {
        let ((pan_x, pan_y), zoom) = fit_transform(1000.0, 500.0, 1048.0, 548.0, 24.0);
        assert!((zoom - 1.0).abs() < 1e-9, "plan déjà à l'échelle : {zoom}");
        assert!((pan_x - 24.0).abs() < 1e-9 && (pan_y - 24.0).abs() < 1e-9);

        // Plan large : le zoom est borné par la largeur.
        let (_, zoom) = fit_transform(2000.0, 100.0, 1048.0, 548.0, 24.0);
        assert!((zoom - (1048.0 - 48.0) / 2000.0).abs() < 1e-9);
    }

    #[test]
    fn fit_degenere_proprement() {
        let ((px, py), zoom) = fit_transform(0.0, 0.0, 800.0, 600.0, 24.0);
        assert_eq!((px, py, zoom), (0.0, 0.0, 1.0));
    }

    #[test]
    fn conversion_client_vers_plan() {
        let rect = (10.0, 20.0, 800.0, 600.0);
        assert_eq!(to_plan((110.0, 70.0), rect, (100.0, 50.0), 1.0), (0.0, 0.0));
        assert_eq!(
            to_plan((210.0, 170.0), rect, (100.0, 50.0), 2.0),
            (50.0, 50.0)
        );
    }

    #[test]
    fn zoom_conserve_le_point_sous_curseur() {
        let pan = (100.0, 50.0);
        let new_pan = zoom_pan_towards(pan, 1.0, 2.0, (300.0, 250.0));
        let before = to_plan((300.0, 250.0), (0.0, 0.0, 0.0, 0.0), pan, 1.0);
        let after = to_plan((300.0, 250.0), (0.0, 0.0, 0.0, 0.0), new_pan, 2.0);
        assert!((before.0 - after.0).abs() < 1e-9);
        assert!((before.1 - after.1).abs() < 1e-9);
    }

    #[test]
    fn dimensions_de_repli() {
        assert_eq!(plan_size(None, None), PLAN_FALLBACK);
        assert_eq!(plan_size(Some(0.0), Some(-5.0)), PLAN_FALLBACK);
        assert_eq!(plan_size(Some(1234.0), None), (1234.0, PLAN_FALLBACK.1));
    }
}
