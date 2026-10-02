//! Panneau d'édition du widget `thermo_chart` (inspecteur) — type de
//! diagramme, fluide/mélange, points du cycle (chaque point = 2 sources
//! métrique×device). Chaque mutation ré-aligne `widget.source` sur les
//! sources des points (le series-batch agrège `source`, pas `options` —
//! règle de validation `thermo_source_missing`).
//!
//! Fichier séparé de `inspector.rs` (qui garde ses helpers privés) ; les
//! mutations passent par `cx.layout.with_mut` comme ailleurs.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{
    SourceRef, ThermoChartOptions, ThermoCyclePoint, ThermoPressureUnit, ThermoSource, Widget,
};

use super::state;
use super::EditorCx;

/// Réaligne `widget.source` sur les sources des points du cycle.
fn sync_thermo_sources(w: &mut Widget) {
    let Some(t) = w.options.thermo.as_ref() else {
        return;
    };
    w.source = t
        .points
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            [
                SourceRef {
                    role: format!("cycle{i}.v1"),
                    metric: p.v1.metric.clone(),
                    device_id: p.v1.device_id.clone(),
                    window: "1h".into(),
                    memory: p.v1.memory.clone(),
                },
                SourceRef {
                    role: format!("cycle{i}.v2"),
                    metric: p.v2.metric.clone(),
                    device_id: p.v2.device_id.clone(),
                    window: "1h".into(),
                    memory: p.v2.memory.clone(),
                },
            ]
        })
        .collect();
}

/// Patche les options thermo du widget sélectionné puis ré-aligne les
/// sources.
fn patch_thermo(
    cx: &mut EditorCx,
    widget_id: &str,
    f: impl FnOnce(&mut ThermoChartOptions) + 'static,
) {
    cx.layout.with_mut(|l| {
        if let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) {
            if let Some(t) = w.options.thermo.as_mut() {
                f(t);
            }
            sync_thermo_sources(w);
        }
    });
}

#[component]
pub fn ThermoPanel(
    mut cx: EditorCx,
    widget_id: String,
    can_write: bool,
    metrics: BTreeMap<String, Vec<String>>,
    memory: BTreeMap<String, Vec<String>>,
) -> Element {
    // Pickers fluide : mélanges de l'org + fluides CoolProp (plus de
    // saisie libre — retour 2026-09-18). Le fetch vit dans le panneau :
    // remount par sélection = 1 GET léger (fluids_list est côté serveur
    // un appel C statique + une requête SQL), l'hisser coûterait un
    // forage de props à travers InspectorBody→widget_panel. Hook appelé
    // AVANT les early-returns (ordre des hooks dioxus).
    let fluid_choices = use_resource(|| async move { crate::api::thermo::fluids().await.ok() });
    let choices = fluid_choices
        .read()
        .as_ref()
        .cloned()
        .flatten()
        .unwrap_or_default();
    let w = state::find_widget(&cx.layout.cloned(), &widget_id);
    let Some(w) = w else {
        return rsx! {};
    };
    let Some(thermo) = w.options.thermo.clone() else {
        return rsx! {};
    };
    let metric_names: Vec<String> = metrics.keys().cloned().collect();
    // Un clone de capture par handler `move` (le corps du composant est un
    // contexte FnMut rsx — cf. école library.rs).
    let wid_diag = widget_id.clone();
    let wid_fluid = widget_id.clone();
    let wid_add = widget_id.clone();
    let add_metrics = metrics.clone();
    let add_names = metric_names.clone();
    let in_list =
        choices.mixtures.contains(&thermo.fluid) || choices.fluids.contains(&thermo.fluid);
    let is_psychro = thermo.diagram == "psychro";
    // Un clone d'id par handler `move` (école library.rs).
    let wid_round = widget_id.clone();
    let wid_unit = widget_id.clone();
    let round_value = match thermo.axis_decimals {
        Some(n) => n.to_string(),
        None => "auto".to_string(),
    };
    let unit_value = if thermo.pressure_unit == ThermoPressureUnit::Bar {
        "bar"
    } else {
        "pa"
    };

    rsx! {
        div { class: "space-y-3",
            // Diagramme
            span { class: "block text-xs font-medium text-gray-500", {t!("insp-thermo-diagram")} }
            select {
                class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                disabled: !can_write,
                value: "{thermo.diagram}",
                onchange: move |e| {
                    let v = e.value();
                    patch_thermo(&mut cx, &wid_diag, move |t| t.diagram = v);
                },
                option { value: "ph", selected: thermo.diagram == "ph", {t!("insp-thermo-diagram-ph")} }
                option { value: "ts", selected: thermo.diagram == "ts", {t!("insp-thermo-diagram-ts")} }
                option { value: "psychro", selected: thermo.diagram == "psychro",
                    {t!("insp-thermo-diagram-psychro")}
                }
            }
            // Fluide / mélange : picker (psychro = air humide figé, pas de
            // fluide à choisir — HAPropsSI n'a pas de paramètre fluide).
            span { class: "block text-xs font-medium text-gray-500", {t!("insp-thermo-fluid")} }
            if is_psychro {
                span { class: "block text-xs italic text-gray-400", {t!("insp-thermo-fluid-air")} }
            } else {
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                    disabled: !can_write,
                    value: "{thermo.fluid}",
                    onchange: move |e| {
                        let v = e.value();
                        patch_thermo(&mut cx, &wid_fluid, move |t| t.fluid = v);
                    },
                    if thermo.fluid.is_empty() {
                        option {
                            key: "{thermo.fluid}",
                            value: "",
                            disabled: true,
                            {t!("insp-thermo-fluid-pick")}
                        }
                    }
                    if !in_list && !thermo.fluid.is_empty() {
                        // Valeur persistée hors catalogues (mélange supprimé,
                        // spec inline) : visible et re-sélectionnable.
                        option { key: "{thermo.fluid}", value: "{thermo.fluid}", "{thermo.fluid}" }
                    }
                    if !choices.mixtures.is_empty() {
                        optgroup { label: t!("insp-thermo-fluid-org").to_string(),
                            for m in &choices.mixtures {
                                option {
                                    key: "{m}",
                                    value: "{m}",
                                    selected: thermo.fluid == *m,
                                    "{m}"
                                }
                            }
                        }
                    }
                    optgroup { label: t!("insp-thermo-fluid-coolprop").to_string(),
                        for f in &choices.fluids {
                            option {
                                key: "{f}",
                                value: "{f}",
                                selected: thermo.fluid == *f,
                                "{f}"
                            }
                        }
                    }
                }
            }
            // Arrondi des axes (auto = significatif selon l'ordre de
            // grandeur).
            span { class: "block text-xs font-medium text-gray-500", {t!("insp-thermo-rounding")} }
            select {
                class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                disabled: !can_write,
                value: "{round_value}",
                onchange: move |e| {
                    let v = e.value();
                    patch_thermo(
                        &mut cx,
                        &wid_round,
                        move |t| {
                            t.axis_decimals = if v == "auto" { None } else { v.parse::<u8>().ok() };
                        },
                    );
                },
                option {
                    key: "auto {round_value}",
                    value: "auto",
                    selected: thermo.axis_decimals.is_none(),
                    {t!("insp-thermo-rounding-auto")}
                }
                for n in 0u8..4 {
                    option {
                        key: "{n}",
                        value: "{n}",
                        selected: thermo.axis_decimals == Some(n),
                        "{n}"
                    }
                }
            }
            // Unité d'affichage de la pression (p-h seulement) — absolu
            // toujours : CoolProp calcule en Pa absolus, la conversion est
            // cosmétique (÷1e5).
            if thermo.diagram == "ph" {
                span { class: "block text-xs font-medium text-gray-500",
                    {t!("insp-thermo-pressure-unit")}
                }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white",
                    disabled: !can_write,
                    value: "{unit_value}",
                    onchange: move |e| {
                        let v = e.value();
                        patch_thermo(
                            &mut cx,
                            &wid_unit,
                            move |t| {
                                t.pressure_unit = if v == "bar" {
                                    ThermoPressureUnit::Bar
                                } else {
                                    ThermoPressureUnit::Pa
                                };
                            },
                        );
                    },
                    option {
                        key: "pa {unit_value}",
                        value: "pa",
                        selected: thermo.pressure_unit == ThermoPressureUnit::Pa,
                        {t!("insp-thermo-unit-pa")}
                    }
                    option {
                        key: "bar {unit_value}",
                        value: "bar",
                        selected: thermo.pressure_unit == ThermoPressureUnit::Bar,
                        {t!("insp-thermo-unit-bar")}
                    }
                }
            }
            // Points du cycle
            span { class: "block text-xs font-medium text-gray-500", {t!("insp-thermo-points")} }
            for pi in 0..thermo.points.len() {
                {
                    let pt = thermo.points[pi].clone();
                    // Un clone d'id par handler `move` (école library.rs —
                    // le corps du for rsx est une closure FnMut).
                    let wid_label = widget_id.clone();
                    let wid_del = widget_id.clone();
                    rsx! {
                        div { key: "{pi}", class: "rounded border border-gray-200 p-2 space-y-1",
                            div { class: "flex gap-1 items-center",
                                input {
                                    class: "flex-1 px-2 py-1 border border-gray-300 rounded text-xs",
                                    placeholder: t!("insp-thermo-label-placeholder"),
                                    value: "{pt.label}",
                                    disabled: !can_write,
                                    oninput: move |e| {
                                        let v = e.value();
                                        let wid = wid_label.clone();
                                        patch_thermo(
                                            &mut cx,
                                            &wid,
                                            move |t| {
                                                if let Some(p) = t.points.get_mut(pi) {
                                                    p.label = v;
                                                }
                                            },
                                        );
                                    },
                                }
                                button {
                                    class: "px-1 text-xs text-red-600 hover:bg-red-50 rounded",
                                    onclick: move |_| {
                                        let wid = wid_del.clone();
                                        patch_thermo(
                                            &mut cx,
                                            &wid,
                                            move |t| {
                                                if pi < t.points.len() {
                                                    t.points.remove(pi);
                                                }
                                            },
                                        );
                                    },
                                    "×"
                                }
                            }
                            div { class: "grid grid-cols-2 gap-1",
                                ThermoSlotPicker {
                                    cx,
                                    widget_id: widget_id.clone(),
                                    point: pi,
                                    slot: 1,
                                    src: pt.v1.clone(),
                                    metrics: metrics.clone(),
                                    memory: memory.clone(),
                                    can_write,
                                }
                                ThermoSlotPicker {
                                    cx,
                                    widget_id: widget_id.clone(),
                                    point: pi,
                                    slot: 2,
                                    src: pt.v2.clone(),
                                    metrics: metrics.clone(),
                                    memory: memory.clone(),
                                    can_write,
                                }
                            }
                        }
                    }
                }
            }
            button {
                class: "px-2 py-1 text-xs text-blue-600 hover:bg-blue-50 rounded",
                disabled: !can_write,
                onclick: move |_| {
                    let add_names = add_names.clone();
                    let add_metrics = add_metrics.clone();
                    patch_thermo(
                        &mut cx,
                        &wid_add,
                        move |t| {
                            let default_metric = add_names.first().cloned().unwrap_or_default();
                            let default_device = add_metrics
                                .get(&default_metric)
                                .and_then(|d| d.first().cloned())
                                .unwrap_or_default();
                            t.points
                                .push(ThermoCyclePoint {
                                    label: format!("P{}", t.points.len() + 1),
                                    input_pair: "PT_INPUTS".into(),
                                    v1: ThermoSource {
                                        metric: default_metric.clone(),
                                        device_id: default_device.clone(),
                                        memory: None,
                                    },
                                    v2: ThermoSource {
                                        metric: add_names.get(1).cloned().unwrap_or(default_metric),
                                        device_id: default_device,
                                        memory: None,
                                    },
                                });
                        },
                    );
                },
                {t!("insp-thermo-add-point")}
            }
        }
    }
}

/// Parses a memory option value (`mem:key` / `mem:key#field`).
fn parse_memory_option(v: &str) -> Option<pnex_core::memory::MemoryRef> {
    let rest = v.strip_prefix("mem:")?;
    let (key, field) = rest.split_once('#').unwrap_or((rest, ""));
    Some(pnex_core::memory::MemoryRef {
        key: key.to_string(),
        field: field.to_string(),
    })
}

/// Memory options of a picker: one per numeric field of each live key
/// (`key` alone for a scalar value), as (value, label).
pub(super) fn memory_options(memory: &BTreeMap<String, Vec<String>>) -> Vec<(String, String)> {
    memory
        .iter()
        .flat_map(|(key, fields)| {
            fields.iter().map(move |f| {
                let r = pnex_core::memory::MemoryRef {
                    key: key.clone(),
                    field: f.clone(),
                };
                let label = if f.is_empty() {
                    key.clone()
                } else {
                    format!("{key} › {f}")
                };
                (r.series_key(), label)
            })
        })
        .collect()
}

/// One source slot (v1/v2) of a cycle point: telemetry metric + device, or
/// a memory value (the device cell then shows a memory badge).
#[component]
fn ThermoSlotPicker(
    mut cx: EditorCx,
    widget_id: String,
    point: usize,
    slot: u8,
    src: ThermoSource,
    metrics: BTreeMap<String, Vec<String>>,
    memory: BTreeMap<String, Vec<String>>,
    can_write: bool,
) -> Element {
    let current = src.series_key();
    let mem_options = memory_options(&memory);
    let stale_memory = src.memory.is_some() && !mem_options.iter().any(|(v, _)| *v == current);
    let devices: Vec<String> = metrics.get(&src.metric).cloned().unwrap_or_default();
    let metric_names: Vec<String> = metrics.keys().cloned().collect();
    let wid_m = widget_id.clone();
    let wid_d = widget_id;
    let patch_slot = move |t: &mut ThermoChartOptions, f: &dyn Fn(&mut ThermoSource)| {
        if let Some(p) = t.points.get_mut(point) {
            f(if slot == 1 { &mut p.v1 } else { &mut p.v2 });
        }
    };
    rsx! {
        select {
            class: "px-1 py-1 border border-gray-300 rounded text-xs bg-white",
            disabled: !can_write,
            onchange: move |e| {
                let v = e.value();
                patch_thermo(
                    &mut cx,
                    &wid_m,
                    move |t| {
                        patch_slot(
                            t,
                            &|s: &mut ThermoSource| match parse_memory_option(&v) {
                                Some(r) => {
                                    s.memory = Some(r.clone());
                                    s.metric.clear();
                                    s.device_id.clear();
                                }
                                None => {
                                    s.memory = None;
                                    s.metric = v.clone();
                                }
                            },
                        );
                    },
                );
            },
            for m in &metric_names {
                option {
                    key: "{m}",
                    value: "{m}",
                    selected: src.memory.is_none() && src.metric == *m,
                    "{m}"
                }
            }
            if !mem_options.is_empty() || stale_memory {
                optgroup { label: t!("insp-memory").to_string(),
                    for (value, label) in mem_options.clone() {
                        option {
                            key: "{value}",
                            value: "{value}",
                            selected: current == value,
                            "{label}"
                        }
                    }
                    if stale_memory {
                        option {
                            key: "{current}",
                            value: "{current}",
                            selected: true,
                            "{current}"
                        }
                    }
                }
            }
        }
        if src.memory.is_some() {
            span { class: "px-1 py-1 text-xs text-green-700 bg-green-50 rounded border border-green-200 truncate",
                {t!("insp-memory-badge")}
            }
        } else {
            select {
                class: "px-1 py-1 border border-gray-300 rounded text-xs bg-white",
                disabled: !can_write,
                onchange: move |e| {
                    let v = e.value();
                    patch_thermo(
                        &mut cx,
                        &wid_d,
                        move |t| {
                            patch_slot(t, &|s: &mut ThermoSource| s.device_id = v.clone());
                        },
                    );
                },
                for d in &devices {
                    option {
                        key: "{d}",
                        value: "{d}",
                        selected: src.device_id == *d,
                        "{d}"
                    }
                }
            }
        }
    }
}
