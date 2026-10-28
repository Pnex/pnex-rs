//! Filtered copy of the monorepo `firmware/` tree into `OUT_DIR`,
//! for embedding via `include_dir!` (see `src/embedded.rs`).
//!
//! The filter mirrors `copy_dir_recursive` (pipeline.rs): never
//! `.git`/`.pio`/`.venv`/`node_modules` — on a dev machine these
//! folders add up to hundreds of MB (the pio venv alone ~54 MB) and
//! would bloat the server binary. `build.sh` is excluded too: it is a
//! local convenience file that historically contained secrets.
//!
//! `rerun-if-changed` is emitted on the `firmware/` **root** (cargo recursive
//! scan): a *new* project (e.g. `generic_esp8266`, which cost an
//! hour of diagnosis when it was never re-embedded) triggers the
//! script. Accepted trade-off: local `.pio`/`.venv` churn re-triggers
//! a recompilation — with no effect on the copy (those folders are filtered),
//! only the incremental server build time pays for the churn.

use std::fs;
use std::path::Path;

const SKIP_DIRS: &[&str] = &[".git", ".pio", ".venv", "node_modules"];

fn copy_filtered(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIP_DIRS.contains(&name.as_ref()) || name == "build.sh" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(name.as_ref());
        if from.is_dir() {
            copy_filtered(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let source = Path::new(&manifest).join("../../firmware");
    let target = Path::new(&out).join("firmware-embed");

    // Start from a clean copy on every run (removes files
    // gone from the source tree).
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target).expect("create firmware-embed");

    if source.is_dir() {
        copy_filtered(&source, &target).expect("copy firmware/ to OUT_DIR");
        // Cargo recursive scan on the root: catches *new*
        // projects/files (per-file alone left the embedding stale).
        println!("cargo:rerun-if-changed={}", source.display());
        // Content-based trigger for cached target/ dirs (Docker cache
        // mount): mtimes are not reliable there, the image build exports a
        // hash of firmware/ instead (deploy/docker/Dockerfile).
        println!("cargo:rerun-if-env-changed=PNEX_FIRMWARE_TREE_HASH");
    } else {
        // Crate compiled outside the monorepo: empty tree — the clear error
        // is raised at runtime (embedded.rs), not at compile time.
        println!("cargo:warning=firmware/ tree not found — embedded source empty");
    }
}
