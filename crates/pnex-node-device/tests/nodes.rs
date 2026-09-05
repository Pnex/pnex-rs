//! Tests des nœuds device/calc/metric — registre `inventory` + client O2
//! contre un serveur HTTP mocké (axum, pattern `spawn_mock_rauthy` du
//! backend). Le remote-write capturé est décodé (snappy+prost) pour vérifier
//! les labels exacts des séries `etl_*`.

use std::sync::Arc;
use std::sync::Mutex;

use axum::routing::{get, post};
use prost::Message;

type CapturedLabels = Arc<Mutex<Vec<Vec<Vec<(String, String)>>>>>;

/// Mock O2 : GET …/query → réponse vector JSON ; POST …/write → décode
/// prompb (snappy) et stocke les labels. Refuse 401 sans Authorization.
async fn spawn_mock_o2() -> (String, CapturedLabels) {
    let captured: CapturedLabels = Arc::default();
    let store = captured.clone();

    async fn query_handler() -> axum::response::Response {
        axum::response::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"status":"success","data":{"resultType":"vector","result":[
                    {"metric":{"__name__":"d1","device_id":"cap-1"},"value":[1786890000.5,"21.5"]}
                ]}}"#,
            ))
            .unwrap()
    }

    async fn write_handler(
        headers: axum::http::HeaderMap,
        body: axum::body::Bytes,
        state: &Mutex<Vec<Vec<Vec<(String, String)>>>>,
    ) -> axum::http::StatusCode {
        if !headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|a| a.starts_with("Basic "))
            .unwrap_or(false)
        {
            return axum::http::StatusCode::UNAUTHORIZED;
        }
        let Ok(mut dec) = Ok::<_, ()>(snap::raw::Decoder::new()) else {
            return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
        };
        let Ok(pb) = dec.decompress_vec(&body) else {
            return axum::http::StatusCode::BAD_REQUEST;
        };
        let Ok(req) = pnex_core::WriteRequest::decode(pb.as_slice()) else {
            return axum::http::StatusCode::BAD_REQUEST;
        };
        for ts in &req.timeseries {
            state.lock().expect("lock").push(vec![ts
                .labels
                .iter()
                .map(|l| (l.name.clone(), l.value.clone()))
                .collect()]);
        }
        axum::http::StatusCode::NO_CONTENT
    }

    let app = axum::Router::new()
        .route(
            "/api/{org}/prometheus/api/v1/query",
            get(|| async { query_handler().await }),
        )
        .route(
            "/api/{org}/prometheus/api/v1/write",
            post(
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let store = store.clone();
                    async move { write_handler(headers, body, &store).await }
                },
            ),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (format!("http://{addr}"), captured)
}

/// Env O2 factice pour `O2Client::from_env`. Les tests partagent UN mock :
/// les vars d'env sont process-global, deux mocks sur des ports différents
/// se marcheraient dessus (tests parallèles).
fn set_env(base: &str) {
    std::env::set_var("OPENOBSERVE_URL", base);
    std::env::set_var("OPENOBSERVE_ROOT_EMAIL", "root@pnex.local");
    std::env::set_var("OPENOBSERVE_ROOT_PASSWORD", "test-pass");
}

#[tokio::test]
async fn client_o2_lit_et_ecrit() {
    let (base, captured) = spawn_mock_o2().await;
    set_env(&base);
    let client = pnex_node_device::o2::O2Client::from_env().expect("client");

    // ── Lecture : dernière valeur de la série (fenêtre fraîche). ──
    let sample = client
        .query_last("pnex_org_1", "d1", "cap-1", 60.0)
        .await
        .expect("query");
    assert_eq!(sample, Some((21.5, 1_786_890_000_500)));

    // ── Écriture : remote-write → labels vérifiés après décodage. ──
    let series = vec![pnex_node_device::o2::etl_series(
        "etl_moyenne".into(),
        "flow_9".into(),
        21.5,
        1_786_890_000_000,
    )];
    client.write("pnex_org_1", series).await.expect("write");

    let writes = captured.lock().expect("lock");
    assert_eq!(writes.len(), 1, "un seul remote-write");
    let labels = &writes[0][0];
    let get = |name: &str| {
        labels
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(get("__name__"), "etl_moyenne");
    assert_eq!(get("device_id"), "flow_9");
    assert_eq!(get("pred_dev"), "virtual_device");
    assert_eq!(get("source_type"), "etl");
    assert_eq!(get("ts_source"), "server");
}

#[test]
fn registre_contient_les_trois_noeuds() {
    use edgelink_core::runtime::registry::RegistryBuilder;
    let reg = RegistryBuilder::default().build().expect("registre");
    for type_name in ["pnex-device", "pnex-calc", "pnex-metric"] {
        let meta = reg
            .get(type_name)
            .unwrap_or_else(|| panic!("nœud {type_name} absent du registre"));
        assert_eq!(meta.type_, type_name);
    }
}

#[test]
fn noms_et_cles_phase6() {
    // Clés de payload device : identiques éditeur/runtime.
    assert_eq!(pnex_core::device_payload_key("cap-1", "D1"), "cap_1_D1");
    // Préfixe etl_ idempotent + normalisé.
    assert_eq!(
        pnex_core::etl_metric_name("Moyenne Serre"),
        "etl_moyenne_serre"
    );
}
