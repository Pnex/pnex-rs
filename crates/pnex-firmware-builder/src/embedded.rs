//! Source firmware embarquée dans le binaire (convergence monorepo).
//!
//! `build.rs` recopie l'arborescence `firmware/` du monorepo (filtrée :
//! jamais `.git`/`.pio`/`.venv`/`build.sh`) dans `OUT_DIR`, et
//! `include_dir!` l'embarque ici — ~430 Ko (le watch `rerun-if-changed`
//! porte sur la racine `firmware/`, donc les projets nouveaux sont
//! ré-embarqués). Le
//! binaire serveur est auto-porteur : sur un Raspi/self-hosted, il build
//! *sa* version du firmware sans clone git ni chemin local. Seule la
//! toolchain (`pio`) reste externe.

use std::fs;
use std::path::Path;

use include_dir::{include_dir, Dir, DirEntry};

static FIRMWARE: Dir<'_> = include_dir!("$OUT_DIR/firmware-embed");

/// Extrait l'arborescence embarquée vers `dst` (workspace tmp du build).
pub fn extract(dst: &Path) -> Result<(), crate::BuildError> {
    if FIRMWARE.entries().is_empty() {
        return Err(crate::BuildError::Source(
            "arborescence embarquée vide — crate compilée sans firmware/ \
             (build.rs du monorepo requis)"
                .into(),
        ));
    }
    extract_dir(&FIRMWARE, dst)
}

fn extract_dir(dir: &Dir<'_>, dst: &Path) -> Result<(), crate::BuildError> {
    let io = |e: std::io::Error| crate::BuildError::Source(format!("extraction embarquée : {e}"));
    fs::create_dir_all(dst).map_err(io)?;
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(sub) => {
                // `file_name` plutôt que `path()` : les chemins include_dir
                // sont relatifs à la racine embarquée, couvre les nouveaux
                // projets, exclut caches/toolchain (cf. build.rs).
                let name = sub.path().file_name().expect("nom de dossier");
                extract_dir(sub, &dst.join(name))?;
            }
            DirEntry::File(file) => {
                let name = file.path().file_name().expect("nom de fichier");
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

    /// L'arborescence embarquée est celle du monorepo : les projets
    /// predefined devices y sont (avec la lib PneX, consommée via
    /// `lib_extra_dirs = ../lib` — son chemin relatif doit résoudre dans
    /// le workspace extrait), les caches/toolchain non.
    #[test]
    fn extraction_preserve_le_layout() {
        let tmp = tempfile::tempdir().expect("tmp");
        extract(tmp.path()).expect("extraction");
        assert!(tmp.path().join("soil_sensor/platformio.ini").is_file());
        assert!(tmp.path().join("generic_esp8266/platformio.ini").is_file());
        assert!(tmp.path().join("lib/pnex/library.json").is_file());
        assert!(tmp.path().join("lib/pnex/src/Pnex.h").is_file());
        assert!(tmp.path().join("lib/pnex/src/pnex_config.h").is_file());
        assert!(tmp.path().join("lib/pnex/src/chacha_crypto.h").is_file());
        assert!(tmp.path().join("common_libs/display").is_dir());
        assert!(!tmp.path().join(".venv").exists());
        assert!(!tmp.path().join("soil_sensor/.pio").exists());
    }
}
