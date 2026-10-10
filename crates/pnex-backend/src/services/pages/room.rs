//! Live collaboration on a page (P4): one in-memory room per page open on
//! this pod, the Yjs sync protocol (`y-protocols`, via `yrs::sync`) with
//! each browser session, and the cross-pod fan-out on the Valkey channel
//! `pnex:doc:<id>` (camera relay school: subscribed while the room has
//! local sessions).
//!
//! An update from a session is size-checked, applied to a scratch copy,
//! refused if it depends on content the page does not have (it would stay
//! pending, unchecked), checked against the schema allowlist, **persisted**,
//! then applied to the room and relayed (local sessions + bus). A pod
//! receiving it from the bus only applies and relays: the origin pod
//! already validated and stored it; a periodic catch-up from the database
//! covers a lost bus message.
//!
//! Presence (awareness) is relayed, never stored. Each session owns the
//! client ids it announced first (no one else can rewrite or clear them),
//! and its presence is cleared for everyone when it leaves.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::StreamExt;
use pnex_core::pages::{MAX_DOC_BYTES, MAX_UPDATE_BYTES};
use redis::AsyncCommands;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use uuid::Uuid;
use yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, ReadTxn, Transact};

use super::{append, apply, compact, encode, load, schema, PageError, COMPACT_AFTER};

/// Frames queued for one browser session before it is dropped as too slow
/// (half-open socket, stalled tab).
pub const DOWNLINK_CAPACITY: usize = 1024;
/// Downlink of one browser session (binary y-protocols messages).
pub type Downlink = mpsc::Sender<Vec<u8>>;

/// Database catch-up period while the room is open (lost bus messages).
const CATCH_UP_EVERY: Duration = Duration::from_secs(30);

struct Session {
    tx: Downlink,
    /// Presence client ids announced by this session, with their last clock.
    clients: HashMap<ClientID, u32>,
}

pub struct Room {
    pub doc_id: Uuid,
    doc: Mutex<Doc>,
    sessions: Mutex<HashMap<Uuid, Session>>,
    /// Updates logged by this pod since the last compaction it triggered.
    pending: AtomicU64,
    relay: AtomicBool,
    valkey_url: Option<String>,
}

static ROOMS: LazyLock<Mutex<HashMap<Uuid, Arc<Room>>>> = LazyLock::new(Default::default);

/// Lock that survives a panic of another holder: a poisoned page must not
/// take down every later session of it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

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

/// Joins (opening if needed) the room of `doc_id` and sends the client the
/// server sync step 1.
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
        lock(&room.sessions).insert(
            session,
            Session {
                tx: tx.clone(),
                clients: HashMap::new(),
            },
        );
        room.clone()
    };
    let open = lock(&ROOMS).get(&doc_id).map(enter);
    let room = match open {
        Some(room) => room,
        None => {
            let loaded = load(db, doc_id).await?;
            let mut rooms = lock(&ROOMS);
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
    let sv = lock(&room.doc).transact().state_vector();
    room.send_to(
        session,
        Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1(),
    );
    // Ask the others for their presence so the newcomer sees them at once.
    room.relay_out(session, Message::AwarenessQuery.encode_v1());
    room.ensure_relay(db.clone());
    Ok(room)
}

/// Leaves the room (clears the session's presence for everyone); the last
/// session out closes it and compacts the page.
pub fn leave(db: &DatabaseConnection, room: &Arc<Room>, session: Uuid) {
    let mut rooms = lock(&ROOMS);
    let (gone, empty) = {
        let mut sessions = lock(&room.sessions);
        let gone = sessions.remove(&session);
        (gone, sessions.is_empty())
    };
    let room_closes = empty
        && rooms
            .get(&room.doc_id)
            .is_some_and(|r| Arc::ptr_eq(r, room));
    if room_closes {
        rooms.remove(&room.doc_id);
    }
    drop(rooms);
    if let Some(gone) = gone.filter(|s| !s.clients.is_empty()) {
        let clients = gone
            .clients
            .into_iter()
            .map(|(id, clock)| {
                let entry = AwarenessUpdateEntry {
                    clock: clock + 1,
                    json: "null".into(),
                };
                (id, entry)
            })
            .collect();
        room.relay_out(
            session,
            Message::Awareness(AwarenessUpdate { clients }).encode_v1(),
        );
    }
    if room_closes {
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
                let diff = lock(&self.doc).transact().encode_state_as_update_v1(&sv);
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
            Message::Awareness(update) => {
                if self.claim_presence(session, &update) {
                    self.relay_out(session, raw.to_vec());
                }
                Outcome::Continue
            }
            Message::AwarenessQuery => {
                self.relay_out(session, raw.to_vec());
                Outcome::Continue
            }
            Message::Auth(_) | Message::Custom(..) => Outcome::Continue,
        }
    }

    /// Records the presence ids of `session`; `false` (frame dropped) when
    /// one of them belongs to another session of this room.
    fn claim_presence(&self, session: Uuid, update: &AwarenessUpdate) -> bool {
        let mut sessions = lock(&self.sessions);
        let taken = update.clients.keys().any(|id| {
            sessions
                .iter()
                .any(|(other, s)| *other != session && s.clients.contains_key(id))
        });
        if taken {
            return false;
        }
        if let Some(me) = sessions.get_mut(&session) {
            for (id, entry) in &update.clients {
                me.clients.insert(*id, entry.clock);
            }
        }
        true
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
        let before = encode(&lock(&self.doc));
        let scratch = Doc::new();
        if apply(&scratch, &before).is_err() || apply(&scratch, &u).is_err() {
            return Outcome::Close(4400, "Unreadable update");
        }
        // A well-behaved client sends updates the page can integrate; a
        // pending one would be stored unchecked and checked later against
        // someone else's edit.
        if scratch.transact().has_missing_updates() {
            return Outcome::Close(4422, "Content refused");
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
        let _ = apply(&lock(&self.doc), &u);
        self.relay_out(session, Message::Sync(SyncMessage::Update(u)).encode_v1());
        if self.pending.fetch_add(1, Ordering::Relaxed) + 1 >= COMPACT_AFTER {
            self.pending.store(0, Ordering::Relaxed);
            spawn_compact(db.clone(), self.doc_id);
        }
        Outcome::Continue
    }

    fn send_to(&self, session: Uuid, frame: Vec<u8>) {
        let mut sessions = lock(&self.sessions);
        let full = sessions
            .get(&session)
            .is_some_and(|s| s.tx.try_send(frame).is_err());
        if full {
            // Dropping its sender ends the session loop (too slow or gone).
            sessions.remove(&session);
        }
    }

    /// To every local session but `from`, then to the other pods.
    fn relay_out(&self, from: Uuid, frame: Vec<u8>) {
        self.broadcast_local(Some(from), &frame);
        publish(self.valkey_url.clone(), self.doc_id, frame);
    }

    fn broadcast_local(&self, except: Option<Uuid>, frame: &[u8]) {
        lock(&self.sessions)
            .retain(|id, s| Some(*id) == except || s.tx.try_send(frame.to_vec()).is_ok());
    }

    fn is_open(self: &Arc<Self>) -> bool {
        lock(&ROOMS)
            .get(&self.doc_id)
            .is_some_and(|r| Arc::ptr_eq(r, self))
    }

    fn ensure_relay(self: &Arc<Self>, db: DatabaseConnection) {
        if self.valkey_url.is_none() || self.relay.swap(true, Ordering::AcqRel) {
            return;
        }
        tokio::spawn(relay_loop(self.clone(), db));
    }

    /// Applies what other pods logged (a bus message may have been missed)
    /// and pushes the difference to local sessions.
    async fn catch_up(&self, db: &DatabaseConnection) {
        let fresh = match load(db, self.doc_id).await {
            Ok(doc) => encode(&doc),
            Err(e) => {
                tracing::warn!(doc_id = %self.doc_id, error = %e, "pages: catch-up failed");
                return;
            }
        };
        let diff = {
            let doc = lock(&self.doc);
            let before = encode(&doc);
            let sv = doc.transact().state_vector();
            let _ = apply(&doc, &fresh);
            if encode(&doc) == before {
                return;
            }
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
            let _ = apply(&lock(&self.doc), &u);
        }
        self.broadcast_local(None, frame);
    }
}

/// Sends `frame` to the other pods (no-op without Valkey).
fn publish(valkey_url: Option<String>, doc_id: Uuid, frame: Vec<u8>) {
    let Some(url) = valkey_url else {
        return;
    };
    let mut payload = crate::services::device_bus::pod_id().as_bytes().to_vec();
    payload.push(b'\n');
    payload.extend_from_slice(&frame);
    tokio::spawn(async move {
        let Some(mut conn) = crate::services::shared_valkey::conn_url(&url).await else {
            return;
        };
        let res: redis::RedisResult<i64> = conn.publish(channel(doc_id), payload).await;
        if let Err(e) = res {
            tracing::warn!(error = %e, "pages bus: publish failed");
        }
    });
}

/// Tag of the custom y-protocols message telling the editors of a page that
/// its comments changed (no payload: they reload them).
pub const MSG_COMMENTS_CHANGED: u8 = 100;

/// Tells every editor of `doc_id`, on every pod, that the comments changed.
pub fn comments_changed(valkey_url: Option<String>, doc_id: Uuid) {
    let frame = Message::Custom(MSG_COMMENTS_CHANGED, Vec::new()).encode_v1();
    let open = lock(&ROOMS).get(&doc_id).cloned();
    if let Some(room) = open {
        room.broadcast_local(None, &frame);
    }
    publish(valkey_url, doc_id, frame);
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
        let mut last_catch_up = tokio::time::Instant::now();
        let mut stream = pubsub.on_message();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
            if !room.is_open() {
                break 'outer;
            }
            if last_catch_up.elapsed() >= CATCH_UP_EVERY {
                room.catch_up(&db).await;
                last_catch_up = tokio::time::Instant::now();
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
