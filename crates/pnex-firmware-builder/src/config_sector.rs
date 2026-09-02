//! Secteur de config device injecté au flash (Brick 0, B0.1 — brick0.md §5).
//!
//! Le firmware générique compile **une fois** : WiFi/hôte/token/device_id ne
//! sont pas dans le .bin, ils vivent dans un **secteur 4 Ko à l'offset
//! 0x200000** écrit dans le même `writeFlash` esptool-js que l'image (2
//! entrées). Format `PNEXCFG1` :
//!
//! ```text
//! 0x00  magic  "PNEXCFG1"  (8 octets)
//! 0x08  version u16 LE (= 1)
//! 0x0A  crc32  u32 LE — CRC IEEE du payload JSON
//! 0x0E  payload JSON compact (wifi_ssid, wifi_password, host, token,
//!       device_id, ws_ssl) — chaînes claires, pas de base64 (B0.1 :
//!       la contrainte -D du build a disparu)
//! 0x0E+len … 0x1000 rempli 0xFF (état flash effacée)
//! ```

use crate::BuildError;
use serde::{Deserialize, Serialize};

/// Taille du secteur (1 page flash ESP8266).
const SECTOR: usize = 4096;
/// Magie du format (8 octets exactement) — version séparée pour évolutions.
const MAGIC: &[u8; 8] = b"PNEXCFG1";
const VERSION: u16 = 1;
/// Offset flash du secteur (flash 4 Mo, hors zone SDK — à re-valider au
/// premier flash réel, brick0.md §10).
pub const CONFIG_OFFSET: u32 = 0x200000;

/// Config device embarquée dans le secteur — chaînes claires (pas de b64 :
/// la contrainte -D du build custom a disparu avec B0.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub host: String,
    pub token: String,
    pub device_id: String,
    /// ws (local) | wss (industriel) — même sémantique que le build custom.
    pub ws_ssl: bool,
}

/// CRC32 IEEE (0xEDB88320, poly réfléchi) — petit helper sans dépendance,
/// suffisant pour détecter un secteur tronqué/corrompu (pas un hash crypto).
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Construit le secteur 4 Ko prêt à flasher à `CONFIG_OFFSET`.
pub fn build(config: &DeviceConfig) -> Result<Vec<u8>, BuildError> {
    let payload = serde_json::to_vec(config)
        .map_err(|e| BuildError::Tool(format!("config device non sérialisable : {e}")))?;
    if payload.len() > SECTOR - 14 {
        return Err(BuildError::Tool(format!(
            "config device trop longue : {} octets (max {})",
            payload.len(),
            SECTOR - 14
        )));
    }
    let mut out = vec![0xFF; SECTOR];
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&VERSION.to_le_bytes());
    out[10..14].copy_from_slice(&crc32(&payload).to_le_bytes());
    out[14..14 + payload.len()].copy_from_slice(&payload);
    Ok(out)
}

/// Parse un secteur (lecture/retour device pour tests et re-flash).
pub fn parse(bytes: &[u8]) -> Option<DeviceConfig> {
    if bytes.len() < 14 || &bytes[..8] != MAGIC || u16::from_le_bytes(bytes[8..10].try_into().ok()?) != VERSION {
        return None;
    }
    let crc = u32::from_le_bytes(bytes[10..14].try_into().ok()?);
    // Le payload s'arrête au premier 0xFF padding (JSON ne contient pas 0xFF).
    let end = bytes[14..].iter().position(|&b| b == 0xFF).map(|i| 14 + i).unwrap_or(bytes.len());
    let payload = &bytes[14..end];
    if crc32(payload) != crc {
        return None;
    }
    serde_json::from_slice(payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_vecteur_standard() {
        // vecteur canonique CRC-32/ISO-HDLC
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn secteur_roundtrip_et_rejet_corrompu() {
        let cfg = DeviceConfig {
            wifi_ssid: "Chez Shan".into(),
            wifi_password: "mot de passe  espace".into(),
            host: "dev1.pnex.io".into(),
            token: "tok".into(),
            device_id: "dev-x".into(),
            ws_ssl: false,
        };
        let sector = build(&cfg).expect("build secteur");
        assert_eq!(sector.len(), 4096);
        assert_eq!(&sector[..8], b"PNEXCFG1");
        assert_eq!(parse(&sector).as_ref(), Some(&cfg));
        // padding 0xFF jusqu'au bout
        assert!(sector[600..].iter().all(|&b| b == 0xFF));
        // corruption payload → None
        let mut bad = sector.clone();
        bad[30] ^= 0x01;
        assert_eq!(parse(&bad), None);
        // magie absente → None
        let mut bad = sector.clone();
        bad[0] = b'X';
        assert_eq!(parse(&bad), None);
    }
}
