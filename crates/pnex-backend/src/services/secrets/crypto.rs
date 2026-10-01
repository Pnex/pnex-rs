//! At-rest encryption of org secrets (secrets.md D112).
//!
//! XChaCha20-Poly1305 with a random 24-byte nonce per write. The AAD binds
//! a ciphertext to its owner (`org_id`, or the platform) and to its row
//! (`secret_id`): a row copied onto another org or another secret fails to
//! open. The master keys live outside the database, in a keyring
//! (`settings.secrets.keys`, env `PNEX_SECRETS_KEYS` in production):
//! `"k2:<b64>,k1:<b64>"` — the first key encrypts, the others only decrypt
//! (rotation, D112 / §7).

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use loco_rs::config::Config;
use rand::RngExt;
use uuid::Uuid;

/// Nonce length of XChaCha20-Poly1305.
pub const NONCE_LEN: usize = 24;
/// Domain separator of the AAD (bumped if the AAD layout ever changes).
const AAD_DOMAIN: &[u8] = b"pnex-secret-v1";
/// Longest accepted key id (stored in `org_secrets.key_id`).
const KEY_ID_MAX: usize = 32;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyringError {
    #[error(
        "secrets keyring is not configured (settings.secrets.keys / PNEX_SECRETS_KEYS): \
         refusing to start"
    )]
    Missing,
    #[error("secrets keyring entry #{0} is not `<id>:<base64 32-byte key>`")]
    Malformed(usize),
    #[error("secrets keyring key id `{0}` is invalid (1-32 chars of [A-Za-z0-9_-])")]
    BadId(String),
    #[error("secrets keyring key `{0}` is not 32 bytes of standard base64")]
    BadKey(String),
    #[error("secrets keyring key id `{0}` appears twice")]
    Duplicate(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretCryptoError {
    /// The row was written with a key that is no longer in the keyring.
    #[error("secret was encrypted with unknown key `{0}`")]
    UnknownKey(String),
    #[error("secret nonce has an invalid length")]
    BadNonce,
    /// Wrong key, tampered ciphertext, or AAD mismatch (row moved).
    #[error("secret failed authentication")]
    Auth,
    #[error("secret plaintext is not valid UTF-8")]
    NotUtf8,
}

/// One encrypted value, as stored in `org_secrets`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sealed {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub key_id: String,
}

pub struct Keyring {
    /// First entry = write key.
    keys: Vec<(String, Key)>,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        let ids: Vec<&str> = self.keys.iter().map(|(id, _)| id.as_str()).collect();
        f.debug_struct("Keyring").field("key_ids", &ids).finish()
    }
}

impl Keyring {
    /// Parses `"k2:<b64>,k1:<b64>"` (whitespace around entries ignored).
    pub fn parse(spec: &str) -> Result<Self, KeyringError> {
        let mut keys: Vec<(String, Key)> = Vec::new();
        for (idx, entry) in spec
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .enumerate()
        {
            let (id, b64) = entry
                .split_once(':')
                .ok_or(KeyringError::Malformed(idx + 1))?;
            let id = id.trim();
            if id.is_empty()
                || id.len() > KEY_ID_MAX
                || !id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(KeyringError::BadId(id.to_string()));
            }
            let raw = B64
                .decode(b64.trim())
                .map_err(|_| KeyringError::BadKey(id.to_string()))?;
            if raw.len() != 32 {
                return Err(KeyringError::BadKey(id.to_string()));
            }
            if keys.iter().any(|(k, _)| k == id) {
                return Err(KeyringError::Duplicate(id.to_string()));
            }
            keys.push((id.to_string(), *Key::from_slice(&raw)));
        }
        if keys.is_empty() {
            return Err(KeyringError::Missing);
        }
        Ok(Self { keys })
    }

    /// Keyring of `settings.secrets.keys`.
    pub fn from_config(config: &Config) -> Result<Self, KeyringError> {
        let spec = config
            .settings
            .as_ref()
            .and_then(|s| s.get("secrets"))
            .and_then(|s| s.get("keys"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        Self::parse(spec)
    }

    /// Id of the write key (stamped on every new row).
    pub fn primary_id(&self) -> &str {
        &self.keys[0].0
    }

    /// Ids of every key, write key first.
    pub fn key_ids(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(|(id, _)| id.as_str())
    }

    /// Encrypts `plaintext` for the row `secret_id` of `org_id`
    /// (`None` = platform secret) with the write key.
    pub fn seal(&self, org_id: Option<i64>, secret_id: Uuid, plaintext: &str) -> Sealed {
        let (key_id, key) = &self.keys[0];
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill(&mut nonce);
        let aad = aad(org_id, secret_id);
        let ciphertext = XChaCha20Poly1305::new(key)
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: &aad,
                },
            )
            // Only fails on messages beyond the cipher's 256 GiB limit.
            .expect("XChaCha20-Poly1305 encryption cannot fail on a secret-sized input");
        Sealed {
            ciphertext,
            nonce: nonce.to_vec(),
            key_id: key_id.clone(),
        }
    }

    /// Decrypts a stored row. Fails closed on any mismatch.
    pub fn open(
        &self,
        org_id: Option<i64>,
        secret_id: Uuid,
        sealed: &Sealed,
    ) -> Result<String, SecretCryptoError> {
        let key = self
            .keys
            .iter()
            .find(|(id, _)| *id == sealed.key_id)
            .map(|(_, k)| k)
            .ok_or_else(|| SecretCryptoError::UnknownKey(sealed.key_id.clone()))?;
        if sealed.nonce.len() != NONCE_LEN {
            return Err(SecretCryptoError::BadNonce);
        }
        let aad = aad(org_id, secret_id);
        let plain = XChaCha20Poly1305::new(key)
            .decrypt(
                XNonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| SecretCryptoError::Auth)?;
        String::from_utf8(plain).map_err(|_| SecretCryptoError::NotUtf8)
    }
}

/// `domain ‖ owner ‖ secret_id`. Owner = `0x00` for the platform, else
/// `0x01 ‖ org_id (i64 BE)` — unambiguous, fixed layout.
fn aad(org_id: Option<i64>, secret_id: Uuid) -> Vec<u8> {
    let mut out = Vec::with_capacity(AAD_DOMAIN.len() + 9 + 16);
    out.extend_from_slice(AAD_DOMAIN);
    match org_id {
        None => out.push(0),
        Some(id) => {
            out.push(1);
            out.extend_from_slice(&id.to_be_bytes());
        }
    }
    out.extend_from_slice(secret_id.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "rJlgG51k+ePf2XrLPnEAXacoz7wUPHiYSxkGT2upQPQ=";
    const K2: &str = "wUlXV5Lc4M+3w93Fed/Sn4NdLc2pw4m2yyc1bCo4jK8=";

    fn ring(spec: &str) -> Keyring {
        Keyring::parse(spec).unwrap()
    }

    #[test]
    fn known_answer_vector() {
        // draft-irtf-cfrg-xchacha-03 §A.3.1 (AEAD_XChaCha20_Poly1305).
        let hex = |s: &str| -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        };
        let key = hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce = hex("404142434445464748494a4b4c4d4e4f5051525354555657");
        let aad = hex("50515253c0c1c2c3c4c5c6c7");
        let msg = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let out = XChaCha20Poly1305::new(Key::from_slice(&key))
            .encrypt(XNonce::from_slice(&nonce), Payload { msg, aad: &aad })
            .unwrap();
        let tag = &out[out.len() - 16..];
        assert_eq!(tag, hex("c0875924c1c7987947deafd8780acf49").as_slice());
        assert_eq!(&out[..8], hex("bd6d179d3e83d43b").as_slice());
    }

    #[test]
    fn round_trip_with_write_key() {
        let kr = ring(&format!("k2:{K2},k1:{K1}"));
        let id = Uuid::new_v4();
        let sealed = kr.seal(Some(42), id, "s3cr3t-ü");
        assert_eq!(sealed.key_id, "k2");
        assert_eq!(sealed.nonce.len(), NONCE_LEN);
        assert!(!sealed.ciphertext.windows(6).any(|w| w == b"s3cr3t"));
        assert_eq!(kr.open(Some(42), id, &sealed).unwrap(), "s3cr3t-ü");
    }

    #[test]
    fn nonce_is_fresh_per_write() {
        let kr = ring(&format!("k1:{K1}"));
        let id = Uuid::new_v4();
        let a = kr.seal(Some(1), id, "same");
        let b = kr.seal(Some(1), id, "same");
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn aad_binds_org_and_secret() {
        let kr = ring(&format!("k1:{K1}"));
        let id = Uuid::new_v4();
        let sealed = kr.seal(Some(1), id, "v");
        assert_eq!(kr.open(Some(2), id, &sealed), Err(SecretCryptoError::Auth));
        assert_eq!(
            kr.open(Some(1), Uuid::new_v4(), &sealed),
            Err(SecretCryptoError::Auth)
        );
        assert_eq!(kr.open(None, id, &sealed), Err(SecretCryptoError::Auth));
        let platform = kr.seal(None, id, "p");
        assert_eq!(kr.open(None, id, &platform).unwrap(), "p");
        assert_eq!(
            kr.open(Some(0), id, &platform),
            Err(SecretCryptoError::Auth)
        );
    }

    #[test]
    fn tampering_fails_closed() {
        let kr = ring(&format!("k1:{K1}"));
        let id = Uuid::new_v4();
        let mut sealed = kr.seal(Some(1), id, "value");
        sealed.ciphertext[0] ^= 1;
        assert_eq!(kr.open(Some(1), id, &sealed), Err(SecretCryptoError::Auth));
        let mut short = kr.seal(Some(1), id, "value");
        short.nonce.pop();
        assert_eq!(
            kr.open(Some(1), id, &short),
            Err(SecretCryptoError::BadNonce)
        );
    }

    #[test]
    fn rotation_reads_old_key_and_rejects_unknown() {
        let old = ring(&format!("k1:{K1}"));
        let id = Uuid::new_v4();
        let sealed = old.seal(Some(7), id, "legacy");
        let rotated = ring(&format!("k2:{K2},k1:{K1}"));
        assert_eq!(rotated.open(Some(7), id, &sealed).unwrap(), "legacy");
        assert_eq!(rotated.primary_id(), "k2");
        let dropped = ring(&format!("k2:{K2}"));
        assert_eq!(
            dropped.open(Some(7), id, &sealed),
            Err(SecretCryptoError::UnknownKey("k1".into()))
        );
        // Same id, different material: authentication fails.
        let swapped = ring(&format!("k1:{K2}"));
        assert_eq!(
            swapped.open(Some(7), id, &sealed),
            Err(SecretCryptoError::Auth)
        );
    }

    #[test]
    fn parse_rejects_bad_specs() {
        assert_eq!(Keyring::parse("").unwrap_err(), KeyringError::Missing);
        assert_eq!(Keyring::parse(" , ").unwrap_err(), KeyringError::Missing);
        assert_eq!(
            Keyring::parse("nocolon").unwrap_err(),
            KeyringError::Malformed(1)
        );
        assert_eq!(
            Keyring::parse(&format!("bad id:{K1}")).unwrap_err(),
            KeyringError::BadId("bad id".into())
        );
        assert_eq!(
            Keyring::parse("k1:c2hvcnQ=").unwrap_err(),
            KeyringError::BadKey("k1".into())
        );
        assert_eq!(
            Keyring::parse(&format!("k1:{K1},k1:{K2}")).unwrap_err(),
            KeyringError::Duplicate("k1".into())
        );
        let kr = Keyring::parse(&format!(" k2:{K2} , k1:{K1} ")).unwrap();
        assert_eq!(kr.key_ids().collect::<Vec<_>>(), vec!["k2", "k1"]);
    }

    #[test]
    fn debug_never_prints_key_material() {
        let kr = ring(&format!("k1:{K1}"));
        let dbg = format!("{kr:?}");
        assert!(dbg.contains("k1"));
        assert!(!dbg.contains(K1));
    }
}
