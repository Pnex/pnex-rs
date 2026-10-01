//! esptool merge-bin fusion (parity with the legacy script): produces the
//! single flashable image at a given base address.
//!
//! Per-SoC offsets (literals from the legacy k8s script):
//! - esp8266 : image unique `firmware.bin` @0x0 — le `.bin` de pio est déjà
//!   flashable tel quel, **pas de merge** ;
//! - esp32/esp32-s2 : bootloader @0x1000, partitions @0x8000, firmware
//!   @0x10000 ;
//! - esp32-c3 (et s3/c6/h2) : **bootloader @0x0** — le ROM boote depuis 0x0
//!   (constat e2e 2026-09-14 : layout 0x1000 = image non bootable, « invalid
//!   header: 0xffffffff » au boot) ;
//! - toujours `--flash-mode dio --flash-freq 40m --flash-size 4MB`.

use std::path::{Path, PathBuf};

/// Nom de chip esptool (`--chip`) depuis la convention `mcu_boards.soc` :
/// « esp32-c3 » → « esp32c3 », « esp32 » → « esp32 », « esp8266 » →
/// « esp8266 » — esptool refuse les tirets (v5, commandes en dash-case).
pub fn chip_name(soc: &str) -> String {
    soc.replace('-', "").to_ascii_lowercase()
}

/// Paires (offset, fichier) à fusionner — `None` pour un SoC à image unique
/// (esp8266) : pas d'appel esptool, `firmware.bin` est l'artefact final.
pub fn merge_offsets(soc: &str) -> Option<&'static [(&'static str, &'static str)]> {
    if soc.eq_ignore_ascii_case("esp8266") {
        None
    } else if soc.replace('-', "").eq_ignore_ascii_case("esp32c3") {
        // ESP32-C3 : le ROM boote depuis 0x0 — le bootloader DOIT y être
        // (layout 0x1000 = image non bootable, « invalid header » au boot).
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

/// Full argv of the esptool merge-bin command (legacy parity). Input
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
        // Variantes esp32 : le C3 boote depuis 0x0 (layout dédié), les
        // autres suivent l'esp32.
        assert_eq!(
            merge_offsets("esp32-c3").expect("c3"),
            &[
                ("0x0", "bootloader.bin"),
                ("0x8000", "partitions.bin"),
                ("0x10000", "firmware.bin")
            ]
        );
        assert!(merge_offsets("esp32c3").is_some());
    }

    /// Le soc de `mcu_boards.soc` (« esp32-c3 », hyphénée) → le nom de chip
    /// esptool (« esp32c3 ») — `--chip esp32-c3` est refusé par esptool.
    #[test]
    fn nom_chip_esptool_depuis_soc() {
        assert_eq!(chip_name("esp8266"), "esp8266");
        assert_eq!(chip_name("esp32"), "esp32");
        assert_eq!(chip_name("esp32-c3"), "esp32c3");
        assert_eq!(chip_name("esp32c3"), "esp32c3");
        assert_eq!(chip_name("ESP32-C3"), "esp32c3");
    }

    /// Ligne esptool complète, commandes multi-mots splitées.
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
