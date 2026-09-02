//! Copie filtrée de l'arborescence `firmware/` du monorepo vers `OUT_DIR`,
//! pour embarquement via `include_dir!` (cf. `src/embedded.rs`).
//!
//! Le filtre reproduit `copy_dir_recursive` (pipeline.rs) : jamais
//! `.git`/`.pio`/`.venv`/`node_modules` — sur une machine de dev ces
//! dossiers totalisent des centaines de Mo (le venv pio seul ~54 Mo) et
//! gonfleraient le binaire serveur. `build.sh` est exclu aussi : c'est un
//! fichier local de convenience qui a historiquement contenu des secrets.
//!
//! `rerun-if-changed` est émis sur la **racine** `firmware/` (scan récursif
//! cargo) : un projet *nouveau* (ex. `generic_esp8266`, qui a coûté une
//! heure de diagnostic quand il n'était jamais ré-embarqué) déclenche le
//! script. Contrepartie assumée : le churn `.pio`/`.venv` local re-déclenche
//! une recompilation — sans effet sur la copie (ces dossiers sont filtrés),
//! seul le temps de build incremental serveur paie le churn.

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

    // Repartir d'une copie propre à chaque exécution (retrait des fichiers
    // disparus de l'arborescence source).
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target).expect("créer firmware-embed");

    if source.is_dir() {
        copy_filtered(&source, &target).expect("copier firmware/ vers OUT_DIR");
        // Scan récursif cargo sur la racine : capte les projets/fichiers
        // *nouveaux* (le per-fichier seul laissait l'embarquement périmé).
        println!("cargo:rerun-if-changed={}", source.display());
    } else {
        // Crate compilée hors monorepo : arborescence vide — l'erreur claire
        // est levée à l'exécution (embedded.rs), pas à la compilation.
        println!("cargo:warning=arborescence firmware/ introuvable — source embarquée vide");
    }
}
