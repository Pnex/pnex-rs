//! The VM executor against a real container CLI, since its value lies in what the
//! CLI actually enforces. Run it where one exists:
//! `PNEX_TEST_CONTAINER_CLI=podman PNEX_TEST_IMAGE=<image with sh, by digest>
//! cargo test -p pnex-runner --test container_cli -- --ignored`

use std::time::Duration;

use pnex_core::runners::RunnerJobSpec;
use pnex_runner::podman::Podman;
use pnex_runner::{Executor, Host, WorkloadStatus};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn host(runner: &str) -> Podman {
    let mut h = Podman::new(
        env("PNEX_TEST_CONTAINER_CLI"),
        None,
        std::env::temp_dir().join("pnex-runner-unused"),
    );
    h.set_runner_id(runner.to_string());
    h
}

fn spec(script: &str) -> RunnerJobSpec {
    RunnerJobSpec {
        image: env("PNEX_TEST_IMAGE"),
        command: vec!["sh".into(), "-c".into(), script.into()],
        timeout_secs: 60,
    }
}

fn unique(name: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{name}-{}-{nanos}", std::process::id())
}

/// Waits for the workload to leave `Running`.
async fn ended(h: &Podman, job: &str) -> WorkloadStatus {
    for _ in 0..240 {
        match h.status(job).await.unwrap() {
            WorkloadStatus::Running => tokio::time::sleep(Duration::from_millis(250)).await,
            done => return done,
        }
    }
    panic!("{job} still running after a minute");
}

#[tokio::test]
#[ignore = "needs a container CLI, see the file header"]
async fn exit_codes_come_back_and_destroy_leaves_nothing() {
    let runner = unique("r");
    let h = host(&runner);
    let (ok, bad) = (unique("ok"), unique("bad"));
    h.start(&ok, &spec("exit 0"), None, None, None)
        .await
        .unwrap();
    h.start(&bad, &spec("exit 7"), None, None, None)
        .await
        .unwrap();
    assert_eq!(ended(&h, &ok).await, WorkloadStatus::Exited(0));
    assert_eq!(ended(&h, &bad).await, WorkloadStatus::Exited(7));

    let mut listed = h.list().await.unwrap();
    listed.sort();
    let mut wanted = vec![ok.clone(), bad.clone()];
    wanted.sort();
    assert_eq!(listed, wanted, "only this runner's workloads");
    assert!(host("another-runner").list().await.unwrap().is_empty());

    h.destroy(&ok).await.unwrap();
    h.destroy(&bad).await.unwrap();
    h.destroy(&bad).await.unwrap();
    assert!(h.list().await.unwrap().is_empty());
    assert_eq!(h.status(&ok).await.unwrap(), WorkloadStatus::Gone);
}

/// Exit 0 only if every hardening holds from inside the container.
#[tokio::test]
#[ignore = "needs a container CLI, see the file header"]
async fn the_limits_hold_from_inside() {
    let h = host(&unique("r"));
    let job = unique("limits");
    let script = r#"
        [ "$(id -u)" = 65532 ] || exit 10
        touch /root-is-writable 2>/dev/null && exit 11
        touch /tmp/ok || exit 12
        grep -q '^CapEff:[[:space:]]*0000000000000000$' /proc/self/status || exit 13
        grep -q '^NoNewPrivs:[[:space:]]*1$' /proc/self/status || exit 14
        exit 0
    "#;
    h.start(&job, &spec(script), None, None, None)
        .await
        .unwrap();
    let status = ended(&h, &job).await;
    h.destroy(&job).await.unwrap();
    assert_eq!(status, WorkloadStatus::Exited(0));
}

#[tokio::test]
#[ignore = "needs a container CLI, see the file header"]
async fn a_missing_image_is_reported_as_such() {
    let h = host(&unique("r"));
    let job = unique("pull");
    let missing = RunnerJobSpec {
        image: format!("localhost:1/pnex-nope@sha256:{}", "0".repeat(64)),
        command: vec![],
        timeout_secs: 60,
    };
    h.start(&job, &missing, None, None, None).await.unwrap();
    assert_eq!(ended(&h, &job).await, WorkloadStatus::ImagePullFailed);
    h.destroy(&job).await.unwrap();
}
