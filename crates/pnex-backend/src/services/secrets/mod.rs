//! Org secrets vault (secrets.md D110–D118).

pub mod binding;
pub mod crypto;
pub mod flow;
pub mod llm;
pub mod notify;
pub mod rekey;
pub mod store;
pub mod wifi;

pub use crypto::{Keyring, KeyringError, Sealed, SecretCryptoError};

use loco_rs::config::Config;

/// Boot guard (D112, same school as Valkey D108): a server without a valid
/// keyring refuses to start rather than storing secrets it cannot read.
pub fn boot_guard(config: &Config) -> Result<(), KeyringError> {
    let ring = Keyring::from_config(config)?;
    tracing::info!(
        write_key = ring.primary_id(),
        keys = ring.key_ids().count(),
        "secrets keyring loaded"
    );
    Ok(())
}
