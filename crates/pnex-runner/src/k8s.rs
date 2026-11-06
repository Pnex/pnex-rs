//! Kubernetes executor: plain calls to the API server with the runner's
//! ServiceAccount, limited to its namespace. PneX holds no kubeconfig (D386).

use std::time::Duration;

use base64::Engine;
use pnex_core::agent_runs::{ARTIFACTS_IN_DIR, ARTIFACTS_OUT_DIR};
use pnex_core::environments::{ScanComponent, ScanFinding, ScanReport};
use pnex_core::runners::{
    is_scan_command, RunnerBuildInput, RunnerContainerLog, RunnerJobLogs, RunnerJobSpec,
    RunnerSandboxInput, RunnerScanInput, LOG_CONTAINERS_MAX, LOG_CONTAINER_MAX_BYTES,
    LOG_TAIL_LINES, SCAN_REPORT_MARKER,
};
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::{capabilities_of, Executor, Host, WorkloadStatus};

const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
const TIMEOUT: Duration = Duration::from_secs(20);
const TOKEN_SECRET: &str = "pnex-runner-token";
const POD_PREFIX: &str = "pnex-job-";
const UID: i64 = 65532;
const BUILD_UID: i64 = 1000;
const BUILD_STATE: &str = "/home/user/.local/share/buildkit";
/// Announced by a runner whose cluster may run the build pod, a deviation from the
/// hardened pod (D701).
pub const BUILD_CAPABILITY: &str = "builder:buildkit";
const SCAN_CAPABILITY: &str = "scanner:trivy";
const SANDBOX_CAPABILITY: &str = "sandbox:pod";
/// A larger report than this reads as a failed scan, not as a runner out of memory.
const SCAN_LOG_MAX_BYTES: u32 = 32 * 1024 * 1024;

pub struct K8s {
    http: reqwest::Client,
    base: String,
    namespace: String,
    runner_id: String,
    runtime_class: Option<String>,
    builds: bool,
    pull_secret: Option<String>,
    job_egress: Vec<String>,
    /// The runner's own image and the address its gates probe (SEC-E4).
    gate: Option<(String, String)>,
}

/// Labels of everything a job owns, so that a stray is found whatever it is.
fn labels(job: &str, runner_id: &str) -> Value {
    json!({
        "app.kubernetes.io/managed-by": "pnex-runner",
        "pnex.io/runner": runner_id,
        "pnex.io/job": job
    })
}

/// Runs the build, then hands the pushed digest to the runner as the termination message.
/// buildctl writes its metadata by renaming a temporary file, which `/dev` refuses
/// (measured on kind, 2026-11-06): it lands in the state volume and is copied from there.
const BUILD_SCRIPT: &str = r#"set -eu
meta="/home/user/.local/share/buildkit/metadata.json"
buildctl-daemonless.sh build "$@" --metadata-file="$meta"
cat "$meta" > /dev/termination-log
"#;

/// `docker login` for one registry, in the form buildctl reads.
fn docker_config(build: &RunnerBuildInput) -> String {
    let auth = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", build.username, build.secret));
    json!({ "auths": { build.registry_host.clone(): { "auth": auth } } }).to_string()
}

/// `(secret, config map, pod)` of a build. The pod is rootless but not hardened like a job
/// pod: `rootlesskit` needs its setuid helpers and an open seccomp profile, so
/// a `RUN` step can reach what the pod holds, hence credentials for this one
/// repository only (D701). Its arguments never go through a shell.
pub fn build_manifests(
    job: &str,
    runner_id: &str,
    spec: &RunnerJobSpec,
    build: &RunnerBuildInput,
    pull_secret: Option<&str>,
) -> (Value, Value, Value) {
    let name = format!("{POD_PREFIX}{job}");
    let secret = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": name, "labels": labels(job, runner_id) },
        "type": "Opaque",
        "stringData": { "config.json": docker_config(build) }
    });
    let mut secret = secret;
    if let Some(cache) = &build.cache {
        let data = &mut secret["stringData"];
        data["AWS_ACCESS_KEY_ID"] = json!(cache.access_key);
        data["AWS_SECRET_ACCESS_KEY"] = json!(cache.secret_key);
        data["AWS_SESSION_TOKEN"] = json!(cache.session_token);
    }
    // Keys of a ConfigMap cannot hold a path: `items` puts each file back in place.
    let mut data = serde_json::Map::new();
    let mut items = vec![json!({ "key": "dockerfile", "path": "Dockerfile" })];
    data.insert("dockerfile".into(), json!(build.source.dockerfile));
    for (i, (path, content)) in build.source.files.iter().enumerate() {
        let key = format!("file{i}");
        items.push(json!({ "key": key, "path": path }));
        data.insert(key, json!(content));
    }
    let context = json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": { "name": name, "labels": labels(job, runner_id) },
        "data": data
    });
    let mut output = format!("type=image,name={},push=true", build.target);
    if !build.tls {
        output.push_str(",registry.insecure=true");
    }
    let mut args = vec![
        "--frontend=dockerfile.v0".to_string(),
        "--local=context=/ctx".to_string(),
        "--local=dockerfile=/ctx".to_string(),
        "--progress=plain".to_string(),
        format!("--output={output}"),
    ];
    for (key, value) in &build.source.build_args {
        args.push("--opt".to_string());
        args.push(format!("build-arg:{key}={value}"));
    }
    let mut container_env = vec![json!({ "name": "DOCKER_CONFIG", "value": "/docker" })];
    if let Some(cache) = &build.cache {
        // A cache that fails slows a build, it does not fail it.
        let location = format!(
            "type=s3,region={},bucket={},prefix={},name=main,endpoint_url={},use_path_style=true,ignore-error=true",
            cache.region, cache.bucket, cache.prefix, cache.endpoint
        );
        args.push(format!("--export-cache={location},mode=max"));
        args.push(format!("--import-cache={location}"));
        for key in [
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
        ] {
            container_env.push(json!({
                "name": key,
                "valueFrom": { "secretKeyRef": { "name": name, "key": key } }
            }));
        }
    }
    let (memory, storage) = match &build.resources {
        Some(r)
            if pnex_core::runners::quantity_is_valid(&r.memory)
                && pnex_core::runners::quantity_is_valid(&r.ephemeral_storage) =>
        {
            (r.memory.clone(), r.ephemeral_storage.clone())
        }
        _ => ("8Gi".to_string(), "40Gi".to_string()),
    };
    let mut pod_spec = json!({
        "restartPolicy": "Never",
        "automountServiceAccountToken": false,
        "enableServiceLinks": false,
        "activeDeadlineSeconds": spec.timeout_secs,
        "terminationGracePeriodSeconds": 5,
        "securityContext": {
            "runAsNonRoot": true,
            "seccompProfile": { "type": "Unconfined" },
            "appArmorProfile": { "type": "Unconfined" }
        },
        "containers": [{
            "name": "job",
            "image": spec.image,
            "imagePullPolicy": "IfNotPresent",
            // Explicit, not the entrypoint of the image: any rootless BuildKit image with
            // `sh` works. The arguments follow the script as its `$@`.
            "command": ["sh", "-c", BUILD_SCRIPT, "build"],
            "args": args,
            "env": container_env,
            "securityContext": {
                "runAsUser": BUILD_UID,
                "runAsGroup": BUILD_UID,
                "privileged": false
            },
            // The global settings of the server (D755), else enough for a Rust or Java image.
            "resources": {
                "requests": { "cpu": "500m", "memory": "1Gi", "ephemeral-storage": "1Gi" },
                "limits": { "cpu": "4", "memory": memory, "ephemeral-storage": storage }
            },
            "volumeMounts": [
                { "name": "context", "mountPath": "/ctx", "readOnly": true },
                { "name": "docker", "mountPath": "/docker", "readOnly": true },
                { "name": "state", "mountPath": BUILD_STATE }
            ]
        }],
        "volumes": [
            { "name": "context", "configMap": { "name": name, "items": items } },
            { "name": "docker", "secret": {
                "secretName": name,
                "items": [{ "key": "config.json", "path": "config.json" }]
            } },
            { "name": "state", "emptyDir": {} }
        ]
    });
    if let Some(pull) = pull_secret {
        pod_spec["imagePullSecrets"] = json!([{ "name": pull }]);
    }
    let pod = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": name, "labels": labels(job, runner_id) },
        "spec": pod_spec
    });
    (secret, context, pod)
}

/// `(secret, pod)` of a scan: the hardened pod of any job, with room for the vulnerability
/// database and, when the registry asks for one, a read login. No write secret: a scan
/// never pushes (D703).
pub fn scan_manifests(
    job: &str,
    runner_id: &str,
    spec: &RunnerJobSpec,
    scan: Option<&RunnerScanInput>,
    pull_secret: Option<&str>,
) -> (Option<Value>, Value) {
    let name = format!("{POD_PREFIX}{job}");
    let mut pod = pod_manifest(job, runner_id, spec, None);
    let pod_spec = &mut pod["spec"];
    pod_spec["volumes"][0]["emptyDir"]["sizeLimit"] = json!("5Gi");
    let container = &mut pod_spec["containers"][0];
    container["resources"] = json!({
        "requests": { "cpu": "100m", "memory": "256Mi", "ephemeral-storage": "1Gi" },
        "limits": { "cpu": "2", "memory": "2Gi", "ephemeral-storage": "6Gi" }
    });
    if let Some(pull) = pull_secret {
        pod["spec"]["imagePullSecrets"] = json!([{ "name": pull }]);
    }
    let Some(scan) = scan else {
        return (None, pod);
    };
    let auth = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", scan.username, scan.secret));
    let config = json!({ "auths": { scan.registry_host.clone(): { "auth": auth } } }).to_string();
    let secret = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": name, "labels": labels(job, runner_id) },
        "type": "Opaque",
        "stringData": { "config.json": config }
    });
    let container = &mut pod["spec"]["containers"][0];
    container["env"] = json!([{ "name": "DOCKER_CONFIG", "value": "/docker" }]);
    container["volumeMounts"]
        .as_array_mut()
        .expect("set above")
        .push(json!({ "name": "docker", "mountPath": "/docker", "readOnly": true }));
    pod["spec"]["volumes"]
        .as_array_mut()
        .expect("set above")
        .push(json!({ "name": "docker", "secret": { "secretName": name } }));
    (Some(secret), pod)
}

/// The document that starts at the first line opening one: logs may start with lines from
/// the container runtime.
fn from_first_document(text: &str) -> Option<&str> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_end() == "{" {
            return text.get(at..);
        }
        at += line.len();
    }
    None
}

/// The SBOM as written, and our format of the findings, from the output of a scan pod.
pub fn parse_scan_output(logs: &str) -> Option<(ScanReport, String)> {
    let (sbom, findings) = logs.split_once(&format!("{SCAN_REPORT_MARKER}\n"))?;
    let sbom = from_first_document(sbom)?.trim();
    let parsed: Value = serde_json::from_str(sbom).ok()?;
    if parsed["bomFormat"] != "CycloneDX" {
        return None;
    }
    let report: Value = serde_json::from_str(from_first_document(findings)?).ok()?;
    if !report["SchemaVersion"].is_number() {
        return None;
    }
    let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_string();
    let mut components = Vec::new();
    let mut found = Vec::new();
    for result in report["Results"].as_array().into_iter().flatten() {
        for p in result["Packages"].as_array().into_iter().flatten() {
            components.push(ScanComponent {
                purl: text(&p["Identifier"], "PURL"),
                name: text(p, "Name"),
                version: text(p, "Version"),
            });
        }
        for v in result["Vulnerabilities"].as_array().into_iter().flatten() {
            found.push(ScanFinding {
                id: text(v, "VulnerabilityID"),
                purl: v["PkgIdentifier"]["PURL"].as_str().map(str::to_string),
                package: text(v, "PkgName"),
                installed: text(v, "InstalledVersion"),
                fixed: v["FixedVersion"]
                    .as_str()
                    .filter(|f| !f.is_empty())
                    .map(str::to_string),
                severity: match text(v, "Severity").to_lowercase().as_str() {
                    s @ ("critical" | "high" | "medium" | "low") => s.to_string(),
                    _ => "unknown".to_string(),
                },
            });
        }
    }
    Some((
        ScanReport {
            components,
            findings: found,
        },
        sbom.to_string(),
    ))
}

/// The hardened security context every container of a sandbox runs with.
fn hardened(uid: i64) -> Value {
    json!({
        "runAsNonRoot": true,
        "runAsUser": uid,
        "runAsGroup": uid,
        "allowPrivilegeEscalation": false,
        "privileged": false,
        "readOnlyRootFilesystem": true,
        "capabilities": { "drop": ["ALL"] },
        "seccompProfile": { "type": "RuntimeDefault" }
    })
}

/// `git` with the token of a repository from the environment of this container only, so that
/// it lands neither in the URL nor in `.git/config` (D714).
pub(crate) const CLONE_SCRIPT: &str = r#"set -eu
if [ -n "${GIT_TOKEN:-}" ]; then
  exec git -c 'credential.helper=!f() { echo username=x-access-token; echo "password=$GIT_TOKEN"; }; f' \
    clone --depth 1 --no-recurse-submodules --branch "$1" -- "$2" "$3"
fi
exec git clone --depth 1 --no-recurse-submodules --branch "$1" -- "$2" "$3"
"#;

/// First container of a sandbox: a `NetworkPolicy` is applied by the network plugin some
/// time after it exists (seconds on kindnet, measured 2026-11-06), so a pod started at once
/// gets out freely meanwhile. Nothing else starts until a public address, which the rule
/// refuses, stops answering; past a minute the pod fails instead of running open. Not the
/// API of the cluster: on its own node, a pod reaches it whatever the rule (SEC-E5).
/// shortcut: a fixed probe; read it from the runner's settings when a site has no internet
/// (there the probe never answers and the gate opens at once, with no internet to leak to).
pub const EGRESS_GATE_SCRIPT: &str = r#"set -u
target="${PNEX_EGRESS_PROBE:-https://1.1.1.1/}"
i=0
while curl -sk -o /dev/null --max-time 2 "$target"; do
  i=$((i + 1))
  if [ "$i" -ge 60 ]; then
    echo "egress rule not applied after a minute: not starting" >&2
    exit 1
  fi
  sleep 1
done
echo "egress rule in place"
"#;

/// Puts the egress gate first in a job pod (SEC-E4): the job starts only once its rule
/// holds. Hardened, no volume, no secret: a TCP connect from the runner's image.
pub fn with_gate(pod: &mut Value, image: &str, probe: &str) {
    let gate = json!({
        "name": "egress-gate",
        "image": image,
        "command": ["/usr/local/bin/pnex-runner", "egress-gate"],
        "env": [{ "name": "PNEX_EGRESS_PROBE", "value": probe }],
        "securityContext": hardened(UID),
        "resources": {
            "requests": { "cpu": "10m", "memory": "16Mi" },
            "limits": { "cpu": "100m", "memory": "32Mi" }
        },
    });
    let spec = &mut pod["spec"];
    match spec["initContainers"].as_array_mut() {
        Some(init) => init.insert(0, gate),
        None => spec["initContainers"] = json!([gate]),
    }
}

/// Containers whose output is the journal of a job, in the order they ran: init containers
/// first (`clone`, services, `setup`), then the job. None for a scan, whose output is its
/// report, stored apart (D728).
pub fn log_containers(pod: &Value) -> Vec<String> {
    let spec = &pod["spec"];
    let command: Vec<String> =
        serde_json::from_value(spec["containers"][0]["command"].clone()).unwrap_or_default();
    if is_scan_command(&command) {
        return vec![];
    }
    ["initContainers", "containers"]
        .iter()
        .flat_map(|key| spec[*key].as_array().into_iter().flatten())
        .filter_map(|c| c["name"].as_str().map(str::to_string))
        .take(LOG_CONTAINERS_MAX)
        .collect()
}

/// `(secret, network policy, pod)` of a sandbox. The order is that of D714 and D717: clone,
/// services, setup, agent. Each secret goes to one container, by reference: none is in the
/// spec, the clone tokens never reach the agent, and `setup`, which runs code from the
/// repository, gets none.
pub fn sandbox_manifests(
    job: &str,
    runner_id: &str,
    spec: &RunnerJobSpec,
    input: &RunnerSandboxInput,
    pull_secret: Option<&str>,
) -> (Value, Value, Value) {
    let name = format!("{POD_PREFIX}{job}");
    let mut data = serde_json::Map::new();
    let has_outputs = input
        .artifacts
        .as_ref()
        .is_some_and(|a| !a.outputs.is_empty());
    let tmp = json!({ "name": "tmp", "mountPath": "/tmp" });
    let workspace = json!({ "name": "workspace", "mountPath": "/workspace" });
    let resources = json!({
        "requests": { "cpu": "100m", "memory": "128Mi", "ephemeral-storage": "128Mi" },
        "limits": { "cpu": "2", "memory": "4Gi", "ephemeral-storage": "20Gi" }
    });
    let plain_env = |extra: &[(&str, &str)]| -> Vec<Value> {
        input
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(extra.iter().copied())
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect()
    };
    let mut init = vec![json!({
        "name": "egress-gate",
        "image": input.init_image,
        "command": ["sh", "-c", EGRESS_GATE_SCRIPT],
        "securityContext": hardened(UID),
        "resources": resources,
        "volumeMounts": [tmp],
    })];
    for (i, repo) in input.repos.iter().enumerate() {
        let mut env = vec![json!({ "name": "HOME", "value": "/tmp" })];
        if repo.token.is_some() {
            let key = format!("clone-token-{i}");
            env.push(json!({
                "name": "GIT_TOKEN",
                "valueFrom": { "secretKeyRef": { "name": name, "key": key } }
            }));
            data.insert(key, json!(repo.token));
        }
        init.push(json!({
            "name": format!("clone-{i}"),
            "image": input.init_image,
            "command": [
                "sh", "-c", CLONE_SCRIPT, "clone",
                repo.git_ref, repo.url, format!("/workspace/{}", repo.path)
            ],
            "env": env,
            "securityContext": hardened(UID),
            "resources": resources,
            "volumeMounts": [workspace, tmp],
        }));
    }
    let mut volumes = vec![
        json!({ "name": "tmp", "emptyDir": { "sizeLimit": "2Gi" } }),
        json!({ "name": "workspace", "emptyDir": { "sizeLimit": "16Gi" } }),
    ];
    // Inputs land before setup and the agent; the collector runs beside them to the end.
    if let Some(artifacts) = &input.artifacts {
        data.insert("artifact-token".into(), json!(artifacts.token));
        volumes.push(json!({ "name": "artifacts", "emptyDir": { "sizeLimit": "16Gi" } }));
        volumes.extend(crate::artifacts::cache_volumes_of(artifacts));
        let (fetch, collect) = crate::artifacts::transfer_containers(
            artifacts,
            &input.init_image,
            &name,
            &hardened(UID),
            &resources,
        );
        init.extend(fetch);
        init.extend(collect);
    }
    for service in &input.services {
        let mut mounts =
            vec![json!({ "name": format!("{}-tmp", service.name), "mountPath": "/tmp" })];
        volumes.push(
            json!({ "name": format!("{}-tmp", service.name), "emptyDir": { "sizeLimit": "1Gi" } }),
        );
        for (i, path) in service.data_paths.iter().enumerate() {
            let volume = format!("{}-data-{i}", service.name);
            mounts.push(json!({ "name": volume, "mountPath": path }));
            volumes.push(json!({ "name": volume, "emptyDir": { "sizeLimit": "4Gi" } }));
        }
        // A native sidecar: it runs from here on, and the next step waits for its probe.
        init.push(json!({
            "name": format!("service-{}", service.name),
            "image": service.image,
            "restartPolicy": "Always",
            "env": service.env.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>(),
            "startupProbe": {
                "exec": { "command": service.ready_command },
                "periodSeconds": 2,
                "failureThreshold": 60
            },
            "securityContext": hardened(service.run_as_user.unwrap_or(UID)),
            "resources": resources,
            "volumeMounts": mounts,
        }));
    }
    // Caches are where dependencies land: setup fills them as much as the agent does.
    let cache_mounts = input
        .artifacts
        .as_ref()
        .map(crate::artifacts::cache_mounts_of)
        .unwrap_or_default();
    if !input.setup.is_empty() {
        let mut mounts = vec![workspace.clone(), tmp.clone()];
        mounts.extend(cache_mounts.iter().cloned());
        init.push(json!({
            "name": "setup",
            "image": spec.image,
            "command": input.setup,
            "workingDir": "/workspace",
            "env": plain_env(&[("HOME", "/tmp")]),
            "securityContext": hardened(UID),
            "resources": resources,
            "volumeMounts": mounts,
        }));
    }
    let mut agent_env = plain_env(&[("HOME", "/tmp")]);
    let mut agent_mounts = vec![workspace.clone(), tmp.clone()];
    if input.artifacts.is_some() {
        agent_env.push(json!({ "name": "PNEX_ARTIFACTS_IN", "value": ARTIFACTS_IN_DIR }));
        agent_env.push(json!({ "name": "PNEX_ARTIFACTS_OUT", "value": ARTIFACTS_OUT_DIR }));
        agent_mounts.extend(crate::artifacts::agent_mounts());
        agent_mounts.extend(cache_mounts);
    }
    for (key, value) in &input.secrets {
        let entry = format!("env-{}", key.to_lowercase().replace('_', "-"));
        agent_env.push(json!({
            "name": key,
            "valueFrom": { "secretKeyRef": { "name": name, "key": entry } }
        }));
        data.insert(entry, json!(value));
    }
    let mut agent = json!({
        "name": "job",
        "image": spec.image,
        "workingDir": "/workspace",
        "env": agent_env,
        "securityContext": hardened(UID),
        "resources": resources,
        "volumeMounts": agent_mounts,
    });
    if !spec.command.is_empty() {
        agent["command"] = json!(spec.command);
    }
    let mut pod_spec = json!({
        "restartPolicy": "Never",
        "automountServiceAccountToken": false,
        "enableServiceLinks": false,
        "activeDeadlineSeconds": spec.timeout_secs,
        "terminationGracePeriodSeconds": if has_outputs { crate::artifacts::COLLECT_GRACE_SECS } else { 5 },
        "securityContext": { "runAsNonRoot": true, "fsGroup": UID },
        "initContainers": init,
        "containers": [agent],
        "volumes": volumes,
    });
    if let Some(pull) = pull_secret {
        pod_spec["imagePullSecrets"] = json!([{ "name": pull }]);
    }
    let secret = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": name, "labels": labels(job, runner_id) },
        "type": "Opaque",
        "stringData": data,
    });
    // Nothing comes in. Out: names, then the listed destinations.
    let mut egress = vec![json!({
        "to": [{
            "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "kube-system" } },
            "podSelector": { "matchLabels": { "k8s-app": "kube-dns" } }
        }],
        "ports": [{ "protocol": "UDP", "port": 53 }, { "protocol": "TCP", "port": 53 }]
    })];
    egress.extend(input.egress.iter().map(|e| {
        json!({
            "to": [{ "ipBlock": { "cidr": e.cidr } }],
            "ports": [{ "protocol": "TCP", "port": e.port }]
        })
    }));
    let policy = json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": name, "labels": labels(job, runner_id) },
        "spec": {
            "podSelector": { "matchLabels": { "pnex.io/job": job } },
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [],
            "egress": egress,
        }
    });
    let pod = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": name, "labels": labels(job, runner_id) },
        "spec": pod_spec,
    });
    (secret, policy, pod)
}

/// Ranges that are never the public internet: the cluster, its nodes, the cloud metadata.
const PRIVATE_V4: [&str; 6] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "127.0.0.0/8",
];
const PRIVATE_V6: [&str; 3] = ["fc00::/7", "fe80::/10", "::1/128"];

/// The rule of a build, scan or probe pod (SEC-E2): nothing comes in; out, names, the public
/// internet (registries, package mirrors) and only the in-cluster destinations the operator
/// lists in `extra`, as a CIDR or `ns:<namespace>` (a cache in the cluster, Q-E13).
pub fn job_policy(job: &str, runner_id: &str, extra: &[String]) -> Value {
    let mut egress = vec![
        json!({
            "to": [{
                "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "kube-system" } },
                "podSelector": { "matchLabels": { "k8s-app": "kube-dns" } }
            }],
            "ports": [{ "protocol": "UDP", "port": 53 }, { "protocol": "TCP", "port": 53 }]
        }),
        json!({ "to": [
            { "ipBlock": { "cidr": "0.0.0.0/0", "except": PRIVATE_V4 } },
            { "ipBlock": { "cidr": "::/0", "except": PRIVATE_V6 } }
        ] }),
    ];
    for entry in extra {
        let to = match entry.strip_prefix("ns:") {
            Some(ns) => json!({
                "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": ns } }
            }),
            None => json!({ "ipBlock": { "cidr": entry } }),
        };
        egress.push(json!({ "to": [to] }));
    }
    json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": format!("{POD_PREFIX}{job}"), "labels": labels(job, runner_id) },
        "spec": {
            "podSelector": { "matchLabels": { "pnex.io/job": job } },
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [],
            "egress": egress,
        }
    })
}

/// The pod of a job: no privilege, no capability, read-only root, no API token, a
/// deadline, bounded resources.
pub fn pod_manifest(
    job: &str,
    runner_id: &str,
    spec: &RunnerJobSpec,
    runtime_class: Option<&str>,
) -> Value {
    let mut container = json!({
        "name": "job",
        "image": spec.image,
        "imagePullPolicy": "IfNotPresent",
        "securityContext": {
            "runAsNonRoot": true,
            "runAsUser": UID,
            "runAsGroup": UID,
            "allowPrivilegeEscalation": false,
            "privileged": false,
            "readOnlyRootFilesystem": true,
            "capabilities": { "drop": ["ALL"] },
            "seccompProfile": { "type": "RuntimeDefault" }
        },
        "resources": {
            "requests": { "cpu": "100m", "memory": "128Mi", "ephemeral-storage": "128Mi" },
            "limits": { "cpu": "1", "memory": "512Mi", "ephemeral-storage": "1Gi" }
        },
        "volumeMounts": [{ "name": "tmp", "mountPath": "/tmp" }]
    });
    if !spec.command.is_empty() {
        container["command"] = json!(spec.command);
    }
    let mut pod_spec = json!({
        "restartPolicy": "Never",
        "automountServiceAccountToken": false,
        "enableServiceLinks": false,
        "hostNetwork": false,
        "hostPID": false,
        "hostIPC": false,
        "activeDeadlineSeconds": spec.timeout_secs,
        "terminationGracePeriodSeconds": 5,
        "securityContext": { "runAsNonRoot": true, "fsGroup": UID },
        "containers": [container],
        "volumes": [{ "name": "tmp", "emptyDir": { "sizeLimit": "512Mi" } }]
    });
    if let Some(class) = runtime_class {
        pod_spec["runtimeClassName"] = json!(class);
    }
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": format!("{POD_PREFIX}{job}"),
            "labels": labels(job, runner_id)
        },
        "spec": pod_spec
    })
}

/// The digest the build pod wrote as its termination message: buildctl's metadata JSON
/// (`containerimage.digest`), or the digest alone.
pub fn reported_digest(pod: &Value) -> Option<String> {
    let message = pod["status"]["containerStatuses"][0]["state"]["terminated"]["message"]
        .as_str()?
        .trim();
    if message.starts_with('{') {
        let metadata: Value = serde_json::from_str(message).ok()?;
        return metadata["containerimage.digest"]
            .as_str()
            .map(str::to_string);
    }
    (!message.is_empty()).then(|| message.to_string())
}

/// Reads the state of a pod from its `status`.
pub fn workload_status(pod: &Value) -> WorkloadStatus {
    let status = &pod["status"];
    if status["reason"] == "DeadlineExceeded" {
        return WorkloadStatus::TimedOut;
    }
    let container = &status["containerStatuses"][0]["state"];
    let waiting = container["waiting"]["reason"].as_str().unwrap_or_default();
    if matches!(
        waiting,
        "ErrImagePull" | "ImagePullBackOff" | "InvalidImageName"
    ) {
        return WorkloadStatus::ImagePullFailed;
    }
    match status["phase"].as_str() {
        Some("Succeeded") => WorkloadStatus::Exited(0),
        Some("Failed") => WorkloadStatus::Exited(
            container["terminated"]["exitCode"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .filter(|c| *c != 0)
                .unwrap_or(1),
        ),
        _ => WorkloadStatus::Running,
    }
}

impl K8s {
    /// From inside a pod: the address comes from the environment, the rest from the
    /// mounted ServiceAccount.
    pub fn in_cluster(
        runtime_class: Option<String>,
        builds: bool,
        pull_secret: Option<String>,
        job_egress: Vec<String>,
        gate: Option<(String, String)>,
    ) -> Result<Self, String> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .map_err(|_| "not running in a Kubernetes pod".to_string())?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_string());
        let ca = std::fs::read(format!("{SA_DIR}/ca.crt")).map_err(|e| e.to_string())?;
        let namespace =
            std::fs::read_to_string(format!("{SA_DIR}/namespace")).map_err(|e| e.to_string())?;
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .add_root_certificate(reqwest::Certificate::from_pem(&ca).map_err(|e| e.to_string())?)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            http,
            base: format!("https://{host}:{port}"),
            namespace: namespace.trim().to_string(),
            runner_id: String::new(),
            runtime_class,
            builds,
            pull_secret,
            job_egress,
            gate,
        })
    }

    /// Kubernetes rotates this token: it is read for each call.
    fn bearer() -> Result<String, String> {
        std::fs::read_to_string(format!("{SA_DIR}/token"))
            .map(|t| t.trim().to_string())
            .map_err(|e| e.to_string())
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/namespaces/{}/{path}", self.base, self.namespace)
    }

    async fn get(&self, path: &str) -> Result<Option<Value>, String> {
        let res = self
            .http
            .get(self.url(path))
            .bearer_auth(Self::bearer()?)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => res
                .json()
                .await
                .map(Some)
                .map_err(|e| e.without_url().to_string()),
            s => Err(format!("GET {path}: {s}")),
        }
    }

    fn policies_url(&self) -> String {
        format!(
            "{}/apis/networking.k8s.io/v1/namespaces/{}/networkpolicies",
            self.base, self.namespace
        )
    }

    async fn create_policy(&self, body: &Value) -> Result<(), String> {
        let res = self
            .http
            .post(self.policies_url())
            .bearer_auth(Self::bearer()?)
            .json(body)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(format!("POST networkpolicies: {}", res.status()))
        }
    }

    async fn create(&self, path: &str, body: &Value) -> Result<(), String> {
        let res = self
            .http
            .post(self.url(path))
            .bearer_auth(Self::bearer()?)
            .json(body)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(format!("POST {path}: {}", res.status()))
        }
    }
}

impl Host for K8s {
    fn capabilities(&self) -> Vec<String> {
        let mut capabilities = capabilities_of("kubernetes", self.runtime_class.as_deref());
        capabilities.push(SANDBOX_CAPABILITY.to_string());
        if self.builds {
            capabilities.push(BUILD_CAPABILITY.to_string());
        }
        capabilities.push(SCAN_CAPABILITY.to_string());
        capabilities
    }

    fn set_runner_id(&mut self, id: String) {
        self.runner_id = id;
    }

    /// Kept in the namespace, because a pod restarts.
    async fn load_credentials(&self) -> Result<Option<(String, String)>, String> {
        let Some(secret) = self.get(&format!("secrets/{TOKEN_SECRET}")).await? else {
            return Ok(None);
        };
        let read = |key: &str| -> Result<String, String> {
            let raw = secret["data"][key].as_str().unwrap_or_default();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw)
                .map_err(|_| format!("{TOKEN_SECRET}: {key} is not base64"))?;
            String::from_utf8(bytes).map_err(|_| format!("{TOKEN_SECRET}: {key} is not text"))
        };
        Ok(Some((read("runner_id")?, read("token")?)))
    }

    async fn save_credentials(&self, runner_id: &str, token: &str) -> Result<(), String> {
        self.create(
            "secrets",
            &json!({
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": { "name": TOKEN_SECRET },
                "type": "Opaque",
                "stringData": { "runner_id": runner_id, "token": token }
            }),
        )
        .await
    }
}

impl Executor for K8s {
    async fn start(
        &self,
        job: &str,
        spec: &RunnerJobSpec,
        build: Option<&RunnerBuildInput>,
        scan: Option<&RunnerScanInput>,
        sandbox: Option<&RunnerSandboxInput>,
    ) -> Result<(), String> {
        if let Some(sandbox) = sandbox {
            let (secret, policy, pod) = sandbox_manifests(
                job,
                &self.runner_id,
                spec,
                sandbox,
                self.pull_secret.as_deref(),
            );
            // The rule first: not one packet leaves before it holds.
            self.create_policy(&policy).await?;
            self.create("secrets", &secret).await?;
            return self.create("pods", &pod).await;
        }
        // The rule first: not one packet leaves before it holds.
        self.create_policy(&job_policy(job, &self.runner_id, &self.job_egress))
            .await?;
        if is_scan_command(&spec.command) {
            let (secret, pod) = scan_manifests(
                job,
                &self.runner_id,
                spec,
                scan,
                self.pull_secret.as_deref(),
            );
            if let Some(secret) = secret {
                self.create("secrets", &secret).await?;
            }
            let mut pod = pod;
            if let Some((image, probe)) = &self.gate {
                with_gate(&mut pod, image, probe);
            }
            return self.create("pods", &pod).await;
        }
        if let Some(build) = build {
            let (secret, context, pod) = build_manifests(
                job,
                &self.runner_id,
                spec,
                build,
                self.pull_secret.as_deref(),
            );
            self.create("secrets", &secret).await?;
            self.create("configmaps", &context).await?;
            let mut pod = pod;
            if let Some((image, probe)) = &self.gate {
                with_gate(&mut pod, image, probe);
            }
            return self.create("pods", &pod).await;
        }
        let mut manifest = pod_manifest(job, &self.runner_id, spec, self.runtime_class.as_deref());
        if let Some((image, probe)) = &self.gate {
            with_gate(&mut manifest, image, probe);
        }
        self.create("pods", &manifest).await
    }

    async fn image_digest(&self, job: &str) -> Option<String> {
        let pod = self.get(&format!("pods/{POD_PREFIX}{job}")).await.ok()??;
        reported_digest(&pod)
    }

    async fn scan_output(&self, job: &str) -> Option<(ScanReport, String)> {
        let pod = self.get(&format!("pods/{POD_PREFIX}{job}")).await.ok()??;
        let command: Vec<String> =
            serde_json::from_value(pod["spec"]["containers"][0]["command"].clone()).ok()?;
        if !is_scan_command(&command) {
            return None;
        }
        let res = self
            .http
            .get(self.url(&format!(
                "pods/{POD_PREFIX}{job}/log?limitBytes={SCAN_LOG_MAX_BYTES}"
            )))
            .bearer_auth(Self::bearer().ok()?)
            .send()
            .await
            .ok()?;
        parse_scan_output(&res.error_for_status().ok()?.text().await.ok()?)
    }

    async fn logs(&self, job: &str) -> RunnerJobLogs {
        let Ok(Some(pod)) = self.get(&format!("pods/{POD_PREFIX}{job}")).await else {
            return RunnerJobLogs::default();
        };
        let mut out = RunnerJobLogs::default();
        for container in log_containers(&pod) {
            let res = self
                .http
                .get(self.url(&format!(
                    "pods/{POD_PREFIX}{job}/log?container={container}\
                     &tailLines={LOG_TAIL_LINES}&limitBytes={LOG_CONTAINER_MAX_BYTES}"
                )))
                .bearer_auth(Self::bearer().unwrap_or_default())
                .send()
                .await;
            // A container that never started has no log: a 400, skipped.
            let Some(text) = (match res {
                Ok(res) if res.status().is_success() => res.text().await.ok(),
                _ => None,
            }) else {
                continue;
            };
            out.containers.push(RunnerContainerLog {
                truncated: text.len() >= LOG_CONTAINER_MAX_BYTES
                    || text.lines().count() >= LOG_TAIL_LINES,
                container,
                text,
            });
        }
        out
    }

    async fn status(&self, job: &str) -> Result<WorkloadStatus, String> {
        Ok(self
            .get(&format!("pods/{POD_PREFIX}{job}"))
            .await?
            .map_or(WorkloadStatus::Gone, |pod| workload_status(&pod)))
    }

    async fn destroy(&self, job: &str) -> Result<(), String> {
        // The pod first: its egress rule goes only once nothing runs under it.
        let name = format!("{POD_PREFIX}{job}");
        let urls = [
            self.url(&format!("pods/{name}")),
            self.url(&format!("secrets/{name}")),
            self.url(&format!("configmaps/{name}")),
            format!("{}/{name}", self.policies_url()),
        ];
        for url in urls {
            let res = self
                .http
                .delete(url)
                .bearer_auth(Self::bearer()?)
                .send()
                .await
                .map_err(|e| e.without_url().to_string())?;
            match res.status() {
                StatusCode::NOT_FOUND => {}
                s if s.is_success() => {}
                s => return Err(format!("DELETE {name}: {s}")),
            }
        }
        Ok(())
    }

    /// Secrets too: a runner stopped between creating a Secret and its pod leaves no pod.
    async fn list(&self) -> Result<Vec<String>, String> {
        let mut jobs = std::collections::BTreeSet::new();
        for kind in ["pods", "secrets"] {
            let list = self
                .get(&format!(
                    "{kind}?labelSelector=pnex.io/runner%3D{}",
                    self.runner_id
                ))
                .await?
                .unwrap_or_default();
            for item in list["items"].as_array().into_iter().flatten() {
                if let Some(job) = item["metadata"]["labels"]["pnex.io/job"].as_str() {
                    jobs.insert(job.to_string());
                }
            }
        }
        Ok(jobs.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RunnerJobSpec {
        RunnerJobSpec {
            image: "x@sha256:abc".into(),
            command: vec!["true".into()],
            timeout_secs: 60,
        }
    }

    #[test]
    fn the_pod_is_hardened() {
        let pod = pod_manifest("j1", "r1", &spec(), Some("gvisor"));
        let s = &pod["spec"];
        assert_eq!(s["automountServiceAccountToken"], false);
        assert_eq!(s["hostNetwork"], false);
        assert_eq!(s["hostPID"], false);
        assert_eq!(s["restartPolicy"], "Never");
        assert_eq!(s["activeDeadlineSeconds"], 60);
        assert_eq!(s["runtimeClassName"], "gvisor");
        let c = &s["containers"][0];
        let sc = &c["securityContext"];
        assert_eq!(sc["runAsNonRoot"], true);
        assert_eq!(sc["allowPrivilegeEscalation"], false);
        assert_eq!(sc["privileged"], false);
        assert_eq!(sc["readOnlyRootFilesystem"], true);
        assert_eq!(sc["capabilities"]["drop"][0], "ALL");
        assert_eq!(sc["seccompProfile"]["type"], "RuntimeDefault");
        assert!(c["resources"]["limits"]["memory"].is_string());
        assert_eq!(pod["metadata"]["labels"]["pnex.io/runner"], "r1");
        assert_eq!(pod["metadata"]["labels"]["pnex.io/job"], "j1");
        assert_eq!(c["image"], "x@sha256:abc");
        assert_eq!(c["command"][0], "true");
    }

    #[test]
    fn without_a_command_the_image_entrypoint_runs() {
        let mut s = spec();
        s.command.clear();
        let pod = pod_manifest("j", "r", &s, None);
        assert!(pod["spec"]["containers"][0].get("command").is_none());
        assert!(pod["spec"].get("runtimeClassName").is_none());
    }

    fn build_input() -> RunnerBuildInput {
        use std::collections::BTreeMap;
        RunnerBuildInput {
            source: pnex_core::environments::EnvironmentSource {
                dockerfile: "FROM x".into(),
                files: BTreeMap::from([("scripts/setup.sh".to_string(), "echo ok".to_string())]),
                build_args: BTreeMap::from([("V".to_string(), "1; rm -rf /".to_string())]),
            },
            target: "r.example/team/app:v3".into(),
            registry_host: "r.example".into(),
            tls: true,
            username: "robot".into(),
            secret: "top-secret".into(),
            cache: None,
            resources: None,
        }
    }

    #[test]
    fn the_cache_credentials_stay_in_the_secret() {
        let mut b = build_input();
        b.cache = Some(pnex_core::runners::RunnerBuildCache {
            endpoint: "http://s3:9000".into(),
            region: "us-east-1".into(),
            bucket: "b".into(),
            prefix: "org_1/buildcache/e/".into(),
            access_key: "temp-access".into(),
            secret_key: "temp-secret".into(),
            session_token: "temp-token".into(),
        });
        let (secret, _, pod) = build_manifests("j", "r", &spec(), &b, None);
        for value in ["temp-access", "temp-secret", "temp-token"] {
            assert!(!pod.to_string().contains(value), "{value} in the pod spec");
            assert!(secret.to_string().contains(value));
        }
        let c = &pod["spec"]["containers"][0];
        // The size of the pod comes from the server, a malformed one falls back.
        let mut sized = b.clone();
        sized.resources = Some(pnex_core::runners::BuildResources {
            memory: "16Gi".into(),
            ephemeral_storage: "100Gi".into(),
        });
        let (_, _, big) = build_manifests("j", "r", &spec(), &sized, None);
        let limits = &big["spec"]["containers"][0]["resources"]["limits"];
        assert_eq!(
            (
                limits["memory"].as_str(),
                limits["ephemeral-storage"].as_str()
            ),
            (Some("16Gi"), Some("100Gi"))
        );
        sized.resources = Some(pnex_core::runners::BuildResources {
            memory: "16Gi; rm".into(),
            ephemeral_storage: "100Gi".into(),
        });
        let (_, _, odd) = build_manifests("j", "r", &spec(), &sized, None);
        assert_eq!(
            odd["spec"]["containers"][0]["resources"]["limits"]["memory"],
            "8Gi"
        );
        // The digest goes through the state volume, never a rename inside `/dev`.
        assert_eq!(c["command"][0], "sh");
        assert!(
            BUILD_SCRIPT.contains(BUILD_STATE) && BUILD_SCRIPT.contains("/dev/termination-log")
        );
        let args = c["args"].to_string();
        assert!(
            args.contains(
                "--import-cache=type=s3,region=us-east-1,bucket=b,prefix=org_1/buildcache/e/"
            ),
            "{args}"
        );
        assert!(args.contains("mode=max"), "{args}");
        assert!(args.contains("endpoint_url=http://s3:9000"), "{args}");
        assert_eq!(
            c["env"][3]["valueFrom"]["secretKeyRef"]["key"],
            "AWS_SESSION_TOKEN"
        );
        // The AWS keys never land in the mounted Docker config.
        let items = &pod["spec"]["volumes"][1]["secret"]["items"];
        assert_eq!(items[0]["key"], "config.json");
        assert_eq!(items.as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_build_pod_holds_no_secret_in_its_spec() {
        let (secret, context, pod) =
            build_manifests("j", "r", &spec(), &build_input(), Some("pull"));
        assert!(!pod.to_string().contains("top-secret"));
        assert!(!context.to_string().contains("top-secret"));
        let config: Value =
            serde_json::from_str(secret["stringData"]["config.json"].as_str().unwrap()).unwrap();
        let auth = config["auths"]["r.example"]["auth"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(auth)
            .unwrap();
        assert_eq!(decoded, b"robot:top-secret");

        // Every object is labelled, so that a stray is found.
        for object in [&secret, &context, &pod] {
            assert_eq!(object["metadata"]["labels"]["pnex.io/job"], "j");
        }
        let s = &pod["spec"];
        assert_eq!(s["automountServiceAccountToken"], false);
        assert_eq!(s["activeDeadlineSeconds"], 60);
        assert_eq!(s["imagePullSecrets"][0]["name"], "pull");
        let c = &s["containers"][0];
        assert_eq!(c["securityContext"]["privileged"], false);
        assert_eq!(c["securityContext"]["runAsUser"], 1000);
        let args: Vec<&str> = c["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert!(args.contains(&"--output=type=image,name=r.example/team/app:v3,push=true"));
        assert!(
            args.contains(&"build-arg:V=1; rm -rf /"),
            "one argument, no shell"
        );
        // A fixed script; the arguments reach buildctl as `"$@"`, never read by the shell.
        assert_eq!(c["command"], json!(["sh", "-c", BUILD_SCRIPT, "build"]));
        assert!(BUILD_SCRIPT.contains("build \"$@\""));

        // A file in a folder goes through `items`, its key stays a plain name.
        let items = s["volumes"][0]["configMap"]["items"].as_array().unwrap();
        assert!(items.contains(&json!({ "key": "file0", "path": "scripts/setup.sh" })));
        assert_eq!(context["data"]["file0"], "echo ok");
        assert_eq!(context["data"]["dockerfile"], "FROM x");
    }

    #[test]
    fn a_registry_without_tls_is_said_so() {
        let mut b = build_input();
        b.tls = false;
        let (_, _, pod) = build_manifests("j", "r", &spec(), &b, None);
        let args = pod["spec"]["containers"][0]["args"].to_string();
        assert!(args.contains("registry.insecure=true"), "{args}");
        assert!(pod["spec"].get("imagePullSecrets").is_none());
    }

    #[test]
    fn the_scan_pod_is_hardened_and_only_reads() {
        let s = RunnerJobSpec {
            image: "s@sha256:abc".into(),
            command: pnex_core::runners::scan_command("r.example/a@sha256:abc"),
            timeout_secs: 60,
        };
        let login = RunnerScanInput {
            registry_host: "r.example".into(),
            username: "reader".into(),
            secret: "read-secret".into(),
        };
        let (secret, pod) = scan_manifests("j", "r", &s, Some(&login), Some("pull"));
        assert!(!pod.to_string().contains("read-secret"));
        assert!(secret.unwrap().to_string().contains("auths"));
        let c = &pod["spec"]["containers"][0];
        assert_eq!(c["securityContext"]["allowPrivilegeEscalation"], false);
        assert_eq!(c["securityContext"]["readOnlyRootFilesystem"], true);
        assert_eq!(
            c["securityContext"]["seccompProfile"]["type"],
            "RuntimeDefault"
        );
        assert_eq!(pod["spec"]["imagePullSecrets"][0]["name"], "pull");
        assert_eq!(pod["spec"]["volumes"][0]["emptyDir"]["sizeLimit"], "5Gi");

        // A registry without a login: no secret at all.
        let (secret, pod) = scan_manifests("j", "r", &s, None, None);
        assert!(secret.is_none());
        assert!(pod["spec"]["containers"][0].get("env").is_none());
    }

    #[test]
    fn the_output_of_a_scan_is_read_past_the_lines_of_the_runtime() {
        let logs = r#"nvidia driver modules are not yet loaded, invoking runc directly
{
  "bomFormat": "CycloneDX",
  "specVersion": "1.6"
}

---pnex-scan-report---
{
  "SchemaVersion": 2,
  "Results": [{
    "Packages": [
      {"Name": "a", "Version": "1", "Identifier": {"PURL": "pkg:apk/a@1"}},
      {"Name": "b", "Version": "1", "Identifier": {"PURL": "pkg:apk/b@1"}}
    ],
    "Vulnerabilities": [
      {"VulnerabilityID": "CVE-1", "PkgName": "a", "InstalledVersion": "1", "FixedVersion": "2", "Severity": "CRITICAL", "PkgIdentifier": {"PURL": "pkg:apk/a@1"}},
      {"VulnerabilityID": "CVE-2", "PkgName": "b", "InstalledVersion": "1", "Severity": "WEIRD"}
    ]
  }]
}
"#;
        let (report, sbom) = parse_scan_output(logs).unwrap();
        assert!(sbom.starts_with('{') && sbom.ends_with('}'), "{sbom}");
        assert!(!sbom.contains("nvidia"));
        assert!(sbom.contains("CycloneDX"));
        assert_eq!(report.components.len(), 2);
        assert_eq!(report.components[1].purl, "pkg:apk/b@1");
        assert_eq!(report.findings[0].severity, "critical");
        assert_eq!(report.findings[0].fixed.as_deref(), Some("2"));
        assert_eq!(report.findings[0].purl.as_deref(), Some("pkg:apk/a@1"));
        assert_eq!(report.findings[1].severity, "unknown");
        assert!(report.is_valid());

        // Cut logs, or anything else than the two documents, read as no result.
        let good_sbom = "{\n\"bomFormat\": \"CycloneDX\"\n}\n";
        for bad in [
            String::new(),
            "not json".to_string(),
            format!("{good_sbom}{SCAN_REPORT_MARKER}\n{{\n\"cut\": "),
            format!("{good_sbom}{SCAN_REPORT_MARKER}\n{{\n\"other\": 1\n}}\n"),
            format!("{{\n\"bomFormat\": \"other\"\n}}\n{SCAN_REPORT_MARKER}\n{{\n\"SchemaVersion\": 2\n}}\n"),
            good_sbom.to_string(),
        ] {
            assert!(parse_scan_output(&bad).is_none(), "{bad}");
        }
        // A clean image has no finding, and that is a result.
        let clean = format!("{good_sbom}{SCAN_REPORT_MARKER}\n{{\n\"SchemaVersion\": 2\n}}\n");
        assert!(parse_scan_output(&clean).unwrap().0.findings.is_empty());
    }

    fn sandbox_input() -> RunnerSandboxInput {
        use pnex_core::runners::{SandboxEgress, SandboxRepo, SandboxService};
        use std::collections::BTreeMap;
        RunnerSandboxInput {
            init_image: "git@sha256:abc".into(),
            repos: vec![SandboxRepo {
                url: "https://forge.example/team/app.git".into(),
                git_ref: "main".into(),
                path: "app".into(),
                token: Some("clone-token-value".into()),
            }],
            services: vec![SandboxService {
                name: "valkey".into(),
                image: "valkey@sha256:abc".into(),
                env: BTreeMap::new(),
                ready_command: vec!["valkey-cli".into(), "ping".into()],
                data_paths: vec!["/data".into()],
                run_as_user: None,
            }],
            setup: vec!["sh".into(), "-c".into(), "npm ci".into()],
            env: BTreeMap::from([("LOG_LEVEL".to_string(), "info".to_string())]),
            secrets: BTreeMap::from([("API_TOKEN".to_string(), "run-secret-value".to_string())]),
            egress: vec![SandboxEgress {
                cidr: "203.0.113.0/24".into(),
                port: 443,
            }],
            artifacts: None,
        }
    }

    #[test]
    fn artifacts_move_in_their_own_containers_with_a_token_the_agent_never_sees() {
        use pnex_core::agent_runs::ArtifactForm;
        use pnex_core::runners::{SandboxArtifact, SandboxArtifacts};
        let mut input = sandbox_input();
        input.artifacts = Some(SandboxArtifacts {
            url: "https://pnex.example/api/v1/agent-transfer".into(),
            token: "pnart_artifact-token-value".into(),
            inputs: vec![SandboxArtifact {
                name: "logs".into(),
                sha256: Some("sha256:ab12".into()),
                form: Some(ArtifactForm::Tar),
            }],
            outputs: vec![SandboxArtifact {
                name: "report".into(),
                sha256: None,
                form: None,
            }],
            caches: vec![pnex_core::agent_runs::RunCache {
                name: "cargo".into(),
                path: "/tmp/.cargo".into(),
                key_files: vec!["app/Cargo.lock".into()],
            }],
        });
        let (secret, _, pod) = sandbox_manifests("j", "r", &spec(), &input, None);
        assert!(!pod.to_string().contains("artifact-token-value"));
        assert_eq!(
            secret["stringData"]["artifact-token"],
            "pnart_artifact-token-value"
        );
        let s = &pod["spec"];
        let init = s["initContainers"].as_array().unwrap();
        let names: Vec<&str> = init.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "egress-gate",
                "clone-0",
                "artifacts-in",
                "artifacts-out",
                "service-valkey",
                "setup"
            ]
        );
        assert_eq!(init[2]["env"][1]["value"], "logs,ab12,tar");
        assert_eq!(init[3]["restartPolicy"], "Always", "a native sidecar");
        assert_eq!(init[3]["env"][1]["value"], "report");
        assert_eq!(init[3]["volumeMounts"][0]["readOnly"], true);
        assert_eq!(
            s["terminationGracePeriodSeconds"],
            crate::artifacts::COLLECT_GRACE_SECS
        );
        // The token goes to the two transfer containers, never to setup or the agent.
        let has_token = |c: &Value| c.to_string().contains("artifact-token");
        assert!(has_token(&init[2]) && has_token(&init[3]));
        assert!(!has_token(&init[0]) && !has_token(&init[5]) && !has_token(&s["containers"][0]));
        let agent = &s["containers"][0];
        let mounts = agent["volumeMounts"].as_array().unwrap();
        assert!(mounts
            .iter()
            .any(|m| m["mountPath"] == ARTIFACTS_IN_DIR && m["readOnly"] == true));
        assert!(mounts
            .iter()
            .any(|m| m["mountPath"] == ARTIFACTS_OUT_DIR && m["readOnly"].is_null()));
        assert!(agent["env"].to_string().contains("PNEX_ARTIFACTS_OUT"));
        // A cache: the agent and setup write it, the fetch fills it, the collector reads it.
        assert!(mounts.iter().any(|m| m["mountPath"] == "/tmp/.cargo"));
        assert_eq!(init[2]["env"][2]["value"], "cargo,app/Cargo.lock");
        let at = |c: &Value, path: &str| {
            c["volumeMounts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["mountPath"] == path)
                .cloned()
        };
        assert_eq!(at(&init[2], "/caches/cargo").unwrap()["readOnly"], false);
        assert_eq!(at(&init[3], "/caches/cargo").unwrap()["readOnly"], true);
        assert_eq!(at(&init[2], "/workspace").unwrap()["readOnly"], true);
        assert!(
            at(&init[5], "/tmp/.cargo").is_some(),
            "setup fills the cache too"
        );

        // Nothing declared: no transfer container, no volume, the short grace.
        let (_, _, pod) = sandbox_manifests("j", "r", &spec(), &sandbox_input(), None);
        assert!(!pod.to_string().contains("artifacts"));
        assert_eq!(pod["spec"]["terminationGracePeriodSeconds"], 5);
    }

    #[test]
    fn a_job_pod_starts_behind_the_gate_of_the_runner_image() {
        let (_, _, mut build) = build_manifests("j", "r", &spec(), &build_input(), None);
        with_gate(&mut build, "reg/pnex-runner@sha256:abc", "10.1.2.3:7171");
        let init = build["spec"]["initContainers"].as_array().unwrap();
        assert_eq!(init[0]["name"], "egress-gate");
        assert_eq!(init[0]["command"][1], "egress-gate");
        assert!(init[0]["volumeMounts"].is_null(), "no volume");
        assert_eq!(
            init[0]["env"],
            json!([{ "name": "PNEX_EGRESS_PROBE", "value": "10.1.2.3:7171" }])
        );
        assert_eq!(init[0]["securityContext"]["readOnlyRootFilesystem"], true);
        // Before whatever init containers a pod already has.
        let mut pod = json!({ "spec": { "initContainers": [{ "name": "x" }] } });
        with_gate(&mut pod, "img", "10.1.2.3:7171");
        assert_eq!(pod["spec"]["initContainers"][1]["name"], "x");
    }

    #[test]
    fn the_journal_follows_the_containers_in_the_order_they_ran() {
        let (_, _, pod) = sandbox_manifests("j", "r", &spec(), &sandbox_input(), None);
        assert_eq!(
            log_containers(&pod),
            ["egress-gate", "clone-0", "service-valkey", "setup", "job"]
        );
        let mut scan = spec();
        scan.command = pnex_core::runners::scan_command("x");
        let (_, pod) = scan_manifests("j", "r", &scan, None, None);
        assert!(
            log_containers(&pod).is_empty(),
            "a scan's output is its report"
        );
    }

    #[test]
    fn a_sandbox_gives_each_secret_to_one_container_only() {
        let (secret, policy, pod) = sandbox_manifests("j", "r", &spec(), &sandbox_input(), None);
        let text = pod.to_string();
        assert!(!text.contains("clone-token-value") && !text.contains("run-secret-value"));
        assert!(!policy.to_string().contains("secret"));
        assert_eq!(secret["stringData"]["clone-token-0"], "clone-token-value");
        assert_eq!(secret["stringData"]["env-api-token"], "run-secret-value");

        let s = &pod["spec"];
        let init = s["initContainers"].as_array().unwrap();
        let names: Vec<&str> = init.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["egress-gate", "clone-0", "service-valkey", "setup"],
            "the gate, then the order of D714"
        );
        let refs = |c: &Value| {
            c["env"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e["valueFrom"]["secretKeyRef"]["key"].as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert!(init[0]["env"].is_null(), "the gate holds nothing");
        assert_eq!(refs(&init[1]), ["clone-token-0"]);
        assert!(refs(&init[3]).is_empty(), "setup runs repository code");
        let agent = &s["containers"][0];
        assert_eq!(
            refs(agent),
            ["env-api-token"],
            "no clone token for the agent"
        );

        // Every container is hardened, and the pod has no API token.
        assert_eq!(s["automountServiceAccountToken"], false);
        for c in init.iter().chain([agent]) {
            assert_eq!(c["securityContext"]["allowPrivilegeEscalation"], false);
            assert_eq!(c["securityContext"]["readOnlyRootFilesystem"], true);
            assert_eq!(c["securityContext"]["capabilities"]["drop"][0], "ALL");
            assert!(c["resources"]["limits"]["memory"].is_string());
        }
        assert_eq!(init[2]["restartPolicy"], "Always");

        // Nothing in, and out only names and the list.
        let p = &policy["spec"];
        assert_eq!(p["podSelector"]["matchLabels"]["pnex.io/job"], "j");
        assert_eq!(p["ingress"], json!([]));
        assert_eq!(p["egress"].as_array().unwrap().len(), 2);
        assert_eq!(p["egress"][1]["to"][0]["ipBlock"]["cidr"], "203.0.113.0/24");
        // A value cannot leave the script: each one is its own argument.
        assert_eq!(init[1]["command"][5], "https://forge.example/team/app.git");
    }

    #[test]
    fn a_job_pod_reaches_the_internet_and_only_the_listed_cluster_destinations() {
        let p = job_policy(
            "j1",
            "r1",
            &["ns:pnex-cache".to_string(), "10.1.2.0/24".to_string()],
        );
        assert_eq!(p["spec"]["podSelector"]["matchLabels"]["pnex.io/job"], "j1");
        assert_eq!(p["spec"]["ingress"], json!([]));
        let egress = p["spec"]["egress"].as_array().unwrap();
        assert_eq!(egress.len(), 4);
        let public = &egress[1]["to"][0]["ipBlock"];
        assert_eq!(public["cidr"], "0.0.0.0/0");
        for private in ["10.0.0.0/8", "169.254.0.0/16", "192.168.0.0/16"] {
            assert!(public["except"]
                .as_array()
                .unwrap()
                .contains(&json!(private)));
        }
        assert_eq!(
            egress[2]["to"][0]["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"],
            "pnex-cache"
        );
        assert_eq!(egress[3]["to"][0]["ipBlock"]["cidr"], "10.1.2.0/24");
    }

    #[test]
    fn pod_states_are_read() {
        let st = |v: Value| workload_status(&json!({ "status": v }));
        assert_eq!(st(json!({"phase": "Pending"})), WorkloadStatus::Running);
        assert_eq!(st(json!({"phase": "Running"})), WorkloadStatus::Running);
        assert_eq!(st(json!({"phase": "Succeeded"})), WorkloadStatus::Exited(0));
        assert_eq!(
            st(json!({"phase": "Failed", "containerStatuses": [
                {"state": {"terminated": {"exitCode": 7}}}
            ]})),
            WorkloadStatus::Exited(7)
        );
        assert_eq!(
            st(json!({"phase": "Failed"})),
            WorkloadStatus::Exited(1),
            "a failed pod without a code never reads as a success"
        );
        assert_eq!(
            st(json!({"phase": "Failed", "reason": "DeadlineExceeded"})),
            WorkloadStatus::TimedOut
        );
        assert_eq!(
            st(json!({"phase": "Pending", "containerStatuses": [
                {"state": {"waiting": {"reason": "ImagePullBackOff"}}}
            ]})),
            WorkloadStatus::ImagePullFailed
        );
    }

    #[test]
    fn the_digest_is_read_from_buildctl_metadata_or_alone() {
        let pod = |message: &str| {
            json!({ "status": { "containerStatuses": [
                { "state": { "terminated": { "message": message } } }
            ] } })
        };
        let metadata = r#"{"containerimage.digest":"sha256:ab","image.name":"r/a:v1"}"#;
        assert_eq!(reported_digest(&pod(metadata)), Some("sha256:ab".into()));
        assert_eq!(
            reported_digest(&pod("sha256:cd\n")),
            Some("sha256:cd".into())
        );
        assert_eq!(reported_digest(&pod("{\"other\":1}")), None);
        assert_eq!(reported_digest(&pod("")), None);
    }
}
