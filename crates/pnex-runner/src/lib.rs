//! Runner of the worker fabric (worker-fabric.md section 14): polls out, holds jobs
//! by lease, and keeps one workload per job. All state lives on the server: a runner
//! that restarts destroys what it no longer knows, and the reaper hands its jobs to
//! another one.

#![allow(async_fn_in_trait)] // internal traits, used through generics on one runtime

pub mod api;
pub mod artifacts;
pub mod firecracker;
pub mod gate;
pub mod k8s;
pub mod oci;
pub mod podman;

use std::collections::HashMap;
use std::time::Duration;

use pnex_core::environments::ScanReport;
use pnex_core::runners::{
    mask_secrets, spec_error, RunnerBuildInput, RunnerClaimedJob, RunnerJobLogs, RunnerJobOutcome,
    RunnerJobSpec, RunnerSandboxInput, RunnerScanInput,
};
use tokio::time::Instant;

pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum ApiError {
    /// The server says this runner does not hold the job any more.
    NotHeld,
    Other(String),
}

/// The server, seen from a runner.
pub trait Api {
    async fn claim(&self) -> Result<Option<RunnerClaimedJob>, ApiError>;
    async fn heartbeat(&self, job: &str) -> Result<(), ApiError>;
    /// Sent before the job finishes: the server still sees it as held.
    async fn upload_sbom(&self, job: &str, document: String) -> Result<(), ApiError>;
    /// Sent before the job finishes, like the SBOM (D719).
    async fn upload_logs(&self, job: &str, logs: &RunnerJobLogs) -> Result<(), ApiError>;
    async fn finish(
        &self,
        job: &str,
        outcome: RunnerJobOutcome,
        exit_code: Option<i32>,
        image_digest: Option<String>,
        scan_report: Option<ScanReport>,
    ) -> Result<(), ApiError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadStatus {
    Running,
    Exited(i32),
    /// Stopped by its deadline.
    TimedOut,
    ImagePullFailed,
    /// Removed by someone else.
    Gone,
}

/// Where jobs run: Kubernetes first, a VM later (D706).
pub trait Executor {
    async fn start(
        &self,
        job: &str,
        spec: &RunnerJobSpec,
        build: Option<&RunnerBuildInput>,
        scan: Option<&RunnerScanInput>,
        sandbox: Option<&RunnerSandboxInput>,
    ) -> Result<(), String>;
    async fn status(&self, job: &str) -> Result<WorkloadStatus, String>;
    /// The digest a finished build reported, read before the workload is destroyed.
    async fn image_digest(&self, _job: &str) -> Option<String> {
        None
    }
    /// What a finished scan reported and its SBOM, read before the workload is destroyed.
    async fn scan_output(&self, _job: &str) -> Option<(ScanReport, String)> {
        None
    }
    /// The last lines of each container, read before the workload is destroyed (D719).
    async fn logs(&self, _job: &str) -> RunnerJobLogs {
        RunnerJobLogs::default()
    }
    /// Idempotent: destroying what is already gone succeeds.
    async fn destroy(&self, job: &str) -> Result<(), String>;
    /// Jobs of every workload this runner owns.
    async fn list(&self) -> Result<Vec<String>, String>;
}

/// What differs between platforms outside of running a job: what the runner
/// announces, and where it keeps its token (a join token works once).
pub trait Host: Executor {
    fn capabilities(&self) -> Vec<String>;
    fn set_runner_id(&mut self, id: String);
    /// `(runner id, token)`.
    async fn load_credentials(&self) -> Result<Option<(String, String)>, String>;
    async fn save_credentials(&self, runner_id: &str, token: &str) -> Result<(), String>;
}

/// `isolation:pod` is never announced on plain runc or crun: only with a hardened
/// runtime, whatever the platform (fabric section 14, D701).
pub fn capabilities_of(platform: &str, runtime: Option<&str>) -> Vec<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    let mut out = vec![
        "executor:sandbox".to_string(),
        format!("arch:{arch}"),
        format!("platform:{platform}"),
    ];
    match runtime {
        Some(runtime) => {
            out.push("isolation:pod".to_string());
            out.push(format!("runtime:{}", runtime.to_lowercase()));
        }
        None => out.push("isolation:none".to_string()),
    }
    out
}

struct Held {
    deadline: Instant,
    lease: Duration,
    lease_until: Instant,
    beat_at: Instant,
    ending: Option<Ending>,
    /// What the claim delivered, masked out of the logs before they leave.
    secrets: Vec<String>,
}

#[derive(Clone)]
struct Ending {
    outcome: RunnerJobOutcome,
    code: Option<i32>,
    digest: Option<String>,
    report: Option<ScanReport>,
    sbom: Option<String>,
    logs: Option<RunnerJobLogs>,
}

impl From<(RunnerJobOutcome, Option<i32>)> for Ending {
    fn from((outcome, code): (RunnerJobOutcome, Option<i32>)) -> Self {
        Self {
            outcome,
            code,
            digest: None,
            report: None,
            sbom: None,
            logs: None,
        }
    }
}

pub struct Runner<A, E> {
    api: A,
    exec: E,
    max_jobs: usize,
    held: HashMap<String, Held>,
}

impl<A: Api, E: Executor> Runner<A, E> {
    pub fn new(api: A, exec: E, max_jobs: usize) -> Self {
        Self {
            api,
            exec,
            max_jobs,
            held: HashMap::new(),
        }
    }

    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// One round: follow held jobs, remove strays, then take new work.
    pub async fn tick(&mut self) {
        let jobs: Vec<String> = self.held.keys().cloned().collect();
        for job in jobs {
            self.follow(&job).await;
        }
        self.remove_strays().await;
        self.take_work().await;
    }

    async fn follow(&mut self, job: &str) {
        let now = Instant::now();
        if self.held[job].ending.is_none() {
            let ending = match self.exec.status(job).await {
                Ok(WorkloadStatus::Running) if now >= self.held[job].deadline => {
                    Some((RunnerJobOutcome::Timeout, None))
                }
                Ok(WorkloadStatus::Running) => None,
                Ok(WorkloadStatus::Exited(0)) => Some((RunnerJobOutcome::Succeeded, Some(0))),
                Ok(WorkloadStatus::Exited(code)) => Some((RunnerJobOutcome::Failed, Some(code))),
                Ok(WorkloadStatus::TimedOut) => Some((RunnerJobOutcome::Timeout, None)),
                Ok(WorkloadStatus::ImagePullFailed) => Some((RunnerJobOutcome::ImagePull, None)),
                Ok(WorkloadStatus::Gone) => Some((RunnerJobOutcome::Internal, None)),
                Err(e) => {
                    tracing::warn!(job, "workload status failed: {e}");
                    None
                }
            };
            if let Some(ending) = ending {
                let mut ending = Ending::from(ending);
                if ending.outcome == RunnerJobOutcome::Succeeded {
                    ending.digest = self.exec.image_digest(job).await;
                    if let Some((report, sbom)) = self.exec.scan_output(job).await {
                        ending.report = Some(report);
                        ending.sbom = Some(sbom);
                    }
                }
                ending.logs = self.masked_logs(job).await;
                self.destroy(job).await;
                if let Some(h) = self.held.get_mut(job) {
                    h.ending = Some(ending);
                }
            }
        }
        if let Some(ending) = self.held[job].ending.clone() {
            if let Some(logs) = &ending.logs {
                // Best effort: logs are for debugging, they never hold the result back.
                match self.api.upload_logs(job, logs).await {
                    Ok(()) => {}
                    Err(ApiError::NotHeld) => {
                        self.held.remove(job);
                        return;
                    }
                    Err(ApiError::Other(e)) => tracing::warn!(job, "logs not delivered: {e}"),
                }
                if let Some(e) = self.held.get_mut(job).and_then(|h| h.ending.as_mut()) {
                    e.logs = None;
                }
            }
            if let Some(sbom) = ending.sbom.clone() {
                match self.api.upload_sbom(job, sbom).await {
                    Ok(()) => {
                        if let Some(Ending { sbom, .. }) =
                            self.held.get_mut(job).and_then(|h| h.ending.as_mut())
                        {
                            *sbom = None;
                        }
                    }
                    Err(ApiError::NotHeld) => {
                        self.held.remove(job);
                        return;
                    }
                    Err(ApiError::Other(e)) => {
                        tracing::warn!(job, "sbom not delivered: {e}");
                        if now >= self.held[job].lease_until {
                            self.held.remove(job);
                        }
                        return;
                    }
                }
            }
            match self
                .api
                .finish(
                    job,
                    ending.outcome,
                    ending.code,
                    ending.digest,
                    ending.report,
                )
                .await
            {
                Ok(()) | Err(ApiError::NotHeld) => {
                    self.held.remove(job);
                }
                Err(ApiError::Other(e)) => {
                    tracing::warn!(job, "finish not delivered: {e}");
                    // Past the lease the job belongs to the reaper.
                    if now >= self.held[job].lease_until {
                        self.held.remove(job);
                    }
                }
            }
            return;
        }
        if now < self.held[job].beat_at {
            return;
        }
        match self.api.heartbeat(job).await {
            Ok(()) => {
                if let Some(h) = self.held.get_mut(job) {
                    h.lease_until = now + h.lease;
                    h.beat_at = now + HEARTBEAT_EVERY;
                }
            }
            Err(ApiError::NotHeld) => {
                tracing::warn!(job, "job lost, destroying its workload");
                // A job a person stopped still takes its logs for a while (D759); one taken
                // over by another runner refuses them, and that is fine.
                if let Some(logs) = self.masked_logs(job).await {
                    if let Err(e) = self.api.upload_logs(job, &logs).await {
                        tracing::debug!(job, "logs of a lost job not taken: {e:?}");
                    }
                }
                self.destroy(job).await;
                self.held.remove(job);
            }
            Err(ApiError::Other(e)) => {
                tracing::warn!(job, "heartbeat failed: {e}");
                // Another runner may take it from here: never two workloads.
                if now >= self.held[job].lease_until {
                    self.destroy(job).await;
                    self.held.remove(job);
                }
            }
        }
    }

    async fn remove_strays(&self) {
        match self.exec.list().await {
            Ok(jobs) => {
                for job in jobs.iter().filter(|j| !self.held.contains_key(*j)) {
                    tracing::info!(job, "destroying a workload without a job");
                    self.destroy(job).await;
                }
            }
            Err(e) => tracing::warn!("workload list failed: {e}"),
        }
    }

    async fn take_work(&mut self) {
        while self.held.len() < self.max_jobs {
            let job = match self.api.claim().await {
                Ok(Some(job)) => job,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!("claim failed: {e:?}");
                    return;
                }
            };
            let now = Instant::now();
            let lease = Duration::from_secs(job.lease_secs.into());
            let mut held = Held {
                deadline: now + Duration::from_secs(job.spec.timeout_secs.into()),
                lease,
                lease_until: now + lease,
                beat_at: now + HEARTBEAT_EVERY,
                ending: None,
                secrets: job.secret_values(),
            };
            // The server checked it too; a runner does not trust what it runs.
            if let Some((field, _)) = spec_error(&job.spec) {
                tracing::warn!(job = job.id, field, "invalid job spec refused");
                held.ending = Some((RunnerJobOutcome::Internal, None).into());
            } else if let Err(e) = self
                .exec
                .start(
                    &job.id,
                    &job.spec,
                    job.build.as_ref(),
                    job.scan.as_ref(),
                    job.sandbox.as_ref(),
                )
                .await
            {
                tracing::warn!(job = job.id, "workload not started: {e}");
                held.ending = Some((RunnerJobOutcome::Internal, None).into());
                self.destroy(&job.id).await;
            }
            self.held.insert(job.id, held);
        }
    }

    /// The last lines of each container, every delivered secret masked; `None` when the
    /// workload left nothing to read.
    async fn masked_logs(&self, job: &str) -> Option<RunnerJobLogs> {
        let mut logs = self.exec.logs(job).await;
        if logs.containers.is_empty() {
            return None;
        }
        let secrets = self
            .held
            .get(job)
            .map(|h| h.secrets.as_slice())
            .unwrap_or(&[]);
        for c in &mut logs.containers {
            c.text = mask_secrets(&c.text, secrets);
        }
        Some(logs)
    }

    async fn destroy(&self, job: &str) {
        if let Err(e) = self.exec.destroy(job).await {
            tracing::warn!(job, "workload not destroyed, retried as a stray: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::runners::RunnerJobKind;
    use std::sync::Mutex;

    const IMAGE: &str = "x@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[derive(Default)]
    struct FakeApi {
        queue: Mutex<Vec<RunnerClaimedJob>>,
        finished: Mutex<Vec<(String, RunnerJobOutcome, Option<i32>)>>,
        beats: Mutex<u32>,
        lost: Mutex<bool>,
        down: Mutex<bool>,
        digest: Mutex<Option<String>>,
        sboms: Mutex<Vec<String>>,
        logs: Mutex<Vec<RunnerJobLogs>>,
    }

    impl Api for &FakeApi {
        async fn claim(&self) -> Result<Option<RunnerClaimedJob>, ApiError> {
            Ok(self.queue.lock().unwrap().pop())
        }
        async fn upload_sbom(&self, _: &str, document: String) -> Result<(), ApiError> {
            self.sboms.lock().unwrap().push(document);
            Ok(())
        }
        async fn upload_logs(&self, _: &str, logs: &RunnerJobLogs) -> Result<(), ApiError> {
            self.logs.lock().unwrap().push(logs.clone());
            Ok(())
        }
        async fn heartbeat(&self, _: &str) -> Result<(), ApiError> {
            if *self.down.lock().unwrap() {
                return Err(ApiError::Other("down".into()));
            }
            if *self.lost.lock().unwrap() {
                return Err(ApiError::NotHeld);
            }
            *self.beats.lock().unwrap() += 1;
            Ok(())
        }
        async fn finish(
            &self,
            job: &str,
            outcome: RunnerJobOutcome,
            code: Option<i32>,
            digest: Option<String>,
            _: Option<ScanReport>,
        ) -> Result<(), ApiError> {
            *self.digest.lock().unwrap() = digest;
            if *self.down.lock().unwrap() {
                return Err(ApiError::Other("down".into()));
            }
            self.finished
                .lock()
                .unwrap()
                .push((job.to_string(), outcome, code));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeExec {
        pods: Mutex<HashMap<String, WorkloadStatus>>,
        refuse: Mutex<bool>,
    }

    impl FakeExec {
        fn set(&self, job: &str, s: WorkloadStatus) {
            self.pods.lock().unwrap().insert(job.to_string(), s);
        }
        fn count(&self) -> usize {
            self.pods.lock().unwrap().len()
        }
    }

    impl Executor for &FakeExec {
        async fn start(
            &self,
            job: &str,
            _: &RunnerJobSpec,
            _: Option<&RunnerBuildInput>,
            _: Option<&RunnerScanInput>,
            _: Option<&RunnerSandboxInput>,
        ) -> Result<(), String> {
            if *self.refuse.lock().unwrap() {
                return Err("refused".into());
            }
            self.set(job, WorkloadStatus::Running);
            Ok(())
        }
        async fn status(&self, job: &str) -> Result<WorkloadStatus, String> {
            Ok(*self
                .pods
                .lock()
                .unwrap()
                .get(job)
                .unwrap_or(&WorkloadStatus::Gone))
        }
        async fn destroy(&self, job: &str) -> Result<(), String> {
            self.pods.lock().unwrap().remove(job);
            Ok(())
        }
        async fn list(&self) -> Result<Vec<String>, String> {
            Ok(self.pods.lock().unwrap().keys().cloned().collect())
        }
        async fn scan_output(&self, job: &str) -> Option<(ScanReport, String)> {
            (job == "scan").then(|| {
                (
                    ScanReport {
                        components: vec![],
                        findings: vec![],
                    },
                    "{}".to_string(),
                )
            })
        }
        async fn logs(&self, _: &str) -> RunnerJobLogs {
            RunnerJobLogs {
                containers: vec![pnex_core::runners::RunnerContainerLog {
                    container: "job".into(),
                    text: "token=very-secret-value\n".into(),
                    truncated: false,
                }],
            }
        }
        async fn image_digest(&self, job: &str) -> Option<String> {
            self.pods
                .lock()
                .unwrap()
                .contains_key(job)
                .then(|| "sha256:abc".to_string())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_sbom_goes_up_before_the_job_finishes() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("scan", IMAGE, 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        exec.set("scan", WorkloadStatus::Exited(0));
        r.tick().await;
        assert_eq!(api.sboms.lock().unwrap().len(), 1);
        assert_eq!(api.finished.lock().unwrap().len(), 1);
        assert_eq!(r.held(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn logs_go_up_masked_before_the_job_finishes() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        let mut j = job("j", IMAGE, 60);
        j.scan = Some(RunnerScanInput {
            registry_host: "r".into(),
            username: "u".into(),
            secret: "very-secret-value".into(),
        });
        queue(&api, vec![j]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        exec.set("j", WorkloadStatus::Exited(1));
        r.tick().await;
        let logs = api.logs.lock().unwrap();
        assert_eq!(logs.len(), 1, "a failed job has its logs too");
        assert_eq!(logs[0].containers[0].text, "token=[masked]\n");
        assert_eq!(api.finished.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_digest_is_read_before_the_pod_goes() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        exec.set("j", WorkloadStatus::Exited(0));
        r.tick().await;
        assert_eq!(*api.digest.lock().unwrap(), Some("sha256:abc".to_string()));
        assert_eq!(exec.count(), 0);
    }

    #[test]
    fn isolation_is_only_announced_with_a_hardened_runtime() {
        let plain = capabilities_of("vm", None);
        assert!(plain.contains(&"isolation:none".to_string()));
        assert!(plain.contains(&"platform:vm".to_string()));
        assert!(!plain.contains(&"isolation:pod".to_string()));
        let hard = capabilities_of("kubernetes", Some("gVisor"));
        assert!(hard.contains(&"isolation:pod".to_string()));
        assert!(hard.contains(&"runtime:gvisor".to_string()));
        assert!(pnex_core::runners::capabilities_are_valid(&hard));
    }

    fn job(id: &str, image: &str, timeout_secs: u32) -> RunnerClaimedJob {
        RunnerClaimedJob {
            id: id.to_string(),
            kind: RunnerJobKind::Probe,
            spec: RunnerJobSpec {
                image: image.to_string(),
                command: vec![],
                timeout_secs,
            },
            lease_secs: 120,
            build: None,
            scan: None,
            sandbox: None,
        }
    }

    fn queue(api: &FakeApi, jobs: Vec<RunnerClaimedJob>) {
        *api.queue.lock().unwrap() = jobs;
    }

    #[tokio::test(start_paused = true)]
    async fn a_job_runs_then_reports_and_leaves_nothing() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        assert_eq!(exec.count(), 1);
        exec.set("j", WorkloadStatus::Exited(0));
        r.tick().await;
        assert_eq!(exec.count(), 0, "pod destroyed");
        assert_eq!(r.held(), 0);
        assert_eq!(
            *api.finished.lock().unwrap(),
            vec![("j".to_string(), RunnerJobOutcome::Succeeded, Some(0))]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_exit_keeps_its_code() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        exec.set("j", WorkloadStatus::Exited(3));
        r.tick().await;
        assert_eq!(
            api.finished.lock().unwrap()[0],
            ("j".to_string(), RunnerJobOutcome::Failed, Some(3))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_runner_enforces_the_timeout_itself() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        tokio::time::advance(Duration::from_secs(61)).await;
        r.tick().await;
        assert_eq!(exec.count(), 0);
        assert_eq!(api.finished.lock().unwrap()[0].1, RunnerJobOutcome::Timeout);
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeats_are_paced() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 600)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        r.tick().await;
        assert_eq!(*api.beats.lock().unwrap(), 0, "not before the period");
        tokio::time::advance(HEARTBEAT_EVERY).await;
        r.tick().await;
        r.tick().await;
        assert_eq!(*api.beats.lock().unwrap(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_lost_job_loses_its_pod() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 600)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        *api.lost.lock().unwrap() = true;
        tokio::time::advance(HEARTBEAT_EVERY).await;
        r.tick().await;
        assert_eq!(exec.count(), 0);
        assert_eq!(r.held(), 0);
        assert!(api.finished.lock().unwrap().is_empty());
        assert_eq!(
            api.logs.lock().unwrap().len(),
            1,
            "a stopped job still sends what its pod wrote"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cut_off_past_its_lease_a_runner_stops_the_workload() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 3600)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        *api.down.lock().unwrap() = true;
        tokio::time::advance(HEARTBEAT_EVERY).await;
        r.tick().await;
        assert_eq!(exec.count(), 1, "within the lease it keeps going");
        tokio::time::advance(Duration::from_secs(120)).await;
        r.tick().await;
        assert_eq!(exec.count(), 0, "never two workloads for one job");
        assert_eq!(r.held(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn an_undelivered_result_is_retried() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", IMAGE, 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        exec.set("j", WorkloadStatus::Exited(0));
        *api.down.lock().unwrap() = true;
        r.tick().await;
        assert_eq!(r.held(), 1);
        *api.down.lock().unwrap() = false;
        r.tick().await;
        assert_eq!(r.held(), 0);
        assert_eq!(api.finished.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_restarted_runner_destroys_strays() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        exec.set("old", WorkloadStatus::Running);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        assert_eq!(exec.count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn capacity_is_respected() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(
            &api,
            vec![
                job("a", IMAGE, 60),
                job("b", IMAGE, 60),
                job("c", IMAGE, 60),
            ],
        );
        let mut r = Runner::new(&api, &exec, 2);
        r.tick().await;
        assert_eq!(exec.count(), 2);
        assert_eq!(api.queue.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn start_failures_and_unpinned_images_are_reported() {
        let (api, exec) = (FakeApi::default(), FakeExec::default());
        *exec.refuse.lock().unwrap() = true;
        queue(&api, vec![job("j", IMAGE, 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        r.tick().await;
        assert_eq!(
            api.finished.lock().unwrap()[0].1,
            RunnerJobOutcome::Internal
        );

        let (api, exec) = (FakeApi::default(), FakeExec::default());
        queue(&api, vec![job("j", "x:latest", 60)]);
        let mut r = Runner::new(&api, &exec, 1);
        r.tick().await;
        assert_eq!(exec.count(), 0, "never started");
        r.tick().await;
        assert_eq!(
            api.finished.lock().unwrap()[0].1,
            RunnerJobOutcome::Internal
        );
    }
}
