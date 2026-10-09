//! Device/agent link encryption, shared by the backend and the edge agent.
//!
//! Inside the TLS tunnel (mutual TLS, D153), every device connection runs
//! the Noise Protocol Framework pattern `Noise_NNpsk0_25519_ChaChaPoly_SHA256`
//! (D156) keyed by the device's 32-byte pre-shared key
//! (`device_tokens.encryption_key`):
//!
//! 1. the device sends the first Noise message (`-> psk, e`) as its first
//!    WebSocket frame;
//! 2. the server answers the second one (`<- e, ee`);
//! 3. both sides then exchange Noise transport messages: ChaCha20-Poly1305
//!    with an implicit counter nonce per direction, so a forged, modified,
//!    reordered or replayed frame fails to decrypt, and a frame of another
//!    connection too (fresh ephemeral keys).
//!
//! **This layer is not redundant with TLS (D155):** TLS agrees its keys with
//! ECDHE, breakable by a future quantum computer ("harvest now, decrypt
//! later"); the 256-bit pre-shared key mixed into Noise is not. Do not remove
//! it as "double encryption".
//!
//! The prologue binds the handshake to the device id, so a key can only open
//! the connection of its own device. Text links carry base64 of the Noise
//! messages, the camera link raw bytes. A payload larger than one Noise
//! message (65535 bytes) is sealed as consecutive chunks in one frame.
//!
//! The legacy D8 format (ChaCha20 without Poly1305) is gone (D157): a device
//! built before this protocol is refused and must be rebuilt.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;

/// Noise protocol name (D156).
pub const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
/// Largest Noise message (spec §3).
pub const NOISE_MAX_MSG: usize = 65535;
/// Poly1305 tag appended to every transport message.
pub const NOISE_TAG_LEN: usize = 16;
/// Largest plaintext of one transport chunk.
pub const NOISE_MAX_CHUNK: usize = NOISE_MAX_MSG - NOISE_TAG_LEN;
/// First handshake message length (`e` public key, empty encrypted payload).
pub const NOISE_MSG1_LEN: usize = 32 + NOISE_TAG_LEN;
/// Second handshake message length (`e` public key, empty encrypted payload).
pub const NOISE_MSG2_LEN: usize = 32 + NOISE_TAG_LEN;
/// Prologue prefix; the device id follows.
pub const NOISE_PROLOGUE_PREFIX: &[u8] = b"PNEX-NOISE-1|";

/// Decodes a base64 32-byte key (`STANDARD`, tolerant to URL-safe).
pub fn decode_key(b64: &str) -> Option<[u8; 32]> {
    let raw = STANDARD
        .decode(b64.trim())
        .or_else(|_| URL_SAFE_NO_PAD.decode(b64.trim()))
        .ok()?;
    raw.try_into().ok()
}

fn prologue(device_id: &str) -> Vec<u8> {
    let mut p = NOISE_PROLOGUE_PREFIX.to_vec();
    p.extend_from_slice(device_id.as_bytes());
    p
}

fn builder<'a>(psk: &'a [u8; 32], prologue: &'a [u8]) -> snow::Builder<'a> {
    snow::Builder::new(NOISE_PARAMS.parse().expect("valid Noise params"))
        .psk(0, psk)
        .expect("psk slot 0")
        .prologue(prologue)
        .expect("prologue set once")
}

/// Device side of the handshake, between its first and second message.
pub struct Initiator(snow::HandshakeState);

impl Initiator {
    /// Starts a handshake: returns the state and the first message to send.
    pub fn start(psk: &[u8; 32], device_id: &str) -> (Self, Vec<u8>) {
        let p = prologue(device_id);
        let hs = builder(psk, &p).build_initiator().expect("initiator");
        Self::first_message(hs)
    }

    /// Deterministic variant (golden vectors / tests).
    pub fn start_with_ephemeral(psk: &[u8; 32], device_id: &str, e: &[u8; 32]) -> (Self, Vec<u8>) {
        let p = prologue(device_id);
        let hs = builder(psk, &p)
            .fixed_ephemeral_key_for_testing_only(e)
            .build_initiator()
            .expect("initiator");
        Self::first_message(hs)
    }

    fn first_message(mut hs: snow::HandshakeState) -> (Self, Vec<u8>) {
        let mut msg = vec![0u8; NOISE_MSG1_LEN];
        let n = hs.write_message(&[], &mut msg).expect("first message");
        msg.truncate(n);
        (Self(hs), msg)
    }

    /// Reads the server's answer; `None` = wrong key, wrong device or
    /// corrupted message.
    pub fn finish(mut self, msg2: &[u8]) -> Option<Link> {
        if msg2.len() > NOISE_MAX_MSG {
            return None;
        }
        let mut payload = vec![0u8; NOISE_MAX_MSG];
        self.0.read_message(msg2, &mut payload).ok()?;
        Some(Link(self.0.into_transport_mode().ok()?))
    }
}

/// Server side of the handshake: reads the device's first message and
/// returns the link plus the answer to send. `None` = wrong key (the
/// device does not hold this token's key), wrong device id, or garbage.
pub fn respond(psk: &[u8; 32], device_id: &str, msg1: &[u8]) -> Option<(Link, Vec<u8>)> {
    let p = prologue(device_id);
    respond_with(builder(psk, &p), msg1)
}

/// Deterministic variant (golden vectors / tests).
pub fn respond_with_ephemeral(
    psk: &[u8; 32],
    device_id: &str,
    msg1: &[u8],
    e: &[u8; 32],
) -> Option<(Link, Vec<u8>)> {
    let p = prologue(device_id);
    respond_with(
        builder(psk, &p).fixed_ephemeral_key_for_testing_only(e),
        msg1,
    )
}

fn respond_with(b: snow::Builder<'_>, msg1: &[u8]) -> Option<(Link, Vec<u8>)> {
    if msg1.len() > NOISE_MAX_MSG {
        return None;
    }
    let mut hs = b.build_responder().ok()?;
    let mut payload = vec![0u8; NOISE_MAX_MSG];
    hs.read_message(msg1, &mut payload).ok()?;
    let mut msg2 = vec![0u8; NOISE_MSG2_LEN];
    let n = hs.write_message(&[], &mut msg2).ok()?;
    msg2.truncate(n);
    Some((Link(hs.into_transport_mode().ok()?), msg2))
}

/// An established link: Noise transport state of one side.
pub struct Link(snow::TransportState);

impl Link {
    /// Seals `plain` as one frame (consecutive Noise messages when it does
    /// not fit one).
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let chunks = plain.len().div_ceil(NOISE_MAX_CHUNK).max(1);
        let mut wire = Vec::with_capacity(plain.len() + chunks * NOISE_TAG_LEN);
        let mut buf = vec![0u8; NOISE_MAX_MSG];
        let mut rest = plain;
        loop {
            let (chunk, tail) = rest.split_at(rest.len().min(NOISE_MAX_CHUNK));
            let n = self
                .0
                .write_message(chunk, &mut buf)
                .expect("chunk within the Noise limits");
            wire.extend_from_slice(&buf[..n]);
            rest = tail;
            if rest.is_empty() {
                break;
            }
        }
        wire
    }

    /// Opens a frame of the peer; `None` = forged, modified, reordered,
    /// replayed, or from another connection. A rejected single-chunk frame
    /// leaves the link usable (the Noise counter only moves on success); a
    /// multi-chunk frame rejected midway desynchronizes it, so callers
    /// receiving large frames (camera) close the link on failure.
    pub fn open(&mut self, wire: &[u8]) -> Option<Vec<u8>> {
        if wire.len() < NOISE_TAG_LEN {
            return None;
        }
        let mut plain = Vec::with_capacity(wire.len());
        let mut buf = vec![0u8; NOISE_MAX_MSG];
        for chunk in wire.chunks(NOISE_MAX_MSG) {
            if chunk.len() < NOISE_TAG_LEN {
                return None;
            }
            let n = self.0.read_message(chunk, &mut buf).ok()?;
            plain.extend_from_slice(&buf[..n]);
        }
        Some(plain)
    }

    /// Text frame: base64 of [`Link::seal`].
    pub fn seal_text(&mut self, plain: &str) -> String {
        STANDARD.encode(self.seal(plain.as_bytes()))
    }

    /// Text frame: [`Link::open`] of base64, UTF-8 required.
    pub fn open_text(&mut self, raw: &str) -> Option<String> {
        let bytes = STANDARD.decode(raw.trim()).ok()?;
        String::from_utf8(self.open(&bytes)?).ok()
    }
}

/// Base64 text form of a handshake message (text links).
pub fn encode_handshake(msg: &[u8]) -> String {
    STANDARD.encode(msg)
}

/// Handshake message out of a text frame.
pub fn decode_handshake(raw: &str) -> Option<Vec<u8>> {
    STANDARD.decode(raw.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PSK: [u8; 32] = [9u8; 32];

    fn pair(device: &str) -> (Link, Link) {
        let (init, msg1) = Initiator::start(&PSK, device);
        let (server, msg2) = respond(&PSK, device, &msg1).expect("respond");
        (init.finish(&msg2).expect("finish"), server)
    }

    #[test]
    fn handshake_and_both_directions() {
        let (mut dev, mut srv) = pair("dev-a");
        let up = dev.seal_text("{\"t\":\"announce\"}");
        assert_eq!(srv.open_text(&up).as_deref(), Some("{\"t\":\"announce\"}"));
        let down = srv.seal_text("PONG");
        assert_eq!(dev.open_text(&down).as_deref(), Some("PONG"));
    }

    #[test]
    fn handshake_messages_have_the_documented_length() {
        let (init, msg1) = Initiator::start(&PSK, "d");
        assert_eq!(msg1.len(), NOISE_MSG1_LEN);
        let (_, msg2) = respond(&PSK, "d", &msg1).unwrap();
        assert_eq!(msg2.len(), NOISE_MSG2_LEN);
        assert!(init.finish(&msg2).is_some());
    }

    #[test]
    fn wrong_key_or_wrong_device_fails_the_handshake() {
        let (_, msg1) = Initiator::start(&PSK, "dev-a");
        assert!(respond(&[1u8; 32], "dev-a", &msg1).is_none(), "other key");
        assert!(respond(&PSK, "dev-b", &msg1).is_none(), "other device id");
        assert!(respond(&PSK, "dev-a", b"short").is_none());
    }

    #[test]
    fn replay_reorder_and_tampering_fail() {
        let (mut dev, mut srv) = pair("d");
        let _first = dev.seal(b"a");
        let second = dev.seal(b"b");
        assert!(srv.open(&second).is_none(), "out of order");

        let (mut dev, mut srv) = pair("d");
        let frame = dev.seal(b"x");
        assert!(srv.open(&frame).is_some());
        assert!(srv.open(&frame).is_none(), "same frame twice");

        let (mut dev, mut srv) = pair("d");
        let mut wire = dev.seal(b"{\"t\":\"write\",\"v\":1}");
        wire[3] ^= 0x01;
        assert!(srv.open(&wire).is_none(), "flipped bit");
    }

    #[test]
    fn frames_do_not_cross_connections() {
        let (mut old_dev, _) = pair("d");
        let captured = old_dev.seal(b"{\"t\":\"write\"}");
        let (_, mut fresh_srv) = pair("d");
        assert!(fresh_srv.open(&captured).is_none());
    }

    #[test]
    fn large_payloads_are_chunked() {
        let (mut dev, mut srv) = pair("cam");
        for len in [
            0,
            1,
            NOISE_MAX_CHUNK,
            NOISE_MAX_CHUNK + 1,
            3 * NOISE_MAX_CHUNK + 7,
        ] {
            let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let wire = dev.seal(&plain);
            assert_eq!(
                wire.len(),
                len + len.div_ceil(NOISE_MAX_CHUNK).max(1) * NOISE_TAG_LEN
            );
            assert_eq!(srv.open(&wire).as_deref(), Some(&plain[..]), "len {len}");
        }
    }

    #[test]
    fn decode_key_requires_32_bytes() {
        assert!(decode_key(&STANDARD.encode([3u8; 32])).is_some());
        assert!(decode_key(&STANDARD.encode([3u8; 31])).is_none());
        assert!(decode_key("!!!").is_none());
    }
}
