//! The sandbox pod on a real cluster: a sandbox test without a real kernel and a real
//! network plugin proves nothing. The agent checks each promise from the inside and exits
//! with the number of the one that fails. Goes through `kubectl`, in a dedicated namespace:
//! `PNEX_TEST_KUBE_NAMESPACE=<ns> PNEX_TEST_INIT_IMAGE=<by digest> PNEX_TEST_SERVICE_IMAGE=<valkey,
//! by digest> [PNEX_TEST_PULL_SECRET=<name>]
//! cargo test -p pnex-runner --test k8s_sandbox -- --ignored`

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use pnex_core::runners::{
    RunnerJobSpec, RunnerSandboxInput, SandboxEgress, SandboxRepo, SandboxService,
};
use pnex_runner::k8s::{sandbox_manifests, workload_status};
use pnex_runner::WorkloadStatus;
use serde_json::Value;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
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

const CLONE_TOKEN: &str = "clone-token-must-stay-in-its-step";
const RUN_SECRET: &str = "run-secret-for-the-agent";

/// Each check prints its name, so that a failure names what it broke.
const AGENT_CHECKS: &str = r#"
check() { echo "check $1"; }
check clone;      [ -f /workspace/app/README ] || exit 10
check setup;      [ "$(cat /workspace/setup-ran)" = done ] || exit 11
check non-root;   [ "$(id -u)" = 65532 ] || exit 12
check read-only;  touch /nope 2>/dev/null && exit 13
check capabilities; grep -q '^CapEff:[[:space:]]*0000000000000000$' /proc/self/status || exit 14
check no-privs;   grep -q '^NoNewPrivs:[[:space:]]*1$' /proc/self/status || exit 15
check secret;     [ "$API_TOKEN" = "run-secret-"for-the-agent ] || exit 16
check plain-env;  [ "$LOG_LEVEL" = info ] || exit 17
check token-env;  env | grep -q clone-token && exit 18
check token-files; grep -rq "clone-token-"must-stay /workspace /tmp 2>/dev/null && exit 19
check git-config; grep -q token /workspace/app/.git/config && exit 20
check no-api-token; [ -e /var/run/secrets/kubernetes.io/serviceaccount/token ] && exit 21
check service
out=$(timeout 10 git ls-remote http://127.0.0.1:6379/x 2>&1 || true)
echo "$out" | grep -qE 'Failed to connect|Connection refused|Could not connect' && { echo "$out"; exit 22; }
check allowed-egress
if ! GIT_CURL_VERBOSE=1 timeout 25 git ls-remote https://github.com/octocat/Hello-World >/tmp/out 2>&1; then
  grep -E 'Trying|Connected|fatal|Could not|timed out' /tmp/out; exit 23
fi
check refused-egress
if timeout 12 git ls-remote https://1.1.1.1/x.git >/tmp/out 2>&1; then exit 24; fi
grep -qE 'not found|403|404|repository' /tmp/out && { cat /tmp/out; exit 25; }
echo all-checks-passed
"#;

#[tokio::test]
#[ignore = "needs a cluster, see the file header"]
async fn a_sandbox_keeps_its_promises() {
    let job = format!("sandbox-test-{}", std::process::id());
    let init_image = env("PNEX_TEST_INIT_IMAGE");
    let spec = RunnerJobSpec {
        image: init_image.clone(),
        command: vec!["sh".into(), "-c".into(), AGENT_CHECKS.into()],
        timeout_secs: 240,
    };
    let input = RunnerSandboxInput {
        init_image,
        repos: vec![SandboxRepo {
            url: "https://github.com/octocat/Hello-World".into(),
            git_ref: "master".into(),
            path: "app".into(),
            token: Some(CLONE_TOKEN.into()),
            push: None,
        }],
        services: vec![SandboxService {
            name: "valkey".into(),
            image: env("PNEX_TEST_SERVICE_IMAGE"),
            env: BTreeMap::new(),
            ready_command: vec!["/usr/bin/valkey-cli".into(), "ping".into()],
            data_paths: vec!["/data".into()],
            run_as_user: None,
        }],
        setup: vec![
            "sh".into(),
            "-c".into(),
            "env | grep -qE 'run-secret|clone-token' && exit 30; echo done > /workspace/setup-ran"
                .into(),
        ],
        env: BTreeMap::from([("LOG_LEVEL".to_string(), "info".to_string())]),
        secrets: BTreeMap::from([("API_TOKEN".to_string(), RUN_SECRET.to_string())]),
        // GitHub's published web range: the forge, and nothing else.
        egress: vec![SandboxEgress {
            cidr: "140.82.112.0/20".into(),
            port: 443,
        }],
        artifacts: None,
    };
    let pull = std::env::var("PNEX_TEST_PULL_SECRET").ok();
    let (secret, policy, pod) = sandbox_manifests(&job, "test", &spec, &input, pull.as_deref());
    let name = pod["metadata"]["name"].as_str().unwrap().to_string();
    for manifest in [&secret, &policy, &pod] {
        kubectl(&["apply", "-f", "-"], Some(manifest));
    }
    let mut status = WorkloadStatus::Running;
    let mut seen = Value::Null;
    for _ in 0..110 {
        seen = serde_json::from_str(&kubectl(&["get", "pod", &name, "-o", "json"], None)).unwrap();
        status = workload_status(&seen);
        if status != WorkloadStatus::Running {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let spec_read = kubectl(&["get", "pod", &name, "-o", "json"], None);
    let logs = kubectl(&["logs", &name, "-c", "job", "--tail=40"], None);
    let setup = kubectl(&["logs", &name, "-c", "setup", "--tail=5"], None);
    let states = seen["status"]["initContainerStatuses"].to_string();
    for kind in ["pod", "secret", "networkpolicy"] {
        kubectl(&["delete", kind, &name, "--ignore-not-found"], None);
    }
    println!("{logs}");
    assert_eq!(
        status,
        WorkloadStatus::Exited(0),
        "{logs}\n{setup}\n{states}"
    );
    assert!(logs.contains("all-checks-passed"), "{logs}");
    for value in [CLONE_TOKEN, RUN_SECRET] {
        assert!(!spec_read.contains(value), "{value} in the pod spec");
        assert!(!logs.contains(value), "{value} in the logs");
    }
}
