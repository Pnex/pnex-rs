//! Per-topic sliding windows shared by the anomaly and forecast nodes, with
//! optional Valkey persistence so a redeploy or a runtime restart keeps the
//! history (a forecast over weeks cannot start from scratch each deploy).
//!
//! Persistence is best-effort: `VALKEY_URL` absent = in-memory only; any
//! Valkey error is logged and the node keeps working on its memory window.
//! Key: `pnex:series:v1:{org}:{flow}:{node}:{topic}` — a LIST of `"t:v"`
//! entries, trimmed to the window and expired after [`SERIES_TTL_SECS`]
//! without writes (a deleted node's history garbage-collects itself).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redis::aio::ConnectionManager;

/// History kept without writes before Valkey drops it (30 days).
pub const SERIES_TTL_SECS: i64 = 30 * 86_400;
/// Hard bound on each Valkey round-trip: persistence never stalls a flow.
const VALKEY_TIMEOUT: Duration = Duration::from_secs(1);
/// Topic length kept in keys / maps.
const TOPIC_MAX_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Unix seconds.
    pub t: f64,
    pub v: f64,
}

#[derive(Debug, Default)]
pub struct Series {
    pub samples: VecDeque<Sample>,
    /// Samples received since the node started (refit cadence, changepoint
    /// bookkeeping) — not persisted.
    pub received: u64,
    /// Absolute index (over `received` + loaded history) of the last
    /// changepoint already reported.
    pub last_changepoint: Option<u64>,
    /// First scoring done: changepoints already in the history at that
    /// point are recorded, never reported (no alert storm on warm-up).
    pub changepoint_primed: bool,
    /// Absolute index of the first sample in `samples`.
    pub offset: u64,
    touched: u64,
}

impl Series {
    pub fn values(&self) -> Vec<f64> {
        self.samples.iter().map(|s| s.v).collect()
    }

    pub fn times(&self) -> Vec<f64> {
        self.samples.iter().map(|s| s.t).collect()
    }
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn clamp_topic(topic: &str) -> String {
    topic.chars().take(TOPIC_MAX_LEN).collect()
}

fn encode(s: &Sample) -> String {
    format!("{}:{}", s.t, s.v)
}

fn decode(raw: &str) -> Option<Sample> {
    let (t, v) = raw.split_once(':')?;
    let s = Sample {
        t: t.parse().ok()?,
        v: v.parse().ok()?,
    };
    (s.t.is_finite() && s.v.is_finite()).then_some(s)
}

struct Persist {
    conn: ConnectionManager,
    prefix: String,
}

/// Windows of one node, keyed by `msg.topic`.
pub struct SeriesStore {
    node: &'static str,
    cap: usize,
    max_series: usize,
    series: Mutex<HashMap<String, Series>>,
    persist: Option<Persist>,
    clock: std::sync::atomic::AtomicU64,
}

impl SeriesStore {
    /// In-memory store (tests, `VALKEY_URL` absent).
    pub fn memory(node: &'static str, cap: usize) -> Self {
        Self {
            node,
            cap,
            max_series: pnex_core::predictive::PREDICT_MAX_SERIES,
            series: Mutex::new(HashMap::new()),
            persist: None,
            clock: Default::default(),
        }
    }

    /// Store persisted under `pnex:series:v1:{org}:{flow}:{node_id}:` when
    /// `VALKEY_URL` is set. An unparseable URL degrades to memory with a
    /// warning (persistence is a comfort, never a deploy blocker).
    pub fn from_env(
        node: &'static str,
        cap: usize,
        org_id: i64,
        flow_id: i64,
        node_id: &str,
    ) -> Self {
        match std::env::var("VALKEY_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
        {
            Some(url) => Self::with_url(node, cap, &url, org_id, flow_id, node_id),
            None => Self::memory(node, cap),
        }
    }

    /// Persisted store on an explicit Valkey URL (test entry point).
    pub fn with_url(
        node: &'static str,
        cap: usize,
        url: &str,
        org_id: i64,
        flow_id: i64,
        node_id: &str,
    ) -> Self {
        let mut store = Self::memory(node, cap);
        let conn = redis::Client::open(url).and_then(|c| {
            ConnectionManager::new_lazy_with_config(c, redis::aio::ConnectionManagerConfig::new())
        });
        match conn {
            Ok(conn) => {
                store.persist = Some(Persist {
                    conn,
                    prefix: format!("pnex:series:v1:{org_id}:{flow_id}:{node_id}:"),
                })
            }
            Err(e) => log::warn!("{node} : series persistence disabled (valkey client: {e})"),
        }
        store
    }

    /// Appends one sample and runs `f` on the updated window. The first
    /// sample of a topic loads the persisted history first.
    pub async fn push<R>(
        &self,
        topic: &str,
        sample: Sample,
        f: impl FnOnce(&mut Series) -> R,
    ) -> R {
        let topic = clamp_topic(topic);
        let known = self
            .series
            .lock()
            .expect("series lock")
            .contains_key(&topic);
        let loaded = if known {
            None
        } else {
            Some(self.load(&topic).await)
        };
        let result = {
            let mut map = self.series.lock().expect("series lock");
            if let Some(history) = loaded {
                self.evict_if_full(&mut map);
                map.entry(topic.clone()).or_insert_with(|| Series {
                    samples: history.into(),
                    ..Default::default()
                });
            }
            let s = map.get_mut(&topic).expect("series inserted above");
            s.samples.push_back(sample);
            s.received += 1;
            while s.samples.len() > self.cap {
                s.samples.pop_front();
                s.offset += 1;
            }
            s.touched = self
                .clock
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            f(s)
        };
        self.save(&topic, sample).await;
        result
    }

    fn evict_if_full(&self, map: &mut HashMap<String, Series>) {
        if map.len() < self.max_series {
            return;
        }
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, s)| s.touched)
            .map(|(k, _)| k.clone())
        {
            log::warn!(
                "{} : more than {} series, dropping `{oldest}`",
                self.node,
                self.max_series
            );
            map.remove(&oldest);
        }
    }

    async fn load(&self, topic: &str) -> Vec<Sample> {
        let Some(p) = &self.persist else {
            return Vec::new();
        };
        let key = format!("{}{topic}", p.prefix);
        let mut conn = p.conn.clone();
        let cap = self.cap as isize;
        let op = async move {
            redis::AsyncCommands::lrange::<_, Vec<String>>(&mut conn, key, -cap, -1).await
        };
        match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
            Ok(Ok(raw)) => raw.iter().filter_map(|r| decode(r)).collect(),
            Ok(Err(e)) => {
                log::warn!("{} : series history not loaded ({e})", self.node);
                Vec::new()
            }
            Err(_) => {
                log::warn!("{} : series history not loaded (valkey timeout)", self.node);
                Vec::new()
            }
        }
    }

    async fn save(&self, topic: &str, sample: Sample) {
        let Some(p) = &self.persist else { return };
        let key = format!("{}{topic}", p.prefix);
        let mut conn = p.conn.clone();
        let cap = self.cap as isize;
        let op = async move {
            redis::pipe()
                .rpush(&key, encode(&sample))
                .ignore()
                .ltrim(&key, -cap, -1)
                .ignore()
                .expire(&key, SERIES_TTL_SECS)
                .ignore()
                .query_async::<()>(&mut conn)
                .await
        };
        match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("{} : series sample not persisted ({e})", self.node),
            Err(_) => log::warn!(
                "{} : series sample not persisted (valkey timeout)",
                self.node
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_codec_roundtrip() {
        let s = Sample {
            t: 1_727_600_000.25,
            v: -3.5,
        };
        assert_eq!(decode(&encode(&s)), Some(s));
        assert_eq!(decode("garbage"), None);
        assert_eq!(decode("1:NaN"), None);
    }

    #[tokio::test]
    async fn window_is_bounded_per_topic() {
        let store = SeriesStore::memory("test", 3);
        for i in 0..5 {
            store
                .push(
                    "a",
                    Sample {
                        t: i as f64,
                        v: i as f64,
                    },
                    |_| (),
                )
                .await;
        }
        store.push("b", Sample { t: 0.0, v: 9.0 }, |_| ()).await;
        let (vals, offset, received) = store
            .push("a", Sample { t: 5.0, v: 5.0 }, |s| {
                (s.values(), s.offset, s.received)
            })
            .await;
        assert_eq!(vals, vec![3.0, 4.0, 5.0]);
        assert_eq!(offset, 3);
        assert_eq!(received, 6);
        let b = store
            .push("b", Sample { t: 1.0, v: 8.0 }, |s| s.values())
            .await;
        assert_eq!(b, vec![9.0, 8.0]);
    }

    #[tokio::test]
    async fn least_recent_series_is_evicted() {
        let mut store = SeriesStore::memory("test", 4);
        store.max_series = 2;
        store.push("a", Sample { t: 0.0, v: 1.0 }, |_| ()).await;
        store.push("b", Sample { t: 0.0, v: 1.0 }, |_| ()).await;
        store.push("a", Sample { t: 1.0, v: 1.0 }, |_| ()).await;
        store.push("c", Sample { t: 0.0, v: 1.0 }, |_| ()).await;
        let map = store.series.lock().unwrap();
        assert!(map.contains_key("a") && map.contains_key("c") && !map.contains_key("b"));
    }
}
