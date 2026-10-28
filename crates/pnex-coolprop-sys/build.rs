//! Compiles the CoolProp C++ library (pinned to tag v8.0.0, submodule
//! `vendor/CoolProp`) as a static library and links it into the crate.
//!
//! If the submodule is not initialised (fresh checkout without
//! `git submodule update --init`), it is cloned on the fly from the official
//! repository at the pinned tag so that `cargo build` stays self-sufficient.
//!
//! Origin: adapted from the coolprop-rs project (MIT, same author).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const COOLPROP_TAG: &str = "v8.0.0";
const COOLPROP_REPO: &str = "https://github.com/CoolProp/CoolProp.git";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("crate must live at <root>/crates/pnex-coolprop-sys")
        .to_path_buf();
    let vendor = workspace_root.join("vendor").join("CoolProp");
    let patch = workspace_root
        .join("vendor")
        .join("patches")
        .join("coolprop")
        .join("exception-guards.patch");

    ensure_vendored(&vendor);
    apply_patches(&patch, &vendor);

    println!("cargo:rerun-if-changed={}", patch.display());
    println!("cargo:rerun-if-changed={}", vendor.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("include").join("CoolPropLib.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("CMakeLists.txt").display()
    );

    let mut cfg = cmake::Config::new(&vendor);
    cfg.define("COOLPROP_STATIC_LIBRARY", "ON")
        .define("COOLPROP_RELEASE", "ON")
        .define("COOLPROP_NO_EXAMPLES", "ON")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        // The static-library target does not set this define itself (only the
        // shared build does), but it is what makes EXPORT_CODE
        // `extern "C"` in CoolPropLib.h — without it every symbol is
        // C++-mangled and impossible to link from Rust.
        .define("CMAKE_CXX_FLAGS", "-DCOOLPROP_LIB");
    // Linux cross-compile (CI: pnex-flow-runtime is checked aarch64/armv7):
    // cmake must know the target and the cross compiler, otherwise it tests
    // the host compiler and refuses. The C cross compilers (gcc-*-linux) were already
    // installed for ring/rquickjs; here the C++ cross compilers are needed.
    if let Some(processor) = cross_processor() {
        cfg.define("CMAKE_SYSTEM_NAME", "Linux")
            .define("CMAKE_SYSTEM_PROCESSOR", processor)
            .define("CMAKE_CXX_COMPILER", cross_cxx())
            // CoolProp's CMakeLists appends `-m${BITNESS}` (-m64/-m32) to the
            // compile flags — x86-only flags that the aarch64/armv7 g++
            // reject. BITNESS "NATIVE" skips that addition (gcc already
            // defaults to the target's natural bitness).
            .define("FORCE_BITNESS_NATIVE", "ON");
    }

    let dst = cfg.build();

    let lib_dir = find_library_dir(&dst).unwrap_or_else(|| {
        panic!(
            "could not locate libCoolProp.a under {} — check the CMake output above",
            dst.display()
        )
    });

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=CoolProp");
    println!("cargo:rustc-link-lib=stdc++");
    println!("cargo:rustc-link-lib=m");
}

fn ensure_vendored(vendor: &Path) {
    if vendor.join("CMakeLists.txt").exists() {
        return;
    }
    if vendor.exists() {
        std::fs::remove_dir_all(vendor)
            .expect("failed to remove incomplete vendor/CoolProp directory");
    }
    // Progress notes go to the build-script log (`cargo -vv`), not to
    // `cargo:warning`: they are expected steps, not something to act on.
    eprintln!(
        "cloning CoolProp {COOLPROP_TAG} into {} (one-time, ~2 min)",
        vendor.display()
    );
    let status = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            COOLPROP_TAG,
            COOLPROP_REPO,
        ])
        .arg(vendor)
        .status()
        .expect("failed to spawn `git` to clone CoolProp");
    if !status.success() {
        panic!(
            "git clone of CoolProp {COOLPROP_TAG} failed; initialize the submodule instead:\n\
             git submodule update --init vendor/CoolProp"
        );
    }
}

/// Applies the ported patch (see `vendor/patches/coolprop/`): the deprecated
/// upstream KSI wrappers (`Props1`, `PropsS`, `cair_sat`) do not catch
/// C++ exceptions; an exception crossing the `extern "C"` boundary
/// is UB for the FFI caller. The patch adds the same try/catch as the
/// other exports. Idempotent: `git apply --check` fails once applied.
fn apply_patches(patch: &Path, vendor: &Path) {
    if !patch.exists() {
        panic!(
            "missing required patch {} — restore it from the repository",
            patch.display()
        );
    }
    let patch_arg = patch.to_string_lossy().into_owned();
    if vendor.join(".git").exists() {
        let check = Command::new("git")
            .args([
                "-C",
                vendor.to_str().unwrap(),
                "apply",
                "--check",
                &patch_arg,
            ])
            .status()
            .expect("failed to spawn `git` for patch check");
        if check.success() {
            let status = Command::new("git")
                .args(["-C", vendor.to_str().unwrap(), "apply", &patch_arg])
                .status()
                .expect("failed to spawn `git` for patch apply");
            assert!(status.success(), "git apply of {} failed", patch.display());
            eprintln!("applied exception-guard patch to vendored CoolProp");
        }
        return;
    }
    // Checkout without .git (tarball): `patch(1)` as fallback, --forward to
    // stay idempotent.
    let status = Command::new("patch")
        .args(["-p1", "--forward", "-d"])
        .arg(vendor)
        .arg(&patch_arg)
        .status()
        .expect("failed to spawn `patch` to apply the exception-guard patch");
    assert!(status.success(), "patch -p1 of {} failed", patch.display());
    eprintln!("applied exception-guard patch to vendored CoolProp (patch(1))");
}

/// cmake processor of the target when cross-compiling Linux
/// (aarch64 / armv7 — the CI's Raspberry Pi targets), otherwise `None`
/// (host build: cmake auto-detects).
fn cross_processor() -> Option<&'static str> {
    let target = env::var("TARGET").unwrap_or_default();
    if target.starts_with("aarch64") {
        Some("aarch64")
    } else if target.starts_with("armv7") {
        Some("arm")
    } else {
        None
    }
}

/// C++ cross compiler matching the target (Debian packages
/// `g++-aarch64-linux-gnu` / `g++-arm-linux-gnueabihf`).
fn cross_cxx() -> &'static str {
    let target = env::var("TARGET").unwrap_or_default();
    if target.starts_with("aarch64") {
        "aarch64-linux-gnu-g++"
    } else {
        "arm-linux-gnueabihf-g++"
    }
}

/// Locates the directory containing libCoolProp.a (or CoolProp.lib on Windows).
fn find_library_dir(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path.file_name()?.to_string_lossy().to_string();
                if name == "libCoolProp.a" || name == "CoolProp.lib" {
                    return path.parent().map(|p| p.to_path_buf());
                }
            }
        }
    }
    None
}
