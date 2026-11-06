//! The artifact transfer scripts, run for real by `sh` with `curl`, `tar` and `sha256sum`
//! against a local server standing for PneX (D729, D744). No cluster needed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use pnex_runner::artifacts::{IN_SCRIPT, OUT_SCRIPT};
use sha2::{Digest, Sha256};

const TOKEN: &str = "pnart_test-token-value";

#[derive(Default)]
struct Server {
    inputs: HashMap<String, Vec<u8>>,
    /// name → (form header, body)
    outputs: HashMap<String, (String, Vec<u8>)>,
    /// (name, key) → archive
    caches: HashMap<(String, String), Vec<u8>>,
}

type Shared = Arc<Mutex<Server>>;

fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
}

async fn serve(server: Shared) -> String {
    let app =
        axum::Router::new()
            .route(
                "/inputs/{name}",
                get(
                    |State(s): State<Shared>,
                     headers: HeaderMap,
                     UrlPath(name): UrlPath<String>| async move {
                        if !authorized(&headers) {
                            return Err(StatusCode::UNAUTHORIZED);
                        }
                        s.lock()
                            .unwrap()
                            .inputs
                            .get(&name)
                            .cloned()
                            .ok_or(StatusCode::NOT_FOUND)
                    },
                ),
            )
            .route(
                "/outputs/{name}",
                put(
                    |State(s): State<Shared>,
                     headers: HeaderMap,
                     UrlPath(name): UrlPath<String>,
                     body: Bytes| async move {
                        if !authorized(&headers) {
                            return StatusCode::UNAUTHORIZED;
                        }
                        let form = headers
                            .get("x-pnex-artifact-form")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        s.lock()
                            .unwrap()
                            .outputs
                            .insert(name, (form, body.to_vec()));
                        StatusCode::NO_CONTENT
                    },
                ),
            )
            .route(
            "/caches/{name}",
            get(
                |State(s): State<Shared>,
                 headers: HeaderMap,
                 UrlPath(name): UrlPath<String>,
                 axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| async move {
                    if !authorized(&headers) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    let key = q.get("key").cloned().unwrap_or_default();
                    let s = s.lock().unwrap();
                    match s.caches.get(&(name.clone(), key)) {
                        Some(a) => Ok(([("x-pnex-cache-exact", "true")], a.clone())),
                        None => s
                            .caches
                            .iter()
                            .find(|((n, _), _)| n == &name)
                            .map(|(_, a)| ([("x-pnex-cache-exact", "false")], a.clone()))
                            .ok_or(StatusCode::NOT_FOUND),
                    }
                },
            )
            .put(
                |State(s): State<Shared>,
                 headers: HeaderMap,
                 UrlPath(name): UrlPath<String>,
                 axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
                 body: Bytes| async move {
                    if !authorized(&headers) {
                        return StatusCode::UNAUTHORIZED;
                    }
                    let key = q.get("key").cloned().unwrap_or_default();
                    s.lock().unwrap().caches.insert((name, key), body.to_vec());
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(server);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn script(root: &Path, url: &str, list: &str, text: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(text)
        .env("PNEX_ARTIFACTS_ROOT", root)
        .env("PNEX_ARTIFACTS_URL", url)
        .env("PNEX_ARTIFACTS_LIST", list)
        .env("PNEX_ARTIFACT_TOKEN", TOKEN);
    cmd
}

/// Names in a tar archive, read back with `tar -t`.
fn tar_names(archive: &[u8], dir: &Path) -> Vec<String> {
    let path = dir.join("listed.tar");
    std::fs::write(&path, archive).unwrap();
    let out = std::process::Command::new("tar")
        .arg("-tf")
        .arg(&path)
        .output()
        .unwrap();
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_start_matches("./").to_string())
        .filter(|l| !l.is_empty())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn inputs_land_checked_and_a_wrong_checksum_stops_everything() {
    let work = tempfile::tempdir().unwrap();
    // A folder input, as an earlier run sent it: a tar archive.
    let src = work.path().join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("sub/a.txt"), "alpha").unwrap();
    let archive = std::process::Command::new("tar")
        .arg("-cf")
        .arg("-")
        .arg("-C")
        .arg(&src)
        .arg(".")
        .output()
        .unwrap()
        .stdout;
    let notes = b"take care of the flaky test".to_vec();
    let server: Shared = Default::default();
    {
        let mut s = server.lock().unwrap();
        s.inputs.insert("notes".into(), notes.clone());
        s.inputs.insert("tree".into(), archive.clone());
    }
    let url = serve(server.clone()).await;
    let root = work.path().join("artifacts");
    let list = format!("notes,{},file tree,{},tar", hex(&notes), hex(&archive));
    let out = script(&root, &url, &list, IN_SCRIPT)
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(root.join("in/notes")).unwrap(), notes);
    assert_eq!(
        std::fs::read_to_string(root.join("in/tree/sub/a.txt")).unwrap(),
        "alpha"
    );

    // A changed input never reaches the agent: the step fails, the file is not placed.
    let root = work.path().join("again");
    let list = format!("notes,{},file", hex(b"something else"));
    let out = script(&root, &url, &list, IN_SCRIPT)
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("checksum mismatch"));
    assert!(!root.join("in/notes").exists());

    // Without the token, nothing comes.
    let mut cmd = script(&root, &url, "notes,x,file", IN_SCRIPT);
    cmd.env("PNEX_ARTIFACT_TOKEN", "wrong");
    assert!(!cmd.output().await.unwrap().status.success());
}

#[tokio::test]
async fn outputs_go_at_the_end_and_links_are_never_followed() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("artifacts");
    let out_dir = root.join("out");
    std::fs::create_dir_all(out_dir.join("tree/deep")).unwrap();
    std::fs::write(out_dir.join("report"), "all green").unwrap();
    std::fs::write(out_dir.join("tree/deep/b.txt"), "beta").unwrap();
    let outside = work.path().join("host-secret");
    std::fs::write(&outside, "never leaves").unwrap();
    std::os::unix::fs::symlink(&outside, out_dir.join("tree/leak")).unwrap();
    std::os::unix::fs::symlink(&outside, out_dir.join("link")).unwrap();

    let server: Shared = Default::default();
    let url = serve(server.clone()).await;
    let child = script(&root, &url, "report tree link absent", OUT_SCRIPT)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        server.lock().unwrap().outputs.is_empty(),
        "nothing before the end of the agent"
    );
    let pid = child.id().unwrap().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(killed.success());
    let done = tokio::time::timeout(std::time::Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("the collector ends after sending")
        .unwrap();
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );

    let s = server.lock().unwrap();
    assert_eq!(
        s.outputs["report"],
        ("file".to_string(), b"all green".to_vec())
    );
    let (form, tar) = &s.outputs["tree"];
    assert_eq!(form, "tar");
    assert_eq!(
        tar_names(tar, work.path()),
        ["deep/b.txt"],
        "regular files only"
    );
    assert!(!s.outputs.contains_key("link"), "a link is skipped");
    assert!(!s.outputs.contains_key("absent"));
    assert!(String::from_utf8_lossy(&done.stderr).contains("output link: a link, skipped"));
}

#[tokio::test]
async fn a_cache_is_keyed_by_its_lock_files_restored_then_saved_once_changed() {
    let work = tempfile::tempdir().unwrap();
    let workspace = work.path().join("workspace");
    std::fs::create_dir_all(workspace.join("app")).unwrap();
    std::fs::write(workspace.join("app/Cargo.lock"), "lock v1").unwrap();
    let server: Shared = Default::default();
    let url = serve(server.clone()).await;
    let run = |root: std::path::PathBuf, caches: std::path::PathBuf| {
        let mut cmd = script(&root, &url, "", IN_SCRIPT);
        cmd.env("PNEX_CACHES_LIST", "cargo,app/Cargo.lock")
            .env("PNEX_WORKSPACE", &workspace)
            .env("PNEX_CACHES_ROOT", caches);
        cmd
    };
    let collect = |root: std::path::PathBuf, caches: std::path::PathBuf| {
        let mut cmd = script(&root, &url, "", OUT_SCRIPT);
        cmd.env("PNEX_CACHES_LIST", "cargo,app/Cargo.lock")
            .env("PNEX_CACHES_ROOT", caches)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd
    };
    async fn end(mut cmd: tokio::process::Command) -> std::process::Output {
        let child = cmd.spawn().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let pid = child.id().unwrap().to_string();
        std::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), child.wait_with_output())
            .await
            .unwrap()
            .unwrap()
    }

    // First run: nothing to restore; what the agent left is saved under the key.
    let (root, caches) = (work.path().join("a1"), work.path().join("c1"));
    let out = run(root.clone(), caches.clone()).output().await.unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("cache cargo: none yet"));
    std::fs::write(caches.join("cargo/registry.idx"), "deps").unwrap();
    let done = end(collect(root, caches)).await;
    assert!(String::from_utf8_lossy(&done.stdout).contains("cache cargo: saved"));
    let first_key = {
        let s = server.lock().unwrap();
        assert_eq!(s.caches.len(), 1);
        s.caches.keys().next().unwrap().1.clone()
    };
    assert_ne!(first_key, "none", "keyed by the lock file");

    // Same lock file: restored as is, and not sent again.
    let (root, caches) = (work.path().join("a2"), work.path().join("c2"));
    let out = run(root.clone(), caches.clone()).output().await.unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("restored, same key"));
    assert_eq!(
        std::fs::read_to_string(caches.join("cargo/registry.idx")).unwrap(),
        "deps"
    );
    let done = end(collect(root, caches)).await;
    assert!(String::from_utf8_lossy(&done.stdout).contains("unchanged key, not sent"));

    // A changed lock file: the older archive starts the run, the new key is saved.
    std::fs::write(workspace.join("app/Cargo.lock"), "lock v2").unwrap();
    let (root, caches) = (work.path().join("a3"), work.path().join("c3"));
    let out = run(root.clone(), caches.clone()).output().await.unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("restored from an older key"));
    end(collect(root, caches)).await;
    assert_eq!(server.lock().unwrap().caches.len(), 2);
}
