//! Client d'ingestion de test — joue le rôle du firmware (mimique du
//! soil_sensor : PING + `key=value` chiffrés ChaCha20, cf.
//! docs/phase0/ws-channels-crypto.md).
//!
//! Usage (valeurs = celles affichées par l'API à la création du device) :
//! ```sh
//! cargo run -p pnex-backend --example ingest_client -- \
//!   --url ws://localhost:5150/ws/sensor/ingest \
//!   --token "$DEVICE_TOKEN" --device-id "$DEVICE_ID" --key "$KEY_B64" \
//!   --metric read_temperature --interval-ms 1000 [--count 20]
//! ```
//! Avec `--hold` : reste connecté sans rien envoyer (teste l'anti-clone).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::{ChaCha20, Key, Nonce};
use rand::RngCore;

fn encrypt(plain: &str, key: &[u8; 32]) -> String {
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let mut buf = plain.as_bytes().to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(&nonce))
        .apply_keystream(&mut buf);
    let mut wire = nonce.to_vec();
    wire.extend_from_slice(&buf);
    STANDARD.encode(wire)
}

fn decrypt(raw: &str, key: &[u8; 32]) -> String {
    let bytes = STANDARD.decode(raw.trim()).expect("frame b64");
    let (nonce, ct) = bytes.split_at(12);
    let mut buf = ct.to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(nonce))
        .apply_keystream(&mut buf);
    String::from_utf8(buf).expect("utf8")
}

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
    assert!(!url.is_empty() && !token.is_empty() && !device_id.is_empty() && !key_b64.is_empty(),
        "--url, --token, --device-id, --key requis");

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
    let send = |plain: &str| encrypt(plain, &key);
    write
        .send(tokio_tungstenite::tungstenite::Message::Text(send("PING").into()))
        .await
        .expect("envoi PING");
    let msg = read.next().await.expect("PONG attendu").expect("ws");
    match msg {
        tokio_tungstenite::tungstenite::Message::Close(frame) => {
            let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
            println!("✗ rejeté par le serveur (close {code})");
            std::process::exit(2);
        }
        other => println!("← {}", decrypt(&other.into_text().expect("texte"), &key)),
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
            .send(tokio_tungstenite::tungstenite::Message::Text(send(&frame).into()))
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
        println!("← {} (frame {i}/{count})", decrypt(&msg.into_text().expect("texte"), &key));
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
    }
    println!("✓ {count} mesures envoyées");
}
