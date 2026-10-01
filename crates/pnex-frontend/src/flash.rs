//! Pont vers le glue JS esptool-js (`js/flasher.js`, bundlé esbuild en
//! `assets/flasher.js`) — flash firmware via Web Serial (Chromium uniquement).
//!
//! Le module JS expose deux globales :
//!   - `window.pnexFlashSupported()` → Web Serial dispo ?
//!   - `window.pnexFlash(entries, onEvent)` → promise du flow complet
//!     (requestPort → sync → writeFlash @0x0 → hard reset) ;
//!     `onEvent` reçoit des chaînes JSON décodées ici en `FlashEvent`
//!     (serde_json — pas de dépendance serde-wasm-bindgen).
//!
//! Desktop targets: no Web Serial in the webview — the flash runs the
//! esptool binary shipped with the app (`native` module). Android: stub.

use serde::Deserialize;

use pnex_core::err_codes;

/// Flash failure: machine code (`err_codes::FLASH_*`, resolved to
/// `err-<kebab>` at display time) + verbatim technical detail (raw
/// JS/esptool text — never translated, shown as a secondary line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashError {
    pub code: &'static str,
    pub detail: Option<String>,
}

impl FlashError {
    /// Bridge-level failure (promise rejection, exception) — the raw JS
    /// text becomes the verbatim detail line. wasm32-only callers today:
    /// native builds only exercise the stub (hence the allow).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn from_js(detail: String) -> Self {
        Self {
            code: err_codes::FLASH_UNAVAILABLE,
            detail: Some(detail),
        }
    }
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{closure::Closure, JsCast, JsValue};

/// Événement du flow de flash, sérialisé en JSON par `js/flasher.js`
/// (`{"type":"stage","stage":"write"}`, `{"type":"progress","percent":42}`…).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlashEvent {
    /// Changement d'étape : `connect` | `write` | `reset`.
    Stage {
        stage: String,
    },
    /// Chip détecté au sync (ex. « ESP32-D0WD-V3 ») — affiché tel quel.
    Chip {
        chip: String,
    },
    /// Progression d'écriture, 0-100.
    Progress {
        percent: u8,
    },
    Done,
    Error {
        message: String,
    },
}

/// Web Serial disponible ? Chrome/Edge/Opera uniquement — false sur
/// Firefox/Safari et sur toute cible non-web.
#[cfg(target_arch = "wasm32")]
pub fn supported() -> bool {
    global_function("pnexFlashSupported")
        .and_then(|f| f.call0(&JsValue::NULL).ok())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Flash les `entries` [(adresse, octets)] en un seul writeFlash (firmware
/// de ports natif. Doit être appelé depuis un handler de clic :
/// `requestPort()` exige un geste utilisateur.
#[cfg(target_arch = "wasm32")]
pub async fn flash<F>(entries: Vec<(u32, Vec<u8>)>, mut on_event: F) -> Result<(), FlashError>
where
    F: FnMut(FlashEvent),
{
    let flash_fn = global_function("pnexFlash").ok_or_else(|| FlashError {
        code: err_codes::FLASH_FLASHER_MISSING,
        detail: Some("window.pnexFlash absent".to_string()),
    })?;

    // [{ data: Uint8Array, address: Number }, ...] — un seul writeFlash
    // esptool-js, toutes entrées confondues (aujourd'hui : l'image unique
    // @0x0 ; le multi-entrées reste disponible si besoin).
    let entries_array = js_sys::Array::new();
    for (address, bytes) in entries {
        let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        let entry = js_sys::Object::new();
        js_sys::Reflect::set(&entry, &"data".into(), &array).ok();
        js_sys::Reflect::set(&entry, &"address".into(), &js_sys::Number::from(address)).ok();
        entries_array.push(&entry);
    }

    // Callback d'événements : la Closure reste vivante jusqu'au retour de
    // l'await (locale possédée), le JS n'en garde qu'un emprunt.
    let closure = Closure::wrap(Box::new(move |json: String| {
        if let Ok(event) = serde_json::from_str::<FlashEvent>(&json) {
            on_event(event);
        }
    }) as Box<dyn FnMut(String)>);

    let promise = flash_fn
        .call2(&JsValue::NULL, &entries_array, closure.as_js_value())
        .map_err(|err| FlashError::from_js(js_error_message(&err)))?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| FlashError {
            code: err_codes::FLASH_NOT_A_PROMISE,
            detail: Some("pnexFlash did not return a promise".to_string()),
        })?;

    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|err| FlashError::from_js(js_error_message(&err)))?;
    Ok(())
}

/// Résout une fonction globale exposée par le bundle flasher.js.
#[cfg(target_arch = "wasm32")]
fn global_function(name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
}

/// Message lisible d'une `JsValue` d'erreur (rejet de promise ou exception).
#[cfg(target_arch = "wasm32")]
fn js_error_message(value: &JsValue) -> String {
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        let message = js_sys::Reflect::get(error, &JsValue::from_str("message"))
            .ok()
            .and_then(|m| m.as_string());
        if let Some(message) = message.filter(|m| !m.is_empty()) {
            return message;
        }
    }
    value
        .as_string()
        .unwrap_or_else(|| "unknown flasher error".to_string())
}

/// Android: no serial flashing (no esptool, no Web Serial in the webview).
#[cfg(target_os = "android")]
pub fn supported() -> bool {
    false
}

#[cfg(target_os = "android")]
pub async fn flash<F>(_entries: Vec<(u32, Vec<u8>)>, _on_event: F) -> Result<(), FlashError>
where
    F: FnMut(FlashEvent),
{
    Err(FlashError {
        code: err_codes::FLASH_UNAVAILABLE,
        detail: Some("serial flash unavailable on this platform".to_string()),
    })
}

/// Desktop (Linux/Windows/macOS): the webview has no Web Serial, so the
/// flash is delegated to the esptool binary shipped next to the app.
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
pub fn supported() -> bool {
    native::locate().is_some()
}

/// Desktop flash: writes the entries to temp files, runs
/// `esptool write-flash <addr> <file>...` (chip + port auto-detected,
/// hard reset afterwards — esptool defaults) and maps its stdout to the
/// same `FlashEvent` stream as the JS glue.
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
pub async fn flash<F>(entries: Vec<(u32, Vec<u8>)>, mut on_event: F) -> Result<(), FlashError>
where
    F: FnMut(FlashEvent),
{
    use futures::StreamExt;

    let esptool = native::locate().ok_or_else(|| FlashError {
        code: err_codes::FLASH_FLASHER_MISSING,
        detail: Some("esptool binary not found next to the app".to_string()),
    })?;
    let images = native::TempImages::write(&entries).map_err(|err| FlashError {
        code: err_codes::FLASH_UNAVAILABLE,
        detail: Some(format!("cannot write firmware image: {err}")),
    })?;

    on_event(FlashEvent::Stage {
        stage: "connect".to_string(),
    });
    let mut rx = native::spawn(&esptool, &images.args()).map_err(|err| FlashError {
        code: err_codes::FLASH_UNAVAILABLE,
        detail: Some(format!("cannot start esptool: {err}")),
    })?;

    let mut outcome = Err("esptool exited without status".to_string());
    while let Some(message) = rx.next().await {
        match message {
            native::Message::Event(event) => on_event(event),
            native::Message::Exit(result) => outcome = result,
        }
    }
    drop(images);
    match outcome {
        Ok(()) => {
            on_event(FlashEvent::Done);
            Ok(())
        }
        Err(detail) => Err(FlashError {
            code: err_codes::FLASH_UNAVAILABLE,
            detail: Some(detail),
        }),
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
mod native {
    use super::FlashEvent;
    use futures::channel::mpsc;
    use std::io::{BufRead, BufReader, Read};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    const EXE: &str = if cfg!(windows) {
        "esptool.exe"
    } else {
        "esptool"
    };

    /// Lookup order: `PNEX_ESPTOOL` (explicit override), `<app dir>/esptool/`,
    /// `<app dir>/`, then `PATH` (developer machines with `uv tool install
    /// esptool`).
    pub fn locate() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("PNEX_ESPTOOL").map(PathBuf::from) {
            return path.is_file().then_some(path);
        }
        let app_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let bundled = app_dir
            .into_iter()
            .flat_map(|dir| [dir.join("esptool").join(EXE), dir.join(EXE)]);
        let on_path = std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths)
                    .map(|dir| dir.join(EXE))
                    .collect()
            })
            .unwrap_or_else(Vec::new);
        bundled.chain(on_path).find(|path| path.is_file())
    }

    /// Firmware images written to the temp dir, removed on drop.
    pub struct TempImages(Vec<(u32, PathBuf)>);

    impl TempImages {
        pub fn write(entries: &[(u32, Vec<u8>)]) -> std::io::Result<Self> {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let mut images = Self(Vec::new());
            for (index, (address, bytes)) in entries.iter().enumerate() {
                let path = std::env::temp_dir().join(format!(
                    "pnex-flash-{}-{stamp}-{index}.bin",
                    std::process::id()
                ));
                std::fs::write(&path, bytes)?;
                images.0.push((*address, path));
            }
            Ok(images)
        }

        pub fn args(&self) -> Vec<String> {
            let mut args = vec!["write-flash".to_string()];
            for (address, path) in &self.0 {
                args.push(format!("{address:#x}"));
                args.push(path.to_string_lossy().into_owned());
            }
            args
        }
    }

    impl Drop for TempImages {
        fn drop(&mut self) {
            for (_, path) in &self.0 {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    pub enum Message {
        Event(FlashEvent),
        Exit(Result<(), String>),
    }

    /// Runs esptool on a blocking thread; progress lines are forwarded as
    /// events, then a final `Exit` carries the status (error detail = last
    /// output lines, verbatim).
    pub fn spawn(
        esptool: &Path,
        args: &[String],
    ) -> std::io::Result<mpsc::UnboundedReceiver<Message>> {
        let mut command = Command::new(esptool);
        command
            .args(args)
            // Plain one-line-per-update progress (no ANSI, no in-place redraw).
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashing up from the GUI app.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        let stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let (tx, rx) = mpsc::unbounded();

        std::thread::spawn(move || {
            let stderr_reader = std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(stderr) = stderr.as_mut() {
                    let _ = stderr.read_to_string(&mut text);
                }
                text
            });
            let mut parser = OutputParser::default();
            let mut tail: Vec<String> = Vec::new();
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).split(b'\n') {
                    let Ok(line) = line else { break };
                    let line = String::from_utf8_lossy(&line).trim_end().to_string();
                    for event in parser.feed(&line) {
                        let _ = tx.unbounded_send(Message::Event(event));
                    }
                    if !line.trim().is_empty() {
                        tail.push(line);
                        if tail.len() > 4 {
                            tail.remove(0);
                        }
                    }
                }
            }
            let stderr_text = stderr_reader.join().unwrap_or_default();
            let result = match child.wait() {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => {
                    let stderr_tail: Vec<&str> = stderr_text
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .collect();
                    let detail = if stderr_tail.is_empty() {
                        tail.join("\n")
                    } else {
                        stderr_tail[stderr_tail.len().saturating_sub(4)..].join("\n")
                    };
                    Err(format!("esptool failed ({status}): {detail}"))
                }
                Err(err) => Err(format!("esptool wait failed: {err}")),
            };
            let _ = tx.unbounded_send(Message::Exit(result));
        });
        Ok(rx)
    }

    /// Maps esptool 5.x stdout lines to flash events (stateful: the
    /// `write`/`reset` stages are emitted once).
    #[derive(Default)]
    pub struct OutputParser {
        writing: bool,
        resetting: bool,
        last_percent: Option<u8>,
    }

    impl OutputParser {
        pub fn feed(&mut self, line: &str) -> Vec<FlashEvent> {
            let mut events = Vec::new();
            let trimmed = line.trim();
            if let Some(chip) = trimmed.strip_prefix("Chip type:") {
                events.push(FlashEvent::Chip {
                    chip: chip.trim().to_string(),
                });
            } else if trimmed.starts_with("Writing at") {
                if !self.writing {
                    self.writing = true;
                    events.push(FlashEvent::Stage {
                        stage: "write".to_string(),
                    });
                }
                if let Some(percent) = percent_of(trimmed) {
                    if self.last_percent != Some(percent) {
                        self.last_percent = Some(percent);
                        events.push(FlashEvent::Progress { percent });
                    }
                }
            } else if trimmed.starts_with("Hard resetting") && !self.resetting {
                self.resetting = true;
                events.push(FlashEvent::Stage {
                    stage: "reset".to_string(),
                });
            }
            events
        }
    }

    /// First `NN.N%` figure of a progress line, floored to 0-100.
    fn percent_of(line: &str) -> Option<u8> {
        let end = line.find('%')?;
        let start = line[..end]
            .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
            .map_or(0, |i| i + 1);
        let value: f32 = line[start..end].parse().ok()?;
        Some(value.clamp(0.0, 100.0) as u8)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_esptool_5_output() {
            let mut parser = OutputParser::default();
            assert!(parser
                .feed("Connected to ESP32 on /dev/ttyUSB0:")
                .is_empty());
            assert_eq!(
                parser.feed("Chip type:          ESP32-D0WD-V3 (revision v3.1)"),
                vec![FlashEvent::Chip {
                    chip: "ESP32-D0WD-V3 (revision v3.1)".into()
                }]
            );
            assert_eq!(
                parser.feed("Writing at 0x00010000 [=====>        ]  42.3% 400.0/945.1 kB "),
                vec![
                    FlashEvent::Stage {
                        stage: "write".into()
                    },
                    FlashEvent::Progress { percent: 42 },
                ]
            );
            assert_eq!(
                parser.feed("Writing at 0x00020000 [==========>   ]  88.0% 831.7/945.1 kB "),
                vec![FlashEvent::Progress { percent: 88 }]
            );
            assert_eq!(
                parser.feed("Hard resetting via RTS pin..."),
                vec![FlashEvent::Stage {
                    stage: "reset".into()
                }]
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FlashEvent;

    /// Contrat JSON du glue `js/flasher.js` — toute divergence cassera ces
    /// décodages (le JS n'est pas couvert par les tests Rust).
    #[test]
    fn decode_evenements_flasher_js() {
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"stage","stage":"write"}"#).unwrap(),
            FlashEvent::Stage {
                stage: "write".into()
            }
        );
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"chip","chip":"ESP32-D0WD-V3"}"#)
                .unwrap(),
            FlashEvent::Chip {
                chip: "ESP32-D0WD-V3".into()
            }
        );
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"progress","percent":42}"#).unwrap(),
            FlashEvent::Progress { percent: 42 }
        );
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"done"}"#).unwrap(),
            FlashEvent::Done
        );
        assert!(matches!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"error","message":"No port selected"}"#)
                .unwrap(),
            FlashEvent::Error { .. }
        ));
    }
}
