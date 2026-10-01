//! Series persistence against a real Valkey. Runs only with
//! `PNEX_TEST_VALKEY_URL` set (skips cleanly otherwise), e.g.
//! `PNEX_TEST_VALKEY_URL=redis://127.0.0.1:6379 cargo test -p pnex-node-predict --test valkey`.

use pnex_node_predict::series::{Sample, SeriesStore};

fn url() -> Option<String> {
    std::env::var("PNEX_TEST_VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

#[tokio::test]
async fn history_survives_a_new_store() {
    let Some(url) = url() else {
        eprintln!("PNEX_TEST_VALKEY_URL absent: skipped");
        return;
    };
    // Unique node id per run: no state leaks between runs.
    let node_id = format!("test-{}", std::process::id());
    let first = SeriesStore::with_url("test", 4, &url, 999_999, 1, &node_id);
    for i in 0..6 {
        first
            .push(
                "t",
                Sample {
                    t: i as f64,
                    v: i as f64 * 10.0,
                },
                |_| (),
            )
            .await;
    }
    drop(first);
    // A fresh store (redeploy / restart) reloads the trimmed window.
    let second = SeriesStore::with_url("test", 4, &url, 999_999, 1, &node_id);
    let values = second
        .push("t", Sample { t: 6.0, v: 60.0 }, |s| s.values())
        .await;
    assert_eq!(values, vec![30.0, 40.0, 50.0, 60.0]);
    // Another topic of the same node starts empty.
    let other = second
        .push("u", Sample { t: 0.0, v: 1.0 }, |s| s.values())
        .await;
    assert_eq!(other, vec![1.0]);
    // Cleanup (the keys would otherwise live for the 30-day TTL).
    let mut conn = redis::Client::open(url.as_str())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let keys: Vec<String> = ["t", "u"]
        .iter()
        .map(|t| format!("pnex:series:v1:999999:1:{node_id}:{t}"))
        .collect();
    let _: () = redis::AsyncCommands::del(&mut conn, keys).await.unwrap();
}
