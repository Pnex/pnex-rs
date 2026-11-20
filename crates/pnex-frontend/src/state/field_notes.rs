//! Field notes waiting to be sent (parts-docs.md §3.8, PD1d): a note and its
//! photos are kept on the device, per org, until the server has them. Without
//! network they wait; [`run`] sends them again every 30 s and right after a note
//! is written. A photo already sent keeps its media object, never sent twice;
//! a note the server refuses stays with its error code until the person drops it.

use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::storage::{self, KeyValueStorage};

const KEY_PREFIX: &str = "pnex.field_notes.";
const RETRY: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Photo {
    pub name: String,
    pub content_type: String,
    /// The bytes in base64 until the photo is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Its media object once sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedNote {
    pub object_id: String,
    pub title: String,
    pub text: String,
    pub photos: Vec<Photo>,
    /// Code of the server's refusal: never sent again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Notes of the current org still on the device (UI counter).
pub static QUEUE: GlobalSignal<Vec<QueuedNote>> = Signal::global(Vec::new);

static SENDING: AtomicBool = AtomicBool::new(false);

fn key() -> Option<String> {
    crate::state::org::current().map(|o| format!("{KEY_PREFIX}{o}"))
}

fn load() -> Vec<QueuedNote> {
    key()
        .and_then(|k| storage::local().get(&k))
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes the queue; `false` when the device could not keep it (web storage full).
fn save(queue: &[QueuedNote]) -> bool {
    let Some(k) = key() else { return false };
    let text = serde_json::to_string(queue).unwrap_or_default();
    storage::local().set(&k, &text);
    let kept = storage::local().get(&k).as_deref() == Some(text.as_str());
    *QUEUE.write() = if kept { queue.to_vec() } else { load() };
    kept
}

/// Replaces `old` by `new` (or drops it) in the stored queue, which may have grown meanwhile.
fn replace(old: &QueuedNote, new: Option<QueuedNote>) {
    let mut queue = load();
    if let Some(i) = queue.iter().position(|n| n == old) {
        match new {
            Some(n) => queue[i] = n,
            None => {
                queue.remove(i);
            }
        }
        save(&queue);
    }
}

/// Keeps a note to send; `false` when the device has no room for it (its photos
/// too large for the browser storage): nothing is kept then.
pub fn push(
    object_id: &str,
    title: &str,
    text: &str,
    photos: Vec<(String, String, Vec<u8>)>,
) -> bool {
    let b64 = base64::engine::general_purpose::STANDARD;
    let note = QueuedNote {
        object_id: object_id.to_string(),
        title: title.to_string(),
        text: text.to_string(),
        photos: photos
            .into_iter()
            .map(|(name, content_type, bytes)| Photo {
                name,
                content_type,
                data: Some(b64.encode(bytes)),
                object_id: None,
            })
            .collect(),
        error: None,
    };
    let mut queue = load();
    queue.push(note);
    if !save(&queue) {
        return false;
    }
    spawn(flush());
    true
}

/// Drops a refused note.
pub fn drop_note(note: &QueuedNote) {
    replace(note, None);
}

/// Sends what waits, in order; stops at the first network failure.
pub async fn flush() {
    if SENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    for note in load().into_iter().filter(|n| n.error.is_none()) {
        match send(&note).await {
            Ok(sent) => replace(&sent, None),
            Err(Stop::Network) => break,
            Err(Stop::Refused(code, sent)) => replace(
                &sent,
                Some(QueuedNote {
                    error: Some(code),
                    ..sent.clone()
                }),
            ),
        }
    }
    *QUEUE.write() = load();
    SENDING.store(false, Ordering::Release);
}

enum Stop {
    Network,
    /// The server's code, and the note as stored now (its photos sent).
    Refused(String, QueuedNote),
}

fn stop(e: api::error::ApiError, note: &QueuedNote) -> Stop {
    match e.status {
        None => Stop::Network,
        // An expired session or a busy server: sent again later.
        Some(s) if s >= 500 || s == 401 || s == 408 || s == 429 => Stop::Network,
        Some(_) => Stop::Refused(e.code.unwrap_or(e.message), note.clone()),
    }
}

/// Sends one note; answers it as stored now (its photos sent), to drop.
async fn send(note: &QueuedNote) -> Result<QueuedNote, Stop> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut stored = note.clone();
    for i in 0..stored.photos.len() {
        if stored.photos[i].object_id.is_some() {
            continue;
        }
        let p = stored.photos[i].clone();
        let bytes = b64
            .decode(p.data.as_deref().unwrap_or_default())
            .unwrap_or_default();
        let params = api::media::UploadParams {
            name: Some(p.name.clone()),
            filename: Some(p.name.clone()),
            content_type: Some(p.content_type.clone()),
            ..Default::default()
        };
        let asset = api::media::upload(&params, bytes)
            .await
            .map_err(|e| stop(e, &stored))?;
        let object: serde_json::Value = api::client::request(
            reqwest::Method::GET,
            &format!("/api/v1/ontology/resolve/media_asset/{}", asset.id),
            None,
        )
        .await
        .map_err(|e| stop(e, &stored))?;
        let mut next = stored.clone();
        next.photos[i] = Photo {
            data: None,
            object_id: object["id"].as_str().map(str::to_string),
            ..p
        };
        // Kept at once: a later failure never sends this photo again.
        replace(&stored, Some(next.clone()));
        stored = next;
    }
    let body = serde_json::json!({
        "type_key": "field_note",
        "title": stored.title,
        "values": {"text": stored.text},
        "link_type": "note_on",
        "link_target_id": stored.object_id,
        "links": stored.photos.iter().filter_map(|p| p.object_id.as_ref())
            .map(|id| serde_json::json!({"link_type": "note_photo", "target_id": id}))
            .collect::<Vec<_>>(),
    });
    api::client::request::<serde_json::Value>(
        reqwest::Method::POST,
        "/api/v1/proposals",
        Some(body),
    )
    .await
    .map_err(|e| stop(e, &stored))?;
    Ok(stored)
}

/// Sends what waits now and every 30 s, for the app's life.
pub async fn run() {
    loop {
        *QUEUE.write() = load();
        flush().await;
        crate::util::sleep(RETRY).await;
    }
}
