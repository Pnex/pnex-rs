//! Test ingestion client — plays the firmware (soil_sensor mimic: PING +
//! `key=value` frames over the Noise NNpsk0 link, D156).
//!
//! Usage (values = those shown by the API when the device is created):
//! ```sh
//! cargo run -p pnex-backend --example ingest_client -- \
//!   --url ws://localhost:5150/ws/sensor/ingest \
//!   --token "$DEVICE_TOKEN" --device-id "$DEVICE_ID" --key "$KEY_B64" \
//!   --metric read_temperature --interval-ms 1000 [--count 20]
//! ```
//! With `--hold`: stays connected without sending anything (anti-clone test).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use pnex_core::frame::{decode_handshake, encode_handshake, Initiator};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut url = String::new();
    let mut token = String::new();
    let mut device_id = String::new();
    let mut key_b64 = String::new();
    let mut metric = "read_temperature".to_string();
    let mut interval_ms = 1000u64;
    let mut count = 20u64;
    let mut hold = false;
    while let Some(arg) = args.next() {
        let mut value = || args.next().expect("valeur manquante");
        match arg.as_str() {
            "--url" => url = value(),
            "--token" => token = value(),
            "--device-id" => device_id = value(),
            "--key" => key_b64 = value(),
            "--metric" => metric = value(),
            "--interval-ms" => interval_ms = value().parse().expect("ms"),
            "--count" => count = value().parse().expect("count"),
            "--hold" => hold = true,
            other => panic!("argument inconnu : {other}"),
        }
    }
    assert!(
        !url.is_empty() && !token.is_empty() && !device_id.is_empty() && !key_b64.is_empty(),
        "--url, --token, --device-id, --key requis"
    );

    let key: [u8; 32] = STANDARD
        .decode(key_b64.trim())
        .expect("clé b64")
        .try_into()
        .expect("clé 32 octets");
    let query = format!(
        "token={}&device_id={}",
        STANDARD.encode(token.trim()),
        STANDARD.encode(device_id.trim()),
    );
    let full = format!("{url}?{query}");
    println!("→ connexion {url}?token=…&device_id=…");

    let (ws, _) = tokio_tungstenite::connect_async(full)
        .await
        .expect("connexion WS");
    let (mut write, mut read) = ws.split();

    use futures_util::{SinkExt, StreamExt};
    // Noise handshake first (text link: base64 of the handshake messages).
    let (init, msg1) = Initiator::start(&key, device_id.trim());
    write
        .send(tokio_tungstenite::tungstenite::Message::Text(
            encode_handshake(&msg1).into(),
        ))
        .await
        .expect("send handshake");
    let answer = read.next().await.expect("handshake answer").expect("ws");
    if let tokio_tungstenite::tungstenite::Message::Close(frame) = &answer {
        let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
        println!("✗ refused by the server (close {code})");
        std::process::exit(2);
    }
    let msg2 = decode_handshake(&answer.into_text().expect("text")).expect("handshake b64");
    let mut link = init.finish(&msg2).expect("Noise handshake");

    write
        .send(tokio_tungstenite::tungstenite::Message::Text(
            link.seal_text("PING").into(),
        ))
        .await
        .expect("send PING");
    let msg = read.next().await.expect("PONG expected").expect("ws");
    match msg {
        tokio_tungstenite::tungstenite::Message::Close(frame) => {
            let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
            println!("✗ refused by the server (close {code})");
            std::process::exit(2);
        }
        other => println!(
            "← {}",
            link.open_text(&other.into_text().expect("text"))
                .expect("authenticated frame")
        ),
    }

    if hold {
        println!("— mode hold : connexion maintenue, Ctrl-C pour quitter");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }

    for i in 1..=count {
        let frame = format!("{metric}={}", 18.0 + i as f64 / 10.0);
        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                link.seal_text(&frame).into(),
            ))
            .await
            .expect("envoi mesure");
        let msg = match read.next().await {
            Some(Ok(m)) => m,
            other => {
                println!("✗ connexion fermée par le serveur : {other:?}");
                return;
            }
        };
        if let tokio_tungstenite::tungstenite::Message::Close(frame) = &msg {
            let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
            println!("✗ rejeté par le serveur (close {code})");
            return;
        }
        println!(
            "← {} (frame {i}/{count})",
            link.open_text(&msg.into_text().expect("text"))
                .expect("authenticated frame")
        );
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
    }
    println!("✓ {count} mesures envoyées");
}
