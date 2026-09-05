//! Spike manuel — vérifie la voie O2 du runtime contre l'OpenObserve réel :
//! remote-write en Basic racine puis relecture `last_over_time`. Usage :
//!
//! ```sh
//! OPENOBSERVE_URL=http://localhost:5080 \
//! OPENOBSERVE_ROOT_EMAIL=root@pnex.local \
//! OPENOBSERVE_ROOT_PASSWORD='Pnex-dev-2026!' \
//! cargo run -p pnex-node-device --example o2_spike -- <org_o2> [valeur]
//! ```
//!
//! Sortie : 0 si écriture + lecture cohérentes, 1 sinon. Ce binaire
//! n'est pas un test automatisé : il sert à trancher la question auth
//! (le passcode d'org ne couvre pas les lectures O2 v0.92.1) sur
//! l'instance locale avant tout e2e complet.

use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(org) = args.next() else {
        eprintln!("usage : o2_spike <org_o2> [valeur]");
        std::process::exit(2);
    };
    let value: f64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(20.25);

    let client = match pnex_node_device::o2::O2Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("client : {e}");
            std::process::exit(2);
        }
    };

    let now = chrono::Utc::now().timestamp_millis();
    let metric = "etl_spike".to_string();
    let device = "flow_0_spike".to_string();

    // ── 1) Écriture remote-write en Basic racine. ──
    let series = vec![pnex_node_device::o2::etl_series(
        metric.clone(),
        device.clone(),
        value,
        now,
    )];
    if let Err(e) = client.write(&org, series).await {
        eprintln!("✗ remote-write racine refusé : {e}");
        std::process::exit(1);
    }
    println!("✓ remote-write racine OK (etl_spike = {value})");

    // O2 a besoin d'un instant pour indexer le point.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ── 2) Relecture PromQL en Basic racine. ──
    match client
        .query_last(&org, "etl_spike", "flow_0_spike", 300.0)
        .await
    {
        Ok(Some((v, _ts))) => {
            let ok = (v - value).abs() < 1e-9;
            println!(
                "{} relecture last_over_time = {v} (attendu {value})",
                if ok { "✓" } else { "✗" }
            );
            std::process::exit(if ok { 0 } else { 1 });
        }
        Ok(None) => {
            eprintln!("✗ relecture : série absente (indexation O2 lente ? réessayer)");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("✗ relecture refusée : {e}");
            std::process::exit(1);
        }
    }
}
