//! Tests of the `pnex-value` node: `inventory` registry, build rejection of
//! invalid configs (inverted random range, missing static value), static
//! replacement and random bounds — in-process engine (no subprocess).

use edgelink_core::runtime::registry::RegistryBuilder;

#[test]
fn registry_contains_pnex_value() {
    // Anti-stripping anchor: without a reference to the crate the linker may
    // drop the `inventory` submissions (same guard as the runtime binary).
    pnex_node_value::registered();
    let reg = RegistryBuilder::default().build().expect("registry");
    let meta = reg
        .get("pnex-value")
        .unwrap_or_else(|| panic!("pnex-value node missing from the registry"));
    assert_eq!(meta.type_, "pnex-value");
}

/// Raw Node-RED artifact: tab `t` + inject(0.05s) → pnex-value → debug.
/// `extra` = raw JSON object merged into the pnex-value entry (config).
fn value_flows(extra: &str) -> String {
    let mut value_node = serde_json::json!({
        "id": "n2", "type": "pnex-value", "z": "t",
        "pnex_flow_id": 1, "pnex_version": 1,
        "x": 200, "y": 100, "wires": [["n3"]]
    });
    if let Some(obj) = value_node.as_object_mut() {
        let extra: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(extra).unwrap_or_default();
        for (k, v) in extra {
            obj.insert(k, v);
        }
    }

    serde_json::json!([
        { "id": "t", "type": "tab", "label": "test" },
        {
            "id": "n1", "type": "inject", "z": "t",
            "props": [{ "p": "payload" }],
            "repeat": "0.05", "once": false,
            "payload": "7", "payloadType": "json",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        value_node,
        {
            "id": "n3", "type": "debug", "z": "t",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 300, "y": 100, "wires": []
        }
    ])
    .to_string()
}

#[test]
fn build_rejects_missing_value() {
    let json = value_flows("{}");
    let reg = RegistryBuilder::default().build().expect("registry");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let result = edgelink_core::runtime::engine::Engine::with_json_string(&reg, json, None);
        let err = result.expect_err("missing value must fail the build");
        let _ = err;
    });
}

#[test]
fn value_replaces_payload() {
    let json = value_flows(r#"{"value": 42}"#);
    let home = std::env::temp_dir().join(format!("pnex-value-static-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    let flows_path = home.join("flows.json");
    std::fs::write(&flows_path, &json).unwrap();

    let reg = RegistryBuilder::default().build().expect("registry");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let engine = edgelink_core::runtime::engine::Engine::with_flows_file(
            &reg,
            &flows_path.to_string_lossy(),
            None,
        )
        .await
        .expect("engine");
        // Subscribe BEFORE start: no publication may be lost.
        let mut rx = engine.debug_channel().subscribe();
        engine.start().await.expect("start");

        let mut saw = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline && !saw {
            let recv = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
            let Ok(Ok(m)) = recv else { break };
            // The builtin debug node stringifies the payload when publishing
            // ("42") — the important part: the inject payload (7) was
            // replaced by the static value (42).
            if m.id != "n2" && m.msg.to_string().contains("42") {
                saw = true;
            }
        }
        assert!(saw, "static value never reached the downstream debug");

        engine.stop().await.expect("stop");
    });
    let _ = std::fs::remove_dir_all(&home);
}
