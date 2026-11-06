//! Firecracker executor (agent-environments.md D749, D762): each job in a micro-VM of its
//! own, booted from the image pinned by digest, destroyed with the job. First slice: plain
//! jobs (a command in an image), without network; the sandbox inputs, the network rule of
//! the host and the transfer of files come next.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use pnex_core::runners::{
    RunnerBuildInput, RunnerContainerLog, RunnerJobLogs, RunnerJobSpec, RunnerSandboxInput,
    RunnerScanInput, LOG_CONTAINER_MAX_BYTES,
};

use crate::{capabilities_of, Executor, Host, WorkloadStatus};

/// Line `/init` prints last, read back as the code of the job.
pub const EXIT_MARK: &str = "pnex-exit=";

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
    vms: Mutex<HashMap<String, Child>>,
    /// The /30 each running VM holds (`vm_net::slot`).
    slots: Mutex<HashMap<String, u16>>,
}

/// `'…'` with each `'` closed, escaped and reopened: no argument is read by the shell.
fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// PID 1 of the VM: mounts what a job expects, runs it, reports its code, powers off. Without
/// `reboot`, the end of PID 1 panics the kernel, and `panic=1 reboot=k` stops the VM at once.
pub fn init_script(command: &[String]) -> String {
    let job: Vec<String> = command.iter().map(|a| quote(a)).collect();
    format!(
        "#!/bin/sh\n\
         mount -t proc proc /proc\n\
         mount -t sysfs sys /sys\n\
         mount -t devtmpfs dev /dev 2>/dev/null\n\
         mount -t tmpfs tmp /tmp\n\
         [ -f /pnex/env ] && . /pnex/env\n\
         cd /\n\
         {}\n\
         echo \"{EXIT_MARK}$?\"\n\
         reboot -f 2>/dev/null\n\
         exit 0\n",
        job.join(" ")
    )
}

/// `export K='v'` lines of the variables of a run, read by `/init` from a root-only file.
pub fn env_file(vars: &[(&String, &String)]) -> String {
    vars.iter()
        .map(|(k, v)| format!("export {k}={}\n", quote(v)))
        .collect()
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

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
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
            vms: Mutex::new(HashMap::new()),
            slots: Mutex::new(HashMap::new()),
        }
    }

    fn job_dir(&self, job: &str) -> PathBuf {
        self.base.join("jobs").join(job)
    }

    /// The image unpacked once per digest; a partial pull never counts as one.
    async fn image_root(&self, image: &str) -> Result<PathBuf, String> {
        let (host, repo, digest) =
            crate::oci::parse_reference(image).ok_or("image is not pinned by digest")?;
        let root = self
            .base
            .join("images")
            .join(digest.trim_start_matches("sha256:"));
        if root.exists() {
            return Ok(root);
        }
        let partial = root.with_extension("partial");
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
        let mut registry =
            crate::oci::Registry::new(&host, &repo, self.insecure.contains(&host), None);
        for layer in registry.layers(&digest).await? {
            let bytes = registry.layer(&layer).await?;
            crate::oci::apply_layer(&partial, &bytes)?;
        }
        std::fs::rename(&partial, &root).map_err(|e| e.to_string())?;
        Ok(root)
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
            if !s.repos.is_empty()
                || !s.services.is_empty()
                || !s.setup.is_empty()
                || s.artifacts.is_some()
            {
                return Err(
                    "repositories, services, setup and files not yet in a micro-VM (D762)".into(),
                );
            }
        }
        if spec.command.is_empty() {
            return Err("a job of a micro-VM names its command".into());
        }
        let image = self.image_root(&spec.image).await?;
        let dir = self.job_dir(job);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Hard links: the job's root costs no copy, and its disk is its own.
        let root = dir.join("root");
        run(Command::new("cp").arg("-al").arg(&image).arg(&root))?;
        let pnex = root.join("pnex");
        let _ = std::fs::remove_dir_all(&pnex);
        std::fs::create_dir_all(&pnex).map_err(|e| e.to_string())?;
        let init = pnex.join("init");
        std::fs::write(&init, init_script(&spec.command)).map_err(|e| e.to_string())?;
        run(Command::new("chmod").arg("755").arg(&init))?;
        let mut network = None;
        if let (Some(s), Some(dns)) = (sandbox, &self.dns) {
            // Variables and secrets of the run in a root-only file of its own disk.
            let vars: Vec<(&String, &String)> = s.env.iter().chain(s.secrets.iter()).collect();
            let env = pnex.join("env");
            std::fs::write(&env, env_file(&vars)).map_err(|e| e.to_string())?;
            run(Command::new("chmod").arg("600").arg(&env))?;
            let resolv = root.join("etc/resolv.conf");
            let _ = std::fs::remove_file(&resolv);
            std::fs::create_dir_all(root.join("etc")).map_err(|e| e.to_string())?;
            std::fs::write(&resolv, format!("nameserver {dns}\n")).map_err(|e| e.to_string())?;
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
            network = Some((tap, crate::vm_net::guest_boot_arg(&slot)));
        }
        let disk = dir.join("disk.ext4");
        let size = dir_bytes(&root) * 13 / 10 + (256 << 20);
        let file = std::fs::File::create(&disk).map_err(|e| e.to_string())?;
        file.set_len(size).map_err(|e| e.to_string())?;
        run(Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(&root)
            .arg(&disk))?;
        std::fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
        let mut boot = "console=ttyS0 reboot=k panic=1 pci=off init=/pnex/init".to_string();
        let mut config = serde_json::json!({
            "boot-source": { "kernel_image_path": self.kernel },
            "drives": [{ "drive_id": "rootfs", "path_on_host": disk, "is_root_device": true, "is_read_only": false }],
            "machine-config": { "vcpu_count": self.vcpus, "mem_size_mib": self.mem_mib }
        });
        if let Some((tap, arg)) = &network {
            boot.push(' ');
            boot.push_str(arg);
            config["network-interfaces"] =
                serde_json::json!([{ "iface_id": "eth0", "host_dev_name": tap }]);
        }
        config["boot-source"]["boot_args"] = serde_json::json!(boot);
        let config_path = dir.join("vm.json");
        std::fs::write(&config_path, config.to_string()).map_err(|e| e.to_string())?;
        let console = std::fs::File::create(dir.join("console.log")).map_err(|e| e.to_string())?;
        let child = Command::new(&self.binary)
            .arg("--no-api")
            .arg("--config-file")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(console.try_clone().map_err(|e| e.to_string())?)
            .stderr(console)
            .spawn()
            .map_err(|e| format!("firecracker not started: {e}"))?;
        self.vms
            .lock()
            .map_err(|_| "vm table poisoned")?
            .insert(job.to_string(), child);
        Ok(())
    }

    async fn status(&self, job: &str) -> Result<WorkloadStatus, String> {
        let mut vms = self.vms.lock().map_err(|_| "vm table poisoned")?;
        let Some(child) = vms.get_mut(job) else {
            return Ok(WorkloadStatus::Gone);
        };
        match child.try_wait().map_err(|e| e.to_string())? {
            None => Ok(WorkloadStatus::Running),
            Some(_) => {
                let console = std::fs::read_to_string(self.job_dir(job).join("console.log"))
                    .unwrap_or_default();
                Ok(WorkloadStatus::Exited(exit_code_of(&console)))
            }
        }
    }

    async fn logs(&self, job: &str) -> RunnerJobLogs {
        let Ok(text) = std::fs::read_to_string(self.job_dir(job).join("console.log")) else {
            return RunnerJobLogs::default();
        };
        let cut = text.len() > LOG_CONTAINER_MAX_BYTES;
        let mut start = text.len().saturating_sub(LOG_CONTAINER_MAX_BYTES);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        RunnerJobLogs {
            containers: vec![RunnerContainerLog {
                container: "vm".into(),
                text: text[start..].to_string(),
                truncated: cut,
            }],
        }
    }

    async fn destroy(&self, job: &str) -> Result<(), String> {
        if let Some(mut child) = self
            .vms
            .lock()
            .map_err(|_| "vm table poisoned")?
            .remove(job)
        {
            let _ = child.kill();
            let _ = child.wait();
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
        match std::fs::remove_dir_all(self.job_dir(job)) {
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

    #[test]
    fn the_command_reaches_the_vm_word_for_word_and_its_code_comes_back() {
        let script = init_script(&["sh".into(), "-c".into(), "echo 'it''s'; exit 3".into()]);
        assert!(
            script.contains(r#"'sh' '-c' 'echo '\''it'\'''\''s'\''; exit 3'"#),
            "{script}"
        );
        assert!(script.starts_with("#!/bin/sh\n"));
        assert_eq!(exit_code_of("boot\npnex-exit=3\n[ 0.1] reboot"), 3);
        assert_eq!(
            exit_code_of("kernel panic"),
            1,
            "no word from the VM is a failure"
        );
        let (k, v) = ("TOKEN".to_string(), "a'b".to_string());
        assert_eq!(env_file(&[(&k, &v)]), "export TOKEN='a'\\''b'\n");
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
