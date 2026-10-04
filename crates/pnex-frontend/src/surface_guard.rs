//! Invariant "no direct dashboard → device link" (D123, D131,
//! docs/architecture/home-dashboards.md §1 bis): surface editors and views
//! read device metadata (pin labels and modes) but never call the device
//! command, OTA, build or flash paths — a surface writes org controls and
//! only a deployed flow acts on a device.

#![cfg(test)]

use std::path::{Path, PathBuf};

/// Surface sources (directories scanned recursively, or single files).
const SURFACE_PATHS: &[&str] = &[
    "components/surface",
    "components/dashboard_editor",
    "components/annotation_editor",
    "components/home_icons",
    "components/dashboard_live.rs",
    "components/dashboard_live_mobile.rs",
    "components/dashboard_widget.rs",
];

/// Device actuation paths a surface must never reach.
const FORBIDDEN: &[&str] = &[
    "pins::command",
    "api::ota",
    "api::firmware",
    "api::builds",
    "crate::flash",
];

fn rust_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn surfaces_never_reach_a_device() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for p in SURFACE_PATHS {
        let path = src.join(p);
        assert!(path.exists(), "{p} is gone: update SURFACE_PATHS");
        rust_files(&path, &mut files);
    }
    let mut hits = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for bad in FORBIDDEN {
                if code.contains(bad) {
                    hits.push(format!("{}:{} → {bad}", f.display(), n + 1));
                }
            }
        }
    }
    assert!(files.len() > 20, "sterile sweep: {} files", files.len());
    assert!(
        hits.is_empty(),
        "a surface references a device actuation path (only a deployed flow acts on a \
         device, home-dashboards.md §1 bis):\n{}",
        hits.join("\n")
    );
}
