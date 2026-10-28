use super::*;

fn graph_with_two() -> FlowGraph {
    FlowGraph {
        nodes: vec![
            make_node("n1", PaletteKind::Inject, Position { x: 0.0, y: 0.0 }),
            make_node("n2", PaletteKind::Debug, Position { x: 200.0, y: 0.0 }),
        ],
    }
}

#[test]
fn ids_unique_even_after_delete() {
    let mut graph = graph_with_two();
    assert_eq!(next_node_id(&graph), "n3");
    remove_node(&mut graph, "n2");
    // n2 deleted: the id is reusable, no conflict possible.
    assert_eq!(next_node_id(&graph), "n2");
}

/// inject → notify, one annotation per stamped var; re-stamp of a
/// trimmed template: the removed var takes its wire (never a shift onto
/// the neighbouring var), the fan-out source keeps its wire if it still
/// feeds another var.
fn notify_wired(vars: &[&str]) -> FlowGraph {
    let mut graph = FlowGraph {
        nodes: vec![
            make_node("i1", PaletteKind::Inject, Position { x: 0.0, y: 0.0 }),
            make_node("nt", PaletteKind::PnexNotify, Position { x: 300.0, y: 0.0 }),
        ],
    };
    if let FlowNodeKind::PnexNotify { config } = &mut graph.nodes[1].kind {
        config.template_vars = vars.iter().map(|s| (*s).to_string()).collect();
    }
    graph
}

#[test]
fn notify_set_input_target_keeps_and_prunes() {
    let mut graph = notify_wired(&["seuil", "value"]);
    // The real drop first sets the runtime wire (add_target), then
    // the anchor annotation.
    add_target(&mut graph, "i1", 0, "nt");
    // Guard: only a stamped var is a valid anchor.
    set_input_target(&mut graph, "i1", 0, "nt", "seuil".into());
    set_input_target(&mut graph, "i1", 0, "nt", "phantom".into());
    let inputs = |g: &FlowGraph| -> Vec<(String, String, usize)> {
        g.nodes[1]
            .inputs
            .iter()
            .map(|w| (w.pin.clone(), w.from.clone(), w.from_port))
            .collect()
    };
    assert_eq!(
        inputs(&graph),
        vec![("seuil".to_string(), "i1".to_string(), 0)]
    );
    // Two sources on distinct vars: fan-in OK (a single
    // annotation per var, the runtime wire stays shared).
    set_input_target(&mut graph, "i1", 0, "nt", "value".into());
    assert_eq!(inputs(&graph).len(), 2);
    // The permanent `trigger` gate row is a valid anchor too.
    set_input_target(&mut graph, "i1", 0, "nt", "trigger".into());
    assert_eq!(inputs(&graph).len(), 3);
    // Prune of the vars not surviving the re-stamp (here "value"):
    // the annotation goes, the runtime wire stays as long as the source
    // still feeds another var ("seuil").
    prune_notify_var_anchors(&mut graph, "nt", &["seuil".into()]);
    assert_eq!(
        inputs(&graph),
        vec![
            ("seuil".to_string(), "i1".to_string(), 0),
            ("trigger".to_string(), "i1".to_string(), 0)
        ]
    );
    assert_eq!(
        wiring_of_source(&graph, "i1"),
        vec![(0, vec!["nt".to_string()])]
    );
    // Last var pruned: the runtime wire goes away with the annotation —
    // but the `trigger` gate annotation survives every re-pick.
    prune_notify_var_anchors(&mut graph, "nt", &[] as &[String]);
    assert_eq!(
        inputs(&graph),
        vec![("trigger".to_string(), "i1".to_string(), 0)]
    );
    assert_eq!(
        wiring_of_source(&graph, "i1"),
        vec![(0, vec!["nt".to_string()])]
    );
}

fn wiring_of_source(graph: &FlowGraph, id: &str) -> Vec<(usize, Vec<String>)> {
    graph
        .nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| {
            n.outputs
                .iter()
                .map(|w| (w.port, w.targets.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// 3-pin reader wired to 3 debug nodes + the device-name port,
/// scenario (G15/G16/VP → two displays): a wire follows its pin, a
/// removed pin takes its wire, never a shift onto the neighbor port.
fn device_read_wired() -> FlowGraph {
    let mut graph = FlowGraph {
        nodes: vec![
            make_node("r", PaletteKind::DeviceRead, Position { x: 0.0, y: 0.0 }),
            make_node("d15", PaletteKind::Debug, Position { x: 300.0, y: 0.0 }),
            make_node("d16", PaletteKind::Debug, Position { x: 300.0, y: 60.0 }),
            make_node("dvp", PaletteKind::Debug, Position { x: 300.0, y: 120.0 }),
            make_node("dt", PaletteKind::Debug, Position { x: 300.0, y: 180.0 }),
        ],
    };
    if let FlowNodeKind::DeviceRead { config } = &mut graph.nodes[0].kind {
        config.device_id = "d".into();
        config.pins = vec!["G15".into(), "G16".into(), "VP".into()];
    }
    add_target(&mut graph, "r", 0, "d15");
    add_target(&mut graph, "r", 1, "d16");
    add_target(&mut graph, "r", 2, "dvp");
    add_target(&mut graph, "r", 3, "dt");
    graph
}

fn wiring_of(graph: &FlowGraph) -> Vec<(usize, Vec<String>)> {
    graph.nodes[0]
        .outputs
        .iter()
        .map(|w| (w.port, w.targets.clone()))
        .collect()
}

#[test]
fn removed_pin_takes_its_wire_no_shift() {
    let mut graph = device_read_wired();
    // G16 unchecked: VP used to inherit the wire under the old behavior.
    rewire_ports_by_label(
        &mut graph,
        "r",
        &["G15".into(), "G16".into(), "VP".into()],
        &["G15".into(), "VP".into()],
    );
    assert_eq!(
        wiring_of(&graph),
        vec![
            (0, vec!["d15".to_string()]),
            (1, vec!["dvp".to_string()]),
            (2, vec!["dt".to_string()]),
        ]
    );
}

#[test]
fn added_pin_steals_no_wire() {
    let mut graph = device_read_wired();
    // G12 added before G15: the new port is born unwired, existing
    // wires follow their label.
    rewire_ports_by_label(
        &mut graph,
        "r",
        &["G15".into(), "G16".into(), "VP".into()],
        &["G12".into(), "G15".into(), "G16".into(), "VP".into()],
    );
    assert_eq!(
        wiring_of(&graph),
        vec![
            (1, vec!["d15".to_string()]),
            (2, vec!["d16".to_string()]),
            (3, vec!["dvp".to_string()]),
            (4, vec!["dt".to_string()]),
        ]
    );
}

#[test]
fn device_change_keeps_only_device_name_port() {
    let mut graph = device_read_wired();
    // Device replaced: pins.clear() — pin wires die, the device-name
    // port survives (rewired first, the only remaining port).
    rewire_ports_by_label(
        &mut graph,
        "r",
        &["G15".into(), "G16".into(), "VP".into()],
        &[],
    );
    assert_eq!(wiring_of(&graph), vec![(0, vec!["dt".to_string()])]);
}

/// Function node "f" with declared inputs `a`/`b` and outputs `x`/`y`,
/// wired from two sources: src1→(a,x), src2→(b,y).
fn function_graph_two_by_two() -> FlowGraph {
    let inp = |name: &str| pnex_core::FunctionInput {
        name: name.into(),
        ty: pnex_core::FunctionType::Number,
        default: None,
        desc: None,
    };
    let out = |name: &str| pnex_core::FunctionOutput {
        name: name.into(),
        ty: pnex_core::FunctionType::Number,
        desc: None,
    };
    let mut graph = FlowGraph {
        nodes: vec![
            make_node("src1", PaletteKind::Inject, Position { x: 0.0, y: 0.0 }),
            make_node("src2", PaletteKind::Value, Position { x: 0.0, y: 100.0 }),
            make_node("f", PaletteKind::Function, Position { x: 300.0, y: 50.0 }),
        ],
    };
    if let FlowNodeKind::PnexFunction { config } = &mut graph.nodes[2].kind {
        config.function_id = 7;
        config.function_name = "f".into();
        config.version_number = 1;
        config.inputs = vec![inp("a"), inp("b")];
        config.outputs = vec![out("x"), out("y")];
    }
    add_target(&mut graph, "src1", 0, "f");
    set_input_target(&mut graph, "src1", 0, "f", "a".into());
    add_target(&mut graph, "src2", 0, "f");
    set_input_target(&mut graph, "src2", 0, "f", "b".into());
    graph
}

#[test]
fn function_input_annotations_one_wire_per_input() {
    let mut graph = function_graph_two_by_two();
    // One wire per input row; a re-drop takes the row over.
    set_input_target(&mut graph, "src1", 0, "f", "b".into());
    let on_b: Vec<&str> = graph.nodes[2]
        .inputs
        .iter()
        .filter(|w| w.pin == "b")
        .map(|w| w.from.as_str())
        .collect();
    assert_eq!(on_b, vec!["src1"]);
    // Fan-out keeps sharing the runtime wire while another input is fed.
    cut_input_wire(&mut graph, "src1", 0, "f", "b");
    let runtime = graph.nodes[0]
        .outputs
        .iter()
        .flat_map(|w| &w.targets)
        .any(|t| t == "f");
    assert!(runtime, "runtime wire must survive while a is fed");
}

#[test]
fn function_output_wires_follow_names() {
    let mut graph = function_graph_two_by_two();
    // Output x (port 0) wired toward src2 as a stand-in target.
    add_target(&mut graph, "f", 0, "src2");
    // Rebase: outputs become y,x (swapped) — wires follow the names.
    let new_outs = vec![
        pnex_core::FunctionOutput {
            name: "y".into(),
            ty: pnex_core::FunctionType::Number,
            desc: None,
        },
        pnex_core::FunctionOutput {
            name: "x".into(),
            ty: pnex_core::FunctionType::Number,
            desc: None,
        },
    ];
    let old_outs = match &graph.nodes[2].kind {
        FlowNodeKind::PnexFunction { config } => config.outputs.clone(),
        _ => unreachable!(),
    };
    rewire_function_outputs(&mut graph, "f", &old_outs, &new_outs);
    let ports_of = |graph: &FlowGraph, t: &str| -> Vec<usize> {
        graph.nodes[2]
            .outputs
            .iter()
            .filter(|w| w.targets.iter().any(|x| x == t))
            .map(|w| w.port)
            .collect()
    };
    // x moved 0 → 1 (now second), targets intact.
    assert_eq!(ports_of(&graph, "src2"), vec![1]);
    // A removed output takes its wire with it.
    let empty: Vec<pnex_core::FunctionOutput> = vec![];
    rewire_function_outputs(&mut graph, "f", &new_outs, &empty);
    assert!(graph.nodes[2].outputs.is_empty(), "wires die with outputs");
}

#[test]
fn function_inputs_pruned_on_rebase() {
    let mut graph = function_graph_two_by_two();
    // Rebase dropping input `b`: its annotation goes, but the src2
    // runtime wire survives only if it feeds another input (it does not
    // — src2 fed only b → wire dropped too).
    let only_a = vec![pnex_core::FunctionInput {
        name: "a".into(),
        ty: pnex_core::FunctionType::Number,
        default: None,
        desc: None,
    }];
    prune_function_inputs(&mut graph, "f", &only_a);
    assert!(graph.nodes[2].inputs.iter().all(|w| w.pin != "b"));
    let src2_wired = graph.nodes[1]
        .outputs
        .iter()
        .flat_map(|w| &w.targets)
        .any(|t| t == "f");
    assert!(!src2_wired, "b orphaned: runtime wire goes with it");
    // src1 still feeds a → untouched.
    let src1_wired = graph.nodes[0]
        .outputs
        .iter()
        .flat_map(|w| &w.targets)
        .any(|t| t == "f");
    assert!(src1_wired, "a survives: runtime wire kept");
}

#[test]
fn write_input_annotations_one_wire_per_pin() {
    let mut graph = FlowGraph {
        nodes: vec![
            make_node("src1", PaletteKind::Inject, Position { x: 0.0, y: 0.0 }),
            make_node("src2", PaletteKind::Value, Position { x: 0.0, y: 100.0 }),
            make_node(
                "w",
                PaletteKind::DeviceWrite,
                Position { x: 300.0, y: 50.0 },
            ),
        ],
    };
    // Configure two output pins on the write node.
    if let FlowNodeKind::DeviceWrite { config } = &mut graph.nodes[2].kind {
        config.device_id = "d".into();
        config.pins = vec!["D2".into(), "D3".into()];
    }
    // A source may fan out to several pins.
    add_target(&mut graph, "src1", 0, "w");
    set_input_target(&mut graph, "src1", 0, "w", "D2".into());
    set_input_target(&mut graph, "src1", 0, "w", "D3".into());
    assert_eq!(graph.nodes[2].inputs.len(), 2);
    // Cutting one pin keeps the runtime wire: the source still feeds
    // the other one (fan-out shares it).
    cut_input_wire(&mut graph, "src1", 0, "w", "D2");
    assert!(graph.nodes[2].inputs.iter().all(|w| w.pin != "D2"));
    let runtime = graph.nodes[0]
        .outputs
        .iter()
        .flat_map(|w| &w.targets)
        .any(|t| t == "w");
    assert!(runtime, "runtime wire must survive while D3 is fed");
    // A pin receives one wire: re-dropping takes it over.
    set_input_target(&mut graph, "src2", 0, "w", "D3".into());
    let on_d3: Vec<&str> = graph.nodes[2]
        .inputs
        .iter()
        .filter(|w| w.pin == "D3")
        .map(|w| w.from.as_str())
        .collect();
    assert_eq!(on_d3, vec!["src2"]);
    // Once src1 feeds no pin any more, cutting its last annotated wire
    // drops the runtime wire too.
    cut_input_wire(&mut graph, "src1", 0, "w", "D3");
    let runtime = graph.nodes[0]
        .outputs
        .iter()
        .flat_map(|w| &w.targets)
        .any(|t| t == "w");
    assert!(!runtime, "runtime wire must go once no pin is fed");
    // Node deletion purges annotations pointing at it.
    remove_node(&mut graph, "src2");
    assert!(graph
        .nodes
        .iter()
        .all(|n| n.id == "w" || n.inputs.is_empty()));
}

#[test]
fn make_node_inject_has_a_trigger() {
    let node = make_node("n9", PaletteKind::Inject, Position { x: 0.0, y: 0.0 });
    match &node.kind {
        FlowNodeKind::Inject { config } => {
            // Default = continuous traffic (repeat 30 s), never again a single
            // "once" shot that made the flow silent after 5 min.
            assert!(config.repeat_secs.is_some());
            assert_eq!(next_node_id(&FlowGraph { nodes: vec![node] }), "n10");
        }
        other => panic!("expected inject, got {other:?}"),
    }
}

#[test]
fn make_node_notify_typed_but_incomplete() {
    // Typed but deliberately incomplete config: the banner's
    // `notify_no_*` violations guide the input (device/red pattern).
    let node = make_node("n1", PaletteKind::PnexNotify, Position { x: 0.0, y: 0.0 });
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(
        codes.contains(&"notify_no_channels".to_string()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"notify_no_template".to_string()),
        "{codes:?}"
    );

    // Completed (channel + template): no notify violation left.
    let mut node = make_node("n1", PaletteKind::PnexNotify, Position { x: 0.0, y: 0.0 });
    if let pnex_core::FlowNodeKind::PnexNotify { config } = &mut node.kind {
        config.channel_ids = vec![uuid::Uuid::from_u128(42)];
        config.template_id = uuid::Uuid::from_u128(7);
    }
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(
        !codes.contains(&"notify_no_channels".to_string()),
        "{codes:?}"
    );
    assert!(
        !codes.contains(&"notify_no_template".to_string()),
        "{codes:?}"
    );
}

#[test]
fn make_node_http_fetch_typed_but_incomplete() {
    // Typed but deliberately incomplete config: the banner's
    // `http_fetch_url_missing` violation guides the input (notify pattern).
    let node = make_node("n1", PaletteKind::HttpFetch, Position { x: 0.0, y: 0.0 });
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(
        codes.contains(&"http_fetch_url_missing".to_string()),
        "{codes:?}"
    );
    // Default timeout: no spurious violation (manual Default
    // aligned with the serde default = 30 s).
    assert!(
        !codes.contains(&"http_fetch_timeout_range".to_string()),
        "{codes:?}"
    );

    // Completed (URL + bearer auth + proxy): no violation left.
    let mut node = make_node("n1", PaletteKind::HttpFetch, Position { x: 0.0, y: 0.0 });
    if let pnex_core::FlowNodeKind::HttpFetch { config } = &mut node.kind {
        config.url = "https://api.exemple.dev/data".into();
        config.auth = pnex_core::HttpFetchAuth::Bearer { token: "t".into() };
        config.proxy = pnex_core::HttpFetchProxy::Custom {
            url: "http://proxy:1".into(),
            username: Some("u".into()),
            password: pnex_core::SecretSlot::Unset,
        };
    }
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn make_node_phase6_typed_but_incomplete() {
    // Typed but deliberately incomplete defaults: the banner guides
    // the input (device_no_reads / calc_bad_expression /
    // metric_name_missing), same rule as red.
    let node = make_node("n1", PaletteKind::DeviceRead, Position { x: 0.0, y: 0.0 });
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(codes.contains(&"device_no_pins".to_string()), "{codes:?}");

    let node = make_node("n1", PaletteKind::DeviceWrite, Position { x: 0.0, y: 0.0 });
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(codes.contains(&"device_no_pins".to_string()), "{codes:?}");

    let node = make_node("n1", PaletteKind::Calc, Position { x: 0.0, y: 0.0 });
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(
        codes.contains(&"calc_bad_expression".to_string()),
        "{codes:?}"
    );

    let node = make_node("n1", PaletteKind::Metric, Position { x: 0.0, y: 0.0 });
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(
        codes.contains(&"metric_name_missing".to_string()),
        "{codes:?}"
    );
}

#[test]
fn make_node_value_ships_json_template() {
    // Json Values ships a usable object template (user decision
    // 2026-09-24): a fresh node is valid, no `value_static_missing`.
    let node = make_node("n1", PaletteKind::Value, Position { x: 0.0, y: 0.0 });
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(codes.is_empty(), "{codes:?}");

    // An emptied document (null) still flags `value_static_missing`.
    let mut node = make_node("n1", PaletteKind::Value, Position { x: 0.0, y: 0.0 });
    if let pnex_core::FlowNodeKind::Value { config } = &mut node.kind {
        config.value = serde_json::Value::Null;
    }
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .iter()
        .map(|v| v.code.clone())
        .collect();
    assert!(
        codes.contains(&"value_static_missing".to_string()),
        "{codes:?}"
    );
}

#[test]
fn make_node_display_is_valid() {
    // The probe has no config to fill in: the default is green right away.
    let node = make_node("n1", PaletteKind::Display, Position { x: 0.0, y: 0.0 });
    assert!(matches!(&node.kind, FlowNodeKind::Display { .. }));
    assert!(pnex_core::validate_graph(&FlowGraph { nodes: vec![node] }).is_empty());
}

#[test]
fn make_node_reg_typed_but_incomplete() {
    // Control cards: typed but empty config → reg_* violations
    // that guide the input (device/pins binding, deadband, …).
    for (kind, expected) in [
        (PaletteKind::RegTtHeat, "reg_bad_device"),
        (PaletteKind::RegTtCool, "reg_bad_device"),
        (PaletteKind::RegPid, "reg_bad_device"),
    ] {
        let node = make_node("n1", kind, Position { x: 0.0, y: 0.0 });
        let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
            .iter()
            .map(|v| v.code.clone())
            .collect();
        assert!(
            codes.contains(&expected.to_string()),
            "{kind:?} → {codes:?}"
        );
        // Missing pins flagged too (double violation useful as a guide).
        assert!(
            codes.contains(&"reg_pin_missing".to_string()),
            "{kind:?} → {codes:?}"
        );
    }

    // A complete card (TT heat) is green right away.
    let mut node = make_node("n1", PaletteKind::RegTtHeat, Position { x: 0.0, y: 0.0 });
    if let FlowNodeKind::RegTtHeat { config } = &mut node.kind {
        config.device_id = "serre-1".into();
        config.sensor_pin = "A0".into();
        config.actuator_pin = "D1".into();
        config.setpoint = 19.0;
        config.deadband = 0.5;
    }
    assert!(pnex_core::validate_graph(&FlowGraph { nodes: vec![node] }).is_empty());
}

#[test]
fn wiring_add_and_remove() {
    let mut graph = graph_with_two();
    add_target(&mut graph, "n1", 0, "n2");
    add_target(&mut graph, "n1", 0, "n2"); // idempotent
    assert_eq!(graph.nodes[0].outputs[0].targets, vec!["n2".to_string()]);
    remove_target(&mut graph, "n1", 0, "n2");
    assert!(graph.nodes[0].outputs.is_empty());
    // Wiring an unknown source: defensive no-op.
    add_target(&mut graph, "ghost", 0, "n2");
    assert!(graph.nodes[1].outputs.is_empty());
}

#[test]
fn node_delete_cleans_inputs() {
    let mut graph = graph_with_two();
    add_target(&mut graph, "n1", 0, "n2");
    remove_node(&mut graph, "n2");
    assert_eq!(graph.nodes.len(), 1);
    assert!(graph.nodes[0].outputs.is_empty());
}

/// inject → split(keys=[temp, hum]) → 2 debug, wired on ports 0-1.
fn split_wired(keys: &[&str]) -> FlowGraph {
    let mut graph = FlowGraph {
        nodes: vec![
            make_node("i1", PaletteKind::Inject, Position { x: 0.0, y: 0.0 }),
            make_node("sp", PaletteKind::JsonSplit, Position { x: 200.0, y: 0.0 }),
            make_node("dbg", PaletteKind::Debug, Position { x: 400.0, y: 0.0 }),
            make_node("dbg2", PaletteKind::Debug, Position { x: 400.0, y: 100.0 }),
        ],
    };
    if let FlowNodeKind::JsonSplit { config } = &mut graph.nodes[1].kind {
        config.keys = keys.iter().map(|s| (*s).to_string()).collect();
    }
    add_target(&mut graph, "sp", 0, "dbg");
    add_target(&mut graph, "sp", 1, "dbg2");
    graph
}

#[test]
fn split_keys_delete_takes_the_wire() {
    // Delete "temp" (port 0): its wire goes; "hum" keeps
    // its own **even though its index shifted** (1 → 0) — rewire-by-label
    // pattern. Renaming a key rewires nothing: the port index is
    // stable (the inspector only patches the config).
    let mut graph = split_wired(&["temp", "hum"]);
    rewire_split_keys(
        &mut graph,
        "sp",
        &["temp".to_string(), "hum".to_string()],
        &["hum".to_string()],
    );
    let ports: Vec<(usize, Vec<String>)> = graph.nodes[1]
        .outputs
        .iter()
        .map(|w| (w.port, w.targets.clone()))
        .collect();
    assert_eq!(
        ports,
        vec![(0, vec!["dbg2".to_string()])],
        "hum keeps its wire, temp takes its own away"
    );
}

/// inject → merge(inputs=[temp]) → debug, annotation set with the wire.
fn merge_wired(inputs: &[&str]) -> FlowGraph {
    let mut graph = FlowGraph {
        nodes: vec![
            make_node("i1", PaletteKind::Inject, Position { x: 0.0, y: 0.0 }),
            make_node("mg", PaletteKind::JsonMerge, Position { x: 200.0, y: 0.0 }),
            make_node("dbg", PaletteKind::Debug, Position { x: 400.0, y: 0.0 }),
        ],
    };
    if let FlowNodeKind::JsonMerge { config } = &mut graph.nodes[1].kind {
        config.inputs = inputs.iter().map(|s| (*s).to_string()).collect();
    }
    add_target(&mut graph, "i1", 0, "mg");
    set_input_target(&mut graph, "i1", 0, "mg", "temp".into());
    graph
}

#[test]
fn merge_inputs_rename_follows_annotation() {
    let mut graph = merge_wired(&["temp"]);
    // Rename the row: same wire, same source, new pin.
    rename_merge_input(&mut graph, "mg", "temp", "humidity");
    let pins: Vec<String> = graph.nodes[1]
        .inputs
        .iter()
        .map(|w| w.pin.clone())
        .collect();
    assert_eq!(pins, vec!["humidity".to_string()]);
    // The runtime wire stays set (add_target from the setup).
    assert_eq!(graph.nodes[0].outputs[0].targets, vec!["mg".to_string()]);
    // Unknown old name: defensive no-op.
    rename_merge_input(&mut graph, "mg", "temp", "x");
    let pins: Vec<String> = graph.nodes[1]
        .inputs
        .iter()
        .map(|w| w.pin.clone())
        .collect();
    assert_eq!(pins, vec!["humidity".to_string()]);
}

#[test]
fn sync_split_keys_auto_follows_upstream() {
    // Split in auto mode wired to a merge (value1/value2): the sync
    // regenerates the keys — hence the ports — and rewires by label.
    let mut graph = FlowGraph {
        nodes: vec![
            make_node("mg", PaletteKind::JsonMerge, Position { x: 0.0, y: 0.0 }),
            make_node("sp", PaletteKind::JsonSplit, Position { x: 200.0, y: 0.0 }),
            make_node("dbg", PaletteKind::Debug, Position { x: 400.0, y: 0.0 }),
        ],
    };
    if let FlowNodeKind::JsonMerge { config } = &mut graph.nodes[0].kind {
        config.inputs = vec!["value1".into(), "value2".into()];
    }
    add_target(&mut graph, "mg", 0, "sp");
    // Wiring triggers the sync through the funnel — here we call it
    // directly: the ports appear.
    sync_split_keys_auto(&mut graph);
    let keys = match &graph.nodes[1].kind {
        FlowNodeKind::JsonSplit { config } => config.keys.clone(),
        _ => vec![],
    };
    assert_eq!(keys, vec!["value1".to_string(), "value2".to_string()]);

    // The split wires its port 0 (= value1) to debug; the merge loses
    // value1 → the wire follows its label and is carried away with it.
    add_target(&mut graph, "sp", 0, "dbg");
    if let FlowNodeKind::JsonMerge { config } = &mut graph.nodes[0].kind {
        config.inputs = vec!["value2".into()];
    }
    sync_split_keys_auto(&mut graph);
    let ports: Vec<(usize, Vec<String>)> = graph.nodes[1]
        .outputs
        .iter()
        .map(|w| (w.port, w.targets.clone()))
        .collect();
    assert_eq!(
        ports,
        vec![],
        "value1 takes its wire away (rewire-by-label pattern)"
    );

    // Manual mode: no sync.
    if let FlowNodeKind::JsonSplit { config } = &mut graph.nodes[1].kind {
        config.auto = false;
        config.keys = vec!["libre".into()];
    }
    sync_split_keys_auto(&mut graph);
    let keys = match &graph.nodes[1].kind {
        FlowNodeKind::JsonSplit { config } => config.keys.clone(),
        _ => vec![],
    };
    assert_eq!(keys, vec!["libre".to_string()], "manual = intact");
}

#[test]
fn claim_input_row_prefers_a_free_row() {
    // temp is taken by i1: a source i2 aiming at the temp row
    // (cursor over it) gets the nearest free row (hum) —
    // never a silent theft. i1, for its part, takes its own row back.
    let graph = merge_wired(&["temp", "hum"]);
    // merge at (200, 0), h = 60: rows at y = 20 (temp) and 40 (hum).
    assert_eq!(
        claim_input_row(&graph, "mg", "i2", 19.0),
        Some("hum".to_string()),
        "row taken by another → free row"
    );
    assert_eq!(
        claim_input_row(&graph, "mg", "i1", 19.0),
        Some("temp".to_string()),
        "its own row stays targetable"
    );
    // Full board for i3: explicit takeover (the nearest anyway).
    let full = {
        let mut g = merge_wired(&["temp", "hum"]);
        set_input_target(&mut g, "i1", 0, "mg", "hum".into());
        g
    };
    assert_eq!(
        claim_input_row(&full, "mg", "i2", 19.0),
        Some("temp".to_string()),
        "board pleine → takeover explicite"
    );
    // No declared row: no choice (bare wire, legacy contract).
    let bare = graph_with_two();
    assert_eq!(claim_input_row(&bare, "n2", "n1", 20.0), None);
}

#[test]
fn merge_inputs_prune_takes_the_wire_if_not_shared() {
    // Port 0 of i1 feeds the "temp" row only: the prune removes
    // the runtime wire. If it also fed another row (fan-out),
    // the wire would stay set.
    let mut graph = merge_wired(&["temp"]);
    prune_merge_inputs(&mut graph, "mg", &[]);
    assert!(graph.nodes[1].inputs.is_empty(), "annotation pruned");
    assert!(graph.nodes[0].outputs.is_empty(), "runtime wire removed");

    // Fan-out: the same port feeds a second row → wire kept.
    let mut graph = merge_wired(&["temp", "hum"]);
    set_input_target(&mut graph, "i1", 0, "mg", "hum".into());
    prune_merge_inputs(&mut graph, "mg", &["hum".to_string()]);
    let pins: Vec<String> = graph.nodes[1]
        .inputs
        .iter()
        .map(|w| w.pin.clone())
        .collect();
    assert_eq!(pins, vec!["hum".to_string()], "hum kept");
    assert_eq!(
        graph.nodes[0].outputs[0].targets,
        vec!["mg".to_string()],
        "shared wire kept"
    );
}

#[test]
fn make_node_camera_source_guides_camera_pick() {
    // Camera left empty on purpose: the banner asks for a camera.
    let node = make_node("c1", PaletteKind::CameraSource, Position { x: 0.0, y: 0.0 });
    match &node.kind {
        FlowNodeKind::CameraSource { config } => {
            assert!(config.device_id.is_empty());
            assert_eq!(config.max_fps, 0.0);
        }
        other => panic!("camera_source expected, got {other:?}"),
    }
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .into_iter()
        .map(|v| v.code)
        .collect();
    assert!(
        codes.contains(&"camera_device_missing".to_string()),
        "{codes:?}"
    );
}

#[test]
fn make_node_video_record_defaults_are_valid() {
    let node = make_node("v1", PaletteKind::VideoRecord, Position { x: 0.0, y: 0.0 });
    match &node.kind {
        FlowNodeKind::VideoRecord { config } => {
            assert_eq!(config, &pnex_core::VideoRecordConfig::default());
            assert!(config.check().is_none());
        }
        other => panic!("video_record expected, got {other:?}"),
    }
    // One output port (one message per written segment).
    assert_eq!(output_count_of(&node), 1);
}

#[test]
fn make_node_vision_detect_guides_model_pick() {
    let node = make_node("v1", PaletteKind::VisionDetect, Position { x: 0.0, y: 0.0 });
    match &node.kind {
        FlowNodeKind::VisionDetect { config } => {
            assert!(config.model_id.is_empty());
            assert_eq!(config.max_fps, 1.0);
            assert_eq!(config.emit, pnex_core::vision::VisionEmit::OnDetection);
        }
        other => panic!("vision_detect expected, got {other:?}"),
    }
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .into_iter()
        .map(|v| v.code)
        .collect();
    assert!(
        codes.contains(&"vision_model_missing".to_string()),
        "{codes:?}"
    );
}

#[test]
fn make_node_event_log_defaults_are_valid() {
    let node = make_node("e1", PaletteKind::EventLog, Position { x: 0.0, y: 0.0 });
    match &node.kind {
        FlowNodeKind::EventLog { config } => assert!(config.check().is_none()),
        other => panic!("event_log expected, got {other:?}"),
    }
    assert_eq!(output_count_of(&node), 1);
}

#[test]
fn saved_graph_secret_references_replace_typed_values() {
    use pnex_core::{HttpFetchAuth, SecretSlot};
    let id = uuid::Uuid::from_u128(11);
    let with_auth = |auth: HttpFetchAuth| {
        let mut node = make_node("h1", PaletteKind::HttpFetch, Position { x: 0.0, y: 0.0 });
        if let FlowNodeKind::HttpFetch { config } = &mut node.kind {
            config.auth = auth;
        }
        FlowGraph { nodes: vec![node] }
    };
    let mut current = with_auth(HttpFetchAuth::Bearer {
        token: "typed".into(),
    });
    let stored = with_auth(HttpFetchAuth::Bearer {
        token: SecretSlot::Ref(id),
    });
    adopt_stored_secrets(&mut current, &stored);
    assert_eq!(current, stored);

    // A field switched away meanwhile (other auth mode) is left alone.
    let mut edited = with_auth(HttpFetchAuth::Header {
        name: "x-key".into(),
        value: "typed".into(),
    });
    let before = edited.clone();
    adopt_stored_secrets(&mut edited, &stored);
    assert_eq!(edited, before);
}

#[test]
fn make_node_media_source_guides_stream_pick() {
    // No stream yet: the banner asks for one; segment mode by default.
    let node = make_node("m1", PaletteKind::MediaSource, Position { x: 0.0, y: 0.0 });
    let FlowNodeKind::MediaSource { config } = &node.kind else {
        panic!("media_source expected, got {:?}", node.kind);
    };
    assert!(config.streams.is_empty());
    assert_eq!(
        config.emit,
        pnex_core::media_ingest::MediaSourceEmit::Segment
    );
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .into_iter()
        .map(|v| v.code)
        .collect();
    assert!(
        codes.contains(&"media_source_no_stream".to_string()),
        "{codes:?}"
    );
}

#[test]
fn make_node_topic_classify_guides_taxonomy_pick() {
    // No taxonomy yet: the banner asks for one; text read from payload.text.
    let node = make_node(
        "t1",
        PaletteKind::TopicClassify,
        Position { x: 0.0, y: 0.0 },
    );
    let FlowNodeKind::TopicClassify { config } = &node.kind else {
        panic!("topic_classify expected, got {:?}", node.kind);
    };
    assert_eq!(config.text_field, "text");
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .into_iter()
        .map(|v| v.code)
        .collect();
    assert!(
        codes.contains(&"topic_classify_no_taxonomy".to_string()),
        "{codes:?}"
    );
}

#[test]
fn make_node_range_upsert_guides_stream_pick() {
    // Scoped on a stream by default, none picked: the banner asks for one.
    let node = make_node("r1", PaletteKind::RangeUpsert, Position { x: 0.0, y: 0.0 });
    let FlowNodeKind::RangeUpsert { config } = &node.kind else {
        panic!("range_upsert expected, got {:?}", node.kind);
    };
    assert_eq!(config.scope_kind, "stream");
    assert_eq!(config.origin, "grid");
    let codes: Vec<String> = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] })
        .into_iter()
        .map(|v| v.code)
        .collect();
    assert!(
        codes.contains(&"range_upsert_scope_invalid".to_string()),
        "{codes:?}"
    );
}

#[test]
fn auto_layout_wraps_long_chains_and_never_overlaps() {
    // A 20-node chain plus a branch off n1: one line would be ~5000 px wide.
    let mut graph = FlowGraph {
        nodes: (1..=21)
            .map(|i| {
                make_node(
                    &format!("n{i}"),
                    PaletteKind::Debug,
                    Position { x: 0.0, y: 0.0 },
                )
            })
            .collect(),
    };
    for i in 1..20 {
        add_target(&mut graph, &format!("n{i}"), 0, &format!("n{}", i + 1));
    }
    add_target(&mut graph, "n1", 0, "n21");
    auto_layout(&mut graph);

    let pos = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .position
            .unwrap()
    };
    // Stages flow left to right inside a band, and the chain wraps to a new band.
    assert!(pos("n2").x > pos("n1").x);
    assert!(
        pos("n20").y > pos("n1").y,
        "long chain must wrap onto several bands"
    );
    // The branch shares n2's column, on its own row.
    assert_eq!(pos("n21").x, pos("n2").x);
    assert_ne!(pos("n21").y, pos("n2").y);
    for a in &graph.nodes {
        for b in &graph.nodes {
            let (pa, pb) = (a.position.unwrap(), b.position.unwrap());
            if a.id != b.id {
                assert!(
                    (pa.x - pb.x).abs() >= crate::components::flow_editor::geometry::NODE_W
                        || (pa.y - pb.y).abs() >= crate::components::flow_editor::geometry::NODE_H,
                    "{} overlaps {}",
                    a.id,
                    b.id
                );
            }
        }
    }
}
