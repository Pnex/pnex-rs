//! esptool merge-bin fusion: produces the single flashable image at a given
//! base address.
//!
//! Per-SoC offsets:
//! - esp8266: single image `firmware.bin` @0x0 — the pio `.bin` is already
//!   flashable as is, **no merge**;
//! - esp32/esp32-s2: bootloader @0x1000, partitions @0x8000, firmware
//!   @0x10000;
//! - esp32-c3 (and s3/c6/h2): **bootloader @0x0** — the ROM boots from 0x0
//!   (e2e finding 2026-09-14: 0x1000 layout = non-bootable image, "invalid
//!   header: 0xffffffff" at boot);
//! - always `--flash-mode dio --flash-freq 40m --flash-size 4MB`.

use std::path::{Path, PathBuf};

/// esptool chip name (`--chip`) from the `mcu_boards.soc` convention:
/// "esp32-c3" → "esp32c3", "esp32" → "esp32", "esp8266" →
/// "esp8266" — esptool refuses hyphens (v5, dash-case commands).
pub fn chip_name(soc: &str) -> String {
    soc.replace('-', "").to_ascii_lowercase()
}

/// (offset, file) pairs to merge — `None` for a single-image SoC
/// (esp8266): no esptool call, `firmware.bin` is the final artifact.
pub fn merge_offsets(soc: &str) -> Option<&'static [(&'static str, &'static str)]> {
    if soc.eq_ignore_ascii_case("esp8266") {
        None
    } else if matches!(chip_name(soc).as_str(), "esp32c3" | "esp32c6") {
        // ESP32-C3/C6: the ROM boots from 0x0 — the bootloader MUST sit
        // there (0x1000 layout = unbootable image, "invalid header").
        Some(&[
            ("0x0", "bootloader.bin"),
            ("0x8000", "partitions.bin"),
            ("0x10000", "firmware.bin"),
        ])
    } else {
        Some(&[
            ("0x1000", "bootloader.bin"),
            ("0x8000", "partitions.bin"),
            ("0x10000", "firmware.bin"),
        ])
    }
}

/// Full argv of the esptool merge-bin command. Input
/// paths are passed as-is (absolute within the workspace).
pub fn merge_args(
    esptool_cmd: &str,
    soc: &str,
    out: &Path,
    inputs: &[(String, PathBuf)],
) -> Vec<String> {
    let mut argv: Vec<String> = esptool_cmd.split_whitespace().map(String::from).collect();
    argv.extend([
        "--chip".into(),
        chip_name(soc),
        "merge-bin".into(),
        "-o".into(),
        out.display().to_string(),
        "--flash-mode".into(),
        "dio".into(),
        "--flash-freq".into(),
        "40m".into(),
        "--flash-size".into(),
        "4MB".into(),
    ]);
    for (offset, path) in inputs {
        argv.push(offset.clone());
        argv.push(path.display().to_string());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_par_soc() {
        assert!(merge_offsets("esp8266").is_none());
        assert!(merge_offsets("ESP8266").is_none());
        let esp32 = merge_offsets("esp32").expect("esp32");
        assert_eq!(
            esp32,
            &[
                ("0x1000", "bootloader.bin"),
                ("0x8000", "partitions.bin"),
                ("0x10000", "firmware.bin")
            ]
        );
        // esp32 variants: the C3 boots from 0x0 (dedicated layout), the
        // others follow the esp32.
        assert_eq!(
            merge_offsets("esp32-c3").expect("c3"),
            &[
                ("0x0", "bootloader.bin"),
                ("0x8000", "partitions.bin"),
                ("0x10000", "firmware.bin")
            ]
        );
        assert!(merge_offsets("esp32c3").is_some());
        assert_eq!(merge_offsets("esp32-c6"), merge_offsets("esp32-c3"));
    }

    /// The `mcu_boards.soc` soc ("esp32-c3", hyphenated) → the esptool chip
    /// name ("esp32c3") — `--chip esp32-c3` is refused by esptool.
    #[test]
    fn nom_chip_esptool_depuis_soc() {
        assert_eq!(chip_name("esp8266"), "esp8266");
        assert_eq!(chip_name("esp32"), "esp32");
        assert_eq!(chip_name("esp32-c3"), "esp32c3");
        assert_eq!(chip_name("esp32c3"), "esp32c3");
        assert_eq!(chip_name("ESP32-C3"), "esp32c3");
    }

    /// Full esptool line, multi-word commands split.
    #[test]
    fn ligne_esptool_conforme() {
        let argv = merge_args(
            "python -m esptool",
            "esp32",
            Path::new("/tmp/out.bin"),
            &[
                ("0x1000".into(), PathBuf::from("/w/bootloader.bin")),
                ("0x10000".into(), PathBuf::from("/w/firmware.bin")),
            ],
        );
        assert_eq!(&argv[..4], &["python", "-m", "esptool", "--chip"]);
        assert_eq!(argv[4], chip_name("esp32"));
        assert!(argv.contains(&"merge-bin".to_string()));
        assert!(argv.windows(2).any(|w| w == ["-o", "/tmp/out.bin"]));
        assert!(argv
            .windows(2)
            .any(|w| w == ["0x1000", "/w/bootloader.bin"]));
        assert!(argv.contains(&"--flash-mode".to_string()));
        assert!(argv.contains(&"dio".to_string()));
        assert!(argv.contains(&"40m".to_string()));
        assert!(argv.contains(&"4MB".to_string()));
    }
}
