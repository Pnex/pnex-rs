//! Signed OTA images (SEC-18, security-tiers.md EX-B1/EX-B2).
//!
//! The server signs, with the Ed25519 key of the instance, a short message
//! that binds the image to its device and its version; the firmware checks
//! the signature with the public key compiled into it before switching the
//! boot slot. Binding the device stops an image built for another device
//! (with that device's credentials) from being accepted; binding the version
//! makes the anti-downgrade check trust a signed number, not a relabelled one.
//!
//! Message layout (fixed, mirrored by `firmware/lib/pnex/src/pnex_ota_sig.cpp`,
//! golden vectors in `firmware/common_libs/pnex-core-cpp/ota_sig_goldens.h`):
//!
//! ```text
//! "PNEX-OTA-1" ‖ len(device_id) u8 ‖ device_id ‖ len(version) u8 ‖ version ‖ sha256(image)
//! ```
//!
//! Signatures, public keys and digests travel as lowercase hex.

/// Domain separator (bumped if the layout ever changes).
pub const DOMAIN: &[u8] = b"PNEX-OTA-1";

/// Message signed for one image, `None` when `device_id` or `version` is
/// longer than 255 bytes (never the case: device ids are 16 characters,
/// versions are build ids).
pub fn signed_message(device_id: &str, version: &str, image_sha256: &[u8; 32]) -> Option<Vec<u8>> {
    let dev = u8::try_from(device_id.len()).ok()?;
    let ver = u8::try_from(version.len()).ok()?;
    let mut out = Vec::with_capacity(DOMAIN.len() + 2 + device_id.len() + version.len() + 32);
    out.extend_from_slice(DOMAIN);
    out.push(dev);
    out.extend_from_slice(device_id.as_bytes());
    out.push(ver);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(image_sha256);
    Some(out)
}

/// Parses a 64-character hex SHA-256 (any case).
pub fn parse_sha256_hex(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_layout_is_length_prefixed() {
        let sha = [0xab; 32];
        let msg = signed_message("dev-1", "42", &sha).unwrap();
        assert_eq!(&msg[..10], b"PNEX-OTA-1");
        assert_eq!(msg[10], 5);
        assert_eq!(&msg[11..16], b"dev-1");
        assert_eq!(msg[16], 2);
        assert_eq!(&msg[17..19], b"42");
        assert_eq!(&msg[19..], &sha);
        // Moving a byte between the fields changes the message.
        assert_ne!(msg, signed_message("dev-14", "2", &sha).unwrap());
        assert!(signed_message(&"x".repeat(256), "1", &sha).is_none());
    }

    #[test]
    fn sha_hex_roundtrip() {
        let sha: [u8; 32] = std::array::from_fn(|i| i as u8 * 7);
        assert_eq!(parse_sha256_hex(&to_hex(&sha)), Some(sha));
        assert_eq!(parse_sha256_hex(&to_hex(&sha).to_uppercase()), Some(sha));
        assert_eq!(parse_sha256_hex("abc"), None);
        assert_eq!(parse_sha256_hex(&"zz".repeat(32)), None);
    }
}
