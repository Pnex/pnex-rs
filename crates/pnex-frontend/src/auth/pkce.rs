//! PKCE RFC 7636 — verifier/challenge S256 et encodage d'URL.
//!
//! Le challenge S256 est exigé par le proxy SSO backend (le S256 est aussi
//! forcé par le client OIDC). Le verifier fait 43 caractères (32 octets
//! d'entropie en base64url sans padding).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// Génère une paire verifier/challenge (méthode S256).
pub fn generate() -> Pkce {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("entropie PKCE indisponible");
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

/// Encodage pour cent form-urlencoded (caractères non réservés conservés).
pub fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vecteur de l'annexe B de la RFC 7636.
    #[test]
    fn challenge_vecteur_rfc7636() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn verifier_taille_et_alphabet() {
        let pkce = generate();
        assert_eq!(pkce.verifier.len(), 43);
        assert!(pkce
            .verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }

    #[test]
    fn urlencode_pieces_jointes() {
        assert_eq!(urlencode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(urlencode("é &?"), "%C3%A9%20%26%3F");
    }
}
