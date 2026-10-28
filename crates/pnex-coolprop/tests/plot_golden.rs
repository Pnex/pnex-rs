//! Golden tests of the diagram engine: Water saturation dome (p-h),
//! psychrometric, cycle-points consistent with PropsSI.

use pnex_coolprop::{
    compute_diagram, cycle_points, AxisRange, CyclePointInput, DiagramKind, DiagramRequest,
    IsolineSpec,
};

#[test]
fn dome_ph_water() {
    let req = DiagramRequest {
        fluid: "Water".into(),
        diagram: DiagramKind::Ph,
        x_range: None,
        y_range: None,
        isolines: vec![],
        n_points: Some(40),
        pressure: None,
    };
    let res = compute_diagram(&req).expect("diagram");
    let dome: Vec<_> = res
        .polylines
        .iter()
        .filter(|c| c.id == "sat_liq" || c.id == "sat_vap")
        .collect();
    assert_eq!(dome.len(), 2, "2 dome branches expected");
    for c in &dome {
        let span = c.points.last().unwrap()[0] - c.points.first().unwrap()[0];
        assert!(span.abs() > 1.0e5, "branch too short: {}", c.id);
    }

    // Latent heat near the TRIPLE point (first grid point,
    // 274.5 K): h_g − h_f ≈ 2 500 kJ/kg (IAPWS). Same Psat on both
    // branches.
    let liq = dome.iter().find(|c| c.id == "sat_liq").unwrap();
    let vap = dome.iter().find(|c| c.id == "sat_vap").unwrap();
    let p_liq = liq.points.first().unwrap()[1];
    let p_vap = vap.points.first().unwrap()[1];
    assert!((p_liq - p_vap).abs() / p_liq < 0.02, "Psat diverge");
    let latent = vap.points.first().unwrap()[0] - liq.points.first().unwrap()[0];
    assert!(
        (latent - 2_500_000.0).abs() < 30_000.0,
        "latent heat expected ≈ 2 500 kJ/kg (triple point), got {latent}"
    );
}

#[test]
fn psychrometric_humid_air() {
    let req = DiagramRequest {
        fluid: "Water".into(),
        diagram: DiagramKind::Psychro,
        x_range: Some(AxisRange {
            min: 0.0,
            max: 40.0,
        }),
        y_range: None,
        isolines: vec![IsolineSpec {
            param: "RH".into(),
            values: vec![0.5],
        }],
        n_points: Some(80),
        pressure: None,
    };
    let res = compute_diagram(&req).expect("psychro");
    // The saturated curve bounds W: W(20 °C sat) ≈ 0.0147 kg/kg.
    let sat = res
        .polylines
        .iter()
        .find(|c| c.id == "RH:1")
        .expect("saturated curve");
    let w20 = sat
        .points
        .iter()
        .min_by(|a, b| (a[0] - 20.0).abs().total_cmp(&(b[0] - 20.0).abs()))
        .unwrap()[1];
    assert!(
        (w20 - 0.0147).abs() < 0.001,
        "W_sat(20 °C) ≈ 0.0147 kg/kg, got {w20}"
    );
    // The 50 % iso-RH stays below saturation.
    let rh50 = res
        .polylines
        .iter()
        .find(|c| c.id == "RH:0.5")
        .expect("iso-RH 50 %");
    let w50 = rh50
        .points
        .iter()
        .min_by(|a, b| (a[0] - 20.0).abs().total_cmp(&(b[0] - 20.0).abs()))
        .unwrap()[1];
    assert!(w50 < w20, "RH 50 % must be below saturation");
}

#[test]
fn cycle_points_consistent_with_props_si() {
    // Point (400 K, 1 bar): AbstractState props converge with PropsSI.
    let pts = cycle_points(
        "Water",
        DiagramKind::Ph,
        &[CyclePointInput {
            label: "vapeur".into(),
            input_pair: "PT_INPUTS".into(),
            v1: 101_325.0,
            v2: 400.0,
        }],
        None,
    )
    .expect("cycle");
    let p0 = &pts[0];
    let h = pnex_coolprop::props_si("Hmass", "T", 400.0, "P", 101_325.0, "Water").unwrap();
    assert!(
        (p0.props["Hmass"] - h).abs() / h.abs() < 1e-6,
        "Hmass AbstractState vs PropsSI"
    );
    assert!((p0.coords[0] - h).abs() < 1.0, "coords.x = Hmass");
    assert!((p0.coords[1] - 101_325.0).abs() < 1.0, "coords.y = P");
}

#[test]
fn ph_r410a_axes_and_base_traces() {
    // User feedback 2026-09-18: R410A p-h unreadable — linear
    // auto-fit → negative y_min on a log axis, and no lines without
    // explicit iso-lines.
    let req = DiagramRequest {
        fluid: "R410A".into(),
        diagram: DiagramKind::Ph,
        x_range: None,
        y_range: None,
        isolines: vec![],
        n_points: Some(60),
        pressure: None,
    };
    let res = compute_diagram(&req).expect("diagram R410A");
    assert!(
        res.y.min > 0.0 && res.y.max > res.y.min,
        "log axis: y.min must stay positive (got min={})",
        res.y.min
    );
    assert!(res.y.log, "p-h y must be log");
    // Low threshold = Psat at the bottom of the dome (fluid-adapted), not the
    // absolute 100 Pa floor — isotherm tails crushed the graph
    // (feedback 2026-09-18: "80.000" at the bottom of every p-h).
    assert!(
        res.y.min > 1.0e3,
        "floor expected ≫ 100 Pa (got min={})",
        res.y.min
    );
    // Base lines on pseudo-pure: iso-T generated, iso-Q ABSENT
    // (CoolProp rejects 0<Q<1 on R410A: "quality must be equal to 0
    // or 1" — physical constraint of the pseudo-pure, not an engine bug).
    let n_t = res
        .polylines
        .iter()
        .filter(|c| c.id.starts_with("T:"))
        .count();
    let n_q = res
        .polylines
        .iter()
        .filter(|c| c.id.starts_with("Q:"))
        .count();
    assert!(n_t >= 3, "at least 3 isotherms expected, got {n_t}");
    assert_eq!(n_q, 0, "iso-Q impossible on pseudo-pure R410A");
    // The dome peaks at Pcrit (the top of the dome is inside the canvas).
    let p_max = res
        .polylines
        .iter()
        .filter(|c| c.id.starts_with("sat_"))
        .flat_map(|c| c.points.iter().map(|p| p[1]))
        .fold(f64::MIN, f64::max);
    assert!(
        p_max <= res.y.max && p_max >= res.y.min * 2.0,
        "dome inside the canvas: p_max={p_max}, y=[{}, {}]",
        res.y.min,
        res.y.max
    );
}
