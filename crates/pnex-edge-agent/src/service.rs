//! OS service integration: systemd (Linux, system or `--user`), Windows
//! Service (SCM). macOS: launchd plist is printed, not installed (not
//! distributed in v1).

use std::path::Path;

use anyhow::{Context, Result};

pub const SERVICE_NAME: &str = "pnex-agent";
#[cfg(unix)]
const SYSTEM_USER: &str = "pnex-agent";

#[cfg(unix)]
fn sh(cmd: &str, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("cannot run {cmd}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "`{cmd} {}` failed ({status})",
            args.join(" ")
        ));
    }
    Ok(())
}

/// systemd unit text (pure, unit-tested).
#[cfg(unix)]
pub fn systemd_unit(exe: &Path, dir: &Path, user: Option<&str>) -> String {
    let user_line = user
        .map(|u| format!("User={u}\nGroup={u}\n"))
        .unwrap_or_default();
    let wanted = if user.is_none() && !crate::config::is_root() {
        "default.target"
    } else {
        "multi-user.target"
    };
    format!(
        "[Unit]\n\
         Description=PNeX edge agent\n\
         Wants=network-online.target\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         {user_line}\
         ExecStart=\"{exe}\" run --dir \"{dir}\"\n\
         Restart=always\n\
         RestartSec=5\n\
         NoNewPrivileges=true\n\
         \n\
         [Install]\n\
         WantedBy={wanted}\n",
        exe = exe.display(),
        dir = dir.display(),
    )
}

#[cfg(target_os = "linux")]
pub fn install(dir: &Path, user_mode: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    if user_mode || !crate::config::is_root() {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
            })
            .join("systemd")
            .join("user");
        std::fs::create_dir_all(&base)?;
        std::fs::write(
            base.join(format!("{SERVICE_NAME}.service")),
            systemd_unit(&exe, dir, None),
        )?;
        sh("systemctl", &["--user", "daemon-reload"])?;
        sh("systemctl", &["--user", "enable", SERVICE_NAME])?;
        sh("systemctl", &["--user", "restart", SERVICE_NAME])?;
        println!("Tip: `loginctl enable-linger $USER` keeps the agent running after logout.");
        return Ok(());
    }
    // Dedicated unprivileged system user owning the agent directory.
    let user = if std::process::Command::new("id")
        .arg(SYSTEM_USER)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        Some(SYSTEM_USER)
    } else if sh(
        "useradd",
        &[
            "--system",
            "--no-create-home",
            "--shell",
            "/usr/sbin/nologin",
            SYSTEM_USER,
        ],
    )
    .is_ok()
    {
        Some(SYSTEM_USER)
    } else {
        eprintln!("warning: could not create the `{SYSTEM_USER}` user — the service runs as root");
        None
    };
    if let Some(u) = user {
        sh(
            "chown",
            &["-R", &format!("{u}:{u}"), &dir.display().to_string()],
        )?;
    }
    std::fs::write(
        format!("/etc/systemd/system/{SERVICE_NAME}.service"),
        systemd_unit(&exe, dir, user),
    )?;
    sh("systemctl", &["daemon-reload"])?;
    sh("systemctl", &["enable", SERVICE_NAME])?;
    sh("systemctl", &["restart", SERVICE_NAME])?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn uninstall(user_mode: bool) -> Result<()> {
    if user_mode || !crate::config::is_root() {
        let _ = sh("systemctl", &["--user", "disable", "--now", SERVICE_NAME]);
        let base = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".config/systemd/user");
        let _ = std::fs::remove_file(base.join(format!("{SERVICE_NAME}.service")));
        let _ = sh("systemctl", &["--user", "daemon-reload"]);
    } else {
        let _ = sh("systemctl", &["disable", "--now", SERVICE_NAME]);
        let _ = std::fs::remove_file(format!("/etc/systemd/system/{SERVICE_NAME}.service"));
        let _ = sh("systemctl", &["daemon-reload"]);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn install(dir: &Path, _user_mode: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    println!(
        "macOS: service installation is not automated yet. Run in foreground:\n  {} run --dir {}",
        exe.display(),
        dir.display()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall(_user_mode: bool) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
pub fn install(dir: &Path, _user_mode: bool) -> Result<()> {
    use std::ffi::OsString;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("cannot open the service manager (run as Administrator)")?;
    // Reinstall: stop + delete the previous registration first.
    if let Ok(existing) = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    ) {
        let _ = existing.stop();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = existing.delete();
        drop(existing);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from("PNeX edge agent"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe()?,
        launch_arguments: vec![
            OsString::from("run"),
            OsString::from("--service"),
            OsString::from("--dir"),
            dir.as_os_str().to_owned(),
        ],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::START | ServiceAccess::CHANGE_CONFIG)
        .context("cannot create the pnex-agent service")?;
    let _ = service.set_description("Buffers local measurements and forwards them to PNeX.");
    service
        .start::<&str>(&[])
        .context("cannot start the pnex-agent service")?;
    Ok(())
}

#[cfg(windows)]
pub fn uninstall(_user_mode: bool) -> Result<()> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    if let Ok(s) = manager.open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE) {
        let _ = s.stop();
        std::thread::sleep(std::time::Duration::from_secs(2));
        s.delete()?;
    }
    Ok(())
}

/// Windows SCM entry point (`pnex-agent run --service`).
#[cfg(windows)]
pub mod win {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::time::Duration;

    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    static DIR: OnceLock<PathBuf> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn run(dir: PathBuf) -> anyhow::Result<()> {
        let _ = DIR.set(dir);
        service_dispatcher::start(super::SERVICE_NAME, ffi_service_main)?;
        Ok(())
    }

    fn status(state: ServiceState, accept: ServiceControlAccept) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        }
    }

    fn service_main(_args: Vec<OsString>) {
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let stop_tx = std::sync::Mutex::new(Some(stop_tx));
        let handler = move |event| match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Some(tx) = stop_tx.lock().expect("stop").take() {
                    let _ = tx.send(());
                }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let Ok(handle) = service_control_handler::register(super::SERVICE_NAME, handler) else {
            return;
        };
        let _ = handle.set_service_status(status(
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ));
        let dir = DIR.get().cloned().unwrap_or_default();
        let result = tokio::runtime::Runtime::new().map(|rt| {
            rt.block_on(crate::run::run(&dir, async move {
                let _ = stop_rx.await;
            }))
        });
        if let Ok(Err(e)) | Err(e) = result.map_err(anyhow::Error::from) {
            tracing::error!("pnex-agent service failed: {e:#}");
        }
        let _ =
            handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty()));
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn unit_runs_the_agent_as_its_user() {
        let u = super::systemd_unit(
            std::path::Path::new("/usr/local/bin/pnex-agent"),
            std::path::Path::new("/var/lib/pnex-agent"),
            Some("pnex-agent"),
        );
        assert!(
            u.contains("ExecStart=\"/usr/local/bin/pnex-agent\" run --dir \"/var/lib/pnex-agent\"")
        );
        assert!(u.contains("User=pnex-agent"));
        assert!(u.contains("Restart=always"));
    }
}
