//! Confinement of the media decoder (media-ingest.md D160, R10).
//!
//! ffmpeg decodes untrusted bytes. It gets them on stdin and writes PCM on
//! stdout; it needs no network and no file write. Bubblewrap needs user
//! namespaces, refused on hardened hosts (Ubuntu 24.04 AppArmor) and in
//! unprivileged containers, so the default confinement only uses what an
//! unprivileged process may apply to itself before `exec`:
//!
//! - seccomp: `socket()` and `io_uring_setup()` fail with `EACCES` (no
//!   network of any family, no unix socket to Valkey or Docker), foreign
//!   syscall ABIs kill the process;
//! - Landlock: the whole filesystem is read-only, TCP bind/connect denied
//!   (best effort on older kernels, logged);
//! - rlimits: no file growth (`RLIMIT_FSIZE` 0), bounded memory and fds;
//! - `PR_SET_PDEATHSIG`: the decoder dies with its supervisor.
//!
//! [`SandboxMode::Bwrap`] adds bubblewrap on top where namespaces work.

/// How the decoder is confined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxMode {
    /// seccomp + Landlock + rlimits (default).
    Kernel,
    /// [`SandboxMode::Kernel`] inside bubblewrap with empty namespaces.
    Bwrap {
        program: String,
        ro_binds: Vec<String>,
    },
    /// Rlimits and parent-death signal only (development).
    None,
}

/// Memory cap of the decoder process.
const DECODER_MAX_MEMORY: u64 = 1024 * 1024 * 1024;
/// Open file cap of the decoder process.
const DECODER_MAX_FILES: u64 = 64;

/// Wraps `argv` for the mode (bubblewrap prefix), returns the final argv.
pub fn wrap_argv(mode: &SandboxMode, argv: Vec<String>) -> Vec<String> {
    let SandboxMode::Bwrap { program, ro_binds } = mode else {
        return argv;
    };
    let mut out = vec![program.clone()];
    for dir in ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"] {
        out.extend(["--ro-bind-try".into(), dir.into(), dir.into()]);
    }
    for dir in ro_binds {
        out.extend(["--ro-bind-try".into(), dir.clone(), dir.clone()]);
    }
    out.extend(
        [
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
            "--",
        ]
        .map(String::from),
    );
    out.extend(argv);
    out
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{SandboxMode, DECODER_MAX_FILES, DECODER_MAX_MEMORY};
    use landlock::{
        Access, AccessFs, AccessNet, Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr, ABI,
    };

    const AUDIT_ARCH: u32 = if cfg!(target_arch = "x86_64") {
        0xC000_003E
    } else if cfg!(target_arch = "aarch64") {
        0xC000_00B7
    } else {
        0
    };
    /// x32 syscalls on x86_64 carry this bit: denied with the rest.
    const X32_SYSCALL_BIT: u32 = 0x4000_0000;
    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

    fn stmt(code: u32, k: u32) -> libc::sock_filter {
        libc::sock_filter {
            code: code as u16,
            jt: 0,
            jf: 0,
            k,
        }
    }

    fn jump(code: u32, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
        libc::sock_filter {
            code: code as u16,
            jt,
            jf,
            k,
        }
    }

    /// BPF program: wrong arch → kill; x32, `socket`, `io_uring_setup` →
    /// `EACCES`; everything else allowed.
    pub(super) fn seccomp_program() -> Vec<libc::sock_filter> {
        let ld_w_abs = libc::BPF_LD | libc::BPF_W | libc::BPF_ABS;
        let jeq = libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K;
        let jge = libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K;
        let ret = libc::BPF_RET | libc::BPF_K;
        vec![
            // [0] arch (offsetof(seccomp_data, arch) = 4)
            stmt(ld_w_abs, 4),
            // [1] native arch → [3], else [2]
            jump(jeq, AUDIT_ARCH, 1, 0),
            // [2]
            stmt(ret, SECCOMP_RET_KILL_PROCESS),
            // [3] syscall number (offset 0)
            stmt(ld_w_abs, 0),
            // [4] x32 → deny [8]
            jump(jge, X32_SYSCALL_BIT, 3, 0),
            // [5] socket → deny [8]
            jump(jeq, libc::SYS_socket as u32, 2, 0),
            // [6] io_uring_setup → deny [8]
            jump(jeq, libc::SYS_io_uring_setup as u32, 1, 0),
            // [7]
            stmt(ret, SECCOMP_RET_ALLOW),
            // [8]
            stmt(ret, SECCOMP_RET_ERRNO | libc::EACCES as u32),
        ]
    }

    /// Landlock ruleset prepared in the parent (no allocation after fork):
    /// read-only filesystem, no TCP. `None` when the kernel has no
    /// Landlock (logged once by the caller).
    fn landlock_ruleset() -> Option<RulesetCreated> {
        let abi = ABI::V4;
        let ruleset = Ruleset::default()
            .handle_access(AccessFs::from_all(abi))
            .ok()?
            .handle_access(AccessNet::from_all(abi))
            .ok()?
            .create()
            .ok()?;
        let root = landlock::PathFd::new("/").ok()?;
        ruleset
            .add_rule(landlock::PathBeneath::new(root, AccessFs::from_read(abi)))
            .ok()
    }

    /// Installs the confinement on `cmd`, applied in the child right
    /// before `exec`.
    pub fn confine(cmd: &mut tokio::process::Command, mode: &SandboxMode) {
        let kernel = !matches!(mode, SandboxMode::None);
        let program = seccomp_program();
        let mut landlock = if kernel { landlock_ruleset() } else { None };
        if kernel && landlock.is_none() {
            tracing::warn!(
                "Landlock unavailable: media decoder confined by seccomp and rlimits only"
            );
        }
        // SAFETY: the closure runs in the forked child before exec; it only
        // calls async-signal-safe syscalls (prctl, setrlimit, seccomp,
        // landlock_restrict_self) on data prepared before the fork.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for (res, val) in [
                    (libc::RLIMIT_AS, DECODER_MAX_MEMORY),
                    (libc::RLIMIT_NOFILE, DECODER_MAX_FILES),
                    (libc::RLIMIT_FSIZE, 0),
                    (libc::RLIMIT_CORE, 0),
                ] {
                    let lim = libc::rlimit {
                        rlim_cur: val as libc::rlim_t,
                        rlim_max: val as libc::rlim_t,
                    };
                    if libc::setrlimit(res, &lim) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                if !kernel {
                    return Ok(());
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(ruleset) = landlock.take() {
                    ruleset
                        .restrict_self()
                        .map_err(|_| std::io::Error::from_raw_os_error(libc::EPERM))?;
                }
                let prog = libc::sock_fprog {
                    len: program.len() as u16,
                    filter: program.as_ptr() as *mut libc::sock_filter,
                };
                if libc::prctl(
                    libc::PR_SET_SECCOMP,
                    libc::SECCOMP_MODE_FILTER,
                    &prog as *const libc::sock_fprog,
                ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::confine;

#[cfg(not(target_os = "linux"))]
pub fn confine(_cmd: &mut tokio::process::Command, _mode: &SandboxMode) {}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::Stdio;

    async fn run(script: &str, mode: Option<&SandboxMode>) -> std::process::Output {
        let mut cmd = tokio::process::Command::new("/bin/bash");
        cmd.arg("-c")
            .arg(script)
            .env_clear()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(mode) = mode {
            confine(&mut cmd, mode);
        }
        cmd.output().await.expect("spawn bash")
    }

    async fn run_confined(script: &str) -> std::process::Output {
        run(script, Some(&SandboxMode::Kernel)).await
    }

    #[tokio::test]
    async fn confined_child_has_no_network() {
        if !std::path::Path::new("/bin/bash").exists() {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                drop(sock);
            }
        });
        let script = format!("exec 3<>/dev/tcp/127.0.0.1/{port} && echo connected");
        // Control: the same script reaches the listener when unconfined.
        let open = run(&script, None).await;
        assert!(String::from_utf8_lossy(&open.stdout).contains("connected"));
        let out = run_confined(&script).await;
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("connected"),
            "socket must be refused"
        );
    }

    #[tokio::test]
    async fn confined_child_cannot_write_files() {
        if !std::path::Path::new("/bin/bash").exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("pnex-sandbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let control = dir.join("control");
        let ok = run(&format!("echo x > {}", control.display()), None).await;
        assert!(ok.status.success() && control.exists(), "control write");
        let target = dir.join("x");
        let out = run_confined(&format!("echo pwned > {} && echo wrote", target.display())).await;
        assert!(!String::from_utf8_lossy(&out.stdout).contains("wrote"));
        let written = std::fs::read_to_string(&target).unwrap_or_default();
        assert!(written.is_empty(), "file must stay empty or absent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn confined_child_still_runs_and_pipes() {
        if !std::path::Path::new("/bin/bash").exists() {
            return;
        }
        let out = run_confined("printf ok").await;
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
    }

    #[test]
    fn bwrap_prefix_isolates_everything() {
        let argv = wrap_argv(
            &SandboxMode::Bwrap {
                program: "bwrap".into(),
                ro_binds: vec!["/opt/ffmpeg".into()],
            },
            vec!["ffmpeg".into(), "-i".into(), "pipe:0".into()],
        );
        assert_eq!(argv[0], "bwrap");
        assert!(argv
            .windows(3)
            .any(|w| w == ["--ro-bind-try", "/opt/ffmpeg", "/opt/ffmpeg"]));
        assert!(argv.contains(&"--unshare-all".to_string()));
        let sep = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(&argv[sep + 1..], ["ffmpeg", "-i", "pipe:0"]);
        assert_eq!(
            wrap_argv(&SandboxMode::Kernel, vec!["ffmpeg".into()]),
            ["ffmpeg"]
        );
    }
}
