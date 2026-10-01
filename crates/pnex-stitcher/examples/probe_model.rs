//! Sonde ONNX : imprime les signatures exactes (noms, dtypes, formes) des
//! entrées/sorties de superpoint.onnx et du matcher LightGlue — référence
//! pour `pose_model.rs`. Usage :
//! `cargo run --release -p pnex-stitcher --features pose-model \
//!  --example probe_model -- deploy/models`

use ort::session::{builder::GraphOptimizationLevel, Session};

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "deploy/models".into());
    for name in ["superpoint.onnx", "superpoint_lightglue_fused_cpu.onnx"] {
        println!("══ {name}");
        let session = Session::builder()
            .expect("builder")
            .with_optimization_level(GraphOptimizationLevel::Level1)
            .expect("opt")
            .commit_from_file(init_log_path(&dir, name))
            .unwrap_or_else(|e| panic!("{name} : {e}"));
        for i in session.inputs() {
            println!("  in  {} → {}", i.name(), i.dtype());
        }
        for o in session.outputs() {
            println!("  out {} → {}", o.name(), o.dtype());
        }
    }
}

fn init_log_path(dir: &str, name: &str) -> String {
    format!("{dir}/{name}")
}
