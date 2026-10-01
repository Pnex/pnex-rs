use super::*;
use crate::functions::{FunctionLanguage, FunctionNodeConfig, FunctionResolver};
use crate::proto::SafeState;
use serde_json::json;

fn node_inject(id: &str) -> FlowNode {
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::Inject {
            config: InjectConfig {
                repeat_secs: Some(1.0),
                ..Default::default()
            },
        },
    }
}

fn node_display(id: &str, targets: &[String]) -> FlowNode {
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: if targets.is_empty() {
            vec![]
        } else {
            vec![FlowWiring {
                port: 0,
                targets: targets.to_vec(),
            }]
        },
        inputs: vec![],
        kind: FlowNodeKind::Display {
            config: DisplayConfig,
        },
    }
}

fn simple_graph() -> FlowGraph {
    FlowGraph {
        nodes: vec![
            node_inject("n1"),
            FlowNode {
                id: "n2".into(),
                name: Some("query".into()),
                position: Some(Position { x: 200.0, y: 100.0 }),
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["n3".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::Value {
                    config: ValueConfig {
                        mode: ValueMode::Static,
                        value: json!(1),
                        min: 0.0,
                        max: 10.0,
                    },
                },
            },
            FlowNode {
                id: "n3".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::Debug {
                    config: DebugConfig::default(),
                },
            },
        ],
    }
}

#[test]
fn graph_serde_roundtrip() {
    let g = simple_graph();
    let json = serde_json::to_string(&g).unwrap();
    let back: FlowGraph = serde_json::from_str(&json).unwrap();
    assert_eq!(back, g);
}

#[test]
fn graph_minimal_form_deserialise() {
    // Forme minimale (champs optionnels absents) acceptée.
    let g: FlowGraph =
        serde_json::from_str(r#"{"nodes":[{"id":"n1","kind":"debug","config":{}}]}"#).unwrap();
    assert_eq!(g.nodes.len(), 1);
    assert!(matches!(&g.nodes[0].kind, FlowNodeKind::Debug { config } if config.active));
}

#[test]
fn validate_accepts_simple_pipeline() {
    assert!(validate_graph(&simple_graph()).is_empty());
}

fn node_http_fetch(id: &str, config: HttpFetchNodeConfig) -> FlowNode {
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::HttpFetch { config },
    }
}

fn http_fetch_complet() -> HttpFetchNodeConfig {
    HttpFetchNodeConfig {
        url: "https://api.exemple.dev/data".into(),
        method: HttpFetchMethod::Post,
        headers: vec![HttpFetchHeader {
            name: "Accept".into(),
            value: "application/json".into(),
        }],
        auth: HttpFetchAuth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
        proxy: HttpFetchProxy::Custom {
            url: "http://proxy.exemple.dev:8001".into(),
            username: Some("pu".into()),
            password: "pp".into(),
        },
        timeout_secs: 15,
        body: Some(r#"{"cible":"x"}"#.into()),
        on_error: HttpFetchOnError::Passthrough,
    }
}

#[test]
fn validate_http_fetch_defaut_guide_la_saisie() {
    let g = FlowGraph {
        nodes: vec![node_http_fetch("h1", HttpFetchNodeConfig::default())],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|v| v.code.clone()).collect();
    assert!(
        codes.contains(&"http_fetch_url_missing".to_string()),
        "{codes:?}"
    );
    assert!(
        !codes.contains(&"http_fetch_timeout_range".to_string()),
        "{codes:?}"
    );
}

#[test]
fn validate_http_fetch_complet_sans_violation() {
    let g = FlowGraph {
        nodes: vec![node_http_fetch("h1", http_fetch_complet())],
    };
    let v = validate_graph(&g);
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn validate_http_fetch_codes_par_violation() {
    let mut cfg = http_fetch_complet();
    cfg.url = "ftp://hors-sujet.example".into();
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node_http_fetch("h1", cfg)],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"http_fetch_url_scheme".to_string()),
        "{codes:?}"
    );

    let mut cfg = http_fetch_complet();
    cfg.headers = vec![HttpFetchHeader {
        name: "  ".into(),
        value: String::new(),
    }];
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node_http_fetch("h1", cfg)],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"http_fetch_header_empty".to_string()),
        "{codes:?}"
    );

    let mut cfg = http_fetch_complet();
    cfg.auth = HttpFetchAuth::Bearer { token: " ".into() };
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node_http_fetch("h1", cfg)],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"http_fetch_auth_incomplete".to_string()),
        "{codes:?}"
    );

    let mut cfg = http_fetch_complet();
    cfg.proxy = HttpFetchProxy::Custom {
        url: String::new(),
        username: None,
        password: crate::SecretSlot::Unset,
    };
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node_http_fetch("h1", cfg)],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"http_fetch_proxy_missing".to_string()),
        "{codes:?}"
    );

    let mut cfg = http_fetch_complet();
    cfg.timeout_secs = 301;
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node_http_fetch("h1", cfg)],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"http_fetch_timeout_range".to_string()),
        "{codes:?}"
    );
}

#[test]
fn value_node_validation_et_projection() {
    // Static sans valeur : violation guidée (école device/calc/metric).
    let node = |value: serde_json::Value, mode: ValueMode, min: f64, max: f64| FlowNode {
        id: "v1".into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::Value {
            config: ValueConfig {
                mode,
                value,
                min,
                max,
            },
        },
    };
    let codes_of = |n: &FlowNode| -> Vec<String> {
        validate_graph(&FlowGraph {
            nodes: vec![n.clone()],
        })
        .iter()
        .map(|v| v.code.clone())
        .collect()
    };

    let n = node(serde_json::Value::Null, ValueMode::Static, 0.0, 10.0);
    let codes = codes_of(&n);
    assert!(
        codes.contains(&"value_static_missing".to_string()),
        "{codes:?}"
    );

    // Static renseigné : plus aucune violation.
    let n = node(serde_json::json!({"k": "v"}), ValueMode::Static, 0.0, 10.0);
    assert!(codes_of(&n).is_empty(), "{:?}", codes_of(&n));

    // Random bornes inversées : violation dédiée.
    let n = node(serde_json::Value::Null, ValueMode::Random, 5.0, 1.0);
    let codes = codes_of(&n);
    assert!(
        codes.contains(&"value_range_invalid".to_string()),
        "{codes:?}"
    );

    // Random bornes valides : plus aucune violation.
    let n = node(serde_json::Value::Null, ValueMode::Random, 1.0, 5.0);
    assert!(codes_of(&n).is_empty(), "{:?}", codes_of(&n));

    // Serde : le mode est sérialisé snake_case et relit tel quel.
    let n = node(serde_json::json!("texte"), ValueMode::Random, 1.0, 5.0);
    let raw = serde_json::to_string(&n).expect("sérialisable");
    assert!(raw.contains(r#""mode":"random""#), "{raw}");
    let back: FlowNode = serde_json::from_str(&raw).expect("roundtrip");
    assert_eq!(back, n);

    // Projection : entrée pnex-value estampillée (flow/version).
    let meta = FlowArtifactMeta {
        flow_id: 3,
        version_number: 7,
        org_id: 1,
        o2_org: String::new(),
    };
    let red = to_red_flows_json(&FlowGraph { nodes: vec![n] }, &meta);
    let arr = red.as_array().expect("entries");
    let entry = &arr[1];
    assert_eq!(entry["type"], serde_json::json!("pnex-value"));
    assert_eq!(entry["mode"], serde_json::json!("random"));
    assert_eq!(entry["min"], serde_json::json!(1.0));
    assert_eq!(entry["max"], serde_json::json!(5.0));
    assert_eq!(entry["pnex_flow_id"], serde_json::json!(3));
    assert_eq!(entry["pnex_version"], serde_json::json!(7));
}

#[test]
fn validate_json_merge_cle_defaut_requise() {
    let node = |default_key: &str| FlowNode {
        id: "m1".into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::JsonMerge {
            config: JsonMergeConfig {
                default_key: default_key.into(),
                inputs: Vec::new(),
            },
        },
    };
    // Clé vide : violation guidée (école value_static_missing).
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node("  ")],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"merge_default_key_missing".to_string()),
        "{codes:?}"
    );
    // Clé renseignée : aucune violation.
    assert!(validate_graph(&FlowGraph {
        nodes: vec![node("valeur")]
    })
    .is_empty());
    // Le split n'a rien à valider : toujours propre.
    let split = FlowNode {
        id: "s1".into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::JsonSplit {
            config: JsonSplitConfig::default(),
        },
    };
    assert!(validate_graph(&FlowGraph { nodes: vec![split] }).is_empty());
}

#[test]
fn json_split_merge_serde_defaults() {
    // Graphs sauvegardés avant l'ajout des champs : `{}` et
    // `{"default_key":…}` désérialisent toujours (serde default) —
    // sinon tout l'artefact devenait `graph_unreadable` au deploy.
    let split: JsonSplitConfig = serde_json::from_str("{}").expect("split legacy");
    assert!(split.keys.is_empty());
    let merge: JsonMergeConfig =
        serde_json::from_str(r#"{"default_key":"value"}"#).expect("merge legacy");
    assert!(merge.inputs.is_empty());
    // Round-trip complet avec champs peuplés (skip si vide).
    let split = JsonSplitConfig {
        keys: vec!["a".into(), "b".into()],
        auto: true,
    };
    let raw = serde_json::to_string(&split).expect("sérialisation");
    assert_eq!(
        serde_json::from_str::<JsonSplitConfig>(&raw).expect("roundtrip"),
        split
    );
    let merge = JsonMergeConfig {
        default_key: "value".into(),
        inputs: vec!["temp".into(), "hum".into()],
    };
    let raw = serde_json::to_string(&merge).expect("sérialisation");
    assert_eq!(
        serde_json::from_str::<JsonMergeConfig>(&raw).expect("roundtrip"),
        merge
    );
}

#[test]
fn validate_json_split_port_hors_plage() {
    // Deux clés déclarées = ports 0-1 ; les cibles doivent exister
    // (sinon dangling_target parasite le comptage des violations).
    let node = |outputs: Vec<FlowWiring>| FlowNode {
        id: "s1".into(),
        name: None,
        position: None,
        outputs,
        inputs: vec![],
        kind: FlowNodeKind::JsonSplit {
            config: JsonSplitConfig {
                keys: vec!["a".into(), "b".into()],
                auto: true,
            },
        },
    };
    let sink = node_display("x", &[]);
    // Ports 0 et 1 câblés : propre.
    let clean = node(vec![
        FlowWiring {
            port: 0,
            targets: vec!["x".into()],
        },
        FlowWiring {
            port: 1,
            targets: vec!["x".into()],
        },
    ]);
    assert!(validate_graph(&FlowGraph {
        nodes: vec![clean, sink.clone()],
    })
    .is_empty());
    // Port 2 câblé : hors plage (câble mort au runtime).
    let over = node(vec![FlowWiring {
        port: 2,
        targets: vec!["x".into()],
    }]);
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![over, sink],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"split_port_out_of_range".to_string()),
        "{codes:?}"
    );
}

#[test]
fn validate_json_merge_entrees_nommees() {
    let node = |inputs: Vec<String>, annotations: Vec<FlowInputWiring>| FlowNode {
        id: "m1".into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: annotations,
        kind: FlowNodeKind::JsonMerge {
            config: JsonMergeConfig {
                default_key: "value".into(),
                inputs,
            },
        },
    };
    // Nom vide : violation dédiée.
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node(vec!["temp".into(), "  ".into()], vec![])],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"merge_input_name_missing".to_string()),
        "{codes:?}"
    );
    // Doublon : violation dédiée.
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node(vec!["temp".into(), "temp".into()], vec![])],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"merge_input_duplicate".to_string()),
        "{codes:?}"
    );
    // Annotation orpheline (ligne supprimée) : violation dédiée.
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![node(
            vec!["temp".into()],
            vec![FlowInputWiring {
                pin: "hum".into(),
                from: "i1".into(),
                from_port: 0,
            }],
        )],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"merge_input_annotation_unknown".to_string()),
        "{codes:?}"
    );
    // Cas propre : annotations alignées sur les déclarations.
    assert!(validate_graph(&FlowGraph {
        nodes: vec![node(
            vec!["temp".into(), "hum".into()],
            vec![
                FlowInputWiring {
                    pin: "temp".into(),
                    from: "i1".into(),
                    from_port: 0,
                },
                FlowInputWiring {
                    pin: "hum".into(),
                    from: "i2".into(),
                    from_port: 0,
                },
            ],
        )],
    })
    .is_empty());
}

#[test]
fn validate_json_merge_cable_nu_rejete() {
    // Un fil nu (source câblée sans annotation) sur un merge avec
    // entrées nommées : violation — sinon le rendu « choisirait » une
    // rangée à la place du câblage réel (fil qui saute de port).
    let merge = |annotations: Vec<FlowInputWiring>| FlowNode {
        id: "m1".into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: annotations,
        kind: FlowNodeKind::JsonMerge {
            config: JsonMergeConfig {
                default_key: "value".into(),
                inputs: vec!["temp".into(), "hum".into()],
            },
        },
    };
    let wired = |targets: Vec<String>| {
        let mut n = node_inject("i1");
        n.outputs = vec![FlowWiring { port: 0, targets }];
        n
    };
    // Fil nu (i1 câblé, aucune annotation) : violation dédiée.
    let codes: Vec<String> = validate_graph(&FlowGraph {
        nodes: vec![wired(vec!["m1".into()]), merge(vec![])],
    })
    .iter()
    .map(|v| v.code.clone())
    .collect();
    assert!(
        codes.contains(&"merge_wire_unbound".to_string()),
        "{codes:?}"
    );
    // Même fil annoté sur « temp » : propre.
    assert!(validate_graph(&FlowGraph {
        nodes: vec![
            wired(vec!["m1".into()]),
            merge(vec![FlowInputWiring {
                pin: "temp".into(),
                from: "i1".into(),
                from_port: 0,
            }]),
        ],
    })
    .is_empty());
    // Merge sans lignes déclarées : le fil nu est le contrat legacy
    // (clé par défaut) — pas de violation.
    let legacy = FlowNode {
        id: "m2".into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::JsonMerge {
            config: JsonMergeConfig {
                default_key: "value".into(),
                inputs: Vec::new(),
            },
        },
    };
    assert!(validate_graph(&FlowGraph {
        nodes: vec![wired(vec!["m2".into()]), legacy],
    })
    .is_empty());
}

#[test]
fn http_fetch_serde_roundtrip_modes_taggues() {
    let raw = r#"{
        "id": "h1",
        "kind": "http_fetch",
        "config": {
            "url": "https://api.exemple.dev/x",
            "method": "post",
            "headers": [{"name": "X-Api-Key", "value": "k"}],
            "auth": {"mode": "bearer", "token": "t"},
            "proxy": {"mode": "custom", "url": "http://p:1", "username": "u"},
            "timeout_secs": 45,
            "body": "{}",
            "on_error": "passthrough"
        }
    }"#;
    let n: FlowNode = serde_json::from_str(raw).unwrap();
    let FlowNodeKind::HttpFetch { config } = &n.kind else {
        panic!("variante attendue");
    };
    assert_eq!(config.method, HttpFetchMethod::Post);
    assert_eq!(config.auth, HttpFetchAuth::Bearer { token: "t".into() });
    assert_eq!(
        config.proxy,
        HttpFetchProxy::Custom {
            url: "http://p:1".into(),
            username: Some("u".into()),
            password: crate::SecretSlot::Unset,
        }
    );
    assert_eq!(config.timeout_secs, 45);
    assert_eq!(config.on_error, HttpFetchOnError::Passthrough);
    // Round-trip complet.
    let json = serde_json::to_string(&n).unwrap();
    let back: FlowNode = serde_json::from_str(&json).unwrap();
    assert_eq!(back, n);
}

#[test]
fn http_fetch_minimal_form_deserialise() {
    let n: FlowNode =
        serde_json::from_str(r#"{"id":"h1","kind":"http_fetch","config":{}}"#).unwrap();
    assert!(
        matches!(&n.kind, FlowNodeKind::HttpFetch { config } if *config == HttpFetchNodeConfig::default())
    );
}

#[test]
fn projection_http_fetch_champs_aplatis() {
    let g = FlowGraph {
        nodes: vec![node_http_fetch("h1", http_fetch_complet())],
    };
    let meta = FlowArtifactMeta {
        flow_id: 12,
        version_number: 3,
        org_id: 7,
        o2_org: "o2-default".into(),
    };
    let entries = to_red_flows_json(&g, &meta);
    let node = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("pnex-http-fetch"))
        .expect("entrée pnex-http-fetch absente de la projection");
    assert_eq!(
        node.get("url").and_then(|v| v.as_str()),
        Some("https://api.exemple.dev/data")
    );
    assert_eq!(node.get("method").and_then(|v| v.as_str()), Some("post"));
    assert_eq!(node.get("timeout_secs").and_then(|v| v.as_u64()), Some(15));
    assert_eq!(
        node.get("auth")
            .and_then(|v| v.get("mode"))
            .and_then(|v| v.as_str()),
        Some("basic")
    );
    assert_eq!(
        node.get("proxy")
            .and_then(|v| v.get("mode"))
            .and_then(|v| v.as_str()),
        Some("custom")
    );
    assert_eq!(
        node.get("pnex_node_id").and_then(|v| v.as_str()),
        Some("h1")
    );
    assert_eq!(node.get("pnex_flow_id").and_then(|v| v.as_i64()), Some(12));
}

#[test]
fn validate_rejects_structure_errors() {
    let mut g = simple_graph();
    g.nodes.push(node_inject("n1")); // dupliqué
    g.nodes[1].outputs.push(FlowWiring {
        port: 1,
        targets: vec!["ghost".into()],
    }); // cible inconnue
    let v = validate_graph(&g);
    let codes: Vec<&str> = v.iter().map(|x| x.code.as_str()).collect();
    assert!(codes.contains(&"duplicate_node_id"), "{v:?}");
    assert!(codes.contains(&"dangling_target"), "{v:?}");
}

#[test]
fn validate_rejects_inject_sans_declencheur() {
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "n1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Inject {
                config: InjectConfig::default(),
            },
        }],
    };
    let v = validate_graph(&g);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].code, "no_trigger");
}

fn node_notify(id: &str, targets: &[String], config: NotifyNodeConfig) -> FlowNode {
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: if targets.is_empty() {
            vec![]
        } else {
            vec![FlowWiring {
                port: 0,
                targets: targets.to_vec(),
            }]
        },
        inputs: vec![],
        kind: FlowNodeKind::PnexNotify { config },
    }
}

#[test]
fn validate_notify_rejete_config_incomplete() {
    // Bare node (editor make_node): no channel, no template, no wired
    // trigger.
    let g = FlowGraph {
        nodes: vec![node_notify("n1", &[], NotifyNodeConfig::default())],
    };
    let violations = validate_graph(&g);
    let codes: Vec<&str> = violations.iter().map(|x| x.code.as_str()).collect();
    assert!(codes.contains(&"notify_no_channels"), "{codes:?}");
    assert!(codes.contains(&"notify_no_template"), "{codes:?}");
    assert!(codes.contains(&"notify_trigger_required"), "{codes:?}");

    // Complete (trigger wired): no notify violation left (the actual
    // existence of the objects is the deploy's business, not
    // validate_graph's).
    let ok = FlowGraph {
        nodes: vec![FlowNode {
            id: "n1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![crate::FlowInputWiring {
                pin: crate::NOTIFY_TRIGGER_PIN.into(),
                from: "logic".into(),
                from_port: 0,
            }],
            kind: FlowNodeKind::PnexNotify {
                config: NotifyNodeConfig {
                    channel_ids: vec![uuid::Uuid::from_u128(42)],
                    template_id: uuid::Uuid::from_u128(7),
                    strict: false,
                    vars: Default::default(),
                    ..Default::default()
                },
            },
        }],
    };
    assert!(validate_graph(&ok).is_empty(), "{:?}", validate_graph(&ok));
}

#[test]
fn notify_serde_roundtrip_minimal() {
    // Forme minimale (tous champs par défaut absents) acceptée —
    // make_node de l'éditeur émet exactement cette config.
    let n: FlowNode =
        serde_json::from_str(r#"{"id":"n1","kind":"pnex_notify","config":{"template_id":null}}"#)
            .unwrap();
    assert!(matches!(&n.kind, FlowNodeKind::PnexNotify { config }
        if config.channel_ids.is_empty() && config.template_id.is_nil() && !config.strict));
}

#[test]
fn projection_notify_refs_sans_snapshot() {
    let meta = FlowArtifactMeta {
        flow_id: 12,
        version_number: 3,
        org_id: 7,
        o2_org: "o2_7".into(),
    };
    let g = FlowGraph {
        nodes: vec![node_notify(
            "n2",
            &["n3".into()],
            NotifyNodeConfig {
                channel_ids: vec![uuid::Uuid::nil()],
                template_id: uuid::Uuid::nil(),
                strict: true,
                vars: [("seuil".to_string(), "{{ msg.value }}".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        )],
    };
    let out = to_red_flows_json(&g, &meta);
    assert_eq!(out[1]["type"], "pnex-notify");
    assert_eq!(out[1]["pnex_node_id"], "n2");
    assert_eq!(out[1]["pnex_flow_id"], 12);
    assert_eq!(out[1]["pnex_org_id"], 7);
    assert_eq!(out[1]["strict"], true);
    assert_eq!(out[1]["vars"]["seuil"], "{{ msg.value }}");
    assert_eq!(out[1]["wires"], json!([["pnexflow12_n3"]]));
    // Pas de snapshot dans la projection : il est estampé au deploy par
    // le backend (reproject_candidate).
    assert!(out[1].get("pnex_notify_channels").is_none());
}

#[test]
fn projection_notify_stample_vars_et_anti_spam() {
    let meta = FlowArtifactMeta {
        flow_id: 33,
        version_number: 1,
        org_id: 7,
        o2_org: String::new(),
    };
    let g = FlowGraph {
        nodes: vec![node_notify(
            "n1",
            &[],
            NotifyNodeConfig {
                template_vars: vec!["seuil".into(), "value".into()],
                anti_spam: Some(crate::AntiSpamConfig {
                    max_msgs: 3,
                    window_secs: 600,
                }),
                ..Default::default()
            },
        )],
    };
    let out = to_red_flows_json(&g, &meta);
    let notify = out.as_array().unwrap().iter().nth(1).unwrap();
    assert_eq!(notify["template_vars"], json!(["seuil", "value"]));
    assert_eq!(
        notify["anti_spam"],
        json!({"max_msgs": 3, "window_secs": 600})
    );
    // Nœud sans stamp ni règle (vieux graphes) : clés projetées vides —
    // json! ne skip pas, mais `[`]/`null` sont équivalents à l'absence
    // pour le runtime (serde default).
    let bare = to_red_flows_json(
        &FlowGraph {
            nodes: vec![node_notify("n1", &[], NotifyNodeConfig::default())],
        },
        &meta,
    );
    let notify = bare.as_array().unwrap().iter().nth(1).unwrap();
    assert_eq!(notify["template_vars"], json!([]));
    assert!(notify["anti_spam"].is_null());
}

#[test]
fn projection_notify_tagger_sur_var_template() {
    // inject i1 → notify n1, fil annoté sur la var stampée « seuil » :
    // la projection doit insérer un tagger (topic = « seuil ») entre les
    // deux, même sémantique que les ancres d'entrée fonction.
    let meta = FlowArtifactMeta {
        flow_id: 34,
        version_number: 2,
        org_id: 7,
        o2_org: String::new(),
    };
    let g = FlowGraph {
        nodes: vec![
            FlowNode {
                id: "i1".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["n1".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::Inject {
                    config: InjectConfig::default(),
                },
            },
            FlowNode {
                id: "n1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![crate::FlowInputWiring {
                    pin: "seuil".into(),
                    from: "i1".into(),
                    from_port: 0,
                }],
                kind: FlowNodeKind::PnexNotify {
                    config: NotifyNodeConfig {
                        template_vars: vec!["seuil".into()],
                        ..Default::default()
                    },
                },
            },
        ],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    let tab = "pnexflow34";
    let tagger_id = format!("{tab}_ti1p0_seuil");
    let inject = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("inject"))
        .expect("inject absent");
    assert_eq!(inject["wires"][0], json!([tagger_id]), "inject → tagger");
    let tagger = out
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(tagger_id.as_str()))
        .expect("tagger absent");
    assert_eq!(tagger["wires"][0][0], format!("{tab}_n1"));
    assert_eq!(tagger["pnex_node_id"], "n1");
    // Le nœud notify est bien projeté (pas de snapshot, école D50).
    let notify = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("pnex-notify"))
        .expect("notify absent");
    assert!(notify.get("pnex_notify_channels").is_none());
}

#[test]
fn projection_notify_trigger_tagger_et_flag() {
    // Wire annotated on the `trigger` gate row → the projection emits a
    // tagger stamping `topic = "trigger"` and stamps the deploy-derived
    // gate flag on the node entry. A sibling notify without the wire
    // stays flag-false (legacy behavior untouched).
    let meta = FlowArtifactMeta {
        flow_id: 35,
        version_number: 1,
        org_id: 7,
        o2_org: String::new(),
    };
    let notify = |id: &str, trigger_wired: bool| FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: if trigger_wired {
            vec![crate::FlowInputWiring {
                pin: "trigger".into(),
                from: "i1".into(),
                from_port: 0,
            }]
        } else {
            vec![]
        },
        kind: FlowNodeKind::PnexNotify {
            config: NotifyNodeConfig {
                channel_ids: vec![uuid::Uuid::from_u128(1)],
                template_id: uuid::Uuid::from_u128(2),
                ..Default::default()
            },
        },
    };
    let g = FlowGraph {
        nodes: vec![
            FlowNode {
                id: "i1".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["n1".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::Inject {
                    config: InjectConfig::default(),
                },
            },
            notify("n1", true),
            notify("n2", false),
        ],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    let tab = "pnexflow35";
    let tagger_id = format!("{tab}_ti1p0_trigger");
    let tagger = out
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(tagger_id.as_str()))
        .expect("trigger tagger absent");
    assert_eq!(tagger["wires"][0][0], format!("{tab}_n1"));
    let is_notify =
        |e: &serde_json::Value| e.get("type").and_then(|t| t.as_str()) == Some("pnex-notify");
    let n1 = out
        .iter()
        .find(|e| is_notify(e) && e.get("pnex_node_id").and_then(|v| v.as_str()) == Some("n1"))
        .expect("n1 absent");
    assert_eq!(n1["pnex_notify_trigger"], json!(true));
    let n2 = out
        .iter()
        .find(|e| is_notify(e) && e.get("pnex_node_id").and_then(|v| v.as_str()) == Some("n2"))
        .expect("n2 absent");
    assert_eq!(n2["pnex_notify_trigger"], json!(false));
}

#[test]
fn validate_notify_trigger_conflit_nom_var() {
    // A template var literally named `trigger` collides with the gate
    // row ONLY when the trigger input is actually wired.
    let wired = FlowGraph {
        nodes: vec![FlowNode {
            id: "n1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![crate::FlowInputWiring {
                pin: "trigger".into(),
                from: "i1".into(),
                from_port: 0,
            }],
            kind: FlowNodeKind::PnexNotify {
                config: NotifyNodeConfig {
                    channel_ids: vec![uuid::Uuid::from_u128(1)],
                    template_id: uuid::Uuid::from_u128(2),
                    template_vars: vec!["trigger".into()],
                    ..Default::default()
                },
            },
        }],
    };
    let violations = validate_graph(&wired);
    let codes: Vec<&str> = violations.iter().map(|c| c.code.as_str()).collect();
    assert!(codes.contains(&"notify_trigger_conflict"), "{codes:?}");

    // Same template, trigger NOT wired: the collision is gone — only
    // the mandatory-gate violation remains (the var would fill
    // normally from its own topic).
    let unwired = FlowGraph {
        nodes: wired
            .nodes
            .into_iter()
            .map(|mut n| {
                n.inputs.clear();
                n
            })
            .collect(),
    };
    let violations = validate_graph(&unwired);
    let codes: Vec<&str> = violations.iter().map(|c| c.code.as_str()).collect();
    assert!(!codes.contains(&"notify_trigger_conflict"), "{codes:?}");
    assert!(codes.contains(&"notify_trigger_required"), "{codes:?}");
}

#[test]
fn validate_notify_anti_spam_rejete_zero() {
    let codes_of = |anti: crate::AntiSpamConfig| {
        validate_graph(&FlowGraph {
            nodes: vec![node_notify(
                "n1",
                &["d".to_string()],
                NotifyNodeConfig {
                    channel_ids: vec![uuid::Uuid::from_u128(42)],
                    template_id: uuid::Uuid::from_u128(7),
                    anti_spam: Some(anti),
                    ..Default::default()
                },
            )],
        })
    };
    let bad = codes_of(crate::AntiSpamConfig {
        max_msgs: 0,
        window_secs: 0,
    });
    assert!(
        bad.iter().any(|c| c.code == "notify_anti_spam_max"),
        "{bad:?}"
    );
    assert!(
        bad.iter().any(|c| c.code == "notify_anti_spam_window"),
        "{bad:?}"
    );
    // Règle valide : aucune violation anti-spam (les violations
    // notify_* restantes visent d'autres nœuds absents ici).
    let ok = codes_of(crate::AntiSpamConfig {
        max_msgs: 3,
        window_secs: 60,
    });
    assert!(
        ok.iter().all(|c| !c.code.starts_with("notify_anti_spam")),
        "{ok:?}"
    );
}

#[test]
fn projection_flows_json_snapshot() {
    let meta = FlowArtifactMeta {
        flow_id: 12,
        version_number: 3,
        org_id: 7,
        o2_org: "o2_7".into(),
    };
    let out = to_red_flows_json(&simple_graph(), &meta);
    assert_eq!(
        out[0],
        json!({
            "id": "pnexflow12", "type": "tab", "label": "Flow #12 v3",
            "pnex_flow_id": 12, "pnex_version": 3, "pnex_org_id": 7,
            "pnex_o2_org": "o2_7",
        })
    );
    // inject : intervalle projeté, payload JSON encodé en chaîne.
    assert_eq!(out[1]["type"], "inject");
    assert_eq!(out[1]["repeat"], 1.0);
    assert_eq!(out[1]["payloadType"], "date");
    // Custom node: custom type + version traceability.
    assert_eq!(out[2]["type"], "pnex-value");
    assert_eq!(out[2]["value"], 1);
    assert_eq!(out[2]["pnex_flow_id"], 12);
    assert_eq!(out[2]["pnex_version"], 3);
    assert_eq!(out[2]["z"], "pnexflow12");
    // debug : câblage vide, capture payload par défaut.
    assert_eq!(out[3]["type"], "debug");
    assert_eq!(out[3]["complete"], "payload");
    // wires : n2 (port 0) → n3 ; n3 sans port de sortie → tableau vide
    // (les ids cibles sont préfixés par tab — uniques dans l'artefact).
    assert_eq!(out[2]["wires"], json!([["pnexflow12_n3"]]));
    assert_eq!(out[3]["wires"], json!([]));
    // position reportée.
    assert_eq!(out[2]["x"], 200.0);
    assert_eq!(out[2]["name"], "query");
}

#[test]
fn projection_rende_les_ids_uniques_entre_flows() {
    // Non-régression : l'éditeur génère des ids courts par flow ("n1"…),
    // deux flows déployés peuvent donc porter les mêmes. Avant préfixage,
    // l'artefact concaténé contenait des ids en collision et le runtime
    // refusait TOUT l'artefact (« Referenced node not found ») — un flow
    // pouvait tuer tous les autres.
    let meta = |flow_id| FlowArtifactMeta {
        flow_id,
        version_number: 1,
        org_id: 7,
        o2_org: "o2_7".into(),
    };
    let a = to_red_flows_json(&simple_graph(), &meta(12));
    let b = to_red_flows_json(&simple_graph(), &meta(13));
    let mut ids: Vec<String> = [a, b]
        .iter()
        .flat_map(|artifact| artifact.as_array().expect("projection = tableau").clone())
        .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    ids.sort();
    let total = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), total, "ids dupliqués dans l'artefact concaténé");

    // Les cibles de wires suivent le même préfixe.
    let b = to_red_flows_json(&simple_graph(), &meta(13));
    assert_eq!(b[2]["wires"], json!([["pnexflow13_n3"]]));
}

fn node_device_read(id: &str, device_id: &str, pins: &[&str]) -> FlowNode {
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::DeviceRead {
            config: DeviceReadConfig {
                device_id: device_id.into(),
                pins: pins.iter().map(|p| (*p).into()).collect(),
                window_secs: 60.0,
            },
        },
    }
}

fn node_device_write(id: &str, device_id: &str, pins: &[&str]) -> FlowNode {
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::DeviceWrite {
            config: DeviceWriteConfig {
                device_id: device_id.into(),
                pins: pins.iter().map(|p| (*p).into()).collect(),
            },
        },
    }
}

#[test]
fn validate_calc_casse_des_variables_device() {
    // Non-régression : une variable calc qui ne diffère d'une clé device
    // du graphe que par la casse est signalée (`proud_puffin_a0` vs
    // `proud_puffin_A0`) — l'erreur ne devait sinon apparaître qu'au
    // runtime (calc rejeté → display/metric en aval muets).
    let g = FlowGraph {
        nodes: vec![
            node_inject("n1"),
            node_device_read("d1", "proud-puffin", &["A0"]),
            FlowNode {
                id: "n3".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::Calc {
                    config: CalcConfig {
                        expression: "proud_puffin_a0".into(),
                    },
                },
            },
        ],
    };
    let violations = validate_graph(&g);
    assert!(
        violations.iter().any(|x| x.code == "calc_case_mismatch"),
        "{violations:?}"
    );

    // Bonne casse (constantes et autres identifiants non concernés) : vert.
    let g = FlowGraph {
        nodes: vec![
            node_inject("n1"),
            node_device_read("d1", "proud-puffin", &["A0"]),
            FlowNode {
                id: "n3".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::Calc {
                    config: CalcConfig {
                        expression: "proud_puffin_A0 * pi".into(),
                    },
                },
            },
        ],
    };
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
}

#[test]
fn validate_coolprop_node() {
    // Config vide → les codes attendus.
    let mut g = FlowGraph {
        nodes: vec![FlowNode {
            id: "cp1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::CoolProp {
                config: CoolPropConfig::default(),
            },
        }],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    for expected in [
        "coolprop_fluid_missing",
        "coolprop_input_missing",
        "coolprop_outputs_missing",
    ] {
        assert!(codes.contains(&expected.to_string()), "{codes:?}");
    }

    // Config complète + déclencheur → vert.
    g.nodes = vec![
        node_inject("n1"),
        FlowNode {
            id: "cp1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::CoolProp {
                config: CoolPropConfig {
                    fluid_spec: "Water".into(),
                    input1: "T".into(),
                    input2: "P".into(),
                    v1_key: "t".into(),
                    v2_key: "p".into(),
                    outputs: vec!["Dmolar".into(), "Hmolar".into()],
                    include_phase: true,
                    ..Default::default()
                },
            },
        },
    ];
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));

    // Projection : type + config + stamps.
    let meta = FlowArtifactMeta {
        flow_id: 7,
        version_number: 2,
        org_id: 42,
        o2_org: "o2-x".into(),
    };
    let red = to_red_flows_json(&g, &meta);
    let entry = red[2].as_object().unwrap();
    assert_eq!(entry["type"], "pnex-coolprop");
    assert_eq!(entry["fluid_spec"], "Water");
    assert_eq!(entry["pnex_flow_id"], 7);
}

fn coolprop_units_config() -> CoolPropConfig {
    CoolPropConfig {
        fluid_spec: "R32[0.5]&R125[0.5]".into(),
        fluid_label: "R410A-site".into(),
        input1: "T".into(),
        input2: "P".into(),
        v1_key: "in1".into(),
        v2_key: "in2".into(),
        unit1: "degC".into(),
        unit2: "barg".into(),
        outputs: vec!["Hmass".into(), "superheat".into()],
        output_units: [("Hmass".to_string(), "kJ/kg".to_string())].into(),
        include_phase: true,
    }
}

#[test]
fn coolprop_units_are_checked_against_the_quantity() {
    let mut config = coolprop_units_config();
    let node = |config: CoolPropConfig| FlowGraph {
        nodes: vec![
            node_inject("n1"),
            FlowNode {
                id: "cp1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::CoolProp { config },
            },
        ],
    };
    assert!(validate_graph(&node(config.clone())).is_empty());

    // A pressure unit on a temperature input is rejected.
    config.unit1 = "barg".into();
    config
        .output_units
        .insert("superheat".into(), "kJ/kg".into());
    let codes: Vec<String> = validate_graph(&node(config.clone()))
        .iter()
        .map(|x| x.code.clone())
        .collect();
    assert_eq!(
        codes,
        ["coolprop_unit_invalid", "coolprop_unit_invalid"],
        "{codes:?}"
    );

    // Same quantity twice / same key twice.
    let mut config = coolprop_units_config();
    config.input2 = "T".into();
    config.unit2 = "degC".into();
    config.v2_key = "in1".into();
    let codes: Vec<String> = validate_graph(&node(config))
        .iter()
        .map(|x| x.code.clone())
        .collect();
    assert!(
        codes.contains(&"coolprop_inputs_identical".to_string()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"coolprop_input_keys_identical".to_string()),
        "{codes:?}"
    );
}

#[test]
fn coolprop_projection_pads_ports_and_routes_named_inputs() {
    let g = FlowGraph {
        nodes: vec![
            FlowNode {
                id: "n1".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["cp1".into()],
                }],
                ..node_inject("n1")
            },
            FlowNode {
                id: "cp1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![FlowInputWiring {
                    pin: "in2".into(),
                    from: "n1".into(),
                    from_port: 0,
                }],
                kind: FlowNodeKind::CoolProp {
                    config: coolprop_units_config(),
                },
            },
        ],
    };
    let meta = FlowArtifactMeta {
        flow_id: 7,
        version_number: 2,
        org_id: 42,
        o2_org: "o2-x".into(),
    };
    let red = to_red_flows_json(&g, &meta);
    let entries = red.as_array().unwrap();
    let cp = entries
        .iter()
        .find(|e| e["type"] == "pnex-coolprop")
        .unwrap();
    // all + 2 outputs + phase.
    assert_eq!(cp["wires"].as_array().unwrap().len(), 4, "{cp}");
    assert_eq!(cp["unit2"], "barg");
    assert_eq!(cp["output_units"]["Hmass"], "kJ/kg");
    // The inject wire goes through a tagger stamping `topic = in2`.
    let tagger = entries
        .iter()
        .find(|e| e["type"] == "function" && e["func"].as_str().is_some_and(|f| f.contains("in2")))
        .unwrap_or_else(|| panic!("no tagger in {red}"));
    assert_eq!(
        tagger["wires"][0][0].as_str().unwrap().ends_with("_cp1"),
        true
    );
}

#[test]
fn validate_device_read_write() {
    // Read : pins vides → device_no_pins.
    let g = FlowGraph {
        nodes: vec![node_device_read("d1", "fuzzy-zebra", &[])],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    assert!(codes.contains(&"device_no_pins".to_string()), "{codes:?}");

    // Read : complet + déclencheur → vert.
    let g = FlowGraph {
        nodes: vec![
            node_inject("n1"),
            node_device_read("d1", "fuzzy-zebra", &["D1", "A0"]),
        ],
    };
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));

    // Read : slug invalide + pin vide + pin dupliquée + fenêtre hors bornes.
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "d1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::DeviceRead {
                config: DeviceReadConfig {
                    device_id: "deux mots".into(),
                    pins: vec!["D1".into(), "  ".into(), "d1".into()],
                    window_secs: 0.5,
                },
            },
        }],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    for code in [
        "device_bad_slug",
        "device_bad_pin",
        "device_duplicate_pin",
        "device_window_range",
    ] {
        assert!(codes.contains(&code.to_string()), "{codes:?}");
    }

    // Write : pins vides → device_no_pins ; complet → vert.
    let g = FlowGraph {
        nodes: vec![node_device_write("w1", "fuzzy-zebra", &[])],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    assert!(codes.contains(&"device_no_pins".to_string()), "{codes:?}");

    let g = FlowGraph {
        nodes: vec![node_device_write("w1", "fuzzy-zebra", &["relais_1"])],
    };
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
}

#[test]
fn write_pin_en_conflit_avec_carte_regulation() {
    // Un device-write et une carte régulation sur la même (device, pin)
    // se battraient pour la sortie — même unicité qu'entre cartes.
    let g = FlowGraph {
        nodes: vec![
            node_device_write("w1", "fuzzy-zebra", &["relais_1"]),
            FlowNode {
                id: "r1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::RegTtHeat {
                    config: RegTtConfig {
                        device_id: "fuzzy-zebra".into(),
                        sensor_pin: "sonde".into(),
                        actuator_pin: "Relais_1".into(),
                        ..RegTtConfig::default()
                    },
                },
            },
        ],
    };
    let violations = validate_graph(&g);
    assert!(
        violations.iter().any(|x| x.code == "reg_actuator_conflict"),
        "{violations:?}"
    );
}

#[test]
fn device_pin_refs_of_parcourt_les_deux_kinds() {
    let g = FlowGraph {
        nodes: vec![
            node_device_read("d1", "fuzzy-zebra", &["D1", "A0"]),
            node_device_write("w1", "fuzzy-zebra", &["Relais_1"]),
            node_device_read("d2", "autre-device", &["D1"]),
            FlowNode {
                id: "r1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::RegTtHeat {
                    config: RegTtConfig {
                        device_id: "fuzzy-zebra".into(),
                        sensor_pin: "Sonde".into(),
                        actuator_pin: "relais_1".into(),
                        ..RegTtConfig::default()
                    },
                },
            },
        ],
    };
    let mut refs = device_pin_refs_of(&g, "fuzzy-zebra");
    refs.sort_by(|a, b| {
        format!("{:?}", a.usage)
            .cmp(&format!("{:?}", b.usage))
            .then(a.pin.cmp(&b.pin))
    });
    assert_eq!(refs.len(), 5, "{refs:?}");
    assert!(refs
        .iter()
        .all(|r| matches!(r.pin.as_str(), "d1" | "a0" | "relais_1" | "sonde")));
    assert!(refs
        .iter()
        .any(|r| r.node_id == "d1" && r.pin == "d1" && r.usage == DevicePinUsage::Read));
    assert!(refs
        .iter()
        .any(|r| r.node_id == "w1" && r.pin == "relais_1" && r.usage == DevicePinUsage::Write));
    assert!(refs
        .iter()
        .any(|r| r.node_id == "r1" && r.pin == "sonde" && r.usage == DevicePinUsage::Read));
    // The card's actuator pin is a write claim: flipping its mode must
    // hit the same 409/stop guard as a device-write pin.
    assert!(refs
        .iter()
        .any(|r| r.node_id == "r1" && r.pin == "relais_1" && r.usage == DevicePinUsage::Write));
}

#[test]
fn device_write_pin_refs_of_collects_write_sources() {
    let g = FlowGraph {
        nodes: vec![
            node_device_write("w1", "fuzzy-zebra", &["Relais_1"]),
            node_device_read("d1", "fuzzy-zebra", &["D1", "A0"]),
            FlowNode {
                id: "r1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::RegTtHeat {
                    config: RegTtConfig {
                        device_id: "fuzzy-zebra".into(),
                        sensor_pin: "Sonde".into(),
                        actuator_pin: "Relais_1".into(),
                        ..RegTtConfig::default()
                    },
                },
            },
            node_device_write("w2", "autre-device", &["D1"]),
        ],
    };
    let mut refs = device_write_pin_refs_of(&g);
    refs.sort_by(|a, b| {
        a.node_id
            .cmp(&b.node_id)
            .then(a.device_id.cmp(&b.device_id))
    });
    // Reads are not claims; both writers of (fuzzy-zebra, relais_1) are.
    assert_eq!(refs.len(), 3, "{refs:?}");
    assert_eq!(refs[0].node_id, "r1");
    assert_eq!(refs[0].device_id, "fuzzy-zebra");
    assert_eq!(refs[0].pin, "relais_1");
    assert_eq!(refs[1].node_id, "w1");
    assert_eq!(refs[1].device_id, "fuzzy-zebra");
    assert_eq!(refs[1].pin, "relais_1");
    assert_eq!(refs[2].node_id, "w2");
    assert_eq!(refs[2].device_id, "autre-device");
    assert_eq!(refs[2].pin, "d1");
}

#[test]
fn calc_et_metric_valides() {
    // Calc : expression invalide rejetée, variables tolérées.
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "c1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Calc {
                config: CalcConfig {
                    expression: "foo(a) + ".into(),
                },
            },
        }],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    assert!(
        codes.contains(&"calc_bad_expression".to_string()),
        "{codes:?}"
    );

    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "c1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Calc {
                config: CalcConfig {
                    expression: "a + b".into(),
                },
            },
        }],
    };
    assert!(validate_graph(&g).is_empty());

    // Metric : nom requis.
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "m1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Metric {
                config: MetricConfig {
                    metric_name: "  ".into(),
                },
            },
        }],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    assert!(
        codes.contains(&"metric_name_missing".to_string()),
        "{codes:?}"
    );
}

#[test]
fn projection_noeuds_phase6_estampilles() {
    let g = FlowGraph {
        nodes: vec![
            node_device_read("d1", "fuzzy-zebra", &["D1"]),
            FlowNode {
                id: "c1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::Calc {
                    config: CalcConfig {
                        expression: "a * 2".into(),
                    },
                },
            },
            FlowNode {
                id: "m1".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::Metric {
                    config: MetricConfig {
                        metric_name: "moyenne".into(),
                    },
                },
            },
        ],
    };
    let out = to_red_flows_json(
        &g,
        &FlowArtifactMeta {
            flow_id: 9,
            version_number: 2,
            org_id: 4,
            o2_org: "o2_4".into(),
        },
    );
    assert_eq!(out[0]["pnex_org_id"], 4);
    assert_eq!(out[1]["type"], "pnex-device-read");
    assert_eq!(out[1]["pnex_flow_id"], 9);
    assert_eq!(out[1]["pnex_version"], 2);
    assert_eq!(out[1]["pnex_org_id"], 4);
    assert_eq!(out[1]["window_secs"], 60.0);
    assert_eq!(out[1]["device_id"], "fuzzy-zebra");
    assert_eq!(out[1]["pins"], json!(["D1"]));
    // Un port par pin + le port « tout » : wires paddés à n+1.
    assert_eq!(out[1]["wires"].as_array().unwrap().len(), 2);
    assert_eq!(out[2]["type"], "pnex-calc");
    assert_eq!(out[2]["expression"], "a * 2");
    assert_eq!(out[3]["type"], "pnex-metric");
    assert_eq!(out[3]["metric_name"], "moyenne");
    assert_eq!(out[3]["pnex_org_id"], 4);
}

#[test]
fn display_sans_config_est_valide() {
    // La sonde n'a aucune config saisie : défaut → graphe vert.
    let g = FlowGraph {
        nodes: vec![node_inject("n1"), node_display("n2", &[])],
    };
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
    // Forme minimale (config absente) acceptée par le désérialiseur.
    let g: FlowGraph = serde_json::from_str(r#"{"nodes":[{"id":"n1","kind":"display"}]}"#).unwrap();
    assert!(matches!(g.nodes[0].kind, FlowNodeKind::Display { .. }));
}

#[test]
fn projection_noeud_display_estampille() {
    let g = FlowGraph {
        nodes: vec![
            node_inject("n1"),
            node_display("n3", &["n4".into()]),
            FlowNode {
                id: "n4".into(),
                name: None,
                position: None,
                outputs: vec![],
                inputs: vec![],
                kind: FlowNodeKind::Debug {
                    config: DebugConfig::default(),
                },
            },
        ],
    };
    let out = to_red_flows_json(
        &g,
        &FlowArtifactMeta {
            flow_id: 12,
            version_number: 3,
            org_id: 7,
            o2_org: "o2_7".into(),
        },
    );
    assert_eq!(out[0]["id"], "pnexflow12");
    assert_eq!(out[2]["type"], "pnex-display");
    // L'id canvas brut est la clé de rattachement panneau/badge.
    assert_eq!(out[2]["pnex_node_id"], "n3");
    assert_eq!(out[2]["pnex_flow_id"], 12);
    assert_eq!(out[2]["pnex_version"], 3);
    assert_eq!(out[2]["pnex_org_id"], 7);
    assert_eq!(out[2]["z"], "pnexflow12");
    assert_eq!(out[2]["wires"], json!([["pnexflow12_n4"]]));
    // Le debug builtin reste un nœud builtin (pas de stamp custom requis).
    assert_eq!(out[3]["type"], "debug");
}

#[test]
fn projection_debug_tosidebar_toujours_vrai() {
    // Garde-fou panneau : sans `tosidebar`, le nœud debug builtin ne
    // publie jamais sur le canal — l'éditeur en dépend.
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "n2".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Debug {
                config: DebugConfig::default(),
            },
        }],
    };
    let out = to_red_flows_json(
        &g,
        &FlowArtifactMeta {
            flow_id: 1,
            version_number: 1,
            org_id: 1,
            o2_org: "o2_1".into(),
        },
    );
    assert_eq!(out[1]["tosidebar"], true);
    assert_eq!(out[1]["active"], true);
}

#[test]
fn contrats_payload_phase6() {
    // Sortie device / entrée calc : objet numérique, bool → 1/0.
    let payload = json!({"cap_1_D1": 21.5, "cap_2_D0": true});
    let map = numeric_map_from_payload(Some(&payload), "calc").unwrap();
    assert_eq!(map["cap_1_D1"], 21.5);
    assert_eq!(map["cap_2_D0"], 1.0);
    // Non-objet → rejet typé.
    assert_eq!(
        numeric_map_from_payload(Some(&json!(42)), "calc")
            .unwrap_err()
            .code,
        "calc_input_contract"
    );
    assert_eq!(
        numeric_map_from_payload(None, "calc").unwrap_err().code,
        "calc_input_contract"
    );
    // Valeur non numérique → rejet avec la clé en cause.
    assert_eq!(
        numeric_map_from_payload(Some(&json!({"k": "texte"})), "calc")
            .unwrap_err()
            .message,
        "valeur non numérique pour la clé « k » (reçu : chaîne)"
    );
    // Entrée metric : nombre, bool, sinon rejet.
    assert_eq!(metric_value_from_payload(Some(&json!(21.5))).unwrap(), 21.5);
    assert_eq!(metric_value_from_payload(Some(&json!(true))).unwrap(), 1.0);
    assert_eq!(
        metric_value_from_payload(Some(&json!({"a": 1})))
            .unwrap_err()
            .code,
        "metric_input_contract"
    );
}

#[test]
fn projection_red_passthrough_conserve_la_config() {
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "r1".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Red {
                type_name: "change".into(),
                config: json!({"rules": [{"p": "payload"}]}),
            },
        }],
    };
    let out = to_red_flows_json(
        &g,
        &FlowArtifactMeta {
            flow_id: 1,
            version_number: 1,
            org_id: 7,
            o2_org: "o2_7".into(),
        },
    );
    assert_eq!(out[1]["type"], "change");
    assert_eq!(out[1]["rules"], json!([{"p": "payload"}]));
    assert_eq!(out[1]["z"], "pnexflow1");
}

#[test]
fn dto_create_flow_deserialise() {
    let json = r#"{
        "name": "t",
        "device_id": 3,
        "author": "alice",
        "graph": {"nodes":[{"id":"n1","kind":"inject","config":{"repeat_secs":5.0}}]}
    }"#;
    let c: CreateFlow = serde_json::from_str(json).unwrap();
    assert_eq!(c.name, "t");
    assert_eq!(c.device_id, Some(3));
    assert_eq!(validate_graph(&c.graph).len(), 0);

    // Concurrence optimiste : UpdateFlow porte la version attendue.
    let u: UpdateFlow =
        serde_json::from_str(r#"{"expected_version_number": 2, "graph": {"nodes": []}}"#).unwrap();
    assert_eq!(u.expected_version_number, 2);
}

fn node_reg_tt(id: &str, kind_heat: bool, patch: impl FnOnce(&mut RegTtConfig)) -> FlowNode {
    let mut config = RegTtConfig {
        device_id: "serre-1".into(),
        sensor_pin: "A0".into(),
        actuator_pin: "D1".into(),
        setpoint: 19.0,
        deadband: 0.5,
        ..Default::default()
    };
    patch(&mut config);
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: if kind_heat {
            FlowNodeKind::RegTtHeat { config }
        } else {
            FlowNodeKind::RegTtCool { config }
        },
    }
}

fn node_reg_pid(id: &str, patch: impl FnOnce(&mut RegPidConfig)) -> FlowNode {
    let mut config = RegPidConfig {
        device_id: "serre-1".into(),
        sensor_pin: "A0".into(),
        actuator_pin: "D1".into(),
        setpoint: 19.0,
        kp: 2.0,
        ki: 0.1,
        kd: 0.5,
        ..Default::default()
    };
    patch(&mut config);
    FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind: FlowNodeKind::RegPid { config },
    }
}

#[test]
fn reg_configs_serde_roundtrip_et_defauts() {
    // Forme minimale : les champs secondaires portent leurs défauts.
    let g: FlowGraph = serde_json::from_str(
        r#"{"nodes":[{"id":"r1","kind":"reg_tt_heat","config":{"device_id":"d1","sensor_pin":"A0","actuator_pin":"D1","setpoint":19.0,"deadband":0.5}}]}"#,
    )
    .unwrap();
    assert!(
        matches!(&g.nodes[0].kind, FlowNodeKind::RegTtHeat { config }
        if config.min_on_secs == 5 && config.min_off_secs == 5
            && config.sample_ms == 5_000 && config.data_timeout_secs == 30
            && config.safe_state == SafeState::Low)
    );
    let json = serde_json::to_string(&g).unwrap();
    let back: FlowGraph = serde_json::from_str(&json).unwrap();
    assert_eq!(back, g);

    // PID + extraction unifiée (les 3 kinds) + kind_str.
    let g = FlowGraph {
        nodes: vec![
            node_reg_tt("r1", true, |_| {}),
            node_reg_tt("r2", false, |_| {}),
            node_reg_pid("r3", |_| {}),
        ],
    };
    let extracted = regulator_configs_of(&g);
    assert_eq!(extracted.len(), 3);
    assert_eq!(extracted[0].0, "r1");
    assert_eq!(extracted[0].1.kind_str(), "tt_heat");
    assert_eq!(extracted[1].1.kind_str(), "tt_cool");
    assert_eq!(extracted[2].1.kind_str(), "pid");
    assert_eq!(extracted[2].1.device_id(), "serre-1");
    assert_eq!(extracted[2].1.sensor_pin(), "A0");
    assert_eq!(extracted[2].1.actuator_pin(), "D1");
    assert_eq!(extracted[2].1.setpoint(), 19.0);
    assert_eq!(extracted[2].1.safe_state(), SafeState::Low);
}

#[test]
fn validate_reg_rejette_les_configs_invalides() {
    // Liaison incomplète + pins égaux.
    let g = FlowGraph {
        nodes: vec![node_reg_tt("r1", true, |c| {
            c.device_id = "deux mots".into();
            c.sensor_pin = "D1".into();
            c.actuator_pin = "d1".into();
        })],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    for code in ["reg_bad_device", "reg_pins_equal"] {
        assert!(codes.contains(&code.to_string()), "{codes:?}");
    }

    // Pin capteur manquant.
    let g = FlowGraph {
        nodes: vec![node_reg_tt("r1", false, |c| c.sensor_pin = "  ".into())],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    assert!(codes.contains(&"reg_pin_missing".to_string()), "{codes:?}");

    // Deadband nul + sample trop court + timeout incohérent.
    let g = FlowGraph {
        nodes: vec![node_reg_tt("r1", true, |c| {
            c.deadband = 0.0;
            c.sample_ms = 100;
            c.data_timeout_secs = 3_600_000;
        })],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    for code in [
        "reg_deadband_range",
        "reg_sample_range",
        "reg_data_timeout_range",
    ] {
        assert!(codes.contains(&code.to_string()), "{codes:?}");
    }

    // PID : gain négatif + NaN + cycle hors bornes.
    let g = FlowGraph {
        nodes: vec![node_reg_pid("r1", |c| {
            c.kp = -1.0;
            c.kd = f64::NAN;
            c.cycle_time_secs = 61;
        })],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    for code in ["reg_pid_gain_range", "reg_cycle_time_range"] {
        assert!(codes.contains(&code.to_string()), "{codes:?}");
    }

    // Configs valides (heat, cool, pid) → vert, sorties distinctes.
    let g = FlowGraph {
        nodes: vec![
            node_reg_tt("r1", true, |_| {}),
            node_reg_tt("r2", false, |c| c.actuator_pin = "D2".into()),
            node_reg_pid("r3", |c| c.actuator_pin = "D3".into()),
            node_inject("n1"),
        ],
    };
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
}

#[test]
fn validate_reg_unicite_actuateur_graphe_entier() {
    // Deux cartes sur la même sortie → conflit (même via 2 devices de
    // labels différents ? non — même device, même pin).
    let g = FlowGraph {
        nodes: vec![
            node_reg_tt("r1", true, |_| {}),
            node_reg_tt("r2", false, |_| {}),
        ],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    assert!(
        codes.contains(&"reg_actuator_conflict".to_string()),
        "{codes:?}"
    );

    // Sorties différentes → pas de conflit.
    let g = FlowGraph {
        nodes: vec![
            node_reg_tt("r1", true, |_| {}),
            node_reg_tt("r2", false, |c| c.actuator_pin = "D2".into()),
        ],
    };
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));

    // La casse du label est normalisée pour la détection.
    let g = FlowGraph {
        nodes: vec![
            node_reg_tt("r1", true, |_| {}),
            node_reg_pid("r2", |c| c.actuator_pin = "d1".into()),
        ],
    };
    let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
    assert!(
        codes.contains(&"reg_actuator_conflict".to_string()),
        "{codes:?}"
    );
}

#[test]
fn projection_reg_estampille() {
    let g = FlowGraph {
        nodes: vec![
            node_reg_tt("r1", true, |_| {}),
            node_reg_tt("r2", false, |c| c.actuator_pin = "D2".into()),
            node_reg_pid("r3", |c| c.actuator_pin = "D3".into()),
        ],
    };
    let out = to_red_flows_json(
        &g,
        &FlowArtifactMeta {
            flow_id: 21,
            version_number: 4,
            org_id: 7,
            o2_org: String::new(),
        },
    );
    // TT chauffage : type custom + liaison + paramètres + stamps.
    assert_eq!(out[1]["type"], "pnex-reg-tt-heat");
    assert_eq!(out[1]["device_id"], "serre-1");
    assert_eq!(out[1]["sensor_pin"], "A0");
    assert_eq!(out[1]["actuator_pin"], "D1");
    assert_eq!(out[1]["setpoint"], 19.0);
    assert_eq!(out[1]["deadband"], 0.5);
    assert_eq!(out[1]["min_on_secs"], 5);
    assert_eq!(out[1]["safe_state"], "low");
    assert_eq!(out[1]["pnex_node_id"], "r1");
    assert_eq!(out[1]["pnex_flow_id"], 21);
    assert_eq!(out[1]["pnex_version"], 4);
    assert_eq!(out[1]["pnex_org_id"], 7);
    assert_eq!(out[1]["z"], "pnexflow21");
    // Clim : type distinct, même config.
    assert_eq!(out[2]["type"], "pnex-reg-tt-cool");
    assert_eq!(out[2]["actuator_pin"], "D2");
    assert_eq!(out[2]["deadband"], 0.5);
    // PID : gains + cycle.
    assert_eq!(out[3]["type"], "pnex-reg-pid");
    assert_eq!(out[3]["actuator_pin"], "D3");
    assert_eq!(out[3]["kp"], 2.0);
    assert_eq!(out[3]["ki"], 0.1);
    assert_eq!(out[3]["kd"], 0.5);
    assert_eq!(out[3]["cycle_time_secs"], 10);
}

// ────────────────── Nœuds fonction (registre « Fonctions ») ──────────────────

use crate::functions::{FunctionInput, FunctionOutput, FunctionSnapshot, FunctionType};

fn fn_config_test() -> FunctionNodeConfig {
    FunctionNodeConfig {
        function_id: 9,
        function_name: "chauffage".into(),
        version_number: 2,
        language: FunctionLanguage::Js,
        inputs: vec![FunctionInput {
            name: "t".into(),
            ty: FunctionType::Number,
            default: None,
            desc: None,
        }],
        outputs: vec![
            FunctionOutput {
                name: "heating".into(),
                ty: FunctionType::Number,
                desc: None,
            },
            FunctionOutput {
                name: "alarm".into(),
                ty: FunctionType::Bool,
                desc: None,
            },
        ],
    }
}

fn fn_snapshot_test(lang: FunctionLanguage) -> FunctionSnapshot {
    FunctionSnapshot {
        function_id: 9,
        version_number: 2,
        language: lang,
        code: match lang {
            FunctionLanguage::Js => "function handle(inputs, msg) { return inputs.t; }".into(),
            FunctionLanguage::Starlark => "def handle(inputs, msg):".into(),
        },
        inputs: vec![FunctionInput {
            name: "t".into(),
            ty: FunctionType::Number,
            default: None,
            desc: None,
        }],
        outputs: vec![
            FunctionOutput {
                name: "heating".into(),
                ty: FunctionType::Number,
                desc: None,
            },
            FunctionOutput {
                name: "alarm".into(),
                ty: FunctionType::Bool,
                desc: None,
            },
        ],
    }
}

fn fn_graph() -> FlowGraph {
    FlowGraph {
        nodes: vec![FlowNode {
            id: "f1".into(),
            name: None,
            position: None,
            outputs: vec![FlowWiring {
                port: 0,
                targets: vec!["d1".into()],
            }],
            inputs: vec![],
            kind: FlowNodeKind::PnexFunction {
                config: fn_config_test(),
            },
        }],
    }
}

#[test]
fn projection_fonction_js_noeud_vendor() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    let mut fns = FunctionResolver::new();
    fns.insert((9, 2), fn_snapshot_test(FunctionLanguage::Js));
    let entries = to_red_flows_json_with(&fn_graph(), &meta, &fns);
    let out = entries.as_array().unwrap();
    let node = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("function"))
        .expect("nœud function vendor absent");
    assert_eq!(node["outputs"], 2);
    let func = node["func"].as_str().expect("func textuel");
    assert!(
        func.contains("function handle(inputs, msg)"),
        "code user inline"
    );
    assert!(func.contains("__pnex_inputs"), "objet inputs généré");
    assert!(func.contains("__pnex_get"), "helpers embarqués");
    assert_eq!(node["pnex_function_id"], 9);
    assert_eq!(node["pnex_function_version"], 2);
    assert_eq!(node["pnex_node_id"], "f1");
    // Deux ports déclarés, un seul câblé → wires padés à 2.
    let wires = node["wires"].as_array().unwrap();
    assert_eq!(wires.len(), 2, "wires padées au nb de ports");
    assert_eq!(wires[0], serde_json::json!(["pnexflow21_d1"]));
    assert_eq!(wires[1], serde_json::json!([]));
}

#[test]
fn projection_fonction_starlark() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    let mut fns = FunctionResolver::new();
    fns.insert((9, 2), fn_snapshot_test(FunctionLanguage::Starlark));
    let entries = to_red_flows_json_with(&fn_graph(), &meta, &fns);
    let out = entries.as_array().unwrap();
    let node = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("pnex-starlark"))
        .expect("nœud pnex-starlark absent");
    assert_eq!(node["code"], "def handle(inputs, msg):");
    assert_eq!(node["pnex_function_id"], 9);
    assert_eq!(node["pnex_function_version"], 2);
}

#[test]
fn projection_fonction_introuvable_comment_noop() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    let entries = to_red_flows_json_with(&fn_graph(), &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    let node = out
        .iter()
        .find(|e| e.get("type") == Some(&serde_json::json!("comment")))
        .expect("entrée comment absente");
    assert!(
        node["name"]
            .as_str()
            .unwrap()
            .contains("fonction introuvable"),
        "nom du comment"
    );
}

#[test]
fn projection_fonction_tagger_sur_ancre_entree() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    // inject i1 → fonction f1, fil annoté sur l'entrée déclarée « t » :
    // la projection doit insérer un tagger (topic = « t ») entre les deux.
    let g = FlowGraph {
        nodes: vec![
            FlowNode {
                id: "i1".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["f1".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::Inject {
                    config: InjectConfig::default(),
                },
            },
            FlowNode {
                id: "f1".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["d1".into()],
                }],
                inputs: vec![crate::FlowInputWiring {
                    pin: "t".into(),
                    from: "i1".into(),
                    from_port: 0,
                }],
                kind: FlowNodeKind::PnexFunction {
                    config: fn_config_test(),
                },
            },
        ],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    // Le fil de l'inject vise le tagger, plus le nœud fonction.
    let tagger_id = "pnexflow21_ti1p0_t";
    let inject = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("inject"))
        .expect("inject absent");
    assert_eq!(
        inject["wires"][0],
        serde_json::json!([tagger_id]),
        "inject → tagger"
    );
    // Le tagger estampe le topic et redresse vers la fonction.
    let tagger = out
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(tagger_id))
        .expect("tagger absent");
    assert_eq!(tagger["type"], serde_json::json!("function"));
    assert!(
        tagger["func"].as_str().unwrap().contains("topic: \"t\""),
        "topic estampé avec le nom de l'entrée"
    );
    assert_eq!(tagger["wires"][0], serde_json::json!(["pnexflow21_f1"]));
    assert_eq!(tagger["pnex_node_id"], "f1");
    // Le fil f1 → d1 (sortie, sans annotation) reste direct.
    let fnode = out
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some("pnexflow21_f1"))
        .expect("nœud fonction absent");
    assert_eq!(fnode["wires"][0], serde_json::json!(["pnexflow21_d1"]));
}

#[test]
fn projection_device_write_tagger_on_wired_input_anchor() {
    let meta = FlowArtifactMeta {
        flow_id: 22,
        version_number: 1,
        org_id: 7,
        o2_org: String::new(),
    };
    // Mirrors the real shape: wires drawn on a device-write's named
    // input anchors (G13/G14 declared in config.pins) must get topic
    // taggers at deploy, exactly like function declared inputs. A wire
    // annotated with an undeclared pin ("G99") must stay direct.
    let mut w1 = node_device_write("w1", "dev-a", &["G13", "G14"]);
    w1.inputs = vec![
        crate::FlowInputWiring {
            pin: "G13".into(),
            from: "i1".into(),
            from_port: 0,
        },
        crate::FlowInputWiring {
            pin: "G14".into(),
            from: "i2".into(),
            from_port: 0,
        },
        crate::FlowInputWiring {
            pin: "G99".into(),
            from: "i3".into(),
            from_port: 0,
        },
    ];
    let g = FlowGraph {
        nodes: vec![
            FlowNode {
                id: "i1".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["w1".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::Inject {
                    config: InjectConfig::default(),
                },
            },
            FlowNode {
                id: "i2".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["w1".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::Inject {
                    config: InjectConfig::default(),
                },
            },
            FlowNode {
                id: "i3".into(),
                name: None,
                position: None,
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["w1".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::Inject {
                    config: InjectConfig::default(),
                },
            },
            w1,
        ],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    // Sources of declared pins are retargeted to their taggers.
    for (src, tagger_id) in [
        ("pnexflow22_i1", "pnexflow22_ti1p0_G13"),
        ("pnexflow22_i2", "pnexflow22_ti2p0_G14"),
    ] {
        let node = out
            .iter()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(src))
            .expect("source node missing");
        assert_eq!(
            node["wires"][0],
            serde_json::json!([tagger_id]),
            "wire of {src} must be retargeted to the tagger"
        );
    }
    // Tagger stamps the pin name as topic and re-wires into the
    // device-write node.
    for (tagger_id, topic) in [
        ("pnexflow22_ti1p0_G13", "G13"),
        ("pnexflow22_ti2p0_G14", "G14"),
    ] {
        let tagger = out
            .iter()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(tagger_id))
            .expect("tagger missing");
        assert_eq!(tagger["type"], serde_json::json!("function"));
        assert!(
            tagger["func"]
                .as_str()
                .unwrap()
                .contains(&format!("topic: \"{topic}\"")),
            "topic must be stamped with the pin name"
        );
        assert_eq!(tagger["wires"][0], serde_json::json!(["pnexflow22_w1"]));
        assert_eq!(tagger["pnex_node_id"], "w1");
    }
    // Undeclared pin: the wire stays direct (no tagger, no routing).
    let i3 = out
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some("pnexflow22_i3"))
        .expect("i3 missing");
    assert_eq!(
        i3["wires"][0],
        serde_json::json!(["pnexflow22_w1"]),
        "undeclared pin must not be routed"
    );
    assert!(
        !out.iter()
            .any(|e| e.get("id").and_then(|v| v.as_str()) == Some("pnexflow22_ti3p0_G99")),
        "no tagger for an undeclared pin"
    );
    // The device-write artifact entry itself is unchanged.
    let wnode = out
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some("pnexflow22_w1"))
        .expect("device-write node missing");
    assert_eq!(wnode["type"], serde_json::json!("pnex-device-write"));
    assert_eq!(wnode["device_id"], serde_json::json!("dev-a"));
    assert_eq!(wnode["pins"], serde_json::json!(["G13", "G14"]));
    assert_eq!(wnode["wires"], serde_json::json!([]));
}

#[test]
fn projection_json_split_noeud_function() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "s1".into(),
            name: None,
            position: None,
            outputs: vec![FlowWiring {
                port: 0,
                targets: vec!["m1".into()],
            }],
            inputs: vec![],
            kind: FlowNodeKind::JsonSplit {
                config: JsonSplitConfig::default(),
            },
        }],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    let node = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("function"))
        .expect("nœud function vendor absent");
    assert_eq!(node["outputs"], 1);
    let func = node["func"].as_str().expect("func textuel");
    assert!(
        func.contains("Object.keys(p).map"),
        "fan-out par clé (objet)"
    );
    assert!(func.contains("Array.isArray(p)"), "branche array");
    assert_eq!(node["pnex_node_id"], "s1");
    assert_eq!(node["pnex_flow_id"], 21);
    assert_eq!(node["pnex_version"], 4);
    assert_eq!(node["pnex_org_id"], 7);
    let wires = node["wires"].as_array().unwrap();
    assert_eq!(wires.len(), 1);
    assert_eq!(wires[0], serde_json::json!(["pnexflow21_m1"]));
}

#[test]
fn projection_json_split_ports_nommes() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    let g = FlowGraph {
        nodes: vec![
            FlowNode {
                id: "s1".into(),
                name: None,
                position: None,
                outputs: vec![
                    FlowWiring {
                        port: 0,
                        targets: vec!["x1".into()],
                    },
                    FlowWiring {
                        port: 1,
                        targets: vec!["x2".into()],
                    },
                ],
                inputs: vec![],
                kind: FlowNodeKind::JsonSplit {
                    config: JsonSplitConfig {
                        keys: vec!["a".into(), "b".into()],
                        auto: false,
                    },
                },
            },
            node_display("x1", &[]),
            node_display("x2", &[]),
        ],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    let node = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("function"))
        .expect("nœud function vendor absent");
    assert_eq!(node["outputs"], 2, "un port par clé déclarée");
    let func = node["func"].as_str().expect("func textuel");
    assert!(
        func.contains(r#"var keys = ["a","b"]"#),
        "littéral de clés inline : {func}"
    );
    let wires = node["wires"].as_array().unwrap();
    assert_eq!(wires.len(), 2, "wires paddées au nombre de ports");
    assert_eq!(wires[0], serde_json::json!(["pnexflow21_x1"]));
    assert_eq!(wires[1], serde_json::json!(["pnexflow21_x2"]));
}

#[test]
fn projection_json_merge_entrees_nommees_taggers() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    let merge = FlowNode {
        id: "m1".into(),
        name: None,
        position: None,
        outputs: vec![FlowWiring {
            port: 0,
            targets: vec!["d1".into()],
        }],
        inputs: vec![FlowInputWiring {
            pin: "temp".into(),
            from: "i1".into(),
            from_port: 0,
        }],
        kind: FlowNodeKind::JsonMerge {
            config: JsonMergeConfig {
                default_key: "value".into(),
                inputs: vec!["temp".into()],
            },
        },
    };
    let mut inject = node_inject("i1");
    inject.outputs = vec![FlowWiring {
        port: 0,
        targets: vec!["m1".into()],
    }];
    let g = FlowGraph {
        nodes: vec![inject, merge, node_display("d1", &[])],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    // Le wire inject → merge passe par le tagger topic « temp ».
    let inject_entry = out
        .iter()
        .find(|e| e.get("id").and_then(|i| i.as_str()) == Some("pnexflow21_i1"))
        .expect("inject absent");
    assert_eq!(
        inject_entry["wires"][0],
        serde_json::json!(["pnexflow21_ti1p0_temp"]),
        "wire rerouté vers le tagger"
    );
    let tagger = out
        .iter()
        .find(|e| e.get("id").and_then(|i| i.as_str()) == Some("pnexflow21_ti1p0_temp"))
        .expect("tagger absent");
    assert_eq!(tagger["pnex_node_id"], "m1", "rattaché au nœud merge");
    assert_eq!(
        tagger["wires"][0],
        serde_json::json!(["pnexflow21_m1"]),
        "tagger → merge (id préfixé)"
    );
    // Le merge lui-même reste un function 1 port, wire direct vers d1.
    let merge_entry = out
        .iter()
        .find(|e| e.get("id").and_then(|i| i.as_str()) == Some("pnexflow21_m1"))
        .expect("merge absent");
    assert_eq!(merge_entry["outputs"], 1);
    assert_eq!(
        merge_entry["wires"][0],
        serde_json::json!(["pnexflow21_d1"])
    );
}

#[test]
fn projection_json_merge_noeud_function() {
    let meta = FlowArtifactMeta {
        flow_id: 21,
        version_number: 4,
        org_id: 7,
        o2_org: String::new(),
    };
    let g = FlowGraph {
        nodes: vec![FlowNode {
            id: "m1".into(),
            name: None,
            position: None,
            outputs: vec![FlowWiring {
                port: 0,
                targets: vec!["d1".into()],
            }],
            inputs: vec![],
            kind: FlowNodeKind::JsonMerge {
                config: JsonMergeConfig {
                    default_key: "value".into(),
                    inputs: Vec::new(),
                },
            },
        }],
    };
    let entries = to_red_flows_json_with(&g, &meta, &FunctionResolver::new());
    let out = entries.as_array().unwrap();
    let node = out
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("function"))
        .expect("nœud function vendor absent");
    assert_eq!(node["outputs"], 1);
    let func = node["func"].as_str().expect("func textuel");
    assert!(
        func.contains("context.get(\"__pnex_merge\")"),
        "accumulateur en contexte nœud"
    );
    assert!(
        func.contains("acc[key] = msg.payload"),
        "une entrée = une variable (plus de shallow-merge objet)"
    );
    assert!(
        func.contains("\"value\""),
        "clé par défaut inline (échappée)"
    );
    assert_eq!(node["pnex_node_id"], "m1");
    let wires = node["wires"].as_array().unwrap();
    assert_eq!(wires[0], serde_json::json!(["pnexflow21_d1"]));
}

#[test]
fn camera_nodes_roundtrip_validate_and_project() {
    let g: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "cam", "kind": "camera_source", "config": {"device_id": "cam-one", "max_fps": 2},
             "outputs": [{"port": 0, "targets": ["rec"]}]},
            {"id": "rec", "kind": "video_record", "config": {"segment_secs": 30}}
        ]
    }))
    .expect("graph");
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
    let FlowNodeKind::VideoRecord { config } = &g.nodes[1].kind else {
        panic!("video_record expected");
    };
    // Defaults fill the omitted fields.
    assert_eq!(config.max_segment_mb, 32);
    assert_eq!(config.retention_days, 7);

    let meta = FlowArtifactMeta {
        flow_id: 4,
        version_number: 2,
        org_id: 9,
        o2_org: String::new(),
    };
    let red = to_red_flows_json(&g, &meta);
    let entries = red.as_array().expect("entries");
    let cam = entries
        .iter()
        .find(|e| e["type"] == "pnex-camera-source")
        .expect("cam");
    assert_eq!(cam["device_id"], "cam-one");
    assert_eq!(cam["pnex_org_id"], 9);
    let rec = entries
        .iter()
        .find(|e| e["type"] == "pnex-video-record")
        .expect("rec");
    assert_eq!(rec["segment_secs"], 30);
    assert_eq!(rec["pnex_node_id"], "rec");

    // Invalid configs are flagged with their machine codes.
    let bad: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "cam", "kind": "camera_source", "config": {"device_id": "", "max_fps": 0}},
            {"id": "rec", "kind": "video_record", "config": {"segment_secs": 1}}
        ]
    }))
    .expect("graph");
    let codes: Vec<String> = validate_graph(&bad).into_iter().map(|v| v.code).collect();
    assert!(
        codes.contains(&"camera_device_missing".to_string()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"video_segment_secs_invalid".to_string()),
        "{codes:?}"
    );
}

#[test]
fn one_video_record_per_camera() {
    // Direct and through a vision node: both reach cam-one → duplicate.
    let g: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "cam", "kind": "camera_source", "config": {"device_id": "cam-one"},
             "outputs": [{"port": 0, "targets": ["rec", "det"]}]},
            {"id": "det", "kind": "vision_detect", "config": {"model_id": "0b0c"},
             "outputs": [{"port": 0, "targets": ["rec2"]}]},
            {"id": "rec", "kind": "video_record", "config": {}},
            {"id": "rec2", "kind": "video_record", "config": {}}
        ]
    }))
    .expect("graph");
    let claims = video_record_claims_of(&g);
    assert_eq!(claims.len(), 2);
    assert!(claims.iter().all(|c| c.device_id == "cam-one"));
    let dup: Vec<FlowViolation> = validate_graph(&g)
        .into_iter()
        .filter(|v| v.code == "video_record_duplicate")
        .collect();
    assert_eq!(dup.len(), 1, "{dup:?}");

    // Two cameras, one recorder each: fine.
    let ok: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "a", "kind": "camera_source", "config": {"device_id": "cam-one"},
             "outputs": [{"port": 0, "targets": ["ra"]}]},
            {"id": "b", "kind": "camera_source", "config": {"device_id": "cam-two"},
             "outputs": [{"port": 0, "targets": ["rb"]}]},
            {"id": "ra", "kind": "video_record", "config": {}},
            {"id": "rb", "kind": "video_record", "config": {}}
        ]
    }))
    .expect("graph");
    assert!(validate_graph(&ok)
        .iter()
        .all(|v| v.code != "video_record_duplicate"));
}

#[test]
fn event_log_and_vision_detect_project() {
    let g: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "cam", "kind": "camera_source", "config": {"device_id": "cam-one"},
             "outputs": [{"port": 0, "targets": ["det"]}]},
            {"id": "det", "kind": "vision_detect",
             "config": {"model_id": "0b0c", "labels": ["person"], "min_score": 0.6},
             "outputs": [{"port": 0, "targets": ["log"]}]},
            {"id": "log", "kind": "event_log", "config": {"stream": "Front door", "level": "warn"}}
        ]
    }))
    .expect("graph");
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
    let meta = FlowArtifactMeta {
        flow_id: 1,
        version_number: 1,
        org_id: 3,
        o2_org: String::new(),
    };
    let red = to_red_flows_json(&g, &meta);
    let entries = red.as_array().expect("entries");
    let det = entries
        .iter()
        .find(|e| e["type"] == "pnex-vision-detect")
        .expect("det");
    assert_eq!(det["emit"], "on_detection");
    assert_eq!(det["max_fps"], 1.0);
    let log = entries
        .iter()
        .find(|e| e["type"] == "pnex-event-log")
        .expect("log");
    assert_eq!(log["level"], "warn");
    assert_eq!(log["pnex_org_id"], 3);

    let bad: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "det", "kind": "vision_detect", "config": {"model_id": ""}},
            {"id": "log", "kind": "event_log", "config": {"stream": "!!"}}
        ]
    }))
    .expect("graph");
    let codes: Vec<String> = validate_graph(&bad).into_iter().map(|v| v.code).collect();
    assert!(
        codes.contains(&"vision_model_missing".to_string()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"event_stream_invalid".to_string()),
        "{codes:?}"
    );
}

#[test]
fn memory_nodes_validate_and_project() {
    use crate::memory::{MemoryReadConfig, MemoryWriteConfig};
    let node = |id: &str, kind: FlowNodeKind| FlowNode {
        id: id.into(),
        name: None,
        position: None,
        outputs: vec![],
        inputs: vec![],
        kind,
    };
    let write = MemoryWriteConfig {
        key: "cycle.p1".into(),
        ttl_secs: 600,
    };
    let read = MemoryReadConfig {
        keys: vec!["cycle.p1".into(), "cycle.p2".into()],
        max_age_secs: 30.0,
    };
    let g = FlowGraph {
        nodes: vec![
            node_inject("n1"),
            node("w1", FlowNodeKind::MemoryWrite { config: write }),
            node("r1", FlowNodeKind::MemoryRead { config: read }),
        ],
    };
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));

    // Kind tags round-trip through the manual deserializer.
    let json = serde_json::to_value(&g).unwrap();
    assert_eq!(json["nodes"][1]["kind"], "memory_write");
    let back: FlowGraph = serde_json::from_value(json).unwrap();
    assert_eq!(back, g);

    let meta = FlowArtifactMeta {
        flow_id: 7,
        version_number: 2,
        org_id: 42,
        o2_org: "o2-x".into(),
    };
    let red = to_red_flows_json(&g, &meta);
    let entries = red.as_array().unwrap();
    let w = entries
        .iter()
        .find(|e| e["type"] == "pnex-memory-write")
        .unwrap();
    assert_eq!(w["key"], "cycle.p1");
    assert_eq!(w["ttl_secs"], 600);
    assert_eq!(w["pnex_org_id"], 42);
    let r = entries
        .iter()
        .find(|e| e["type"] == "pnex-memory-read")
        .unwrap();
    // Object port + one port per key.
    assert_eq!(r["wires"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(r["pnex_org_id"], 42);

    // Default configs are flagged (empty key / no key).
    let bad = FlowGraph {
        nodes: vec![
            node(
                "w1",
                FlowNodeKind::MemoryWrite {
                    config: Default::default(),
                },
            ),
            node(
                "r1",
                FlowNodeKind::MemoryRead {
                    config: Default::default(),
                },
            ),
        ],
    };
    let codes: Vec<String> = validate_graph(&bad)
        .iter()
        .map(|x| x.code.clone())
        .collect();
    assert_eq!(
        codes,
        ["memory_key_invalid", "memory_keys_empty"],
        "{codes:?}"
    );
}

#[test]
fn anomaly_and_forecast_project_with_padded_ports() {
    let g: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "in", "kind": "inject", "config": {"repeat_secs": 60.0},
             "outputs": [{"port": 0, "targets": ["an", "fc"]}]},
            // Empty config = usable defaults (as dropped from the palette).
            {"id": "an", "kind": "anomaly", "outputs": [{"port": 1, "targets": ["dbg"]}]},
            {"id": "fc", "kind": "forecast",
             "config": {"model": "linear", "threshold": 80.0, "horizon": 120},
             "outputs": [{"port": 2, "targets": ["dbg"]}]},
            {"id": "dbg", "kind": "debug"}
        ]
    }))
    .expect("graph");
    assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
    let meta = FlowArtifactMeta {
        flow_id: 5,
        version_number: 3,
        org_id: 7,
        o2_org: String::new(),
    };
    let red = to_red_flows_json(&g, &meta);
    let entries = red.as_array().expect("entries");
    let an = entries
        .iter()
        .find(|e| e["type"] == "pnex-anomaly")
        .expect("anomaly");
    assert_eq!(an["method"], "robust_z");
    assert_eq!(an["window"], 200);
    assert_eq!(an["pnex_node_id"], "an");
    assert_eq!(an["pnex_org_id"], 7);
    // Wires padded to the port count: the boolean port 1 keeps its index.
    let wires = an["wires"].as_array().expect("wires");
    assert_eq!(wires.len(), crate::predictive::ANOMALY_PORT_COUNT);
    assert!(wires[0].as_array().unwrap().is_empty());
    assert_eq!(wires[1].as_array().unwrap().len(), 1);
    let fc = entries
        .iter()
        .find(|e| e["type"] == "pnex-forecast")
        .expect("forecast");
    assert_eq!(fc["model"], "linear");
    assert_eq!(fc["threshold"], 80.0);
    assert_eq!(fc["direction"], "above");
    assert_eq!(
        fc["wires"].as_array().unwrap().len(),
        crate::predictive::FORECAST_PORT_COUNT
    );

    let bad: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "an", "kind": "anomaly", "config": {"window": 4}},
            {"id": "fc", "kind": "forecast", "config": {"horizon": 0}}
        ]
    }))
    .expect("graph");
    let codes: Vec<String> = validate_graph(&bad).into_iter().map(|v| v.code).collect();
    assert!(
        codes.contains(&"predict_window_invalid".to_string()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"forecast_horizon_invalid".to_string()),
        "{codes:?}"
    );
}

#[test]
fn removed_pnex_sql_kind_fails_to_parse_cleanly() {
    // Saved graphs still carrying the removed `pnex_sql` node must surface a
    // typed parse error (never a panic) so load/deploy can skip them.
    let raw = json!({ "nodes": [
        { "id": "q", "kind": "pnex_sql", "config": { "query": "SELECT 1" } },
    ]});
    let err = serde_json::from_value::<FlowGraph>(raw).expect_err("removed kind");
    assert!(err.to_string().contains("pnex_sql"), "{err}");
}
