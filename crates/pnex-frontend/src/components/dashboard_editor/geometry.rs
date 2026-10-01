//! Géométrie pure du canvas SCADA (école `flow_editor/geometry.rs`) —
//! conversion client→canvas, snap, hit-tests, accroches et coudes des
//! traits. Tout est testable sans DOM ; la seule branche web est
//! [`canvas_rect`].

use pnex_core::{DashboardLayout, Widget, WireSide};

/// Grille de snap (px) — plus fine que le flow editor (10 px) : un
/// synoptique s'aligne visuellement, pas structurellement.
pub const GRID: f64 = 10.0;
pub const ZOOM_MIN: f64 = 0.4;
pub const ZOOM_MAX: f64 = 2.0;

/// Taille par défaut d'un widget posé (px) par type.
pub fn default_size(widget_type: &str) -> (i64, i64) {
    match widget_type {
        "gauge" => (240, 200),
        "stat" => (200, 140),
        "line" => (320, 200),
        "indicator" => (220, 120),
        "text" => (260, 120),
        "thermo_chart" => (480, 360),
        "symbol" => (80, 80),
        _ => (240, 160),
    }
}

/// Point client (px CSS) → coordonnées canvas (px document).
pub fn to_canvas(client: (f64, f64), rect: (f64, f64), pan: (f64, f64), zoom: f64) -> (f64, f64) {
    (
        (client.0 - rect.0 - pan.0) / zoom,
        (client.1 - rect.1 - pan.1) / zoom,
    )
}

/// Snap sur la grille.
pub fn snap(v: f64) -> f64 {
    (v / GRID).round() * GRID
}

/// Zoom molette vers le curseur (école flow editor).
pub fn zoom_pan_towards(
    zoom: f64,
    pan: (f64, f64),
    cursor: (f64, f64),
    factor: f64,
) -> (f64, (f64, f64)) {
    let new_zoom = (zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
    let k = new_zoom / zoom;
    let new_pan = (
        cursor.0 - (cursor.0 - pan.0) * k,
        cursor.1 - (cursor.1 - pan.1) * k,
    );
    (new_zoom, new_pan)
}

/// Le point est-il dans le widget ? (réservé au hit-test mathématique si
/// les widgets passent en rendu SVG pur — V1 : le hit est le DOM).
#[allow(dead_code)]
pub fn hit_test(w: &Widget, p: (f64, f64)) -> bool {
    p.0 >= w.x as f64 && p.0 <= (w.x + w.w) as f64 && p.1 >= w.y as f64 && p.1 <= (w.y + w.h) as f64
}

/// Distance au handle de resize (coin bas-droit), `None` au-delà.
pub fn resize_handle_at(w: &Widget, p: (f64, f64), tolerance: f64) -> bool {
    let corner = ((w.x + w.w) as f64, (w.y + w.h) as f64);
    (p.0 - corner.0).abs() <= tolerance && (p.1 - corner.1).abs() <= tolerance
}

/// Point d'accroche d'un trait : **milieu du côté** demandé — pur.
pub fn anchor_point(w: &Widget, side: WireSide) -> (f64, f64) {
    let (x, y, wd, ht) = (w.x as f64, w.y as f64, w.w as f64, w.h as f64);
    match side {
        WireSide::Top => (x + wd / 2.0, y),
        WireSide::Bottom => (x + wd / 2.0, y + ht),
        WireSide::Left => (x, y + ht / 2.0),
        WireSide::Right => (x + wd, y + ht / 2.0),
    }
}

/// Cherche un widget par id (aucun panique sur document incohérent).
pub fn widget_of<'a>(layout: &'a DashboardLayout, id: &str) -> Option<&'a Widget> {
    layout.widgets.iter().find(|w| w.id == id)
}

/// Coude **orthogonal** entre deux accroches (école câbles flow) : on
/// part perpendiculairement au côté d'origine, on rejoint
/// perpendiculairement au côté d'arrivée. Pur — rendu en SVG path.
pub fn wire_path(
    layout: &DashboardLayout,
    from_id: &str,
    from_side: WireSide,
    to_id: &str,
    to_side: WireSide,
) -> Option<String> {
    let a = widget_of(layout, from_id)?;
    let b = widget_of(layout, to_id)?;
    let (ax, ay) = anchor_point(a, from_side);
    let (bx, by) = anchor_point(b, to_side);
    // 40 % de la distance en segment d'écartement, minimum 14 px.
    let gap = ((bx - ax).abs().max((by - ay).abs()) * 0.4).max(14.0);
    let (a_dx, a_dy) = match from_side {
        WireSide::Top => (0.0, -gap),
        WireSide::Bottom => (0.0, gap),
        WireSide::Left => (-gap, 0.0),
        WireSide::Right => (gap, 0.0),
    };
    let (b_dx, b_dy) = match to_side {
        WireSide::Top => (0.0, -gap),
        WireSide::Bottom => (0.0, gap),
        WireSide::Left => (-gap, 0.0),
        WireSide::Right => (gap, 0.0),
    };
    // Milieu des deux segments d'écartement, coude orthogonal au milieu.
    let m1 = (ax + a_dx, ay + a_dy);
    let m2 = (bx + b_dx, by + b_dy);
    let mid = ((m1.0 + m2.0) / 2.0, (m1.1 + m2.1) / 2.0);
    Some(format!(
        "M {ax:.1} {ay:.1} L {m1x:.1} {m1y:.1} L {midx:.1} {midy:.1} L {m2x:.1} {m2y:.1} L {bx:.1} {by:.1}",
        m1x = m1.0,
        m1y = m1.1,
        midx = mid.0,
        midy = mid.1,
        m2x = m2.0,
        m2y = m2.1
    ))
}

/// Mesure du rect du canvas dans le DOM (web uniquement — l'éditeur est
/// web-only V1, école `flow_editor/geometry.rs::canvas_rect`).
#[cfg(target_arch = "wasm32")]
pub fn canvas_rect() -> Option<(f64, f64)> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let element = document.get_element_by_id("dashboard-canvas")?;
    let rect = element.get_bounding_client_rect();
    Some((rect.left(), rect.top()))
}

/// Cible native : pas de canvas (l'éditeur ne tourne qu'en CSR web pour
/// l'instant) — les gestes ne démarrent jamais sans rect.
#[cfg(not(target_arch = "wasm32"))]
pub fn canvas_rect() -> Option<(f64, f64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::{CanvasSpec, SourceRef, WidgetOptions};

    fn widget(x: i64, y: i64, w: i64, h: i64) -> Widget {
        Widget {
            id: "w1".into(),
            widget_type: "gauge".into(),
            title: "T".into(),
            x,
            y,
            w,
            h,
            source: vec![],
            options: WidgetOptions::default(),
        }
    }

    fn layout(widgets: Vec<Widget>) -> DashboardLayout {
        DashboardLayout {
            canvas: CanvasSpec {
                width: 1600,
                height: 900,
                background: None,
            },
            widgets,
            wires: vec![],
        }
    }

    #[test]
    fn to_canvas_et_snap() {
        let p = to_canvas((130.0, 250.0), (30.0, 50.0), (0.0, 0.0), 1.0);
        assert_eq!(p, (100.0, 200.0));
        // zoom 2 : un point client deux fois plus loin sur le canvas.
        let p = to_canvas((230.0, 250.0), (30.0, 50.0), (0.0, 0.0), 2.0);
        assert_eq!(p, (100.0, 100.0));
        assert_eq!(snap(103.0), 100.0);
        assert_eq!(snap(106.0), 110.0);
    }

    #[test]
    fn zoom_toward_cursor_ne_deplace_pas_le_point() {
        let (z, pan) = zoom_pan_towards(1.0, (0.0, 0.0), (200.0, 100.0), 1.25);
        assert!((z - 1.25).abs() < 1e-9);
        // Le point du document sous le curseur reste sous le curseur.
        let before = to_canvas((200.0, 100.0), (0.0, 0.0), (0.0, 0.0), 1.0);
        let after = to_canvas((200.0, 100.0), (0.0, 0.0), pan, z);
        assert!((before.0 - after.0).abs() < 1e-6);
        assert!((before.1 - after.1).abs() < 1e-6);
        let _ = layout(vec![]);
    }

    #[test]
    fn hit_et_resize() {
        let w = widget(100, 100, 240, 200);
        assert!(hit_test(&w, (110.0, 120.0)));
        assert!(!hit_test(&w, (90.0, 120.0)));
        assert!(resize_handle_at(&w, (340.0, 300.0), 6.0));
        assert!(!resize_handle_at(&w, (300.0, 300.0), 6.0));
    }

    #[test]
    fn accroches_aux_milieux_des_cotes() {
        let w = widget(100, 100, 240, 200);
        assert_eq!(anchor_point(&w, WireSide::Top), (220.0, 100.0));
        assert_eq!(anchor_point(&w, WireSide::Bottom), (220.0, 300.0));
        assert_eq!(anchor_point(&w, WireSide::Left), (100.0, 200.0));
        assert_eq!(anchor_point(&w, WireSide::Right), (340.0, 200.0));
    }

    #[test]
    fn coude_orthogonal_entre_deux_accroches() {
        let a = widget(100, 100, 240, 200);
        let mut b = widget(500, 100, 240, 200);
        b.id = "w2".into();
        let l = layout(vec![a, b]);
        let path = wire_path(&l, "w1", WireSide::Right, "w2", WireSide::Left).unwrap();
        assert!(path.starts_with("M 340.0 200.0"), "{path}");
        assert!(path.ends_with("L 500.0 200.0"), "{path}");
        assert!(wire_path(&l, "w1", WireSide::Right, "inconnu", WireSide::Left).is_none());
    }

    #[test]
    fn tailles_par_defaut_couvertes() {
        for kind in pnex_core::VIZ_WIDGET_TYPES {
            let (w, h) = default_size(kind);
            assert!(w >= pnex_core::WIDGET_MIN && h >= pnex_core::WIDGET_MIN);
        }
        let _ = SourceRef {
            role: "primary".into(),
            metric: "m".into(),
            device_id: "d".into(),
            window: "1h".into(),
            memory: None,
        };
    }
}
