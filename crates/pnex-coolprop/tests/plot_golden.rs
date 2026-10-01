//! Golden tests du moteur de diagrammes : dôme de saturation Water (p-h),
//! psychrométrique, cycle-points cohérents avec PropsSI.

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
    assert_eq!(dome.len(), 2, "2 branches de dôme attendues");
    for c in &dome {
        let span = c.points.last().unwrap()[0] - c.points.first().unwrap()[0];
        assert!(span.abs() > 1.0e5, "branche trop courte : {}", c.id);
    }

    // Chaleur latente près du point TRIPLE (premier point de la grille,
    // 274.5 K) : h_g − h_f ≈ 2 500 kJ/kg (IAPWS). Psat identique sur les
    // deux branches.
    let liq = dome.iter().find(|c| c.id == "sat_liq").unwrap();
    let vap = dome.iter().find(|c| c.id == "sat_vap").unwrap();
    let p_liq = liq.points.first().unwrap()[1];
    let p_vap = vap.points.first().unwrap()[1];
    assert!((p_liq - p_vap).abs() / p_liq < 0.02, "Psat divergentes");
    let latent = vap.points.first().unwrap()[0] - liq.points.first().unwrap()[0];
    assert!(
        (latent - 2_500_000.0).abs() < 30_000.0,
        "chaleur latente attendue ≈ 2 500 kJ/kg (point triple), eu {latent}"
    );
}

#[test]
fn psychrometrique_air_humide() {
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
    // La courbe saturée borne le W : W(20 °C sat) ≈ 0.0147 kg/kg.
    let sat = res
        .polylines
        .iter()
        .find(|c| c.id == "RH:1")
        .expect("courbe saturée");
    let w20 = sat
        .points
        .iter()
        .min_by(|a, b| (a[0] - 20.0).abs().total_cmp(&(b[0] - 20.0).abs()))
        .unwrap()[1];
    assert!(
        (w20 - 0.0147).abs() < 0.001,
        "W_sat(20 °C) ≈ 0.0147 kg/kg, eu {w20}"
    );
    // L'iso-RH 50 % reste sous la saturation.
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
    assert!(w50 < w20, "RH 50 % doit être sous la saturation");
}

#[test]
fn cycle_points_cohérents_avec_props_si() {
    // Point (400 K, 1 bar) : les props AbstractState convergent avec PropsSI.
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
fn ph_r410a_axes_et_traits_de_base() {
    // Retour utilisateur 2026-09-18 : p-h R410A illisible — auto-fit
    // linéaire → y_min négatif sur un axe log, et aucun trait sans
    // iso-lignes explicites.
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
        "axe log : y.min doit rester positif (eu min={})",
        res.y.min
    );
    assert!(res.y.log, "y du p-h doit être log");
    // Seuil bas = Psat au bas du dôme (fluide-adapté), pas le plancher
    // absolu de 100 Pa — les queues d'isothermes écrasaient le graphe
    // (retour 2026-09-18 : « 80.000 » en bas de tous les p-h).
    assert!(
        res.y.min > 1.0e3,
        "plancher attendu ≫ 100 Pa (eu min={})",
        res.y.min
    );
    // Traits de base sur pseudo-pur : iso-T générée, iso-Q ABSENTE
    // (CoolProp rejette 0<Q<1 sur R410A : « quality must be equal to 0
    // or 1 » — contrainte physique du pseudo-pur, pas un bug moteur).
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
    assert!(n_t >= 3, "au moins 3 isothermes attendues, eu {n_t}");
    assert_eq!(n_q, 0, "iso-Q impossible sur pseudo-pur R410A");
    // Le dôme culmine à Pcrit (le haut du dôme est dans le canvas).
    let p_max = res
        .polylines
        .iter()
        .filter(|c| c.id.starts_with("sat_"))
        .flat_map(|c| c.points.iter().map(|p| p[1]))
        .fold(f64::MIN, f64::max);
    assert!(
        p_max <= res.y.max && p_max >= res.y.min * 2.0,
        "dôme dans le canvas : p_max={p_max}, y=[{}, {}]",
        res.y.min,
        res.y.max
    );
}
