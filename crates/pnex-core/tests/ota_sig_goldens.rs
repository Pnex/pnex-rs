//! Signed OTA golden vectors, Rust = C++ (SEC-18).
//!
//! Signs fixed messages of `pnex_core::ota_sig` with a fixed Ed25519 key
//! (`ed25519-dalek`, the server's signer) and checks that
//! `firmware/common_libs/pnex-core-cpp/ota_sig_goldens.h` is up to date. The
//! firmware host test (`firmware/core-cpp-tests`) verifies the same
//! signatures with the C++ checker (`lib/pnex/src/pnex_ota_sig.cpp`,
//! Monocypher), and refuses the tampered ones.
//!
//! Regen: `PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --test ota_sig_goldens`

use std::fmt::Write as _;
use std::path::PathBuf;

use ed25519_dalek::{Signer, SigningKey};
use pnex_core::ota_sig::{signed_message, to_hex};

const DEVICE_ID: &str = "golden-dev";
const VERSION: &str = "1234";

fn seed() -> [u8; 32] {
    std::array::from_fn(|i| 0xa0 + i as u8)
}

fn image_sha() -> [u8; 32] {
    std::array::from_fn(|i| (i as u8).wrapping_mul(13).wrapping_add(5))
}

fn render() -> String {
    let key = SigningKey::from_bytes(&seed());
    let msg = signed_message(DEVICE_ID, VERSION, &image_sha()).unwrap();
    let sig = key.sign(&msg);
    // Sanity: the reference verifies its own signature.
    key.verifying_key().verify_strict(&msg, &sig).unwrap();

    let mut out = String::from(
        "// GENERATED FILE — do not edit. Reference: pnex_core::ota_sig (Rust ed25519-dalek).\n\
         // Regen:\n\
         //   PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --test ota_sig_goldens\n\
         // Verified by firmware/core-cpp-tests against lib/pnex/src/pnex_ota_sig.cpp (SEC-18).\n\
         #pragma once\n\n\
         namespace pnex_ota_sig_goldens {\n\n",
    );
    let _ = writeln!(out, "static const char DEVICE_ID[] = \"{DEVICE_ID}\";");
    let _ = writeln!(out, "static const char VERSION[] = \"{VERSION}\";");
    let _ = writeln!(
        out,
        "static const char PUBKEY_HEX[] = \"{}\";",
        to_hex(key.verifying_key().as_bytes())
    );
    let _ = writeln!(
        out,
        "static const char SHA256_HEX[] = \"{}\";",
        to_hex(&image_sha())
    );
    let _ = writeln!(
        out,
        "static const char SIG_HEX[] = \"{}\";",
        to_hex(&sig.to_bytes())
    );
    out.push_str("\n}  // namespace pnex_ota_sig_goldens\n");
    out
}

fn header_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../firmware/common_libs/pnex-core-cpp/ota_sig_goldens.h")
}

#[test]
fn ota_sig_goldens_are_up_to_date() {
    let path = header_path();
    let fresh = render();
    if std::env::var("PNEX_REGEN_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&path, &fresh).expect("write ota_sig_goldens.h");
        return;
    }
    let current =
        std::fs::read_to_string(&path).expect("ota_sig_goldens.h missing — run the regen");
    assert!(
        current == fresh,
        "ota_sig_goldens.h is stale — run the regen (see the module docs)"
    );
}
