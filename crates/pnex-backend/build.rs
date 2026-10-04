//! Rebuilds the server when an assistant knowledge card changes: the cards
//! are embedded with `include_dir!`, which cargo does not track by itself.

fn main() {
    println!("cargo:rerun-if-changed=assistant-kb");
}
