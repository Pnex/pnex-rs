//! Copie filtrée de l'arborescence `firmware/` du monorepo vers `OUT_DIR`,
//! pour embarquement via `include_dir!` (cf. `src/embedded.rs`).
//!
//! Le filtre reproduit `copy_dir_recursive` (pipeline.rs) : jamais
//! `.git`/`.pio`/`.venv`/`node_modules` — sur une machine de dev ces
//! dossiers totalisent des centaines de Mo (le venv pio seul ~54 Mo) et
//! gonfleraient le binaire serveur. `build.sh` est exclu aussi : c'est un
//! fichier local de convenience qui a historiquement contenu des secrets.
//!
//! `rerun-if-changed` est émis **par fichier** de la copie filtrée (pas sur
//! le dossier entier) : les allers-retours de `.pio`/`.venv` ne déclenchent
//! pas de recompilation du serveur. Contrepartie assumée : un fichier
//! *nouveau* dans firmware/ ne re-déclenche pas le script (toucher un
//! fichier existant ou `cargo clean -p pnex-firmware-builder` suffit).

use std::fs;
use std::path::Path;

const SKIP_DIRS: &[&str] = &[".git", ".pio", ".venv", "node_modules"];

fn copy_filtered(src: &Path, dst: &Path, tracked: &mut Vec<String>) -> std::io::Result<()> {
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
            copy_filtered(&from, &to, tracked)?;
        } else {
            fs::copy(&from, &to)?;
            tracked.push(from.display().to_string());
        }
    }
    Ok(())
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let source = Path::new(&manifest).join("../../firmware");
    let target = Path::new(&out).join("firmware-embed");

    // Repartir d'une copie propre à chaque exécution (retrait des fichiers
    // disparus de l'arborescence source).
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target).expect("créer firmware-embed");

    let mut tracked = Vec::new();
    if source.is_dir() {
        copy_filtered(&source, &target, &mut tracked).expect("copier firmware/ vers OUT_DIR");
    } else {
        // Crate compilée hors monorepo : arborescence vide — l'erreur claire
        // est levée à l'exécution (embedded.rs), pas à la compilation.
        println!("cargo:warning=arborescence firmware/ introuvable — source embarquée vide");
    }
    for file in tracked {
        println!("cargo:rerun-if-changed={file}");
    }
}
