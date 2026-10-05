//! Custom firmware (custom-firmware.md D87–D94) — shared contract between
//! the backend (storage, build) and the frontend IDE:
//!
//! - [`generic_project`]: the server-built PlatformIO project a custom sketch
//!   replaces the `main.cpp` of (board, secrets and PneX lib unchanged);
//! - [`LIB_CATALOG`]: pinned library catalog (D92) — the only libraries a
//!   custom sketch may pull, all preloaded in the build worker for offline;
//! - [`check_main_cpp`]: lexical guard on the sketch (D91, complement of the
//!   build sandbox — never a replacement);
//! - [`parse_gcc_diagnostics`]: compiler output → per-line diagnostics (D90);
//! - API DTOs (projects, revisions, builds).

use serde::{Deserialize, Serialize};

use crate::caps::Soc;

/// Starter sketch of a new project — the library example, single source.
pub const STARTER_SKETCH: &str =
    include_str!("../../../firmware/lib/pnex/examples/CustomMetrics/src/main.cpp");

/// Defensive cap on the sketch size (bytes).
pub const MAX_MAIN_CPP_BYTES: usize = 256 * 1024;

/// Server-built generic project of a SoC — a custom build copies it and
/// swaps its `src/main.cpp` for the revision's.
pub fn generic_project(soc: Soc) -> &'static str {
    match soc {
        Soc::Esp8266 => "generic_esp8266",
        Soc::Esp32 => "generic_esp32",
        Soc::Esp32C3 => "generic_esp32c3",
        Soc::Esp32C6 => "generic_esp32c6",
        Soc::Esp32S3 => "generic_esp32s3",
    }
}

/// Every chip family a project can target (wire ids = [`Soc::name`]).
pub const CHIP_FAMILIES: [Soc; 5] = [
    Soc::Esp32,
    Soc::Esp32C3,
    Soc::Esp32C6,
    Soc::Esp32S3,
    Soc::Esp8266,
];

// ───────────────────────────── Library catalog ─────────────────────────────

/// One pinned library of the catalog (D92).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibEntry {
    /// Stable catalog id (API + revisions) — also the i18n key suffix
    /// `fw-lib-<id>` of its description.
    pub id: &'static str,
    /// Display name (library name, not translated).
    pub name: &'static str,
    /// PlatformIO `lib_deps` spec, exact version.
    pub pio_spec: &'static str,
    /// Main header to include.
    pub header: &'static str,
    /// Compatible SoCs — empty = all.
    pub socs: &'static [Soc],
}

impl LibEntry {
    /// Compatible with this SoC?
    pub fn supports(&self, soc: Soc) -> bool {
        self.socs.is_empty() || self.socs.contains(&soc)
    }
}

const ESP32_ONLY: &[Soc] = &[Soc::Esp32, Soc::Esp32C3, Soc::Esp32S3];

/// Pinned library catalog (D92). Transport clients (MQTT, cloud HTTP…) are
/// deliberately absent: the transport is PneX. Adding an entry = one line
/// here + `fw-lib-<id>` in both locale files + the worker prefetch list
/// (`deploy/docker/prewarm/lib_catalog.txt`, see [`prewarm_lib_list`]).
pub const LIB_CATALOG: &[LibEntry] = &[
    LibEntry {
        id: "bme280",
        name: "Adafruit BME280",
        pio_spec: "adafruit/Adafruit BME280 Library@2.3.0",
        header: "Adafruit_BME280.h",
        socs: &[],
    },
    LibEntry {
        id: "bmp280",
        name: "Adafruit BMP280",
        pio_spec: "adafruit/Adafruit BMP280 Library@3.0.0",
        header: "Adafruit_BMP280.h",
        socs: &[],
    },
    LibEntry {
        id: "sht",
        name: "Sensirion SHT",
        pio_spec: "sensirion/arduino-sht@1.2.6",
        header: "SHTSensor.h",
        socs: &[],
    },
    LibEntry {
        id: "aht20",
        name: "Adafruit AHTX0",
        pio_spec: "adafruit/Adafruit AHTX0@2.0.6",
        header: "Adafruit_AHTX0.h",
        socs: &[],
    },
    LibEntry {
        id: "bh1750",
        name: "BH1750",
        pio_spec: "claws/BH1750@1.3.0",
        header: "BH1750.h",
        socs: &[],
    },
    LibEntry {
        id: "ina219",
        name: "Adafruit INA219",
        pio_spec: "adafruit/Adafruit INA219@1.2.3",
        header: "Adafruit_INA219.h",
        socs: &[],
    },
    LibEntry {
        id: "ads1x15",
        name: "Adafruit ADS1X15",
        pio_spec: "adafruit/Adafruit ADS1X15@2.6.2",
        header: "Adafruit_ADS1X15.h",
        socs: &[],
    },
    LibEntry {
        id: "onewire",
        name: "OneWire",
        pio_spec: "paulstoffregen/OneWire@2.3.8",
        header: "OneWire.h",
        socs: &[],
    },
    LibEntry {
        id: "ds18b20",
        name: "DallasTemperature",
        pio_spec: "milesburton/DallasTemperature@4.0.6",
        header: "DallasTemperature.h",
        socs: &[],
    },
    LibEntry {
        id: "dht",
        name: "DHT sensor library",
        pio_spec: "adafruit/DHT sensor library@1.4.7",
        header: "DHT.h",
        socs: &[],
    },
    LibEntry {
        id: "neopixel",
        name: "Adafruit NeoPixel",
        pio_spec: "adafruit/Adafruit NeoPixel@1.15.5",
        header: "Adafruit_NeoPixel.h",
        socs: &[],
    },
    LibEntry {
        id: "esp32servo",
        name: "ESP32Servo",
        pio_spec: "madhephaestus/ESP32Servo@3.2.1",
        header: "ESP32Servo.h",
        socs: ESP32_ONLY,
    },
    LibEntry {
        id: "ssd1306",
        name: "Adafruit SSD1306",
        pio_spec: "adafruit/Adafruit SSD1306@2.5.17",
        header: "Adafruit_SSD1306.h",
        socs: &[],
    },
    LibEntry {
        id: "unified_sensor",
        name: "Adafruit Unified Sensor",
        pio_spec: "adafruit/Adafruit Unified Sensor@1.1.15",
        header: "Adafruit_Sensor.h",
        socs: &[],
    },
];

/// Catalog entry by id.
pub fn lib_by_id(id: &str) -> Option<&'static LibEntry> {
    LIB_CATALOG.iter().find(|e| e.id == id)
}

/// API view of a catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibCatalogItem {
    pub id: String,
    pub name: String,
    pub pio_spec: String,
    pub header: String,
    /// Compatible chip families (wire ids) — empty = all.
    #[serde(default)]
    pub chip_families: Vec<String>,
}

impl From<&LibEntry> for LibCatalogItem {
    fn from(e: &LibEntry) -> Self {
        Self {
            id: e.id.into(),
            name: e.name.into(),
            pio_spec: e.pio_spec.into(),
            header: e.header.into(),
            chip_families: e.socs.iter().map(|s| s.name().to_string()).collect(),
        }
    }
}

/// Resolves catalog ids for a SoC into their PlatformIO specs (dedup, order
/// kept). Errors carry the machine code and the offending id.
pub fn resolve_lib_deps(
    ids: &[String],
    soc: Soc,
) -> Result<Vec<&'static str>, (&'static str, String)> {
    let mut specs: Vec<&'static str> = Vec::new();
    for id in ids {
        let Some(entry) = lib_by_id(id) else {
            return Err(("firmware-lib-unknown", id.clone()));
        };
        if !entry.supports(soc) {
            return Err(("firmware-lib-incompatible", id.clone()));
        }
        if !specs.contains(&entry.pio_spec) {
            specs.push(entry.pio_spec);
        }
    }
    Ok(specs)
}

/// Libraries the builder image preinstalls for custom builds, one
/// `<generic project> <pio spec>` line per catalog entry a SoC supports. The
/// image bakes them next to each generic project's own `lib_deps`, so a
/// custom build compiles inside the network-less sandbox (air-gapped sites).
pub fn prewarm_lib_list() -> String {
    let mut out = String::from(
        "# Generated from pnex_core::firmware::LIB_CATALOG — do not edit.\n\
         # Regenerate: PNEX_REGEN_LIB_CATALOG=1 cargo test -p pnex-core prewarm_lib_list\n",
    );
    for soc in CHIP_FAMILIES {
        for e in LIB_CATALOG.iter().filter(|e| e.supports(soc)) {
            out.push_str(&format!("{} {}\n", generic_project(soc), e.pio_spec));
        }
    }
    out
}

// ───────────────────────────── Sketch guard (D91) ─────────────────────────────

/// A refused construct of the sketch, 1-based line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceViolation {
    pub line: u32,
    /// Machine code (`firmware-include-*`, `firmware-incbin`…).
    pub code: String,
}

/// Lexical guard on the sketch: includes must be literal and relative
/// (no absolute path, no `..`, no macro-expanded include), `#embed` and
/// assembler `.incbin` are refused (both read arbitrary files into the
/// binary). Best-effort by design: the sandbox (D91) is the real barrier.
pub fn check_main_cpp(src: &str) -> Result<(), Vec<SourceViolation>> {
    let mut out = Vec::new();
    if src.len() > MAX_MAIN_CPP_BYTES {
        out.push(SourceViolation {
            line: 1,
            code: "firmware-source-too-large".into(),
        });
        return Err(out);
    }
    let cleaned = strip_comments(&splice_lines(src));
    for (idx, line) in cleaned.lines().enumerate() {
        let line_no = idx as u32 + 1;
        if line.to_ascii_lowercase().contains("incbin") {
            out.push(SourceViolation {
                line: line_no,
                code: "firmware-incbin".into(),
            });
        }
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix('#') else {
            continue;
        };
        let rest = rest.trim_start();
        let directive: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let arg = rest[directive.len()..].trim();
        match directive.as_str() {
            "embed" => out.push(SourceViolation {
                line: line_no,
                code: "firmware-embed".into(),
            }),
            "include" | "include_next" | "import" => {
                if let Some(code) = include_violation(arg) {
                    out.push(SourceViolation {
                        line: line_no,
                        code: code.into(),
                    });
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        Ok(())
    } else {
        Err(out)
    }
}

fn include_violation(arg: &str) -> Option<&'static str> {
    let path = if let Some(r) = arg.strip_prefix('<') {
        r.split_once('>').map(|(p, _)| p)
    } else if let Some(r) = arg.strip_prefix('"') {
        r.split_once('"').map(|(p, _)| p)
    } else {
        None
    };
    let Some(path) = path else {
        return Some("firmware-include-not-literal");
    };
    let path = path.trim();
    if path.starts_with('/') || path.starts_with('\\') || path.contains(':') {
        return Some("firmware-include-absolute");
    }
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Some("firmware-include-parent");
    }
    None
}

/// Joins backslash-newline continuations (translation phase 2) while
/// keeping the line count: the spliced line is padded with newlines after it.
fn splice_lines(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut pending_newlines = 0;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Backslash, optional trailing spaces/CR, newline = splice.
            let mut look = chars.clone();
            let mut ws = 0;
            while matches!(look.peek(), Some(' ') | Some('\t') | Some('\r')) {
                look.next();
                ws += 1;
            }
            if look.peek() == Some(&'\n') {
                for _ in 0..=ws {
                    chars.next();
                }
                pending_newlines += 1;
                continue;
            }
        }
        if c == '\n' {
            out.push('\n');
            for _ in 0..pending_newlines {
                out.push('\n');
            }
            pending_newlines = 0;
            continue;
        }
        out.push(c);
    }
    out
}

/// Removes `//` and `/* */` comments (a block comment becomes one space,
/// its newlines kept), leaving string and char literals intact.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                out.push(c);
                while let Some(d) = chars.next() {
                    out.push(d);
                    if d == '\\' {
                        if let Some(e) = chars.next() {
                            out.push(e);
                        }
                    } else if d == c || d == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                out.push(' ');
                let mut prev = '\0';
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && d == '/' {
                        break;
                    }
                    prev = d;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

// ───────────────────────────── Diagnostics (D90) ─────────────────────────────

/// One compiler diagnostic. `in_sketch` = points at the user's `main.cpp`
/// (projected in the editor gutter); others are listed with their file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileDiagnostic {
    pub file: String,
    pub line: u32,
    #[serde(default)]
    pub column: u32,
    /// `error` | `warning` | `note`.
    pub severity: String,
    /// Verbatim compiler message (runtime diagnostic — not translated).
    pub message: String,
    pub in_sketch: bool,
}

/// Parses gcc-style `file:line:col: severity: message` lines. Only
/// `error`/`warning`/`note` are kept; duplicates are dropped; the sketch is
/// recognized by its `src/main.cpp` suffix (the workspace path is a tmp dir).
pub fn parse_gcc_diagnostics(output: &str) -> Vec<CompileDiagnostic> {
    let mut out: Vec<CompileDiagnostic> = Vec::new();
    for raw in output.lines() {
        let Some(d) = parse_diag_line(raw.trim()) else {
            continue;
        };
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

fn parse_diag_line(line: &str) -> Option<CompileDiagnostic> {
    for severity in ["error", "warning", "note"] {
        let marker = format!(": {severity}: ");
        let Some(pos) = line.find(&marker) else {
            continue;
        };
        let head = &line[..pos];
        let message = line[pos + marker.len()..].trim().to_string();
        // head = file:line[:col]
        let mut parts = head.rsplitn(3, ':');
        let last = parts.next()?;
        let mid = parts.next()?;
        let (file, line_no, column) = match parts.next() {
            Some(file) => match (mid.parse::<u32>(), last.parse::<u32>()) {
                (Ok(l), Ok(c)) => (file, l, c),
                _ => continue,
            },
            None => match last.parse::<u32>() {
                Ok(l) => (mid, l, 0),
                Err(_) => continue,
            },
        };
        let in_sketch = file.ends_with("src/main.cpp") || file == "main.cpp";
        let file = if in_sketch {
            "main.cpp".to_string()
        } else {
            short_path(file)
        };
        return Some(CompileDiagnostic {
            file,
            line: line_no,
            column,
            severity: severity.into(),
            message,
            in_sketch,
        });
    }
    None
}

/// Keeps the tail of a path after the build workspace (`.pio/libdeps/…`,
/// `lib/pnex/…`) — never leaks the tmp workspace layout to the user.
fn short_path(file: &str) -> String {
    for anchor in [".pio/libdeps/", "lib/pnex/", "common_libs/", "framework-"] {
        if let Some(pos) = file.find(anchor) {
            return file[pos..].to_string();
        }
    }
    file.rsplit('/').next().unwrap_or(file).to_string()
}

// ───────────────────────────── API DTOs ─────────────────────────────

/// Project list item (no code).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirmwareProjectSummary {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Wire id of the target chip family (`esp32`, `esp8266`…).
    pub chip_family: String,
    pub current_revision_number: i64,
    /// Devices attached to the project.
    #[serde(default)]
    pub device_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Project + current revision (the editor loads this).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirmwareProjectDetail {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub chip_family: String,
    pub current_revision_number: i64,
    pub main_cpp: String,
    /// Catalog ids.
    #[serde(default)]
    pub lib_deps: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Creation: project + revision 1 (`main_cpp` absent = starter sketch).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFirmwareProject {
    pub name: String,
    pub chip_family: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub main_cpp: Option<String>,
    #[serde(default)]
    pub lib_deps: Vec<String>,
}

/// New revision (append-only) and/or metadata. Optimistic concurrency:
/// `expected_revision_number` must be the current one, else 409. A save
/// whose content hash equals the current revision creates nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveFirmwareRevision {
    pub expected_revision_number: i64,
    #[serde(default)]
    pub main_cpp: Option<String>,
    #[serde(default)]
    pub lib_deps: Option<Vec<String>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// History entry — full content so any past revision can be restored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirmwareRevisionSummary {
    pub id: i64,
    pub revision_number: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub main_cpp: String,
    #[serde(default)]
    pub lib_deps: Vec<String>,
    pub created_at: String,
}

/// Compile-only check state (`GET /firmware-projects/{id}/checks/{check_id}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirmwareCheckStatus {
    pub check_id: String,
    /// `running` | `succeeded` | `failed` | `error` (infra: timeout, tool).
    pub status: String,
    pub revision_number: i64,
    #[serde(default)]
    pub diagnostics: Vec<CompileDiagnostic>,
    /// Compiler output tail (runtime diagnostic, verbatim).
    #[serde(default)]
    pub log_tail: String,
}

/// Custom command sent to a device (D88) — `POST /devices/{id}/custom-commands`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomCommandRequest {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The builder image installs what the catalog offers: the committed list
    /// must follow `LIB_CATALOG` (regenerate it with the env var below).
    #[test]
    fn prewarm_lib_list_matches_the_catalog() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/docker/prewarm/lib_catalog.txt"
        );
        let expected = prewarm_lib_list();
        if std::env::var_os("PNEX_REGEN_LIB_CATALOG").is_some() {
            std::fs::write(path, &expected).expect("write lib_catalog.txt");
        }
        let actual = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "lib_catalog.txt is stale: PNEX_REGEN_LIB_CATALOG=1 cargo test -p pnex-core prewarm_lib_list"
        );
    }

    #[test]
    fn catalog_ids_unique_and_specs_pinned() {
        let mut ids: Vec<&str> = LIB_CATALOG.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), LIB_CATALOG.len(), "duplicate catalog id");
        for e in LIB_CATALOG {
            let (_, version) = e.pio_spec.rsplit_once('@').expect("pinned spec");
            assert!(
                version.split('.').all(|p| p.parse::<u32>().is_ok()),
                "{} must pin an exact version",
                e.id
            );
        }
    }

    #[test]
    fn resolve_lib_deps_checks_catalog_and_soc() {
        let ok = resolve_lib_deps(
            &["bme280".into(), "bme280".into(), "dht".into()],
            Soc::Esp8266,
        )
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(
            resolve_lib_deps(&["nope".into()], Soc::Esp32).unwrap_err(),
            ("firmware-lib-unknown", "nope".to_string())
        );
        assert_eq!(
            resolve_lib_deps(&["esp32servo".into()], Soc::Esp8266)
                .unwrap_err()
                .0,
            "firmware-lib-incompatible"
        );
    }

    #[test]
    fn starter_sketch_passes_the_guard() {
        assert!(check_main_cpp(STARTER_SKETCH).is_ok());
        assert!(STARTER_SKETCH.contains("pnex.loop()"));
    }

    fn codes(src: &str) -> Vec<(u32, String)> {
        check_main_cpp(src)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|v| (v.line, v.code))
            .collect()
    }

    #[test]
    fn guard_refuses_file_reading_tricks() {
        assert!(codes("#include <Wire.h>\n#include \"local.h\"\n").is_empty());
        assert_eq!(
            codes("#include \"/proc/self/environ\"")[0].1,
            "firmware-include-absolute"
        );
        assert_eq!(
            codes("#include <../../etc/passwd>")[0].1,
            "firmware-include-parent"
        );
        assert_eq!(
            codes("#  include_next <a/../../b>")[0].1,
            "firmware-include-parent"
        );
        assert_eq!(
            codes("#define P </etc/passwd>\n#include P")[0],
            (2, "firmware-include-not-literal".into())
        );
        assert_eq!(codes("#embed \"x\"")[0].1, "firmware-embed");
        assert_eq!(
            codes("asm(\".incbin \\\"/etc/hosts\\\"\");")[0].1,
            "firmware-incbin"
        );
        // Comment between '#' and the directive.
        assert_eq!(
            codes("#/* x */include \"/etc/hosts\"")[0].1,
            "firmware-include-absolute"
        );
        // Backslash-newline splice inside the directive name; line kept.
        assert_eq!(
            codes("int a;\n#inc\\\nlude \"/etc/hosts\"")[0],
            (2, "firmware-include-absolute".into())
        );
        // A commented-out include is not code.
        assert!(codes("// #include \"/etc/hosts\"\n/* #include </x> */").is_empty());
        assert_eq!(
            codes(&"x".repeat(MAX_MAIN_CPP_BYTES + 1))[0].1,
            "firmware-source-too-large"
        );
    }

    #[test]
    fn gcc_diagnostics_are_projected_on_the_sketch() {
        let out = "Compiling .pio/build/esp32/src/main.cpp.o\n\
            /tmp/.tmpAbc/generic_esp32/src/main.cpp:42:7: error: 'foo' was not declared in this scope\n\
            /tmp/.tmpAbc/generic_esp32/src/main.cpp:42:7: error: 'foo' was not declared in this scope\n\
            /tmp/.tmpAbc/generic_esp32/.pio/libdeps/esp32/DHT/DHT.h:10: warning: something\n\
            src/main.cpp: In function 'void loop()':\n";
        let d = parse_gcc_diagnostics(out);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].file, "main.cpp");
        assert_eq!((d[0].line, d[0].column), (42, 7));
        assert!(d[0].in_sketch);
        assert_eq!(d[0].severity, "error");
        assert_eq!(d[1].file, ".pio/libdeps/esp32/DHT/DHT.h");
        assert!(!d[1].in_sketch);
        assert_eq!(d[1].column, 0);
    }
}
