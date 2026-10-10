//! Shared runtime state: queue handle, writer channel, live counters for
//! `GET /v1/status` and the locally seen keys.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::{mpsc, oneshot, Notify};

use crate::queue::{Queue, StoredPoint};

/// One durable append request (group-committed by the writer thread).
pub struct WriteReq {
    pub points: Vec<StoredPoint>,
    pub done: oneshot::Sender<Result<u64, String>>,
}

/// Uplink connection state (`GET /v1/status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    Connecting,
    Connected,
    Disconnected,
    /// Credentials refused (token rotated or device deleted): reinstall.
    Revoked,
}

pub struct Shared {
    pub queue: Queue,
    pub writer: mpsc::Sender<WriteReq>,
    /// Wakes the uplink when new points are committed.
    pub wake: Notify,
    pub link: Mutex<Link>,
    pub last_error: Mutex<Option<String>>,
    pub accepted: AtomicU64,
    pub sent: AtomicU64,
    pub acked_up_to: AtomicU64,
    pub dropped: AtomicU64,
    pub last_ack_ms: AtomicI64,
    pub max_batch: AtomicU64,
    pub server_max_keys: AtomicU64,
    pub shutting_down: AtomicBool,
    /// This agent is a capture box (announces `media_capture`, lot 6b).
    pub media_capture: AtomicBool,
    /// Media segments acknowledged by the server / dropped (queue full or
    /// refused).
    pub media_sent: AtomicU64,
    pub media_dropped: AtomicU64,
    /// Keys accepted locally since start → (unit, last value kind).
    pub keys: Mutex<BTreeMap<String, Option<String>>>,
}

impl Shared {
    pub fn new(queue: Queue, writer: mpsc::Sender<WriteReq>) -> Arc<Self> {
        Arc::new(Self {
            queue,
            writer,
            wake: Notify::new(),
            link: Mutex::new(Link::Connecting),
            last_error: Mutex::new(None),
            accepted: AtomicU64::new(0),
            sent: AtomicU64::new(0),
            acked_up_to: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            last_ack_ms: AtomicI64::new(0),
            max_batch: AtomicU64::new(500),
            server_max_keys: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
            media_capture: AtomicBool::new(false),
            media_sent: AtomicU64::new(0),
            media_dropped: AtomicU64::new(0),
            keys: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn set_link(&self, link: Link, error: Option<String>) {
        *self.link.lock().expect("link") = link;
        if error.is_some() || link == Link::Connected {
            *self.last_error.lock().expect("last_error") = error;
        }
    }

    pub fn link(&self) -> Link {
        *self.link.lock().expect("link")
    }
}

/// `GET /v1/status` body.
#[derive(Debug, Serialize)]
pub struct Status {
    pub version: &'static str,
    pub link: Link,
    pub last_error: Option<String>,
    pub queue_depth: u64,
    pub accepted: u64,
    pub sent: u64,
    pub dropped: u64,
    pub acked_up_to: u64,
    /// Epoch milliseconds of the last server acknowledgement (0 = never).
    pub last_ack_ms: i64,
    pub epoch: String,
    pub media_segments_sent: u64,
    pub media_segments_dropped: u64,
}

impl Status {
    pub fn of(s: &Shared) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            link: s.link(),
            last_error: s.last_error.lock().expect("last_error").clone(),
            queue_depth: s.queue.len().unwrap_or(0),
            accepted: s.accepted.load(Ordering::Relaxed),
            sent: s.sent.load(Ordering::Relaxed),
            dropped: s.dropped.load(Ordering::Relaxed),
            acked_up_to: s.acked_up_to.load(Ordering::Relaxed),
            last_ack_ms: s.last_ack_ms.load(Ordering::Relaxed),
            epoch: s.queue.epoch().to_string(),
            media_segments_sent: s.media_sent.load(Ordering::Relaxed),
            media_segments_dropped: s.media_dropped.load(Ordering::Relaxed),
        }
    }
}
