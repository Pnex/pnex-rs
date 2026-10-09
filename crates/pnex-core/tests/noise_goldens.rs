//! Noise golden vectors, Rust = C++ (D156).
//!
//! `pnex_core::frame` (Rust `snow`) is the reference of the device link; this
//! test runs a handshake with fixed ephemeral keys and a few transport frames,
//! then checks that `firmware/common_libs/pnex-core-cpp/noise_goldens.h` is up
//! to date. The firmware host test (`firmware/core-cpp-tests`) replays the
//! same bytes with the C++ initiator (`lib/pnex/src/pnex_noise.cpp`), so the
//! two sides cannot drift apart without breaking CI.
//!
//! Regen: `PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --features frame-crypto --test noise_goldens`

#![cfg(feature = "frame-crypto")]

use std::fmt::Write as _;
use std::path::PathBuf;

use pnex_core::frame::{respond_with_ephemeral, Initiator};

const DEVICE_ID: &str = "golden-dev";

fn psk() -> [u8; 32] {
    std::array::from_fn(|i| i as u8 + 1)
}

fn e_device() -> [u8; 32] {
    std::array::from_fn(|i| 0x40 + i as u8)
}

fn e_server() -> [u8; 32] {
    std::array::from_fn(|i| 0x80 + i as u8)
}

/// Payload larger than one Noise message (chunked seal).
fn large_payload() -> Vec<u8> {
    (0..70_000u32).map(|i| (i % 251) as u8).collect()
}

fn hex_array(name: &str, bytes: &[u8]) -> String {
    let mut s = format!("static const uint8_t {name}[{}] = {{", bytes.len());
    for (i, b) in bytes.iter().enumerate() {
        if i % 12 == 0 {
            s.push_str("\n    ");
        }
        let _ = write!(s, "0x{b:02x},");
    }
    s.push_str("\n};\n");
    s
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

fn render() -> String {
    let (init, msg1) = Initiator::start_with_ephemeral(&psk(), DEVICE_ID, &e_device());
    let (mut server, msg2) =
        respond_with_ephemeral(&psk(), DEVICE_ID, &msg1, &e_server()).expect("respond");
    let mut device = init.finish(&msg2).expect("finish");

    let up_ping = device.seal(b"PING");
    let up_announce = device.seal(br#"{"t":"announce"}"#);
    let up_large = device.seal(&large_payload());
    let down_pong = server.seal(b"PONG");
    // Sanity: the reference itself round-trips.
    assert_eq!(server.open(&up_ping).as_deref(), Some(&b"PING"[..]));

    let mut out = String::from(
        "// GENERATED FILE — do not edit. Reference: pnex_core::frame (Rust snow).\n\
         // Regen:\n\
         //   PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --features frame-crypto --test noise_goldens\n\
         // Replayed by firmware/core-cpp-tests against lib/pnex/src/pnex_noise.cpp (D156).\n\
         #pragma once\n\n\
         #include <cstddef>\n\
         #include <cstdint>\n\n\
         namespace pnex_noise_goldens {\n\n",
    );
    let _ = writeln!(out, "static const char DEVICE_ID[] = \"{DEVICE_ID}\";");
    out.push_str(&hex_array("PSK", &psk()));
    out.push_str(&hex_array("E_DEVICE", &e_device()));
    out.push_str(&hex_array("MSG1", &msg1));
    out.push_str(&hex_array("MSG2", &msg2));
    out.push_str(&hex_array("UP_PING", &up_ping));
    out.push_str(&hex_array("UP_ANNOUNCE", &up_announce));
    out.push_str(&hex_array("DOWN_PONG", &down_pong));
    let _ = writeln!(
        out,
        "static const size_t LARGE_LEN = {};",
        large_payload().len()
    );
    let _ = writeln!(
        out,
        "static const size_t UP_LARGE_LEN = {};",
        up_large.len()
    );
    out.push_str(&hex_array("UP_LARGE_SHA256", &sha256(&up_large)));
    out.push_str("\n}  // namespace pnex_noise_goldens\n");
    out
}

fn header_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../firmware/common_libs/pnex-core-cpp/noise_goldens.h")
}

#[test]
fn noise_goldens_are_up_to_date() {
    let path = header_path();
    let fresh = render();
    if std::env::var("PNEX_REGEN_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&path, &fresh).expect("write noise_goldens.h");
        return;
    }
    let current = std::fs::read_to_string(&path).expect("noise_goldens.h missing — run the regen");
    assert!(
        current == fresh,
        "noise_goldens.h is stale — run the regen (see the module docs)"
    );
}
