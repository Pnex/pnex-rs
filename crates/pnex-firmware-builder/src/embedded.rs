//! Firmware source embedded in the binary (monorepo convergence).
//!
//! `build.rs` copies the monorepo `firmware/` tree (filtered:
//! never `.git`/`.pio`/`.venv`/`build.sh`) into `OUT_DIR`, and
//! `include_dir!` embeds it here — ~430 KB (the `rerun-if-changed` watch
//! covers the `firmware/` root, so new projects are
//! re-embedded). The
//! server binary is self-contained: on a Raspi/self-hosted, it builds
//! *its* firmware version with no git clone or local path. Only the
//! toolchain (`pio`) stays external.

use std::fs;
use std::path::Path;

use include_dir::{include_dir, Dir, DirEntry};

static FIRMWARE: Dir<'_> = include_dir!("$OUT_DIR/firmware-embed");

/// Extracts the embedded tree to `dst` (tmp workspace of the build).
pub fn extract(dst: &Path) -> Result<(), crate::BuildError> {
    if FIRMWARE.entries().is_empty() {
        return Err(crate::BuildError::Source(
            "embedded tree empty — crate compiled without firmware/ \
             (monorepo build.rs required)"
                .into(),
        ));
    }
    extract_dir(&FIRMWARE, dst)
}

fn extract_dir(dir: &Dir<'_>, dst: &Path) -> Result<(), crate::BuildError> {
    let io = |e: std::io::Error| crate::BuildError::Source(format!("embedded extraction: {e}"));
    fs::create_dir_all(dst).map_err(io)?;
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(sub) => {
                // `file_name` rather than `path()`: include_dir paths
                // are relative to the embedded root, covers new
                // projects, excludes caches/toolchain (see build.rs).
                let name = sub.path().file_name().expect("folder name");
                extract_dir(sub, &dst.join(name))?;
            }
            DirEntry::File(file) => {
                let name = file.path().file_name().expect("file name");
                fs::write(dst.join(name), file.contents()).map_err(io)?;
            }
        }
    }
    Ok(())
}

/// Fingerprint of the embedded firmware tree — stable for a given server
/// binary, changes whenever the binary is rebuilt with different firmware
/// sources. Stamped on build_records at build time; a mismatch at read time
/// flags the build stale ("the server binary embedding the firmware sources
/// has changed since this build").
pub fn source_fingerprint() -> &'static str {
    static FP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FP.get_or_init(|| {
        use sha2::{Digest, Sha256};
        let mut files: Vec<_> = FIRMWARE.files().collect();
        files.sort_by_key(|f| f.path().as_os_str());
        let mut h = Sha256::new();
        for f in files {
            h.update(f.path().to_string_lossy().as_bytes());
            h.update([0u8]);
            h.update(f.contents());
        }
        let digest = h.finalize();
        // 16 hex chars — collision risk negligible for a staleness tripwire.
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()[..16]
            .to_string()
    })
    .as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded tree is the monorepo's: the predefined devices
    /// projects are in it (with the PneX lib, consumed via
    /// `lib_extra_dirs = ../lib` — its relative path must resolve in
    /// the extracted workspace), the caches/toolchain are not.
    #[test]
    fn extraction_preserves_the_layout() {
        let tmp = tempfile::tempdir().expect("tmp");
        extract(tmp.path()).expect("extraction");
        assert!(tmp.path().join("generic_esp8266/platformio.ini").is_file());
        assert!(tmp.path().join("lib/pnex/library.json").is_file());
        assert!(tmp.path().join("lib/pnex/src/Pnex.h").is_file());
        assert!(tmp.path().join("lib/pnex/src/pnex_config.h").is_file());
        assert!(tmp.path().join("lib/pnex/src/pnex_crypto.h").is_file());
        assert!(tmp.path().join("lib/pnex/src/pnex_noise.cpp").is_file());
        assert!(tmp
            .path()
            .join("lib/pnex/src/monocypher/monocypher.c")
            .is_file());
        assert!(tmp.path().join("common_libs/pnex-core-cpp").is_dir());
        assert!(!tmp.path().join(".venv").exists());
        assert!(!tmp.path().join("generic_esp8266/.pio").exists());
    }
}
