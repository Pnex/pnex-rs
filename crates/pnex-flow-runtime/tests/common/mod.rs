//! Common harness for the runtime E2E tests: spawns the real binary
//! (`CARGO_BIN_EXE_pnex-flow-runtime`) + collects the stdout JSON events.

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub struct RuntimeProc {
    pub child: Child,
    lines: Receiver<String>,
}

impl RuntimeProc {
    /// Spawns the headless runtime on a flows.json, stdout parsed line by line
    /// in a dedicated thread (the process never blocks on stdout).
    pub fn spawn(flows_path: &Path, home: &Path) -> RuntimeProc {
        Self::spawn_with_env(flows_path, home, [])
    }

    /// Variant with additional environment variables (e.g. VALKEY_URL).
    pub fn spawn_with_env<const N: usize>(
        flows_path: &Path,
        home: &Path,
        env: [(&str, &str); N],
    ) -> RuntimeProc {
        let bin = env!("CARGO_BIN_EXE_pnex-flow-runtime");
        let mut cmd = Command::new(bin);
        cmd.arg(flows_path).arg("--home").arg(home);
        // Mock servers listen on loopback: no egress filtering in tests.
        cmd.env("PNEX_EGRESS", "open");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pnex-flow-runtime");

        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        RuntimeProc { child, lines: rx }
    }

    /// Waits (with a global timeout) for a line matching the predicate.
    pub fn wait_for(
        &self,
        pred: impl Fn(&serde_json::Value) -> bool,
        timeout: Duration,
    ) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.lines.try_recv() {
                Ok(line) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        if pred(&v) {
                            return v;
                        }
                    }
                }
                Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(25)),
                Err(TryRecvError::Disconnected) => panic!("runtime exited prematurely"),
            }
        }
        panic!("expected event not received after {timeout:?}");
    }
}

impl Drop for RuntimeProc {
    fn drop(&mut self) {
        // SIGINT (clean stop) then SIGKILL as backup.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGINT);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Minimal Node-RED flows.json: tab + inject(0.1 s, text payload) + debug.
/// NB: no `topic` prop — without a value, the EdgeLinkd deserialiser
/// refuses it (`invalid type: null, expected a string`).
pub fn inject_debug_flows(payload: &str) -> String {
    serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "test", "pnex_flow_id": 1 },
        {
            "id": "n1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": payload, "payloadType": "str",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 200, "y": 100, "wires": []
        }
    ])
    .to_string()
}

pub fn tmp_home(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pnex-flow-rt-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn is_debug_with(v: &serde_json::Value, needle: &str) -> bool {
    v.get("event").and_then(|e| e.as_str()) == Some("debug") && v.to_string().contains(needle)
}
