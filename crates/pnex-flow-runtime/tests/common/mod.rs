//! Harnais commun des tests E2E du runtime : spawn du vrai binaire
//! (`CARGO_BIN_EXE_pnex-flow-runtime`) + collecte des événements JSON stdout.

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
    /// Spawn le runtime headless sur un flows.json, stdout parsé ligne à ligne
    /// dans un thread dédié (le process ne se bloque jamais sur stdout).
    pub fn spawn(flows_path: &Path, home: &Path) -> RuntimeProc {
        Self::spawn_with_env(flows_path, home, [])
    }

    /// Variante avec variables d'environnement additionnelles (ex. DATABASE_URL).
    pub fn spawn_with_env<const N: usize>(
        flows_path: &Path,
        home: &Path,
        env: [(&str, &str); N],
    ) -> RuntimeProc {
        let bin = env!("CARGO_BIN_EXE_pnex-flow-runtime");
        let mut cmd = Command::new(bin);
        cmd.arg(flows_path).arg("--home").arg(home);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pnex-flow-runtime");

        let stdout = child.stdout.take().expect("stdout pipé");
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

    /// Attend la fin du process avec un code non nul (graphe rejeté).
    pub fn wait_for_exit_failure(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(!status.success(), "le runtime devait échouer, code : {status}");
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("try_wait : {e}"),
            }
        }
        panic!("le runtime ne s'est pas arrêté après {timeout:?}");
    }

    /// Attend (avec délai global) qu'une ligne satisfasse le prédicat.
    pub fn wait_for(&self, pred: impl Fn(&serde_json::Value) -> bool, timeout: Duration) -> serde_json::Value {
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
                Err(TryRecvError::Disconnected) => panic!("runtime terminé prématurément"),
            }
        }
        panic!("événement attendu non reçu après {timeout:?}");
    }
}

impl Drop for RuntimeProc {
    fn drop(&mut self) {
        // SIGINT (arrêt propre) puis SIGKILL en secours.
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

/// flows.json Node-RED minimal : tab + inject(0.1 s, payload texte) + debug.
/// NB : pas de prop `topic` — sans valeur, le désérialiseur EdgeLinkd la
/// refuse (`invalid type: null, expected a string`).
pub fn inject_debug_flows(payload: &str) -> String {
    serde_json::json!([
        { "id": "t", "type": "tab", "label": "test" },
        {
            "id": "n1", "type": "inject", "z": "t",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": payload, "payloadType": "str",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "debug", "z": "t",
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
    v.get("event").and_then(|e| e.as_str()) == Some("debug")
        && v.to_string().contains(needle)
}
