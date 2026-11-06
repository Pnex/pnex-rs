//! Firecracker executor (agent-environments.md D749, D762): each job in a micro-VM of its
//! own, booted from the image pinned by digest, destroyed with the job.
//!
//! A sandbox keeps the order of its pod (D714, D717) with no token ever inside the VM: the
//! runner clones the repositories and fetches the inputs and caches on the host, with the
//! transfer scripts of the pods; `setup` boots a first VM with neither secret nor input;
//! the agent boots a fresh disk of the image where only the workspace and the caches of
//! that first boot are carried over, so `setup` cannot touch what runs with the secrets.
//! At the end the runner reads the outputs and caches off the disk (`debugfs`, no mount,
//! no root) and sends them. Services are not run in a VM yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use pnex_core::agent_runs::{ARTIFACTS_IN_DIR, ARTIFACTS_OUT_DIR};
use pnex_core::runners::{
    RunnerBuildInput, RunnerContainerLog, RunnerJobLogs, RunnerJobSpec, RunnerSandboxInput,
    RunnerScanInput, LOG_CONTAINER_MAX_BYTES,
};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::{capabilities_of, Executor, Host, WorkloadStatus};

/// Line `/init` prints last, read back as the code of the job.
pub const EXIT_MARK: &str = "pnex-exit=";

/// Free room on the disk of a sandbox, as the volumes of its pod: sparse, so only what the
/// agent writes takes space on the host.
const SANDBOX_HEADROOM: u64 = 16 << 30;
const PLAIN_HEADROOM: u64 = 256 << 20;

pub struct Firecracker {
    /// Images unpacked by digest, and one folder per job.
    pub base: PathBuf,
    pub binary: PathBuf,
    pub kernel: PathBuf,
    pub vcpus: u32,
    pub mem_mib: u32,
    /// Registries reached over plain HTTP: on the runner's own network only.
    pub insecure: Vec<String>,
    pub credentials: PathBuf,
    /// Resolver given to sandbox VMs; `None`: no network, plain jobs only.
    pub dns: Option<String>,
    runner_id: String,
    jobs: Mutex<HashMap<String, Held>>,
    /// The /30 each running VM holds (`vm_net::slot`).
    slots: Mutex<HashMap<String, u16>>,
}

/// A job in progress: its task (prepare, boot, collect) and what it was given.
struct Held {
    task: JoinHandle<()>,
    sandbox: Option<RunnerSandboxInput>,
}

/// What the task of a job needs of its runner.
#[derive(Clone)]
struct Machine {
    base: PathBuf,
    binary: PathBuf,
    kernel: PathBuf,
    vcpus: u32,
    mem_mib: u32,
    insecure: Vec<String>,
    dns: Option<String>,
}

/// `'…'` with each `'` closed, escaped and reopened: no argument is read by the shell.
fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// PID 1 of the VM: mounts what a job expects, runs it, reports its code, powers off. Without
/// `reboot`, the end of PID 1 panics the kernel, and `panic=1 reboot=k` stops the VM at once.
pub fn init_script(command: &[String], workdir: &str) -> String {
    let job: Vec<String> = command.iter().map(|a| quote(a)).collect();
    format!(
        "#!/bin/sh\n\
         mount -t proc proc /proc\n\
         mount -t sysfs sys /sys\n\
         mount -t devtmpfs dev /dev 2>/dev/null\n\
         mount -t tmpfs tmp /tmp\n\
         [ -f /pnex/env ] && . /pnex/env\n\
         cd {}\n\
         {}\n\
         code=$?\n\
         sync\n\
         echo \"{EXIT_MARK}$code\"\n\
         reboot -f 2>/dev/null\n\
         exit 0\n",
        quote(workdir),
        job.join(" ")
    )
}

/// `export K='v'` lines of the variables of a boot, read by `/init` from a root-only file. A
/// name that is not one is dropped: it would be read by the shell.
pub fn env_file(vars: &[(String, String)]) -> String {
    let is_name = |k: &str| {
        k.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    vars.iter()
        .filter(|(k, _)| is_name(k))
        .map(|(k, v)| format!("export {k}={}\n", quote(v)))
        .collect()
}

/// Variables of one boot: `setup` gets the plain ones only, the agent its secrets and where
/// its files are, as in a pod.
pub fn boot_env(s: &RunnerSandboxInput, agent: bool) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = s.env.clone().into_iter().collect();
    vars.push(("HOME".into(), "/tmp".into()));
    if agent {
        vars.extend(s.secrets.clone());
        if s.artifacts.is_some() {
            vars.push(("PNEX_ARTIFACTS_IN".into(), ARTIFACTS_IN_DIR.into()));
            vars.push(("PNEX_ARTIFACTS_OUT".into(), ARTIFACTS_OUT_DIR.into()));
        }
    }
    vars
}

/// The code the console reports; a VM that died without saying reads as a failure.
pub fn exit_code_of(console: &str) -> i32 {
    console
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix(EXIT_MARK))
        .and_then(|c| c.trim().parse().ok())
        .unwrap_or(1)
}

async fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// `root/<path>` of an absolute path of the guest; a path that climbs out is refused.
fn inside(root: &Path, path: &str) -> Result<PathBuf, String> {
    crate::oci::safe_join(root, Path::new(path.trim_start_matches('/')))
}

/// Copies `src` to `dest` without a byte copied: hard links, `dest` replaced.
async fn link_tree(src: &Path, dest: &Path) -> Result<(), String> {
    remove(dest);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if src.exists() {
        run(Command::new("cp").arg("-al").arg(src).arg(dest)).await
    } else {
        std::fs::create_dir_all(dest).map_err(|e| e.to_string())
    }
}

fn remove(path: &Path) {
    let _ = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => Ok(()),
    };
}

/// The folder `path` of the guest's disk into `dest` (replaced; empty when the guest left
/// nothing there). Links come out as links, never followed; every file is made readable to
/// the runner, which sends or repacks them.
pub async fn extract(disk: &Path, path: &str, dest: &Path) -> Result<(), String> {
    if path.contains(['"', '\n', '\\']) || !path.starts_with('/') {
        return Err(format!("{path}: not a path of the guest"));
    }
    let stage = dest.with_extension("extract");
    remove(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    run(Command::new("debugfs")
        .arg("-R")
        .arg(format!("rdump \"{path}\" \"{}\"", stage.display()))
        .arg(disk))
    .await?;
    remove(dest);
    let name = Path::new(path).file_name().ok_or("no folder name")?;
    let got = stage.join(name);
    if got.is_dir() {
        let mut stack = vec![got.clone()];
        while let Some(d) = stack.pop() {
            crate::oci::readable_by_owner(&d);
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                crate::oci::readable_by_owner(&p);
                if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
                    stack.push(p);
                }
            }
        }
        std::fs::rename(&got, dest).map_err(|e| e.to_string())?;
    } else {
        std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    }
    remove(&stage);
    Ok(())
}

/// A line of the transfer journal of a job, read back as its `transfer` container.
fn note(dir: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("transfer.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// A shell script of the pods run on the host, its output in the transfer journal.
async fn host_script(
    dir: &Path,
    script: &str,
    args: &[&str],
    env: &[(&str, String)],
) -> Result<(), String> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("transfer.log"))
        .map_err(|e| e.to_string())?;
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg("pnex")
        .args(args)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .status()
        .await
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("transfer step failed ({status})"))
    }
}

/// Environment of the transfer scripts: the folders of this job stand for the volumes.
fn transfer_env(
    dir: &Path,
    a: &pnex_core::runners::SandboxArtifacts,
) -> Vec<(&'static str, String)> {
    vec![
        (
            "PNEX_ARTIFACTS_ROOT",
            dir.join("artifacts").display().to_string(),
        ),
        (
            "PNEX_WORKSPACE",
            dir.join("workspace").display().to_string(),
        ),
        ("PNEX_CACHES_ROOT", dir.join("caches").display().to_string()),
        ("PNEX_ARTIFACTS_URL", a.url.clone()),
        ("PNEX_CACHES_LIST", crate::artifacts::caches_list(a)),
        ("PNEX_ARTIFACT_TOKEN", a.token.clone()),
    ]
}

impl Machine {
    /// The image unpacked once per digest; a partial pull never counts as one, and two jobs
    /// pulling the same image at once each unpack their own.
    async fn image_root(&self, job: &str, image: &str) -> Result<PathBuf, String> {
        let (host, repo, digest) =
            crate::oci::parse_reference(image).ok_or("image is not pinned by digest")?;
        let root = self
            .base
            .join("images")
            .join(digest.trim_start_matches("sha256:"));
        if root.exists() {
            return Ok(root);
        }
        let partial = root.with_extension(format!("partial-{job}"));
        remove(&partial);
        std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
        let mut registry =
            crate::oci::Registry::new(&host, &repo, self.insecure.contains(&host), None);
        for layer in registry.layers(&digest).await? {
            let bytes = registry.layer(&layer).await?;
            crate::oci::apply_layer(&partial, &bytes)?;
        }
        if std::fs::rename(&partial, &root).is_err() && root.exists() {
            remove(&partial);
        }
        Ok(root)
    }

    /// The disk of one boot: the image, the command, and the folders of the job placed in.
    async fn disk(
        &self,
        dir: &Path,
        image: &Path,
        command: &[String],
        boot: Option<(&RunnerSandboxInput, bool)>,
        name: &str,
    ) -> Result<PathBuf, String> {
        // Hard links: the job's root costs no copy; what is written is unlinked first.
        let root = dir.join("root");
        remove(&root);
        run(Command::new("cp").arg("-al").arg(image).arg(&root)).await?;
        let pnex = root.join("pnex");
        remove(&pnex);
        std::fs::create_dir_all(&pnex).map_err(|e| e.to_string())?;
        let workdir = if boot.is_some() { "/workspace" } else { "/" };
        let init = pnex.join("init");
        std::fs::write(&init, init_script(command, workdir)).map_err(|e| e.to_string())?;
        set_mode(&init, 0o755)?;
        let mut headroom = PLAIN_HEADROOM;
        if let Some((s, agent)) = boot {
            headroom = SANDBOX_HEADROOM;
            let env = pnex.join("env");
            std::fs::write(&env, env_file(&boot_env(s, agent))).map_err(|e| e.to_string())?;
            set_mode(&env, 0o600)?;
            if let Some(dns) = &self.dns {
                let resolv = root.join("etc/resolv.conf");
                remove(&resolv);
                std::fs::create_dir_all(root.join("etc")).map_err(|e| e.to_string())?;
                std::fs::write(&resolv, format!("nameserver {dns}\n"))
                    .map_err(|e| e.to_string())?;
            }
            link_tree(&dir.join("workspace"), &root.join("workspace")).await?;
            for c in s.artifacts.iter().flat_map(|a| &a.caches) {
                link_tree(&dir.join("caches").join(&c.name), &inside(&root, &c.path)?).await?;
            }
            if agent && s.artifacts.is_some() {
                link_tree(&dir.join("artifacts/in"), &inside(&root, ARTIFACTS_IN_DIR)?).await?;
                std::fs::create_dir_all(inside(&root, ARTIFACTS_OUT_DIR)?)
                    .map_err(|e| e.to_string())?;
            }
        }
        let disk = dir.join(name);
        // Whole MiB: the guest sees whole sectors only.
        let size = (dir_bytes(&root) * 13 / 10 + headroom).div_ceil(1 << 20) << 20;
        let file = std::fs::File::create(&disk).map_err(|e| e.to_string())?;
        file.set_len(size).map_err(|e| e.to_string())?;
        // No journal: `debugfs` reads what the guest wrote, it never replays one.
        run(Command::new("mkfs.ext4")
            .args(["-q", "-F", "-O", "^has_journal", "-d"])
            .arg(&root)
            .arg(&disk))
        .await?;
        remove(&root);
        Ok(disk)
    }

    /// Boots the disk, waits for the VM to stop, returns the code it reported. Dropping the
    /// future (destruction of the job) kills the VM.
    async fn boot(
        &self,
        dir: &Path,
        disk: &Path,
        net: Option<&(String, String)>,
        console_name: &str,
    ) -> Result<i32, String> {
        let mut args = "console=ttyS0 reboot=k panic=1 pci=off init=/pnex/init".to_string();
        let mut config = serde_json::json!({
            "boot-source": { "kernel_image_path": self.kernel },
            "drives": [{ "drive_id": "rootfs", "path_on_host": disk, "is_root_device": true, "is_read_only": false }],
            "machine-config": { "vcpu_count": self.vcpus, "mem_size_mib": self.mem_mib }
        });
        if let Some((tap, arg)) = net {
            args.push(' ');
            args.push_str(arg);
            config["network-interfaces"] =
                serde_json::json!([{ "iface_id": "eth0", "host_dev_name": tap }]);
        }
        config["boot-source"]["boot_args"] = serde_json::json!(args);
        let config_path = dir.join(format!("{console_name}.json"));
        std::fs::write(&config_path, config.to_string()).map_err(|e| e.to_string())?;
        let console_path = dir.join(console_name);
        let console = std::fs::File::create(&console_path).map_err(|e| e.to_string())?;
        let mut child = Command::new(&self.binary)
            .arg("--no-api")
            .arg("--config-file")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(console.try_clone().map_err(|e| e.to_string())?)
            .stderr(console)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("firecracker not started: {e}"))?;
        child.wait().await.map_err(|e| e.to_string())?;
        let text = std::fs::read_to_string(&console_path).unwrap_or_default();
        Ok(exit_code_of(&text))
    }

    /// Everything of a job, up to its code.
    async fn run_job(
        &self,
        job: &str,
        dir: &Path,
        spec: &RunnerJobSpec,
        sandbox: Option<&RunnerSandboxInput>,
        net: Option<&(String, String)>,
    ) -> Result<i32, String> {
        let image = self.image_root(job, &spec.image).await?;
        let Some(s) = sandbox else {
            let disk = self
                .disk(dir, &image, &spec.command, None, "disk.ext4")
                .await?;
            return self.boot(dir, &disk, None, "console.log").await;
        };
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir.join("tmp")).map_err(|e| e.to_string())?;
        for repo in &s.repos {
            let dest = inside(&workspace, &repo.path)?;
            let mut env = vec![("HOME", dir.join("tmp").display().to_string())];
            env.extend(repo.token.clone().map(|t| ("GIT_TOKEN", t)));
            let dest = dest.display().to_string();
            host_script(
                dir,
                crate::k8s::CLONE_SCRIPT,
                &[&repo.git_ref, &repo.url, &dest],
                &env,
            )
            .await
            .map_err(|e| format!("clone of {}: {e}", repo.path))?;
        }
        if let Some(a) = &s.artifacts {
            if !a.inputs.is_empty() || !a.caches.is_empty() {
                let mut env = transfer_env(dir, a);
                env.push(("PNEX_ARTIFACTS_LIST", crate::artifacts::inputs_list(a)));
                host_script(dir, crate::artifacts::IN_SCRIPT, &[], &env).await?;
            }
        }
        let caches: Vec<_> = s.artifacts.iter().flat_map(|a| &a.caches).collect();
        if !s.setup.is_empty() {
            let disk = self
                .disk(dir, &image, &s.setup, Some((s, false)), "setup.ext4")
                .await?;
            let code = self.boot(dir, &disk, net, "console-setup.log").await?;
            extract(&disk, "/workspace", &workspace).await?;
            for c in &caches {
                extract(&disk, &c.path, &dir.join("caches").join(&c.name)).await?;
            }
            if code != 0 {
                note(dir, &format!("setup ended with {code}: no agent"));
                self.collect(dir, &disk, s).await;
                return Ok(code);
            }
            remove(&disk);
        }
        let disk = self
            .disk(dir, &image, &spec.command, Some((s, true)), "disk.ext4")
            .await?;
        let code = self.boot(dir, &disk, net, "console.log").await?;
        self.collect(dir, &disk, s).await;
        Ok(code)
    }

    /// Outputs and caches off the disk, sent as the collector of a pod sends them: also when
    /// the agent failed; what cannot be sent is in the journal, never a failure of the job.
    async fn collect(&self, dir: &Path, disk: &Path, s: &RunnerSandboxInput) {
        let Some(a) = &s.artifacts else { return };
        if a.outputs.is_empty() && a.caches.is_empty() {
            return;
        }
        let mut got = extract(disk, ARTIFACTS_OUT_DIR, &dir.join("artifacts/out")).await;
        for c in &a.caches {
            if got.is_ok() {
                got = extract(disk, &c.path, &dir.join("caches").join(&c.name)).await;
            }
        }
        if let Err(e) = got {
            note(dir, &format!("outputs not read from the disk: {e}"));
            return;
        }
        let mut env = transfer_env(dir, a);
        env.push(("PNEX_ARTIFACTS_LIST", crate::artifacts::outputs_list(a)));
        env.push(("PNEX_COLLECT_NOW", "1".into()));
        if let Err(e) = host_script(dir, crate::artifacts::OUT_SCRIPT, &[], &env).await {
            note(dir, &format!("collector: {e}"));
        }
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
}

fn read_tail(path: &Path, container: &str) -> Option<RunnerContainerLog> {
    let text = std::fs::read_to_string(path).ok()?;
    let cut = text.len() > LOG_CONTAINER_MAX_BYTES;
    let mut start = text.len().saturating_sub(LOG_CONTAINER_MAX_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Some(RunnerContainerLog {
        container: container.into(),
        text: text[start..].to_string(),
        truncated: cut,
    })
}

impl Firecracker {
    pub fn new(
        base: PathBuf,
        binary: PathBuf,
        kernel: PathBuf,
        insecure: Vec<String>,
        credentials: PathBuf,
    ) -> Self {
        Self {
            base,
            binary,
            kernel,
            vcpus: 2,
            mem_mib: 1024,
            insecure,
            credentials,
            dns: None,
            runner_id: String::new(),
            jobs: Mutex::new(HashMap::new()),
            slots: Mutex::new(HashMap::new()),
        }
    }

    fn job_dir(&self, job: &str) -> PathBuf {
        self.base.join("jobs").join(job)
    }

    fn machine(&self) -> Machine {
        Machine {
            base: self.base.clone(),
            binary: self.binary.clone(),
            kernel: self.kernel.clone(),
            vcpus: self.vcpus,
            mem_mib: self.mem_mib,
            insecure: self.insecure.clone(),
            dns: self.dns.clone(),
        }
    }

    /// The tap and the rules of a sandbox VM, before anything of it runs.
    fn network(
        &self,
        job: &str,
        s: &RunnerSandboxInput,
        dns: &str,
    ) -> Result<(String, String), String> {
        let n = {
            let mut slots = self.slots.lock().map_err(|_| "slot table poisoned")?;
            let taken: std::collections::HashSet<u16> = slots.values().copied().collect();
            let n = (0..16384u16)
                .find(|n| !taken.contains(n))
                .ok_or("no free network slot")?;
            slots.insert(job.to_string(), n);
            n
        };
        let slot = crate::vm_net::slot(n);
        let tap = crate::vm_net::up(job, &slot, &s.egress, dns)?;
        Ok((tap, crate::vm_net::guest_boot_arg(&slot)))
    }
}

impl Executor for Firecracker {
    async fn start(
        &self,
        job: &str,
        spec: &RunnerJobSpec,
        build: Option<&RunnerBuildInput>,
        scan: Option<&RunnerScanInput>,
        sandbox: Option<&RunnerSandboxInput>,
    ) -> Result<(), String> {
        if build.is_some() || scan.is_some() {
            return Err("builds and scans stay on Kubernetes (D749)".into());
        }
        if let Some(s) = sandbox {
            if self.dns.is_none() {
                return Err("a sandbox VM needs the network of this runner".into());
            }
            if !s.services.is_empty() {
                return Err("services not yet in a micro-VM (D762)".into());
            }
        }
        if spec.command.is_empty() {
            return Err("a job of a micro-VM names its command".into());
        }
        crate::oci::parse_reference(&spec.image).ok_or("image is not pinned by digest")?;
        let dir = self.job_dir(job);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let net = match (sandbox, &self.dns) {
            (Some(s), Some(dns)) => Some(self.network(job, s, dns)?),
            _ => None,
        };
        let (machine, id, spec, input) = (
            self.machine(),
            job.to_string(),
            spec.clone(),
            sandbox.cloned(),
        );
        let task = tokio::spawn(async move {
            let dir = machine.base.join("jobs").join(&id);
            let code = match machine
                .run_job(&id, &dir, &spec, input.as_ref(), net.as_ref())
                .await
            {
                Ok(code) => code,
                Err(e) => {
                    note(&dir, &format!("job not run: {e}"));
                    1
                }
            };
            let _ = std::fs::write(dir.join("exit"), code.to_string());
        });
        self.jobs.lock().map_err(|_| "job table poisoned")?.insert(
            job.to_string(),
            Held {
                task,
                sandbox: sandbox.cloned(),
            },
        );
        Ok(())
    }

    async fn status(&self, job: &str) -> Result<WorkloadStatus, String> {
        if let Ok(code) = std::fs::read_to_string(self.job_dir(job).join("exit")) {
            return Ok(WorkloadStatus::Exited(code.trim().parse().unwrap_or(1)));
        }
        let jobs = self.jobs.lock().map_err(|_| "job table poisoned")?;
        Ok(match jobs.get(job) {
            // A task that ended without its code panicked.
            Some(h) if h.task.is_finished() => WorkloadStatus::Exited(1),
            Some(_) => WorkloadStatus::Running,
            None => WorkloadStatus::Gone,
        })
    }

    async fn logs(&self, job: &str) -> RunnerJobLogs {
        let dir = self.job_dir(job);
        RunnerJobLogs {
            containers: [
                ("transfer.log", "transfer"),
                ("console-setup.log", "setup"),
                ("console.log", "vm"),
            ]
            .iter()
            .filter_map(|(file, name)| read_tail(&dir.join(file), name))
            .collect(),
        }
    }

    async fn destroy(&self, job: &str) -> Result<(), String> {
        let held = self
            .jobs
            .lock()
            .map_err(|_| "job table poisoned")?
            .remove(job);
        let dir = self.job_dir(job);
        if let Some(h) = held {
            if !h.task.is_finished() {
                h.task.abort();
                let _ = h.task.await;
                // Stopped before its end (cancel, deadline): what it left is still sent.
                if let Some(s) = &h.sandbox {
                    let disk = ["disk.ext4", "setup.ext4"]
                        .iter()
                        .map(|d| dir.join(d))
                        .find(|d| d.exists());
                    if let Some(disk) = disk {
                        self.machine().collect(&dir, &disk, s).await;
                    }
                }
            }
        }
        if self
            .slots
            .lock()
            .map_err(|_| "slot table poisoned")?
            .remove(job)
            .is_some()
        {
            crate::vm_net::down(job)?;
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn list(&self) -> Result<Vec<String>, String> {
        let mut jobs: Vec<String> = std::fs::read_dir(self.base.join("jobs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        jobs.sort();
        Ok(jobs)
    }
}

impl Host for Firecracker {
    fn capabilities(&self) -> Vec<String> {
        let mut c = capabilities_of("vm", None);
        c.retain(|c| c != "isolation:none");
        c.push("isolation:vm".into());
        c.push("runtime:firecracker".into());
        if self.dns.is_some() {
            c.push("sandbox:vm".into());
        }
        c
    }

    fn set_runner_id(&mut self, id: String) {
        self.runner_id = id;
    }

    async fn load_credentials(&self) -> Result<Option<(String, String)>, String> {
        crate::podman::load_file_credentials(&self.credentials)
    }

    async fn save_credentials(&self, runner_id: &str, token: &str) -> Result<(), String> {
        crate::podman::save_file_credentials(&self.credentials, runner_id, token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_command_reaches_the_vm_word_for_word_and_its_code_comes_back() {
        let script = init_script(
            &["sh".into(), "-c".into(), "echo 'it''s'; exit 3".into()],
            "/workspace",
        );
        assert!(
            script.contains(r#"'sh' '-c' 'echo '\''it'\'''\''s'\''; exit 3'"#),
            "{script}"
        );
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("cd '/workspace'\n"));
        assert_eq!(exit_code_of("boot\npnex-exit=3\n[ 0.1] reboot"), 3);
        assert_eq!(
            exit_code_of("kernel panic"),
            1,
            "no word from the VM is a failure"
        );
        let vars = [
            ("TOKEN".to_string(), "a'b".to_string()),
            ("X;reboot".to_string(), "no".to_string()),
        ];
        assert_eq!(env_file(&vars), "export TOKEN='a'\\''b'\n", "names only");
    }

    #[test]
    fn setup_never_gets_the_secrets_the_agent_does() {
        let s = RunnerSandboxInput {
            init_image: String::new(),
            repos: vec![],
            services: vec![],
            setup: vec!["make".into()],
            env: BTreeMap::from([("MODE".to_string(), "fix".to_string())]),
            secrets: BTreeMap::from([("PNEX_RUN_TOKEN".to_string(), "pnagr_x".to_string())]),
            egress: vec![],
            artifacts: None,
        };
        let names =
            |agent| -> Vec<String> { boot_env(&s, agent).into_iter().map(|(k, _)| k).collect() };
        assert_eq!(names(false), ["MODE", "HOME"]);
        assert_eq!(names(true), ["MODE", "HOME", "PNEX_RUN_TOKEN"]);
    }

    #[tokio::test]
    async fn what_the_guest_leaves_on_its_disk_comes_out_links_unfollowed() {
        if std::process::Command::new("debugfs")
            .arg("-V")
            .output()
            .is_err()
        {
            eprintln!("debugfs absent: skipped");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("root");
        std::fs::create_dir_all(root.join("artifacts/out/tree")).unwrap();
        std::fs::write(root.join("artifacts/out/report"), "green").unwrap();
        std::fs::write(root.join("artifacts/out/tree/b"), "beta").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("artifacts/out/leak")).unwrap();
        let disk = work.path().join("disk.ext4");
        std::fs::File::create(&disk)
            .unwrap()
            .set_len(32 << 20)
            .unwrap();
        run(Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(&root)
            .arg(&disk))
        .await
        .unwrap();
        let out = work.path().join("out");
        extract(&disk, "/artifacts/out", &out).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("report")).unwrap(),
            "green"
        );
        assert_eq!(std::fs::read_to_string(out.join("tree/b")).unwrap(), "beta");
        assert!(std::fs::symlink_metadata(out.join("leak"))
            .unwrap()
            .file_type()
            .is_symlink());
        // A folder the guest never made reads as empty; a path not of the guest is refused.
        extract(&disk, "/nothing", &out).await.unwrap();
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
        assert!(extract(&disk, "rel\"; rm", &out).await.is_err());
        assert!(inside(&root, "/a/../../x").is_err());
    }

    #[test]
    fn a_firecracker_runner_announces_a_vm_never_a_pod() {
        let fc = Firecracker::new("/tmp/x".into(), "fc".into(), "k".into(), vec![], "c".into());
        let caps = fc.capabilities();
        assert!(caps.contains(&"isolation:vm".to_string()));
        assert!(!caps
            .iter()
            .any(|c| c == "isolation:none" || c == "isolation:pod" || c == "sandbox:pod"));
        assert!(pnex_core::runners::capabilities_are_valid(&caps));
    }
}
