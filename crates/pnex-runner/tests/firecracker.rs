//! A job in a real micro-VM (D762), driven through the executor like the runner does.
//! `PNEX_TEST_FC_BIN=<firecracker> PNEX_TEST_FC_KERNEL=<vmlinux>
//! PNEX_TEST_FC_IMAGE=<host/repo@sha256:...> PNEX_TEST_FC_INSECURE=<host>
//! cargo test -p pnex-runner --test firecracker -- --ignored` (needs `/dev/kvm`).

use std::time::{Duration, Instant};

use pnex_core::runners::RunnerJobSpec;
use pnex_runner::firecracker::Firecracker;
use pnex_runner::{Executor, WorkloadStatus};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

#[tokio::test]
#[ignore = "needs firecracker, a guest kernel, /dev/kvm and a registry"]
async fn a_job_runs_in_its_own_micro_vm_and_leaves_nothing() {
    let work = tempfile::tempdir().unwrap();
    let fc = Firecracker::new(
        work.path().to_path_buf(),
        env("PNEX_TEST_FC_BIN").into(),
        env("PNEX_TEST_FC_KERNEL").into(),
        std::env::var("PNEX_TEST_FC_INSECURE").into_iter().collect(),
        work.path().join("credentials.json"),
    );
    let spec = |command: &str| RunnerJobSpec {
        image: env("PNEX_TEST_FC_IMAGE"),
        command: vec!["sh".into(), "-c".into(), command.into()],
        timeout_secs: 120,
    };
    let started = Instant::now();
    fc.start(
        "vm-a",
        &spec("echo hello from the vm; uname -r; exit 3"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let status = loop {
        match fc.status("vm-a").await.unwrap() {
            WorkloadStatus::Running if started.elapsed() < Duration::from_secs(90) => {
                tokio::time::sleep(Duration::from_millis(200)).await
            }
            other => break other,
        }
    };
    let took = started.elapsed();
    assert_eq!(status, WorkloadStatus::Exited(3), "the code of the job");
    let logs = fc.logs("vm-a").await;
    assert!(
        logs.containers[0].text.contains("hello from the vm"),
        "{}",
        logs.containers[0].text
    );
    println!("pull, disk and run in {took:?}");

    // A second job reuses the unpacked image: only its own disk is made.
    let again = Instant::now();
    fc.start("vm-b", &spec("exit 0"), None, None, None)
        .await
        .unwrap();
    while fc.status("vm-b").await.unwrap() == WorkloadStatus::Running {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(fc.status("vm-b").await.unwrap(), WorkloadStatus::Exited(0));
    println!("second job in {:?}", again.elapsed());

    // A job that never ends is killed by its destruction; nothing stays behind.
    fc.start("vm-c", &spec("sleep 600"), None, None, None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(fc.status("vm-c").await.unwrap(), WorkloadStatus::Running);
    for job in ["vm-a", "vm-b", "vm-c"] {
        fc.destroy(job).await.unwrap();
    }
    assert!(fc.list().await.unwrap().is_empty());
    assert_eq!(fc.status("vm-c").await.unwrap(), WorkloadStatus::Gone);
}
