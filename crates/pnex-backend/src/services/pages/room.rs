//! Live collaboration on a page (P4): one in-memory room per page open on
//! this pod, the Yjs sync protocol (`y-protocols`, via `yrs::sync`) with
//! each browser session, and the cross-pod fan-out on the Valkey channel
//! `pnex:doc:<id>` (camera relay school: subscribed while the room has
//! local sessions).
//!
//! An update from a session is size-checked, applied to a scratch copy,
//! checked against the schema allowlist, **persisted**, then applied to the
//! room and relayed (local sessions + bus). A pod receiving it from the bus
//! only applies and relays: the origin pod already validated and stored
//! it. Presence (awareness) is relayed as is and never stored.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use pnex_core::pages::{MAX_DOC_BYTES, MAX_UPDATE_BYTES};
use redis::AsyncCommands;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use uuid::Uuid;
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, Transact};

use super::{append, apply, compact, encode, load, schema, PageError, COMPACT_AFTER};

/// Downlink of one browser session (binary y-protocols messages).
pub type Downlink = mpsc::UnboundedSender<Vec<u8>>;

pub struct Room {
    pub doc_id: Uuid,
    doc: Mutex<Doc>,
    sessions: Mutex<HashMap<Uuid, Downlink>>,
    /// Updates logged by this pod since the last compaction it triggered.
    pending: AtomicU64,
    relay: AtomicBool,
    valkey_url: Option<String>,
}

static ROOMS: LazyLock<Mutex<HashMap<Uuid, Arc<Room>>>> = LazyLock::new(Default::default);

/// What the websocket loop does after a message.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    /// Close the session with this code and reason.
    Close(u16, &'static str),
}

fn channel(doc_id: Uuid) -> String {
    format!("pnex:doc:{doc_id}")
}

/// Joins (opening if needed) the room of `doc_id`; returns it with the
/// first message for the client (server sync step 1).
pub async fn join(
    db: &DatabaseConnection,
    valkey_url: Option<String>,
    doc_id: Uuid,
    session: Uuid,
    tx: Downlink,
) -> Result<Arc<Room>, PageError> {
    // Join and leave both hold ROOMS: a session never lands in a room that
    // is being dropped.
    let enter = |room: &Arc<Room>| {
        room.sessions
            .lock()
            .expect("sessions")
            .insert(session, tx.clone());
        room.clone()
    };
    let open = ROOMS.lock().expect("rooms").get(&doc_id).map(enter);
    let room = match open {
        Some(room) => room,
        None => {
            let loaded = load(db, doc_id).await?;
            let mut rooms = ROOMS.lock().expect("rooms");
            // Another session may have opened it while this one loaded.
            let room = rooms.entry(doc_id).or_insert_with(|| {
                Arc::new(Room {
                    doc_id,
                    doc: Mutex::new(loaded),
                    sessions: Mutex::default(),
                    pending: AtomicU64::new(0),
                    relay: AtomicBool::new(false),
                    valkey_url,
                })
            });
            enter(room)
        }
    };
    let sv = room.doc.lock().expect("doc").transact().state_vector();
    let _ = tx.send(Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1());
    // Ask the others for their presence so the newcomer sees them at once.
    room.relay_out(session, Message::AwarenessQuery.encode_v1());
    room.ensure_relay(db.clone());
    Ok(room)
}

/// Leaves the room; the last session out closes it and compacts the page.
pub fn leave(db: &DatabaseConnection, room: &Arc<Room>, session: Uuid) {
    let mut rooms = ROOMS.lock().expect("rooms");
    let empty = {
        let mut sessions = room.sessions.lock().expect("sessions");
        sessions.remove(&session);
        sessions.is_empty()
    };
    if empty
        && rooms
            .get(&room.doc_id)
            .is_some_and(|r| Arc::ptr_eq(r, room))
    {
        rooms.remove(&room.doc_id);
        drop(rooms);
        spawn_compact(db.clone(), room.doc_id);
    }
}

fn spawn_compact(db: DatabaseConnection, doc_id: Uuid) {
    tokio::spawn(async move {
        if let Err(e) = compact(&db, doc_id).await {
            tracing::warn!(%doc_id, error = %e, "pages: compaction failed");
        }
    });
}

impl Room {
    /// Handles one binary message of `session`.
    pub async fn handle(
        &self,
        db: &DatabaseConnection,
        session: Uuid,
        user_id: i64,
        can_write: bool,
        raw: &[u8],
    ) -> Outcome {
        // Presence frames are relayed as is: bound them like updates.
        if raw.len() > MAX_UPDATE_BYTES {
            return Outcome::Close(4413, "Message too large");
        }
        let Ok(msg) = Message::decode_v1(raw) else {
            return Outcome::Close(4400, "Unreadable message");
        };
        match msg {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                let diff = self
                    .doc
                    .lock()
                    .expect("doc")
                    .transact()
                    .encode_state_as_update_v1(&sv);
                self.send_to(
                    session,
                    Message::Sync(SyncMessage::SyncStep2(diff)).encode_v1(),
                );
                Outcome::Continue
            }
            Message::Sync(SyncMessage::SyncStep2(u) | SyncMessage::Update(u)) => {
                // Viewers are read-only (R2): their updates are dropped here,
                // whatever the client claims.
                if !can_write {
                    return Outcome::Continue;
                }
                self.write(db, session, user_id, u).await
            }
            Message::Awareness(_) | Message::AwarenessQuery => {
                self.relay_out(session, raw.to_vec());
                Outcome::Continue
            }
            Message::Auth(_) | Message::Custom(..) => Outcome::Continue,
        }
    }

    async fn write(
        &self,
        db: &DatabaseConnection,
        session: Uuid,
        user_id: i64,
        u: Vec<u8>,
    ) -> Outcome {
        if u.len() > MAX_UPDATE_BYTES {
            return Outcome::Close(4413, "Update too large");
        }
        // shortcut: the scratch copy costs O(page) per update; fine for
        // pages of a few hundred KB, check only the touched branches beyond.
        let before = encode(&self.doc.lock().expect("doc"));
        let scratch = Doc::new();
        if apply(&scratch, &before).is_err() || apply(&scratch, &u).is_err() {
            return Outcome::Close(4400, "Unreadable update");
        }
        let after = encode(&scratch);
        if after == before {
            return Outcome::Continue; // nothing new (typical sync step 2)
        }
        if after.len() > MAX_DOC_BYTES {
            return Outcome::Close(4413, "Page too large");
        }
        if let Err(e) = schema::validate(&scratch) {
            tracing::warn!(doc_id = %self.doc_id, reason = %e.0, "pages: update refused by the schema");
            return Outcome::Close(4422, "Content refused");
        }
        if let Err(e) = append(db, self.doc_id, u.clone(), user_id).await {
            tracing::warn!(doc_id = %self.doc_id, error = %e, "pages: update not persisted");
            return Outcome::Close(1011, "Not saved");
        }
        let _ = apply(&self.doc.lock().expect("doc"), &u);
        self.relay_out(session, Message::Sync(SyncMessage::Update(u)).encode_v1());
        if self.pending.fetch_add(1, Ordering::Relaxed) + 1 >= COMPACT_AFTER {
            self.pending.store(0, Ordering::Relaxed);
            spawn_compact(db.clone(), self.doc_id);
        }
        Outcome::Continue
    }

    fn send_to(&self, session: Uuid, frame: Vec<u8>) {
        if let Some(tx) = self.sessions.lock().expect("sessions").get(&session) {
            let _ = tx.send(frame);
        }
    }

    /// To every local session but `from`, then to the other pods.
    fn relay_out(&self, from: Uuid, frame: Vec<u8>) {
        self.broadcast_local(Some(from), &frame);
        let Some(url) = self.valkey_url.clone() else {
            return;
        };
        let channel = channel(self.doc_id);
        let mut payload = crate::services::device_bus::pod_id().as_bytes().to_vec();
        payload.push(b'\n');
        payload.extend_from_slice(&frame);
        tokio::spawn(async move {
            let Some(mut conn) = crate::services::shared_valkey::conn_url(&url).await else {
                return;
            };
            let res: redis::RedisResult<i64> = conn.publish(channel, payload).await;
            if let Err(e) = res {
                tracing::warn!(error = %e, "pages bus: publish failed");
            }
        });
    }

    fn broadcast_local(&self, except: Option<Uuid>, frame: &[u8]) {
        for (id, tx) in self.sessions.lock().expect("sessions").iter() {
            if Some(*id) != except {
                let _ = tx.send(frame.to_vec());
            }
        }
    }

    fn is_open(self: &Arc<Self>) -> bool {
        ROOMS
            .lock()
            .expect("rooms")
            .get(&self.doc_id)
            .is_some_and(|r| Arc::ptr_eq(r, self))
    }

    fn ensure_relay(self: &Arc<Self>, db: DatabaseConnection) {
        if self.valkey_url.is_none() || self.relay.swap(true, Ordering::AcqRel) {
            return;
        }
        tokio::spawn(relay_loop(self.clone(), db));
    }

    /// Applies what other pods logged (a bus message may have been missed
    /// while subscribing) and pushes the difference to local sessions.
    async fn catch_up(&self, db: &DatabaseConnection) {
        let fresh = match load(db, self.doc_id).await {
            Ok(doc) => encode(&doc),
            Err(e) => {
                tracing::warn!(doc_id = %self.doc_id, error = %e, "pages: catch-up failed");
                return;
            }
        };
        let diff = {
            let doc = self.doc.lock().expect("doc");
            let sv = doc.transact().state_vector();
            let _ = apply(&doc, &fresh);
            let txn = doc.transact();
            txn.encode_state_as_update_v1(&sv)
        };
        self.broadcast_local(None, &Message::Sync(SyncMessage::Update(diff)).encode_v1());
    }

    /// A frame relayed by another pod.
    fn on_bus(&self, payload: &[u8]) {
        let Some(split) = payload.iter().position(|b| *b == b'\n') else {
            return;
        };
        if &payload[..split] == crate::services::device_bus::pod_id().as_bytes() {
            return;
        }
        let frame = &payload[split + 1..];
        if let Ok(Message::Sync(SyncMessage::Update(u))) = Message::decode_v1(frame) {
            let _ = apply(&self.doc.lock().expect("doc"), &u);
        }
        self.broadcast_local(None, frame);
    }
}

async fn relay_loop(room: Arc<Room>, db: DatabaseConnection) {
    let Some(url) = room.valkey_url.clone() else {
        return;
    };
    let mut backoff = Duration::from_millis(500);
    'outer: while room.is_open() {
        let pubsub = match redis::Client::open(url.as_str()) {
            Ok(client) => client.get_async_pubsub().await,
            Err(e) => {
                tracing::error!(error = %e, "pages relay: invalid valkey url");
                break;
            }
        };
        let mut pubsub = match pubsub {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "pages relay: pubsub connection failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
        };
        if let Err(e) = pubsub.subscribe(channel(room.doc_id)).await {
            tracing::warn!(error = %e, "pages relay: subscribe failed");
            tokio::time::sleep(backoff).await;
            continue;
        }
        backoff = Duration::from_millis(500);
        room.catch_up(&db).await;
        let mut stream = pubsub.on_message();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
            if !room.is_open() {
                break 'outer;
            }
            match next {
                Err(_) => continue,
                Ok(None) => break, // connection lost: resubscribe
                Ok(Some(msg)) => {
                    if let Ok(payload) = msg.get_payload::<Vec<u8>>() {
                        room.on_bus(&payload);
                    }
                }
            }
        }
    }
    room.relay.store(false, Ordering::Release);
}
