//! Source firmware embarquée dans le binaire (convergence monorepo).
//!
//! `build.rs` recopie l'arborescence `firmware/` du monorepo (filtrée :
//! jamais `.git`/`.pio`/`.venv`/`build.sh`) dans `OUT_DIR`, et
//! `include_dir!` l'embarque ici — ~430 Ko pour 41 fichiers trackés. Le
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
                // sont relatifs à la racine embarquée, on reconstruit depuis
                // du dossier courant.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// L'arborescence embarquée est celle du monorepo : les projets
    /// predefined devices y sont, les caches/toolchain non.
    #[test]
    fn extraction_preserve_le_layout() {
        let tmp = tempfile::tempdir().expect("tmp");
        extract(tmp.path()).expect("extraction");
        assert!(tmp.path().join("soil_sensor/platformio.ini").is_file());
        assert!(tmp.path().join("4_chan_relay/platformio.ini").is_file());
        assert!(tmp.path().join("common_libs/config/config.h").is_file());
        assert!(tmp.path().join("common_libs/crypto/chacha_crypto.h").is_file());
        assert!(!tmp.path().join(".venv").exists());
        assert!(!tmp.path().join("soil_sensor/.pio").exists());
    }
}
