//! Compile la bibliothèque C++ CoolProp (épinglée au tag v8.0.0, submodule
//! `vendor/CoolProp`) en bibliothèque statique et la lie au crate.
//!
//! Si le submodule n'est pas initialisé (checkout frais sans
//! `git submodule update --init`), il est cloné à la une depuis le dépôt
//! officiel au tag épinglé pour que `cargo build` reste autosuffisant.
//!
//! Origine : adapté du projet coolprop-rs (MIT, même auteur).

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
        // La cible static-library ne pose pas ce define elle-même (seul le
        // build shared le fait), mais c'est lui qui rend EXPORT_CODE
        // `extern "C"` dans CoolPropLib.h — sans lui chaque symbole est
        // manglé C++ et impossible à lier depuis Rust.
        .define("CMAKE_CXX_FLAGS", "-DCOOLPROP_LIB");
    // Cross-compil Linux (CI : pnex-flow-runtime est vérifié aarch64/armv7) :
    // cmake doit connaître la cible et le compilateur croisé, sinon il teste
    // le compilateur hôte et refuse. Les C croisés (gcc-*-linux) étaient déjà
    // installés pour ring/rquickjs ; il faut ici les C++ croisés.
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

/// Applique le patch porté (voir `vendor/patches/coolprop/`) : les wrappers
/// KSI dépréciés d'amont (`Props1`, `PropsS`, `cair_sat`) ne rattrapent pas
/// les exceptions C++ ; une exception qui traverse la frontière `extern "C"`
/// est UB pour l'appelant FFI. Le patch ajoute le même try/catch que les
/// autres exports. Idempotent : `git apply --check` échoue une fois appliqué.
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
    // Checkout sans .git (tarball) : `patch(1)` en repli, --forward pour
    // rester idempotent.
    let status = Command::new("patch")
        .args(["-p1", "--forward", "-d"])
        .arg(vendor)
        .arg(&patch_arg)
        .status()
        .expect("failed to spawn `patch` to apply the exception-guard patch");
    assert!(status.success(), "patch -p1 of {} failed", patch.display());
    eprintln!("applied exception-guard patch to vendored CoolProp (patch(1))");
}

/// Processeur cmake de la cible si l'on cross-compile Linux
/// (aarch64 / armv7 — cibles Raspberry Pi de la CI), sinon `None`
/// (build hôte : cmake auto-détecte).
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

/// Compilateur C++ croisé correspondant à la cible (paquets Debian
/// `g++-aarch64-linux-gnu` / `g++-arm-linux-gnueabihf`).
fn cross_cxx() -> &'static str {
    let target = env::var("TARGET").unwrap_or_default();
    if target.starts_with("aarch64") {
        "aarch64-linux-gnu-g++"
    } else {
        "arm-linux-gnueabihf-g++"
    }
}

/// Localise le répertoire contenant libCoolProp.a (ou CoolProp.lib sous Windows).
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
