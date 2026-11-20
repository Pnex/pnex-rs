//! Rebuilds the server when an assistant knowledge card or a pack file
//! changes: both are embedded with `include_dir!`, which cargo does not track
//! by itself (a new pipeline YAML was silently skipped before).

fn main() {
    println!("cargo:rerun-if-changed=assistant-kb");
    println!("cargo:rerun-if-changed=packs");
}
