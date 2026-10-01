use super::*;
use pnex_core::{
    thermo_default_unit, thermo_quantity, ThermoGroup, THERMO_GROUPS, THERMO_INPUT_PAIRS,
    THERMO_QUANTITIES,
};

const SELECT_CLASS: &str =
    "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white disabled:bg-gray-50 disabled:text-gray-400";

/// Writes `next` into the selected node; output wires follow their output
/// id and input annotations follow a renamed input key.
fn commit(cx: &mut EditorCx, mut cfg: Signal<CoolPropConfig>, next: CoolPropConfig) {
    let old = cfg.peek().clone();
    cfg.set(next.clone());
    let Some(id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| {
        state::rewire_coolprop(graph, &id, &old, &next);
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
            if let FlowNodeKind::CoolProp { config } = &mut node.kind {
                *config = next;
            }
        }
    });
}

/// Human label of a quantity (raw CoolProp name outside the catalogue).
fn quantity_name(id: &str) -> String {
    match thermo_quantity(id) {
        Some(q) => t!(q.label_key).to_string(),
        None => id.to_string(),
    }
}

fn group_label(group: ThermoGroup) -> String {
    match group {
        ThermoGroup::State => t!("flows-coolprop-group-state"),
        ThermoGroup::Saturation => t!("flows-coolprop-group-saturation"),
        ThermoGroup::Transport => t!("flows-coolprop-group-transport"),
        ThermoGroup::Fluid => t!("flows-coolprop-group-fluid"),
    }
    .to_string()
}

/// Checks/unchecks an output. Catalogue outputs keep the catalogue order
/// (stable ports); outputs outside the catalogue (legacy configs) stay first.
fn toggle_output(cfg: &mut CoolPropConfig, id: &str, on: bool) {
    let mut picked: Vec<String> = cfg.outputs.iter().filter(|o| *o != id).cloned().collect();
    if on {
        picked.push(id.to_string());
        let unit = thermo_default_unit(id);
        if !unit.is_empty() {
            cfg.output_units
                .entry(id.to_string())
                .or_insert_with(|| unit.to_string());
        }
    } else {
        cfg.output_units.remove(id);
    }
    let custom = picked
        .iter()
        .filter(|o| thermo_quantity(o).is_none())
        .cloned();
    let catalogue = THERMO_QUANTITIES
        .iter()
        .filter(|q| picked.iter().any(|o| o == q.id))
        .map(|q| q.id.to_string());
    cfg.outputs = custom.chain(catalogue).collect();
}

/// Unit picker of one quantity; quantities outside the catalogue are SI.
#[component]
fn UnitSelect(
    quantity: String,
    unit: String,
    disabled: bool,
    on_change: EventHandler<String>,
) -> Element {
    let Some(q) = thermo_quantity(&quantity) else {
        return rsx! {
            select { class: SELECT_CLASS, disabled: true,
                option { "SI" }
            }
        };
    };
    let current = if q.dimension.unit(&unit).is_some() {
        unit.clone()
    } else {
        String::new()
    };
    rsx! {
        select {
            class: SELECT_CLASS,
            disabled,
            value: "{current}",
            onchange: move |e| on_change.call(e.value()),
            if current.is_empty() {
                option { key: "si-{quantity}", value: "", selected: true, "SI" }
            }
            for u in q.dimension.units().iter() {
                option {
                    key: "{quantity}-{u.id}",
                    value: "{u.id}",
                    selected: current == u.id,
                    "{u.symbol}"
                }
            }
        }
    }
}

/// Form of the `pnex-coolprop` node: fluid picker (org mixtures + CoolProp
/// fluids), a pair of known quantities with their units (one input row
/// each on the canvas), and checkable outputs with their units (one output
/// port each, plus the all-outputs port 0).
#[component]
pub(super) fn CoolPropForm(mut cx: EditorCx, initial: CoolPropConfig, can_write: bool) -> Element {
    let cfg = use_signal(move || initial.clone());
    let fluid_choices = use_resource(|| async move { crate::api::thermo::fluids().await.ok() });
    let choices = fluid_choices
        .read()
        .as_ref()
        .cloned()
        .flatten()
        .unwrap_or_default();
    let c = cfg.read().clone();

    let fluid_value = if c.fluid_label.is_empty() {
        c.fluid_spec.clone()
    } else {
        c.fluid_label.clone()
    };
    let in_list = choices.mixtures.contains(&fluid_value) || choices.fluids.contains(&fluid_value);
    let specs = choices.mixture_specs.clone();
    let mixtures: Vec<(String, bool)> = choices
        .mixtures
        .iter()
        .map(|m| (m.clone(), choices.mixture_specs.contains_key(m)))
        .collect();

    let pair_value = format!("{}|{}", c.input1, c.input2);
    let pair_known = THERMO_INPUT_PAIRS
        .iter()
        .any(|(a, b)| *a == c.input1 && *b == c.input2);
    let pairs: Vec<(String, String)> = THERMO_INPUT_PAIRS
        .iter()
        .map(|(a, b)| {
            (
                format!("{a}|{b}"),
                format!("{} + {}", quantity_name(a), quantity_name(b)),
            )
        })
        .collect();
    let inputs = vec![
        (0usize, c.input1.clone(), c.unit1.clone()),
        (1usize, c.input2.clone(), c.unit2.clone()),
    ];

    // Output rows grouped for display: (id, label, checked, unit).
    let custom_outputs: Vec<String> = c
        .outputs
        .iter()
        .filter(|o| thermo_quantity(o).is_none())
        .cloned()
        .collect();
    let groups: Vec<(String, Vec<(String, String, bool, String)>)> = THERMO_GROUPS
        .iter()
        .map(|g| {
            let rows = THERMO_QUANTITIES
                .iter()
                .filter(|q| q.group == *g)
                .map(|q| {
                    let checked = c.outputs.iter().any(|o| o == q.id);
                    (
                        q.id.to_string(),
                        t!(q.label_key).to_string(),
                        checked,
                        c.output_unit(q.id).to_string(),
                    )
                })
                .collect();
            (group_label(*g), rows)
        })
        .collect();

    rsx! {
        div { class: "space-y-4",
            // Fluid / mixture picker.
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-coolprop-fluid")} }
                select {
                    class: SELECT_CLASS,
                    disabled: !can_write,
                    value: "{fluid_value}",
                    onchange: move |e| {
                        let v = e.value();
                        let mut next = cfg.peek().clone();
                        next.fluid_spec = specs.get(&v).cloned().unwrap_or_else(|| v.clone());
                        next.fluid_label = v;
                        commit(&mut cx, cfg, next);
                    },
                    if !in_list {
                        option { key: "current-{fluid_value}", value: "{fluid_value}", selected: true, "{fluid_value}" }
                    }
                    if !mixtures.is_empty() {
                        optgroup { label: t!("insp-thermo-fluid-org").to_string(),
                            for (m, convertible) in mixtures.iter() {
                                option {
                                    key: "mix-{m}",
                                    value: "{m}",
                                    disabled: !*convertible,
                                    selected: fluid_value == *m,
                                    if *convertible {
                                        "{m}"
                                    } else {
                                        "{m} — {t!(\"flows-coolprop-mixture-unavailable\")}"
                                    }
                                }
                            }
                        }
                    }
                    optgroup { label: t!("insp-thermo-fluid-coolprop").to_string(),
                        for f in choices.fluids.iter() {
                            option { key: "fluid-{f}", value: "{f}", selected: fluid_value == *f, "{f}" }
                        }
                    }
                }
            }

            // Known quantities: the pair, then one unit per input row.
            div { class: "space-y-2",
                span { class: "text-xs font-medium text-gray-500 block", {t!("flows-coolprop-inputs")} }
                select {
                    class: SELECT_CLASS,
                    disabled: !can_write,
                    value: "{pair_value}",
                    onchange: move |e| {
                        let v = e.value();
                        let Some((a, b)) = v.split_once('|') else { return; };
                        let mut next = cfg.peek().clone();
                        next.input1 = a.to_string();
                        next.input2 = b.to_string();
                        next.unit1 = thermo_default_unit(a).to_string();
                        next.unit2 = thermo_default_unit(b).to_string();
                        commit(&mut cx, cfg, next);
                    },
                    if !pair_known {
                        option { key: "current-{pair_value}", value: "{pair_value}", selected: true,
                            "{c.input1} + {c.input2}"
                        }
                    }
                    for (value, label) in pairs.iter() {
                        option { key: "{value}", value: "{value}", selected: pair_value == *value, "{label}" }
                    }
                }
                for (idx, qid, unit) in inputs.into_iter() {
                    div { key: "in-{idx}-{qid}", class: "flex items-center gap-2",
                        span { class: "flex-1 text-sm text-gray-700 truncate", {quantity_name(&qid)} }
                        div { class: "w-28 shrink-0",
                            UnitSelect {
                                quantity: qid.clone(),
                                unit: unit.clone(),
                                disabled: !can_write,
                                on_change: move |u: String| {
                                    let mut next = cfg.peek().clone();
                                    if idx == 0 {
                                        next.unit1 = u;
                                    } else {
                                        next.unit2 = u;
                                    }
                                    commit(&mut cx, cfg, next);
                                },
                            }
                        }
                    }
                }
                p { class: "text-xs text-gray-400", {t!("flows-coolprop-inputs-help")} }
            }

            // Outputs, grouped; a checked row shows its unit picker.
            div { class: "space-y-2",
                span { class: "text-xs font-medium text-gray-500 block", {t!("flows-coolprop-outputs")} }
                for id in custom_outputs.into_iter() {
                    label { key: "custom-{id}", class: "flex items-center gap-2 text-sm text-gray-700",
                        input {
                            r#type: "checkbox",
                            checked: true,
                            disabled: !can_write,
                            onchange: move |_| {
                                let mut next = cfg.peek().clone();
                                toggle_output(&mut next, &id, false);
                                commit(&mut cx, cfg, next);
                            },
                        }
                        span { class: "font-mono", "{id}" }
                        span { class: "text-xs text-gray-400", {t!("flows-coolprop-custom-output")} }
                    }
                }
                for (group, rows) in groups.into_iter() {
                    div { key: "group-{group}", class: "space-y-1",
                        span { class: "text-[11px] uppercase tracking-wide text-gray-400 block pt-1", "{group}" }
                        for (id, label, checked, unit) in rows.into_iter() {
                            div { key: "out-{id}", class: "flex items-center gap-2 min-h-[32px]",
                                label { class: "flex-1 flex items-center gap-2 text-sm text-gray-700 min-w-0",
                                    input {
                                        r#type: "checkbox",
                                        checked,
                                        disabled: !can_write,
                                        onchange: {
                                            let id = id.clone();
                                            move |e: FormEvent| {
                                                let on = e.checked();
                                                let mut next = cfg.peek().clone();
                                                toggle_output(&mut next, &id, on);
                                                commit(&mut cx, cfg, next);
                                            }
                                        },
                                    }
                                    span { class: "truncate", "{label}" }
                                }
                                if checked {
                                    div { class: "w-28 shrink-0",
                                        UnitSelect {
                                            quantity: id.clone(),
                                            unit: unit.clone(),
                                            disabled: !can_write,
                                            on_change: {
                                                let id = id.clone();
                                                move |u: String| {
                                                    let mut next = cfg.peek().clone();
                                                    next.output_units.insert(id.clone(), u);
                                                    commit(&mut cx, cfg, next);
                                                }
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700 pt-1",
                    input {
                        r#type: "checkbox",
                        checked: c.include_phase,
                        disabled: !can_write,
                        onchange: move |e| {
                            let mut next = cfg.peek().clone();
                            next.include_phase = e.checked();
                            commit(&mut cx, cfg, next);
                        },
                    }
                    {t!("flows-coolprop-phase")}
                }
                p { class: "text-xs text-gray-400", {t!("flows-coolprop-outputs-help")} }
            }

            // Advanced: input row names (also object payload keys).
            details { class: "text-sm",
                summary { class: "text-xs text-gray-500 cursor-pointer", {t!("flows-coolprop-advanced")} }
                div { class: "grid grid-cols-2 gap-2 pt-2",
                    label { class: "block",
                        span { class: "text-xs text-gray-500 mb-1 block", {t!("flows-coolprop-v1key")} }
                        input {
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                            value: "{c.v1_key}",
                            disabled: !can_write,
                            onchange: move |e| {
                                let mut next = cfg.peek().clone();
                                next.v1_key = e.value().trim().to_string();
                                commit(&mut cx, cfg, next);
                            },
                        }
                    }
                    label { class: "block",
                        span { class: "text-xs text-gray-500 mb-1 block", {t!("flows-coolprop-v2key")} }
                        input {
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                            value: "{c.v2_key}",
                            disabled: !can_write,
                            onchange: move |e| {
                                let mut next = cfg.peek().clone();
                                next.v2_key = e.value().trim().to_string();
                                commit(&mut cx, cfg, next);
                            },
                        }
                    }
                }
                p { class: "text-xs text-gray-400 pt-1", {t!("flows-coolprop-advanced-help")} }
            }
        }
    }
}
