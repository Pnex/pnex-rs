//! The build pod of the Kubernetes executor on a real cluster, since only a real kernel
//! says whether rootless BuildKit runs. Goes through `kubectl`, in a dedicated namespace:
//! `PNEX_TEST_KUBE_NAMESPACE=<ns> PNEX_TEST_BUILDER_IMAGE=<by digest>
//! PNEX_TEST_PULL_SECRET=<name> PNEX_TEST_REGISTRY_HOST=<host>
//! PNEX_TEST_REGISTRY_TARGET=<host/repo:tag> PNEX_TEST_REGISTRY_USER=<user>
//! PNEX_TEST_REGISTRY_SECRET=<secret>
//! cargo test -p pnex-runner --test k8s_build -- --ignored`

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use pnex_core::environments::EnvironmentSource;
use pnex_core::runners::RunnerScanInput;
use pnex_core::runners::{digest_is_valid, RunnerBuildCache, RunnerBuildInput, RunnerJobSpec};
use pnex_runner::k8s::{
    build_manifests, job_policy, parse_scan_output, pod_manifest, reported_digest, scan_manifests,
    workload_status,
};
use pnex_runner::WorkloadStatus;
use serde_json::Value;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

/// In-cluster destinations the jobs may reach, `PNEX_TEST_JOB_EGRESS` (a cache in the cluster).
fn extra_egress() -> Vec<String> {
    std::env::var("PNEX_TEST_JOB_EGRESS")
        .map(|v| v.split(',').map(str::to_string).collect())
        .unwrap_or_default()
}

fn kubectl(args: &[&str], stdin: Option<&Value>) -> String {
    let namespace = env("PNEX_TEST_KUBE_NAMESPACE");
    let mut child = Command::new("kubectl")
        .args(["-n", &namespace])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("kubectl");
    if let Some(value) = stdin {
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(value.to_string().as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "kubectl {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
#[ignore = "needs a cluster and a registry, see the file header"]
async fn the_build_pod_pushes_and_reports_its_digest() {
    let job = format!("build-test-{}", std::process::id());
    let spec = RunnerJobSpec {
        image: env("PNEX_TEST_BUILDER_IMAGE"),
        command: vec![],
        timeout_secs: 300,
    };
    let input = RunnerBuildInput {
        source: EnvironmentSource {
            dockerfile: "FROM cgr.dev/chainguard/wolfi-base\nARG GREETING\n\
                         COPY scripts/hello.sh /hello.sh\n\
                         RUN test \"$GREETING\" = \"hello; world\" && sh /hello.sh > /built.txt\n"
                .into(),
            files: BTreeMap::from([("scripts/hello.sh".to_string(), "echo built".to_string())]),
            build_args: BTreeMap::from([("GREETING".to_string(), "hello; world".to_string())]),
        },
        target: env("PNEX_TEST_REGISTRY_TARGET"),
        registry_host: env("PNEX_TEST_REGISTRY_HOST"),
        tls: true,
        username: env("PNEX_TEST_REGISTRY_USER"),
        secret: env("PNEX_TEST_REGISTRY_SECRET"),
        // With a store: `PNEX_TEST_CACHE_ENDPOINT`, `_BUCKET`, `_PREFIX`, `_ACCESS`, `_SECRET`,
        // `_TOKEN`.
        cache: std::env::var("PNEX_TEST_CACHE_ENDPOINT")
            .ok()
            .map(|endpoint| RunnerBuildCache {
                endpoint,
                region: "us-east-1".to_string(),
                bucket: env("PNEX_TEST_CACHE_BUCKET"),
                prefix: env("PNEX_TEST_CACHE_PREFIX"),
                access_key: env("PNEX_TEST_CACHE_ACCESS"),
                secret_key: env("PNEX_TEST_CACHE_SECRET"),
                session_token: env("PNEX_TEST_CACHE_TOKEN"),
            }),
    };
    let pull = std::env::var("PNEX_TEST_PULL_SECRET").ok();
    let (secret, context, pod) = build_manifests(&job, "test", &spec, &input, pull.as_deref());
    let policy = job_policy(&job, "test", &extra_egress());
    let name = pod["metadata"]["name"].as_str().unwrap().to_string();
    for manifest in [&policy, &secret, &context, &pod] {
        kubectl(&["apply", "-f", "-"], Some(manifest));
    }
    let mut status = WorkloadStatus::Running;
    let mut seen = Value::Null;
    for _ in 0..120 {
        seen = serde_json::from_str(&kubectl(&["get", "pod", &name, "-o", "json"], None)).unwrap();
        status = workload_status(&seen);
        if status != WorkloadStatus::Running {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let logs = kubectl(&["logs", &name], None);
    for kind in ["pod", "secret", "configmap", "networkpolicy"] {
        kubectl(&["delete", kind, &name, "--ignore-not-found"], None);
    }
    assert_eq!(status, WorkloadStatus::Exited(0), "{logs}");
    let digest = reported_digest(&seen).expect("a digest");
    assert!(digest_is_valid(&digest), "{digest}");
    println!("pushed {digest}");
    println!("cached steps: {}", logs.matches("CACHED").count());
}

/// `PNEX_TEST_SCANNER_IMAGE=<by digest> PNEX_TEST_SCAN_TARGET=<image>` on top of the above.
/// A public image with a known flaw, such as `alpine:3.10`, shows findings come back.
#[tokio::test]
#[ignore = "needs a cluster and a registry, see the file header"]
async fn the_scan_pod_reports_findings() {
    let job = format!("scan-test-{}", std::process::id());
    let spec = RunnerJobSpec {
        image: env("PNEX_TEST_SCANNER_IMAGE"),
        command: pnex_core::runners::scan_command(&env("PNEX_TEST_SCAN_TARGET")),
        timeout_secs: 400,
    };
    let login = RunnerScanInput {
        registry_host: env("PNEX_TEST_REGISTRY_HOST"),
        username: env("PNEX_TEST_REGISTRY_USER"),
        secret: env("PNEX_TEST_REGISTRY_SECRET"),
    };
    let pull = std::env::var("PNEX_TEST_PULL_SECRET").ok();
    let (secret, pod) = scan_manifests(&job, "test", &spec, Some(&login), pull.as_deref());
    let name = pod["metadata"]["name"].as_str().unwrap().to_string();
    kubectl(&["apply", "-f", "-"], Some(&job_policy(&job, "test", &[])));
    kubectl(&["apply", "-f", "-"], Some(&secret.unwrap()));
    kubectl(&["apply", "-f", "-"], Some(&pod));
    let mut status = WorkloadStatus::Running;
    for _ in 0..200 {
        let seen =
            serde_json::from_str(&kubectl(&["get", "pod", &name, "-o", "json"], None)).unwrap();
        status = workload_status(&seen);
        if status != WorkloadStatus::Running {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let logs = kubectl(&["logs", &name], None);
    for kind in ["pod", "secret", "networkpolicy"] {
        kubectl(&["delete", kind, &name, "--ignore-not-found"], None);
    }
    assert_eq!(status, WorkloadStatus::Exited(0), "{logs}");
    let (report, sbom) = parse_scan_output(&logs).expect("a report");
    assert!(report.is_valid());
    println!(
        "components {} findings {:?} sbom {} bytes",
        report.components.len(),
        report.summary(),
        sbom.len()
    );
    assert!(!report.components.is_empty());
    assert!(report.components.iter().all(|c| c.purl.starts_with("pkg:")));
}

/// SEC-E2: under its rule a job pod reaches the internet, not the cluster.
/// `PNEX_TEST_PROBE_IMAGE=<image with sh and curl> PNEX_TEST_INTERNAL_URL=<http service of
/// the namespace>` on top of the above.
#[tokio::test]
#[ignore = "needs a cluster, see the file header"]
async fn a_job_pod_reaches_the_internet_not_the_cluster() {
    let job = format!("egress-test-{}", std::process::id());
    let script = format!(
        "curl -s -o /dev/null -m 10 https://www.google.com && echo PUBLIC_OK; \
         curl -sS -o /dev/null -m 5 {} 2>&1; echo INTERNAL_DONE",
        env("PNEX_TEST_INTERNAL_URL")
    );
    let spec = RunnerJobSpec {
        image: env("PNEX_TEST_PROBE_IMAGE"),
        command: vec!["sh".into(), "-c".into(), script],
        timeout_secs: 300,
    };
    let mut pod = pod_manifest(&job, "test", &spec, None);
    if let Ok(pull) = std::env::var("PNEX_TEST_PULL_SECRET") {
        pod["spec"]["imagePullSecrets"] = serde_json::json!([{ "name": pull }]);
    }
    let name = pod["metadata"]["name"].as_str().unwrap().to_string();
    kubectl(&["apply", "-f", "-"], Some(&job_policy(&job, "test", &[])));
    kubectl(&["apply", "-f", "-"], Some(&pod));
    let mut status = WorkloadStatus::Running;
    for _ in 0..150 {
        let seen =
            serde_json::from_str(&kubectl(&["get", "pod", &name, "-o", "json"], None)).unwrap();
        status = workload_status(&seen);
        if status != WorkloadStatus::Running {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let logs = kubectl(&["logs", &name], None);
    for kind in ["pod", "networkpolicy"] {
        kubectl(&["delete", kind, &name, "--ignore-not-found"], None);
    }
    assert_eq!(status, WorkloadStatus::Exited(0), "{logs}");
    assert!(logs.contains("PUBLIC_OK"), "{logs}");
    // A refused or answered request would mean the packet got through.
    assert!(logs.contains("timed out"), "{logs}");
}
