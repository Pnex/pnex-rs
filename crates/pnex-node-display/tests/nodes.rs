//! Tests du nœud `pnex-display` : registre `inventory`, rejet au build sans
//! `pnex_node_id`, passthrough + publication au canal debug (moteur
//! in-process — pas de sous-process).

use edgelink_core::runtime::registry::RegistryBuilder;

#[test]
fn registre_contient_pnex_display() {
    // Ancre anti-élagage : sans référence au crate, le linker peut jeter les
    // soumissions `inventory` (même garde que le binaire runtime).
    pnex_node_display::registered();
    let reg = RegistryBuilder::default().build().expect("registre");
    let meta = reg
        .get("pnex-display")
        .unwrap_or_else(|| panic!("nœud pnex-display absent du registre"));
    assert_eq!(meta.type_, "pnex-display");
}

/// Artefact RAW Node-RED : tab `t` + inject(0.05 s, payload JSON) →
/// pnex-display("n2") → debug(tosidebar).
fn display_flows() -> String {
    serde_json::json!([
        { "id": "t", "type": "tab", "label": "test" },
        {
            "id": "n1", "type": "inject", "z": "t",
            "props": [{ "p": "payload" }],
            "repeat": "0.05", "once": false,
            "payload": "{\"k\":1}", "payloadType": "json",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "pnex-display", "z": "t",
            "pnex_node_id": "n2", "pnex_flow_id": 1, "pnex_version": 1, "pnex_org_id": 1,
            "x": 200, "y": 100, "wires": [["n3"]]
        },
        {
            "id": "n3", "type": "debug", "z": "t",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 300, "y": 100, "wires": []
        }
    ])
    .to_string()
}

#[test]
fn build_refuse_sans_pnex_node_id() {
    // L'identité est estampillée par la projection : un artefact qui ne la
    // porte pas est rejeté au déploiement (fail-loud).
    let json = serde_json::json!([
        { "id": "t", "type": "tab", "label": "test" },
        {
            "id": "n2", "type": "pnex-display", "z": "t",
            "x": 200, "y": 100, "wires": []
        },
    ])
    .to_string();
    let reg = RegistryBuilder::default().build().expect("registre");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async move {
        let result = edgelink_core::runtime::engine::Engine::with_json_string(&reg, json, None);
        let err = result
            .expect_err("build sans pnex_node_id doit échouer");
        let _ = err; // BadFlowsJson — le message exact n'est pas contractuel
    });
}

#[test]
fn passthrough_et_publication_debug() {
    let home = std::env::temp_dir().join(format!("pnex-display-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    let flows_path = home.join("flows.json");
    std::fs::write(&flows_path, display_flows()).unwrap();

    let reg = RegistryBuilder::default().build().expect("registre");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async move {
        let engine =
            edgelink_core::runtime::engine::Engine::with_flows_file(&reg, &flows_path.to_string_lossy(), None)
                .await
                .expect("moteur");
        // Subscribe AVANT start : aucune publication ne doit être perdue.
        let mut rx = engine.debug_channel().subscribe();

        engine.start().await.expect("start");

        // (a) La sonde publie : id canvas brut + valeur NON stringifiée.
        let mut saw_display = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline && !saw_display {
            let recv = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
            let Ok(Ok(m)) = recv else { break };
            if m.id == "n2" {
                assert_eq!(m.format.as_deref(), Some("pnex-display"));
                assert_eq!(m.msg, serde_json::json!({"k": 1}), "valeur brute attendue");
                assert_eq!(m.property.as_deref(), Some("payload"));
                saw_display = true;
            }
        }
        assert!(saw_display, "aucune publication de la sonde sur le canal debug");

        // (b) Le passthrough est intact : le debug builtin en aval reçoit le
        // payload et publie sa version stringifiée (id = hex moteur).
        let mut saw_downstream = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline && !saw_downstream {
            let recv = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
            let Ok(Ok(m)) = recv else { break };
            if m.id != "n2" && m.msg.to_string().contains("k") {
                saw_downstream = m.format.as_deref() != Some("pnex-display");
            }
        }
        assert!(saw_downstream, "le debug en aval n'a rien reçu (passthrough cassé ?)");

        engine.stop().await.expect("stop");
    });
    let _ = std::fs::remove_dir_all(&home);
}
