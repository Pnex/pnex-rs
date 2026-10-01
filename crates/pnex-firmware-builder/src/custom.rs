//! Custom firmware builds (custom-firmware.md D89–D92).
//!
//! A custom build is a generic build whose project gets the revision's
//! `src/main.cpp` and extra pinned `lib_deps`; the `platformio.ini` itself
//! is never user-editable (it carries the secrets and the library policy —
//! `extra_scripts` there would be arbitrary code execution).
//!
//! Compiling user C++ is running user code on the worker, so a custom
//! build can be wrapped in a [`Sandbox`] (bubblewrap): no network, fresh
//! pid namespace (no `/proc/<server pid>/environ`), read-only host except
//! the job workspace and the PlatformIO core dir, empty `$HOME`.

use std::path::{Path, PathBuf};

use crate::BuildError;

/// User sketch compiled in place of the generic `main.cpp`.
#[derive(Debug, Clone, Default)]
pub struct CustomSource {
    /// Full `main.cpp` (already checked by `pnex_core::firmware::check_main_cpp`).
    pub main_cpp: String,
    /// PlatformIO specs appended to `lib_deps` (resolved from the catalog).
    pub lib_specs: Vec<String>,
}

/// Process isolation for `pio run` (D91).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sandbox {
    /// bubblewrap wrapper. `allow_network` = registry resolution allowed
    /// (lib_deps not prefetched) — the filesystem isolation stays.
    Bwrap {
        program: String,
        allow_network: bool,
    },
}

/// Per-build options beyond the generic pipeline.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub custom: Option<CustomSource>,
    pub sandbox: Option<Sandbox>,
}

/// Writes the sketch and the extra libraries into the staged project.
pub(crate) fn apply_custom_source(project: &Path, custom: &CustomSource) -> Result<(), BuildError> {
    let io = |e: std::io::Error| BuildError::Source(format!("custom source: {e}"));
    let src = project.join("src");
    // The generic project may hold other sources: the sketch is the only
    // translation unit of a custom build.
    if src.is_dir() {
        std::fs::remove_dir_all(&src).map_err(io)?;
    }
    std::fs::create_dir_all(&src).map_err(io)?;
    std::fs::write(src.join("main.cpp"), &custom.main_cpp).map_err(io)?;
    if !custom.lib_specs.is_empty() {
        let ini_path = project.join("platformio.ini");
        let ini = std::fs::read_to_string(&ini_path).map_err(io)?;
        let patched = inject_lib_deps(&ini, &custom.lib_specs)
            .ok_or_else(|| BuildError::Source("platformio.ini without lib_deps".into()))?;
        std::fs::write(&ini_path, patched).map_err(io)?;
    }
    Ok(())
}

/// Appends `specs` to every `lib_deps =` block of the ini (one per env).
/// `None` when the ini declares no `lib_deps` (the generic projects all do).
pub(crate) fn inject_lib_deps(ini: &str, specs: &[String]) -> Option<String> {
    let lines: Vec<&str> = ini.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + specs.len());
    let mut found = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        out.push(line.to_string());
        let is_key = line
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == "lib_deps")
            && !line.starts_with(char::is_whitespace);
        if !is_key {
            i += 1;
            continue;
        }
        found = true;
        // Continuation lines of the multi-line value are indented; comments
        // and blank lines inside the block are kept in place.
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            let cont = next.starts_with(char::is_whitespace) && !next.trim().is_empty();
            if !cont {
                break;
            }
            out.push(next.to_string());
            i += 1;
        }
        for spec in specs {
            out.push(format!("    {spec}"));
        }
    }
    found.then(|| out.join("\n") + "\n")
}

/// Wraps a command argv in the sandbox. `workspace` (read-write) holds the
/// staged project; `pio_core` (read-write) is the PlatformIO core dir
/// (toolchains, platforms, package cache); `tool_dirs` are the read-only
/// locations the tool binaries live in (resolved program dir, uv venv…).
pub fn sandbox_argv(
    sandbox: &Sandbox,
    argv: &[String],
    workspace: &Path,
    pio_core: &Path,
    tool_dirs: &[PathBuf],
) -> Vec<String> {
    let Sandbox::Bwrap {
        program,
        allow_network,
    } = sandbox;
    let mut a: Vec<String> = vec![program.clone()];
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| (*s).to_string()));
    // System directories, read-only (toolchain shared libs, python, certs).
    for dir in ["/usr", "/etc", "/lib", "/lib64", "/bin", "/sbin", "/opt"] {
        push(&["--ro-bind-try", dir, dir]);
    }
    push(&["--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]);
    // Fresh pid namespace: the server's /proc/<pid>/environ is unreachable.
    push(&[
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--die-with-parent",
        "--new-session",
    ]);
    if !allow_network {
        push(&["--unshare-net"]);
    }
    let mut a2 = a;
    for dir in tool_dirs {
        let d = dir.display().to_string();
        a2.extend(["--ro-bind-try".into(), d.clone(), d]);
    }
    let core = pio_core.display().to_string();
    a2.extend(["--bind".into(), core.clone(), core]);
    let ws = workspace.display().to_string();
    a2.extend([
        "--bind".into(),
        ws.clone(),
        ws.clone(),
        "--chdir".into(),
        ws,
    ]);
    a2.push("--".into());
    a2.extend(argv.iter().cloned());
    a2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_lib_deps_appends_to_each_block() {
        let ini = "[env:a]\nboard = x\nlib_deps =\n    SPI\n    ; note\n    foo/bar@^1\nbuild_flags = -D X\n\n[env:b]\nlib_deps = one\n";
        let out = inject_lib_deps(ini, &["adafruit/DHT sensor library@1.4.7".into()]).unwrap();
        assert_eq!(
            out,
            "[env:a]\nboard = x\nlib_deps =\n    SPI\n    ; note\n    foo/bar@^1\n    adafruit/DHT sensor library@1.4.7\nbuild_flags = -D X\n\n[env:b]\nlib_deps = one\n    adafruit/DHT sensor library@1.4.7\n"
        );
        assert!(inject_lib_deps("[env:a]\nboard = x\n", &["a".into()]).is_none());
    }

    #[test]
    fn generic_inis_accept_lib_injection() {
        // Every generic project a custom build can target declares lib_deps.
        for soc in pnex_core::firmware::CHIP_FAMILIES {
            let project = pnex_core::firmware::generic_project(soc);
            let path = format!(
                "{}/../../firmware/{project}/platformio.ini",
                env!("CARGO_MANIFEST_DIR")
            );
            let ini = std::fs::read_to_string(&path).expect("generic ini");
            assert!(
                inject_lib_deps(&ini, &["x/y@1.0.0".into()]).is_some(),
                "{project}"
            );
        }
    }

    #[test]
    fn apply_custom_source_replaces_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/main.cpp"), "old").unwrap();
        std::fs::write(project.join("src/extra.cpp"), "old").unwrap();
        std::fs::write(
            project.join("platformio.ini"),
            "[env:e]\nlib_deps =\n    SPI\n",
        )
        .unwrap();
        let custom = CustomSource {
            main_cpp: "new".into(),
            lib_specs: vec!["a/b@1.0.0".into()],
        };
        apply_custom_source(project, &custom).unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("src/main.cpp")).unwrap(),
            "new"
        );
        assert!(!project.join("src/extra.cpp").exists());
        assert!(std::fs::read_to_string(project.join("platformio.ini"))
            .unwrap()
            .contains("    a/b@1.0.0"));
    }

    #[test]
    fn sandbox_argv_isolates_network_and_pid() {
        let sb = Sandbox::Bwrap {
            program: "bwrap".into(),
            allow_network: false,
        };
        let argv = sandbox_argv(
            &sb,
            &["/home/u/.local/bin/pio".into(), "run".into()],
            Path::new("/tmp/ws/generic_esp32"),
            Path::new("/home/u/.platformio"),
            &[PathBuf::from("/home/u/.local")],
        );
        let joined = argv.join(" ");
        assert_eq!(argv[0], "bwrap");
        assert!(joined.contains("--unshare-net"));
        assert!(joined.contains("--unshare-pid"));
        assert!(joined.contains("--bind /home/u/.platformio /home/u/.platformio"));
        assert!(joined.contains("--ro-bind-try /home/u/.local /home/u/.local"));
        assert!(joined.ends_with("-- /home/u/.local/bin/pio run"));
        // $HOME is never bound as a whole (repo, configs, keys stay hidden).
        assert!(!joined.contains("--bind /home/u /home/u"));
        let open = Sandbox::Bwrap {
            program: "bwrap".into(),
            allow_network: true,
        };
        assert!(!sandbox_argv(
            &open,
            &["pio".into()],
            Path::new("/w"),
            Path::new("/c"),
            &[]
        )
        .contains(&"--unshare-net".to_string()));
    }
}
