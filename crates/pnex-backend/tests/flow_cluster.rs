//! Flow execution cluster (D106): several workers in one test process, the
//! real placement controller and worker loops, the fake runtime fixture.
//!
//! Covers: per-org sharding (each flow runs on exactly one worker),
//! failover of a crashed worker, fencing of the internal write routes,
//! rebalancing toward a new worker, graceful drain, supersession of a
//! worker id, and the real pod-to-pod HTTP path (a worker served on a TCP
//! port, reached through `/internal/flow-cluster/*`).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL). Timings come from `test.yaml`
//! (`settings.flow.cluster`: 200 ms heartbeats, 1.5 s worker TTL).

mod common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use loco_rs::app::AppContext;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::flow::FlowSettings;
use pnex_backend::services::flow_cluster::{store, transport, ClusterSettings, WorkerNode};
use serial_test::serial;

const APP_WORKER: &str = "pod-a";
const CLUSTER_TOKEN: &str = "cluster-secret-test";
const RUNTIME_TOKEN: &str = "flow-runtime-token-test";

fn state_dir(tag: &str) -> String {
    format!("/tmp/pnex-flow-cluster-{}-{tag}", std::process::id())
}

async fn with_cluster_app<F, Fut>(tag: &str, f: F)
where
    F: FnOnce(axum_test::TestServer, String, AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe {
        std::env::set_var("RAUTHY_URL", &base);
        std::env::set_var("PNEX_FLOW_ENABLED", "true");
        std::env::set_var(
            "PNEX_FLOW_RUNTIME_CMD",
            "./tests/fixtures/flow/fake_runtime.sh",
        );
        std::env::set_var("PNEX_FLOW_STATE_DIR", state_dir(&format!("{tag}-a")));
        std::env::set_var("PNEX_FLOW_RELOAD_ACK_SECS", "5");
        std::env::set_var("PNEX_FLOW_DEBUG_TOOLS", "true");
        std::env::set_var("PNEX_FLOW_WORKER_ID", APP_WORKER);
        std::env::set_var("PNEX_FLOW_CLUSTER_TOKEN", CLUSTER_TOKEN);
        std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", RUNTIME_TOKEN);
    }
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, base, ctx).await;
            for node in transport::local_workers() {
                node.crash_for_tests().await;
            }
        },
    )
    .await;
}

fn cluster_cfg(ctx: &AppContext, id: &str) -> ClusterSettings {
    let mut cfg = ClusterSettings::from_config(&ctx.config);
    cfg.worker_id = id.into();
    cfg
}

/// Starts an extra in-process worker with its own runtime state dir.
async fn start_worker(
    ctx: &AppContext,
    id: &str,
    tag: &str,
    advertise_url: &str,
) -> Arc<WorkerNode> {
    let mut flow = FlowSettings::from_config(&ctx.config);
    flow.state_dir = state_dir(&format!("{tag}-{id}"));
    let mut cfg = cluster_cfg(ctx, id);
    cfg.advertise_url = advertise_url.into();
    WorkerNode::start(ctx.db.clone(), flow, cfg)
        .await
        .expect("worker registration")
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// One user (and its personal org) per index.
async fn user_org(server: &axum_test::TestServer, base: &str, i: usize) -> (String, i64) {
    let token = common::valid_token(
        base,
        &format!("00000000-0000-0000-0000-{:012}", 100 + i),
        &format!("user{i}"),
        &format!("user{i}@example.com"),
    );
    let org = server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(&token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org");
    (token, org)
}

async fn create_and_deploy(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
) -> i64 {
    let res = server
        .post("/api/v1/flows")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "name": name,
            "graph": {
                "nodes": [
                    {
                        "id": "n1", "kind": "inject",
                        "config": { "repeat_secs": 5.0, "payload": {"k": 1} },
                        "outputs": [{ "port": 0, "targets": ["n2"] }]
                    },
                    { "id": "n2", "kind": "debug", "config": {} }
                ]
            },
        }))
        .await;
    assert_eq!(res.status_code(), 201, "create {name}: {}", res.text());
    let id = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
    let dep = server
        .post(&format!("/api/v1/flows/{id}/deploy"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({ "version_number": 1 }))
        .await;
    assert_eq!(dep.status_code(), 200, "deploy {name}: {}", dep.text());
    id
}

async fn wait_for<F, Fut>(what: &str, timeout: Duration, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out after {timeout:?} waiting for: {what}");
}

async fn owner(ctx: &AppContext, org: i64) -> Option<String> {
    store::placement(&ctx.db, org)
        .await
        .unwrap()
        .map(|p| p.worker_id)
}

/// Flow ids of the tabs of the runtime artifact of a worker (on disk: what
/// its runtime child really loads).
fn artifact_flows(dir: &str) -> HashSet<i64> {
    let raw = std::fs::read_to_string(format!("{dir}/flows.json")).unwrap_or_else(|_| "[]".into());
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["type"] == "tab")
        .filter_map(|n| n["pnex_flow_id"].as_i64())
        .collect()
}

fn app_worker() -> Arc<WorkerNode> {
    transport::local(APP_WORKER).expect("app worker running")
}

// ─────────────────────────── Sharding ───────────────────────────

#[tokio::test]
#[serial]
async fn orgs_are_sharded_and_each_flow_runs_on_exactly_one_worker() {
    let tag = "shard";
    with_cluster_app(tag, |server, base, ctx| async move {
        let wb = start_worker(&ctx, "pod-b", tag, "").await;
        let wa = app_worker();
        let mut flows = Vec::new();
        for i in 0..4 {
            let (token, org) = user_org(&server, &base, i).await;
            let id = create_and_deploy(&server, &token, org, &format!("f{i}")).await;
            flows.push((token, org, id));
        }
        // New orgs go to the least loaded worker: two each.
        let mut per_worker = std::collections::HashMap::<String, usize>::new();
        for (_, org, _) in &flows {
            *per_worker
                .entry(owner(&ctx, *org).await.expect("placed"))
                .or_default() += 1;
        }
        assert_eq!(per_worker.get(APP_WORKER), Some(&2), "{per_worker:?}");
        assert_eq!(per_worker.get("pod-b"), Some(&2), "{per_worker:?}");

        // Each flow runs on exactly its owner, never twice.
        let on_a = wa.running_flows().await;
        let on_b = wb.running_flows().await;
        assert!(on_a.is_disjoint(&on_b), "a={on_a:?} b={on_b:?}");
        for (_, org, id) in &flows {
            let o = owner(&ctx, *org).await.unwrap();
            let (mine, other) = if o == APP_WORKER {
                (&on_a, &on_b)
            } else {
                (&on_b, &on_a)
            };
            assert!(
                mine.contains(id) && !other.contains(id),
                "flow {id} owner {o}"
            );
        }
        // The runtime child of each worker loads only its own tabs.
        assert_eq!(artifact_flows(&state_dir(&format!("{tag}-a"))), on_a);
        assert_eq!(artifact_flows(&state_dir(&format!("{tag}-pod-b"))), on_b);

        // Runtime status is answered by the owner, whichever it is.
        for (token, org, id) in &flows {
            let rt = server
                .get(&format!("/api/v1/flows/{id}/runtime"))
                .add_header("Authorization", bearer(token))
                .add_header("X-Org-Id", org.to_string())
                .await
                .json::<serde_json::Value>();
            assert_eq!(rt["running"], true, "flow {id}: {rt}");
            assert_eq!(rt["engine_status"], "running", "flow {id}: {rt}");
        }

        // Stop on the owner: the tab leaves that runtime only.
        let (token, org, id) = &flows[1];
        let o = owner(&ctx, *org).await.unwrap();
        let res = server
            .post(&format!("/api/v1/flows/{id}/stop"))
            .add_header("Authorization", bearer(token))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let node = if o == APP_WORKER {
            wa.clone()
        } else {
            wb.clone()
        };
        assert!(!node.running_flows().await.contains(id));
    })
    .await;
}

// ─────────────────────── Failover + fencing ───────────────────────

#[tokio::test]
#[serial]
async fn crashed_worker_orgs_fail_over_and_its_writes_are_fenced() {
    let tag = "failover";
    with_cluster_app(tag, |server, base, ctx| async move {
        let wb = start_worker(&ctx, "pod-b", tag, "").await;
        let wa = app_worker();
        let mut flows = Vec::new();
        for i in 0..4 {
            let (token, org) = user_org(&server, &base, i).await;
            flows.push((org, create_and_deploy(&server, &token, org, &format!("f{i}")).await));
        }
        let b_orgs: Vec<i64> = {
            let mut v = Vec::new();
            for (org, _) in &flows {
                if owner(&ctx, *org).await.as_deref() == Some("pod-b") {
                    v.push(*org);
                }
            }
            v
        };
        assert!(!b_orgs.is_empty(), "pod-b owns orgs");
        let old_fence = wb.fence();
        let epochs_before: Vec<i64> = {
            let mut v = Vec::new();
            for org in &b_orgs {
                v.push(store::placement(&ctx.db, *org).await.unwrap().unwrap().epoch);
            }
            v
        };

        // pod-b dies without deregistering.
        wb.crash_for_tests().await;
        let t0 = Instant::now();
        wait_for("pod-b orgs moved to pod-a", Duration::from_secs(10), || {
            let ctx = ctx.clone();
            let b_orgs = b_orgs.clone();
            async move {
                for org in &b_orgs {
                    if owner(&ctx, *org).await.as_deref() != Some(APP_WORKER) {
                        return false;
                    }
                }
                true
            }
        })
        .await;
        let failover = t0.elapsed();
        // Never before the TTL (1.5 s) counted from the last heartbeat,
        // which may precede the crash by one period (200 ms): a slow worker
        // is not a dead worker.
        assert!(failover >= Duration::from_millis(1250), "failover too early: {failover:?}");
        for (org, epoch) in b_orgs.iter().zip(epochs_before) {
            let p = store::placement(&ctx.db, *org).await.unwrap().unwrap();
            assert_eq!(p.epoch, epoch + 1, "epoch bumped on move (fencing token)");
        }
        // Every flow now runs on pod-a.
        let all: HashSet<i64> = flows.iter().map(|(_, id)| *id).collect();
        wait_for("pod-a runs every flow", Duration::from_secs(5), || {
            let wa = wa.clone();
            let all = all.clone();
            async move { wa.running_flows().await == all }
        })
        .await;

        // Fencing: a write stamped by the dead pod-b is refused; the same
        // write stamped by the owner goes through the fence (404: no such
        // device, i.e. the route went on).
        let org = b_orgs[0];
        let write = |fence: String| {
            server
                .post("/internal/flow/device-write")
                .add_header("x-pnex-flow-token", RUNTIME_TOKEN)
                .add_header(pnex_core::FLOW_WORKER_HEADER, fence)
                .json(&serde_json::json!({ "org_id": org, "device_id": "nope", "values": {"r": true} }))
        };
        let refused = write(old_fence.clone()).await;
        assert_eq!(refused.status_code(), 409, "{}", refused.text());
        assert_eq!(refused.json::<serde_json::Value>()["code"], "fenced");
        let allowed = write(wa.fence()).await;
        assert_eq!(allowed.status_code(), 404, "{}", allowed.text());
        // Same for events and notify deliveries.
        let ev = server
            .post("/internal/flow/event")
            .add_header("x-pnex-flow-token", RUNTIME_TOKEN)
            .add_header(pnex_core::FLOW_WORKER_HEADER, old_fence)
            .json(&serde_json::json!({ "org_id": org, "stream": "ev_s", "level": "info" }))
            .await;
        assert_eq!(ev.status_code(), 409, "{}", ev.text());
    })
    .await;
}

// ─────────────────────── Rebalance + drain ───────────────────────

#[tokio::test]
#[serial]
async fn a_new_worker_takes_its_share_and_a_draining_one_hands_back() {
    let tag = "rebalance";
    with_cluster_app(tag, |server, base, ctx| async move {
        let wa = app_worker();
        let mut flows = Vec::new();
        for i in 0..8 {
            let (token, org) = user_org(&server, &base, i).await;
            flows.push((
                org,
                create_and_deploy(&server, &token, org, &format!("f{i}")).await,
            ));
        }
        let all: HashSet<i64> = flows.iter().map(|(_, id)| *id).collect();
        assert_eq!(
            wa.running_flows().await,
            all,
            "single worker runs everything"
        );

        // pod-b joins: the controller moves orgs until the spread is within
        // tolerance (8 orgs of 1 flow, min spread 4 → at least 2 move).
        let wb = start_worker(&ctx, "pod-b", tag, "").await;
        wait_for("pod-b gets a share", Duration::from_secs(10), || {
            let wb = wb.clone();
            async move { wb.running_flows().await.len() >= 2 }
        })
        .await;
        wait_for(
            "runtimes settle on disjoint sets",
            Duration::from_secs(10),
            || {
                let (wa, wb, all) = (wa.clone(), wb.clone(), all.clone());
                async move {
                    let (a, b) = (wa.running_flows().await, wb.running_flows().await);
                    a.is_disjoint(&b) && a.union(&b).copied().collect::<HashSet<_>>() == all
                }
            },
        )
        .await;

        // pod-b drains: its orgs go back to pod-a before it stops.
        wb.shutdown(true).await;
        assert!(wb.is_stopped());
        for (org, _) in &flows {
            assert_eq!(
                owner(&ctx, *org).await.as_deref(),
                Some(APP_WORKER),
                "org {org}"
            );
        }
        assert!(
            store::worker(&ctx.db, "pod-b").await.unwrap().is_none(),
            "deregistered"
        );
        wait_for(
            "pod-a runs everything again",
            Duration::from_secs(5),
            || {
                let (wa, all) = (wa.clone(), all.clone());
                async move { wa.running_flows().await == all }
            },
        )
        .await;
    })
    .await;
}

// ─────────────────────── Supersession ───────────────────────

#[tokio::test]
#[serial]
async fn a_superseded_worker_fences_itself() {
    let tag = "supersede";
    with_cluster_app(tag, |server, base, ctx| async move {
        let wb = start_worker(&ctx, "pod-b", tag, "").await;
        let (token, org) = user_org(&server, &base, 0).await;
        // Force the org onto pod-b.
        store::insert_placement(&ctx.db, org, "pod-b")
            .await
            .unwrap();
        let id = create_and_deploy(&server, &token, org, "f").await;
        assert!(wb.running_flows().await.contains(&id));

        // Another process registers the same id (a replacement pod that
        // reused the name while the old one still runs).
        store::register_worker(&ctx.db, "pod-b", wb.boot + 1, "", 0)
            .await
            .unwrap();
        wait_for("old pod-b stops", Duration::from_secs(5), || {
            let wb = wb.clone();
            async move { wb.is_stopped() }
        })
        .await;
        assert!(wb.running_flows().await.is_empty(), "runtime halted");
        assert!(artifact_flows(&state_dir(&format!("{tag}-pod-b"))).is_empty());
    })
    .await;
}

// ─────────────────────── Pod to pod over HTTP ───────────────────────

/// Serves the internal cluster routes of this process on a real TCP port
/// (the second "pod").
async fn serve_cluster_routes(ctx: &AppContext) -> String {
    let routes = pnex_backend::controllers::flow_cluster::routes();
    let mut router = axum::Router::new();
    for h in routes.handlers {
        router = router.route(&h.uri, h.method);
    }
    let router = router.with_state(ctx.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
#[serial]
async fn deploy_stop_and_status_reach_a_remote_worker_over_http() {
    let tag = "http";
    with_cluster_app(tag, |server, base, ctx| async move {
        let url = serve_cluster_routes(&ctx).await;
        let wb = start_worker(&ctx, "pod-remote", tag, &url).await;
        transport::force_remote_for_tests("pod-remote");

        // Wrong / missing token: 401, fail-closed.
        let http = reqwest::Client::new();
        let denied = http
            .get(format!("{url}/internal/flow-cluster/flows/1/runtime"))
            .header("x-pnex-cluster-token", "wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), 401);

        let (token, org) = user_org(&server, &base, 0).await;
        store::insert_placement(&ctx.db, org, "pod-remote")
            .await
            .unwrap();
        let id = create_and_deploy(&server, &token, org, "remote").await;
        assert!(wb.running_flows().await.contains(&id), "deployed over HTTP");
        assert!(!app_worker().running_flows().await.contains(&id));

        let rt = server
            .get(&format!("/api/v1/flows/{id}/runtime"))
            .add_header("Authorization", bearer(&token))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(rt["running"], true, "{rt}");
        assert_eq!(rt["engine_status"], "running", "{rt}");

        // Debug feed of the remote runtime (the fixture prints a debug
        // line on every reload).
        wait_for("remote debug feed", Duration::from_secs(5), || {
            let server = &server;
            let token = token.clone();
            async move {
                let feed = server
                    .get(&format!("/api/v1/flows/{id}/debug"))
                    .add_header("Authorization", bearer(&token))
                    .add_header("X-Org-Id", org.to_string())
                    .await
                    .json::<serde_json::Value>();
                feed["entries"].as_array().is_some_and(|e| !e.is_empty())
            }
        })
        .await;

        let stop = server
            .post(&format!("/api/v1/flows/{id}/stop"))
            .add_header("Authorization", bearer(&token))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(stop.status_code(), 200, "{}", stop.text());
        assert!(!wb.running_flows().await.contains(&id), "stopped over HTTP");
    })
    .await;
}

// ─────────────────────── Leases ───────────────────────

#[tokio::test]
#[serial]
async fn leases_have_a_single_holder_expire_and_can_be_released() {
    with_cluster_app("lease", |_server, _base, ctx| async move {
        let db = &ctx.db;
        assert!(store::try_lease(db, "task:test", "pod-1", 400)
            .await
            .unwrap());
        assert!(
            !store::try_lease(db, "task:test", "pod-2", 400)
                .await
                .unwrap(),
            "held"
        );
        assert!(
            store::try_lease(db, "task:test", "pod-1", 400)
                .await
                .unwrap(),
            "renewed"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            store::try_lease(db, "task:test", "pod-2", 400)
                .await
                .unwrap(),
            "taken over once expired"
        );
        assert!(!store::try_lease(db, "task:test", "pod-1", 400)
            .await
            .unwrap());
        store::release_lease(db, "task:test", "pod-1")
            .await
            .unwrap(); // not the holder: no-op
        assert!(!store::try_lease(db, "task:test", "pod-1", 400)
            .await
            .unwrap());
        store::release_lease(db, "task:test", "pod-2")
            .await
            .unwrap();
        assert!(
            store::try_lease(db, "task:test", "pod-1", 400)
                .await
                .unwrap(),
            "released"
        );

        // Exactly one placement controller leads in this process.
        let lease = pnex_backend::models::_entities::flow_leases::Entity::find_by_id(
            pnex_backend::services::flow_cluster::PLACEMENT_LEASE.to_string(),
        );
        use sea_orm::EntityTrait;
        let row = lease.one(db).await.unwrap().expect("controller lease held");
        assert!(row.holder.starts_with(APP_WORKER), "{}", row.holder);
    })
    .await;
}
