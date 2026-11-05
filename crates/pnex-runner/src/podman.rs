//! Executor for a VM or a bare machine: one hardened container per job, through the
//! rootless `podman` CLI of the runner's own user. Same contract and same limits as
//! the Kubernetes pod (D706). Docker's CLI is compatible, but rootful Docker gives
//! its user the host, so it is an explicit choice (`PNEX_RUNNER_CONTAINER_CLI`).

use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pnex_core::runners::{RunnerBuildInput, RunnerJobSpec, RunnerSandboxInput, RunnerScanInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;

use crate::{capabilities_of, Executor, Host, WorkloadStatus};

const NAME_PREFIX: &str = "pnex-job-";
const UID: u32 = 65532;
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    InProgress,
    PullFailed,
    Failed,
}

pub struct Podman {
    cli: String,
    runner_id: String,
    runtime: Option<String>,
    credentials: PathBuf,
    /// `run` pulls the image and can last minutes: it must not hold up heartbeats.
    starts: Arc<Mutex<HashMap<String, Start>>>,
}

#[derive(Serialize, Deserialize)]
struct Credentials {
    runner_id: String,
    token: String,
}

/// The container of a job: unprivileged, no capability, read-only root, bounded.
pub fn run_args(
    job: &str,
    runner_id: &str,
    spec: &RunnerJobSpec,
    runtime: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--detach".into(),
        format!("--name={NAME_PREFIX}{job}"),
        format!("--label=pnex.io/runner={runner_id}"),
        format!("--label=pnex.io/job={job}"),
        format!("--user={UID}:{UID}"),
        "--read-only".into(),
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(),
        "--memory=512m".into(),
        "--cpus=1".into(),
        "--pids-limit=512".into(),
        "--tmpfs=/tmp:rw,size=512m".into(),
        "--pull=missing".into(),
    ];
    if let Some(runtime) = runtime {
        args.push(format!("--runtime={runtime}"));
    }
    // As in a pod, `command` replaces the entrypoint, not only its arguments.
    if let Some(first) = spec.command.first() {
        args.push(format!("--entrypoint={first}"));
    }
    args.push(spec.image.clone());
    args.extend(spec.command.iter().skip(1).cloned());
    args
}

/// Reads `inspect --format '{{json .State}}'`.
pub fn state_status(state: &Value) -> WorkloadStatus {
    match state["Status"].as_str() {
        Some("exited") | Some("stopped") => WorkloadStatus::Exited(
            state["ExitCode"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(1),
        ),
        Some("dead") => WorkloadStatus::Exited(1),
        _ => WorkloadStatus::Running,
    }
}

fn start_failure(stderr: &str) -> Start {
    let e = stderr.to_lowercase();
    if ["pull", "manifest", "unauthorized", "not found"]
        .iter()
        .any(|w| e.contains(w))
    {
        Start::PullFailed
    } else {
        Start::Failed
    }
}

impl Podman {
    pub fn new(cli: String, runtime: Option<String>, credentials: PathBuf) -> Self {
        Self {
            cli,
            runner_id: String::new(),
            runtime,
            credentials,
            starts: Arc::default(),
        }
    }

    async fn call(&self, args: &[&str]) -> Result<std::process::Output, String> {
        tokio::time::timeout(CALL_TIMEOUT, Command::new(&self.cli).args(args).output())
            .await
            .map_err(|_| format!("{} {} timed out", self.cli, args[0]))?
            .map_err(|e| format!("{}: {e}", self.cli))
    }
}

impl Executor for Podman {
    async fn start(
        &self,
        job: &str,
        spec: &RunnerJobSpec,
        build: Option<&RunnerBuildInput>,
        _scan: Option<&RunnerScanInput>,
        sandbox: Option<&RunnerSandboxInput>,
    ) -> Result<(), String> {
        // shortcut: no sandbox on a VM yet, and refusing beats running an agent without its
        // clone step, its services and its egress rule; to do with Q-E7.
        if sandbox.is_some() {
            return Err("this runner does not run sandboxes".to_string());
        }
        // shortcut: this runner never announces `builder:buildkit`, so no build reaches
        // it; add rootless BuildKit under Podman when a VM has to build.
        if build.is_some() {
            return Err("this runner does not build images".to_string());
        }
        let args = run_args(job, &self.runner_id, spec, self.runtime.as_deref());
        let starts = self.starts.clone();
        let cli = self.cli.clone();
        let job = job.to_string();
        starts
            .lock()
            .expect("starts lock")
            .insert(job.clone(), Start::InProgress);
        tokio::spawn(async move {
            let done = Command::new(&cli).args(&args).output().await;
            let result = match done {
                Ok(o) if o.status.success() => None,
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    tracing::warn!(job, "{cli} run failed: {}", stderr.trim());
                    Some(start_failure(&stderr))
                }
                Err(e) => {
                    tracing::warn!(job, "{cli} run failed: {e}");
                    Some(Start::Failed)
                }
            };
            let mut starts = starts.lock().expect("starts lock");
            // Absent: destroyed meanwhile; what `run` created is then a stray.
            if let Some(start) = starts.get_mut(&job) {
                match result {
                    Some(failure) => *start = failure,
                    None => {
                        starts.remove(&job);
                    }
                }
            }
        });
        Ok(())
    }

    async fn status(&self, job: &str) -> Result<WorkloadStatus, String> {
        let start = self.starts.lock().expect("starts lock").get(job).copied();
        match start {
            Some(Start::InProgress) => return Ok(WorkloadStatus::Running),
            Some(Start::PullFailed) => return Ok(WorkloadStatus::ImagePullFailed),
            Some(Start::Failed) => return Ok(WorkloadStatus::Gone),
            None => {}
        }
        let name = format!("{NAME_PREFIX}{job}");
        let out = self
            .call(&["inspect", "--format", "{{json .State}}", &name])
            .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
            return if stderr.contains("no such") {
                Ok(WorkloadStatus::Gone)
            } else {
                Err(format!("inspect: {}", stderr.trim()))
            };
        }
        let state: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        Ok(state_status(&state))
    }

    async fn destroy(&self, job: &str) -> Result<(), String> {
        self.starts.lock().expect("starts lock").remove(job);
        let name = format!("{NAME_PREFIX}{job}");
        let out = self.call(&["rm", "--force", "--volumes", &name]).await?;
        let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
        if out.status.success() || stderr.contains("no such") {
            Ok(())
        } else {
            Err(format!("rm: {}", stderr.trim()))
        }
    }

    async fn list(&self) -> Result<Vec<String>, String> {
        let filter = format!("label=pnex.io/runner={}", self.runner_id);
        let out = self
            .call(&["ps", "--all", "--filter", &filter, "--format", "{{.Names}}"])
            .await?;
        if !out.status.success() {
            return Err(format!(
                "ps: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let names = String::from_utf8_lossy(&out.stdout);
        let starting = self.starts.lock().expect("starts lock");
        let mut jobs: Vec<String> = names
            .lines()
            .filter_map(|n| n.trim().strip_prefix(NAME_PREFIX))
            .map(str::to_string)
            .collect();
        jobs.extend(starting.keys().cloned());
        jobs.sort();
        jobs.dedup();
        Ok(jobs)
    }
}

impl Host for Podman {
    fn capabilities(&self) -> Vec<String> {
        capabilities_of("vm", self.runtime.as_deref())
    }

    fn set_runner_id(&mut self, id: String) {
        self.runner_id = id;
    }

    async fn load_credentials(&self) -> Result<Option<(String, String)>, String> {
        match std::fs::read(&self.credentials) {
            Ok(raw) => {
                let c: Credentials = serde_json::from_slice(&raw)
                    .map_err(|_| format!("{} is not valid", self.credentials.display()))?;
                Ok(Some((c.runner_id, c.token)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", self.credentials.display())),
        }
    }

    /// Readable by the runner's user only.
    async fn save_credentials(&self, runner_id: &str, token: &str) -> Result<(), String> {
        use std::io::Write;
        if let Some(dir) = self.credentials.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let body = serde_json::to_vec(&Credentials {
            runner_id: runner_id.to_string(),
            token: token.to_string(),
        })
        .map_err(|e| e.to_string())?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.credentials)
            .and_then(|mut f| f.write_all(&body))
            .map_err(|e| format!("{}: {e}", self.credentials.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(command: &[&str]) -> RunnerJobSpec {
        RunnerJobSpec {
            image: "x@sha256:abc".into(),
            command: command.iter().map(|s| s.to_string()).collect(),
            timeout_secs: 60,
        }
    }

    #[test]
    fn the_container_is_hardened_like_the_pod() {
        let args = run_args("j1", "r1", &spec(&[]), None);
        for wanted in [
            "--name=pnex-job-j1",
            "--label=pnex.io/runner=r1",
            "--label=pnex.io/job=j1",
            "--user=65532:65532",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--memory=512m",
            "--pids-limit=512",
        ] {
            assert!(args.iter().any(|a| a == wanted), "{wanted} missing");
        }
        assert!(!args.iter().any(|a| a.contains("privileged")));
        assert!(!args.iter().any(|a| a.contains(".sock")));
        assert_eq!(args.last().unwrap(), "x@sha256:abc");
    }

    #[test]
    fn the_command_replaces_the_entrypoint_and_stays_after_the_image() {
        let args = run_args(
            "j",
            "r",
            &spec(&["sh", "-c", "--privileged"]),
            Some("runsc"),
        );
        assert!(args.contains(&"--entrypoint=sh".to_string()));
        assert!(args.contains(&"--runtime=runsc".to_string()));
        let image = args.iter().position(|a| a == "x@sha256:abc").unwrap();
        assert_eq!(&args[image + 1..], ["-c", "--privileged"]);
    }

    #[test]
    fn container_states_are_read() {
        assert_eq!(
            state_status(&json!({"Status": "running"})),
            WorkloadStatus::Running
        );
        assert_eq!(
            state_status(&json!({"Status": "created"})),
            WorkloadStatus::Running
        );
        assert_eq!(
            state_status(&json!({"Status": "exited", "ExitCode": 0})),
            WorkloadStatus::Exited(0)
        );
        assert_eq!(
            state_status(&json!({"Status": "exited", "ExitCode": 7})),
            WorkloadStatus::Exited(7)
        );
        assert_eq!(
            state_status(&json!({"Status": "exited"})),
            WorkloadStatus::Exited(1),
            "an exit without a code never reads as a success"
        );
    }

    #[test]
    fn pull_failures_are_told_apart() {
        assert!(start_failure("Error: initializing source: manifest unknown") == Start::PullFailed);
        assert!(start_failure("Error: crun: mount failed") == Start::Failed);
    }

    #[tokio::test]
    async fn credentials_stay_private_and_survive_a_restart() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pnex-runner-{}", std::process::id()));
        let path = dir.join("nested/credentials.json");
        let host = Podman::new("podman".into(), None, path.clone());
        assert_eq!(host.load_credentials().await.unwrap(), None);
        host.save_credentials("r1", "t1").await.unwrap();
        assert_eq!(
            host.load_credentials().await.unwrap(),
            Some(("r1".to_string(), "t1".to_string()))
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(
            host.save_credentials("r2", "t2").await.is_err(),
            "never overwritten"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
