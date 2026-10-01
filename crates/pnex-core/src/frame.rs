//! Device/agent WebSocket frame encryption (D8), shared by the backend and
//! the edge agent: `base64(nonce[12] ‖ ChaCha20(plaintext))`, a fresh random
//! nonce per frame, 32-byte key from `device_tokens.encryption_key`.
//!
//! No Poly1305 (D8): the C++ firmware speaks the exact same format, the
//! integrity/authenticity layer is the TLS tunnel.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::{ChaCha20, Key, Nonce};

/// Decodes a base64 32-byte key (`STANDARD`, tolerant to URL-safe).
pub fn decode_key(b64: &str) -> Option<[u8; 32]> {
    let raw = STANDARD
        .decode(b64.trim())
        .or_else(|_| URL_SAFE_NO_PAD.decode(b64.trim()))
        .ok()?;
    raw.try_into().ok()
}

/// Decrypts a frame: `base64(nonce 12 ‖ ct)` → UTF-8 plaintext. `None` =
/// unreadable (bad base64, short frame, non-UTF-8 plaintext).
pub fn decrypt_frame(raw: &str, key: &[u8; 32]) -> Option<String> {
    let trimmed = raw.trim();
    let bytes = STANDARD
        .decode(trimmed)
        .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed))
        .ok()?;
    if bytes.len() < 12 {
        return None;
    }
    let (nonce, ct) = bytes.split_at(12);
    let mut buf = ct.to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(nonce)).apply_keystream(&mut buf);
    String::from_utf8(buf).ok()
}

/// Encrypts a frame with a fresh OS-random nonce.
pub fn encrypt_frame(plain: &str, key: &[u8; 32]) -> String {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("OS random source unavailable");
    encrypt_frame_with_nonce(plain, key, nonce)
}

/// Deterministic variant (golden vectors / tests).
pub fn encrypt_frame_with_nonce(plain: &str, key: &[u8; 32], nonce: [u8; 12]) -> String {
    let mut buf = plain.as_bytes().to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(&nonce)).apply_keystream(&mut buf);
    let mut wire = nonce.to_vec();
    wire.extend_from_slice(&buf);
    STANDARD.encode(wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_fresh_nonce() {
        let key = [7u8; 32];
        let a = encrypt_frame("{\"t\":\"batch\"}", &key);
        let b = encrypt_frame("{\"t\":\"batch\"}", &key);
        assert_ne!(a, b, "nonce must be fresh per frame");
        assert_eq!(
            decrypt_frame(&a, &key).as_deref(),
            Some("{\"t\":\"batch\"}")
        );
    }

    #[test]
    fn rejects_short_or_garbage_frames() {
        let key = [1u8; 32];
        assert!(decrypt_frame("", &key).is_none());
        assert!(decrypt_frame("!!!", &key).is_none());
        assert!(decrypt_frame(&STANDARD.encode([0u8; 5]), &key).is_none());
    }

    #[test]
    fn decode_key_requires_32_bytes() {
        assert!(decode_key(&STANDARD.encode([3u8; 32])).is_some());
        assert!(decode_key(&STANDARD.encode([3u8; 31])).is_none());
    }
}
