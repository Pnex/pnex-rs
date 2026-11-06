//! A job in a real micro-VM (D762), driven through the executor like the runner does.
//! `PNEX_TEST_FC_BIN=<firecracker> PNEX_TEST_FC_KERNEL=<vmlinux>
//! PNEX_TEST_FC_IMAGE=<host/repo@sha256:...> PNEX_TEST_FC_INSECURE=<host>
//! cargo test -p pnex-runner --test firecracker -- --ignored` (needs `/dev/kvm`).

use std::time::{Duration, Instant};

use pnex_core::runners::RunnerJobSpec;
use pnex_runner::firecracker::Firecracker;
use pnex_runner::{Executor, WorkloadStatus};

mod support;

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

/// Run as root of a network namespace (`CAP_NET_ADMIN`), e.g. in a container given
/// `--cap-add NET_ADMIN --device /dev/kvm --device /dev/net/tun`.
#[tokio::test]
#[ignore = "needs firecracker, a guest kernel, /dev/kvm, a registry and CAP_NET_ADMIN"]
async fn a_sandbox_vm_reaches_its_listed_destinations_only() {
    use pnex_core::runners::{RunnerSandboxInput, SandboxEgress};
    use std::collections::BTreeMap;
    // Two servers on the host side of the VM: one listed, one not.
    let serve = || async {
        let l = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                if let Ok((mut s, _)) = l.accept().await {
                    use tokio::io::AsyncWriteExt;
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 7\r\nconnection: close\r\n\r\nreached")
                        .await;
                }
            }
        });
        port
    };
    let (open, closed) = (serve().await, serve().await);
    let work = tempfile::tempdir().unwrap();
    let mut fc = Firecracker::new(
        work.path().to_path_buf(),
        env("PNEX_TEST_FC_BIN").into(),
        env("PNEX_TEST_FC_KERNEL").into(),
        std::env::var("PNEX_TEST_FC_INSECURE").into_iter().collect(),
        work.path().join("credentials.json"),
    );
    fc.dns = Some("172.31.0.1".into());
    let host = "172.31.0.1"; // host side of the first slot
    let input = RunnerSandboxInput {
        init_image: String::new(),
        repos: vec![],
        services: vec![],
        setup: vec![],
        env: BTreeMap::from([("MODE".to_string(), "fix".to_string())]),
        secrets: BTreeMap::from([("PNEX_RUN_TOKEN".to_string(), "pnagr_vm-token".to_string())]),
        egress: vec![SandboxEgress {
            cidr: format!("{host}/32"),
            port: open,
        }],
        artifacts: None,
    };
    let script = format!(
        "echo mode=$MODE token=$PNEX_RUN_TOKEN; \
         curl -s -m 5 http://{host}:{open}/ && echo ' <- listed'; \
         curl -s -m 3 http://{host}:{closed}/ || echo 'unlisted refused'"
    );
    let spec = RunnerJobSpec {
        image: env("PNEX_TEST_FC_IMAGE"),
        command: vec!["sh".into(), "-c".into(), script],
        timeout_secs: 120,
    };
    fc.start("net-a", &spec, None, None, Some(&input))
        .await
        .unwrap();
    let started = Instant::now();
    while fc.status("net-a").await.unwrap() == WorkloadStatus::Running
        && started.elapsed() < Duration::from_secs(60)
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let console = console_of(&fc, "net-a", "vm").await;
    println!("{console}");
    assert!(
        console.contains("mode=fix token=pnagr_vm-token"),
        "the run's variables"
    );
    assert!(
        console.contains("reached <- listed"),
        "the listed destination answers"
    );
    assert!(
        console.contains("unlisted refused"),
        "the other one never does"
    );
    fc.destroy("net-a").await.unwrap();
    // The tap and the table went with the VM.
    let links = std::process::Command::new("ip")
        .args(["link"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&links.stdout).contains("pnxneta"));
}

async fn console_of(fc: &Firecracker, job: &str, container: &str) -> String {
    fc.logs(job)
        .await
        .containers
        .into_iter()
        .find(|c| c.container == container)
        .map(|c| c.text)
        .unwrap_or_default()
}

/// The whole of a sandbox in VMs: repository, inputs and caches placed by the runner, setup
/// in a first boot without secrets, the agent on a fresh image, outputs and caches sent.
/// Same host as the test above, with `git` and `curl`.
#[tokio::test]
#[ignore = "needs firecracker, a guest kernel, /dev/kvm, a registry and CAP_NET_ADMIN"]
async fn a_sandbox_vm_gets_its_repository_files_and_caches_and_setup_never_its_secrets() {
    use pnex_core::agent_runs::{ArtifactForm, RunCache};
    use pnex_core::runners::{RunnerSandboxInput, SandboxArtifact, SandboxArtifacts, SandboxRepo};
    use std::collections::BTreeMap;
    let work = tempfile::tempdir().unwrap();
    // A repository on the host, cloned like a remote one.
    let repo = work.path().join("repo");
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(&repo)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    std::fs::create_dir_all(&repo).unwrap();
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("hello.txt"), "from the repo\n").unwrap();
    std::fs::write(repo.join("deps.lock"), "lock v1\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);

    let notes = b"take care of the flaky test\n".to_vec();
    let server: support::Shared = Default::default();
    server
        .lock()
        .unwrap()
        .inputs
        .insert("notes".into(), notes.clone());
    let url = support::serve(server.clone()).await;

    let mut fc = Firecracker::new(
        work.path().join("fc"),
        env("PNEX_TEST_FC_BIN").into(),
        env("PNEX_TEST_FC_KERNEL").into(),
        std::env::var("PNEX_TEST_FC_INSECURE").into_iter().collect(),
        work.path().join("credentials.json"),
    );
    fc.dns = Some("172.31.0.1".into());
    let input = RunnerSandboxInput {
        init_image: String::new(),
        repos: vec![SandboxRepo {
            url: format!("file://{}", repo.display()),
            git_ref: "main".into(),
            path: "app".into(),
            token: None,
        }],
        services: vec![],
        setup: vec![
            "sh".into(),
            "-c".into(),
            "echo built > app/built; mkdir -p /root/.deps; echo dep > /root/.deps/x; \
             touch /setup-was-here; echo setup-token=${PNEX_RUN_TOKEN:-none}"
                .into(),
        ],
        env: BTreeMap::from([("MODE".to_string(), "fix".to_string())]),
        secrets: BTreeMap::from([("PNEX_RUN_TOKEN".to_string(), "pnagr_vm-token".to_string())]),
        egress: vec![],
        artifacts: Some(SandboxArtifacts {
            url,
            token: support::TOKEN.into(),
            inputs: vec![SandboxArtifact {
                name: "notes".into(),
                sha256: Some(format!("sha256:{}", support::hex(&notes))),
                form: Some(ArtifactForm::File),
            }],
            outputs: vec![SandboxArtifact {
                name: "report".into(),
                sha256: None,
                form: None,
            }],
            caches: vec![RunCache {
                name: "deps".into(),
                path: "/root/.deps".into(),
                key_files: vec!["app/deps.lock".into()],
            }],
        }),
    };
    let agent = "cat app/hello.txt app/built $PNEX_ARTIFACTS_IN/notes /root/.deps/x; \
                 echo token=$PNEX_RUN_TOKEN; echo done > $PNEX_ARTIFACTS_OUT/report; \
                 test ! -e /setup-was-here";
    let spec = RunnerJobSpec {
        image: env("PNEX_TEST_FC_IMAGE"),
        command: vec!["sh".into(), "-c".into(), agent.into()],
        timeout_secs: 120,
    };
    let started = Instant::now();
    fc.start("box-a", &spec, None, None, Some(&input))
        .await
        .unwrap();
    let status = loop {
        match fc.status("box-a").await.unwrap() {
            WorkloadStatus::Running if started.elapsed() < Duration::from_secs(120) => {
                tokio::time::sleep(Duration::from_millis(200)).await
            }
            other => break other,
        }
    };
    let (transfer, setup, vm) = (
        console_of(&fc, "box-a", "transfer").await,
        console_of(&fc, "box-a", "setup").await,
        console_of(&fc, "box-a", "vm").await,
    );
    println!("--- transfer\n{transfer}\n--- setup\n{setup}\n--- vm\n{vm}");
    println!("whole sandbox in {:?}", started.elapsed());
    // Before any assertion: a failed one must not leave the tap to the next test.
    fc.destroy("box-a").await.unwrap();
    assert_eq!(
        status,
        WorkloadStatus::Exited(0),
        "the agent ran on a fresh image"
    );
    assert!(setup.contains("setup-token=none"), "setup gets no secret");
    for seen in [
        "from the repo",
        "built",
        "take care of the flaky test",
        "dep",
        "token=pnagr_vm-token",
    ] {
        assert!(vm.contains(seen), "the agent sees {seen}");
    }
    {
        let s = server.lock().unwrap();
        assert_eq!(
            s.outputs["report"],
            ("file".to_string(), b"done\n".to_vec())
        );
        assert_eq!(s.caches.len(), 1, "the cache is saved under its key");
    }
    assert!(fc.list().await.unwrap().is_empty());

    // Stopped before its end: what the agent left is still sent, in the background (D763).
    server.lock().unwrap().outputs.clear();
    let spec = RunnerJobSpec {
        command: vec![
            "sh".into(),
            "-c".into(),
            "echo partial > $PNEX_ARTIFACTS_OUT/report; sync; sleep 600".into(),
        ],
        ..spec
    };
    let input = RunnerSandboxInput {
        setup: vec![],
        ..input
    };
    fc.start("box-b", &spec, None, None, Some(&input))
        .await
        .unwrap();
    while !console_of(&fc, "box-b", "vm")
        .await
        .contains("Successfully started")
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    fc.destroy("box-b").await.unwrap();
    assert_eq!(fc.status("box-b").await.unwrap(), WorkloadStatus::Gone);
    let sent = Instant::now();
    while server.lock().unwrap().outputs.is_empty() && sent.elapsed() < Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        server.lock().unwrap().outputs["report"].1,
        b"partial\n".to_vec()
    );
    while !fc.list().await.unwrap().is_empty() || work.path().join("fc/jobs/box-b").exists() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
