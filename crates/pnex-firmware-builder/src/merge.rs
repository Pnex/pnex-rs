//! Fusion esptool merge-bin (parité du script Django) : produit l'image
//! unique flashable à une adresse de base donnée.
//!
//! Offsets par SoC (littéraux du script k8s Django) :
//! - esp8266 : image unique `firmware.bin` @0x0 — le `.bin` de pio est déjà
//!   flashable tel quel, **pas de merge** ;
//! - esp32 (toutes variantes) : bootloader @0x1000, partitions @0x8000,
//!   firmware @0x10000, toujours `--flash-mode dio --flash-freq 40m
//!   --flash-size 4MB`.

use std::path::{Path, PathBuf};

/// Paires (offset, fichier) à fusionner — `None` pour un SoC à image unique
/// (esp8266) : pas d'appel esptool, `firmware.bin` est l'artefact final.
pub fn merge_offsets(soc: &str) -> Option<&'static [(&'static str, &'static str)]> {
    if soc.eq_ignore_ascii_case("esp8266") {
        None
    } else {
        Some(&[
            ("0x1000", "bootloader.bin"),
            ("0x8000", "partitions.bin"),
            ("0x10000", "firmware.bin"),
        ])
    }
}

/// Argv complet de la commande esptool merge-bin (parité Django). Les
/// chemins d'entrées sont passés tels quels (absolus dans le workspace).
pub fn merge_args(
    esptool_cmd: &str,
    soc: &str,
    out: &Path,
    inputs: &[(String, PathBuf)],
) -> Vec<String> {
    let mut argv: Vec<String> = esptool_cmd.split_whitespace().map(String::from).collect();
    argv.extend([
        "--chip".into(),
        soc.to_string(),
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
        // Variantes esp32 (c3, s3…) → même layout.
        assert!(merge_offsets("esp32c3").is_some());
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
        assert_eq!(argv[4], "esp32");
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
