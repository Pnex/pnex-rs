//! Durable, ordered point queue (redb, ACID, pure Rust).
//!
//! - `points`: `seq (u64) → JSON StoredPoint`, `seq` strictly increasing and
//!   never reused (persisted `next_seq`), so a server-side high-water mark
//!   per `epoch` deduplicates replays across reconnects and restarts;
//! - `meta`: `epoch` (random id of this queue file) and `next_seq`.
//!
//! A point is acknowledged to the local producer only after its write
//! transaction is committed (fsync); it leaves the queue only on the
//! server's `BatchAck`.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};

const POINTS: TableDefinition<u64, &[u8]> = TableDefinition::new("points");
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

/// A point as stored on disk (the wire `BatchPoint` minus `seq`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredPoint {
    pub key: String,
    pub value: serde_json::Value,
    pub ts_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub record: bool,
}

#[derive(Clone)]
pub struct Queue {
    db: Arc<Database>,
    epoch: String,
}

fn meta_u64(v: Option<redb::AccessGuard<'_, &[u8]>>) -> u64 {
    v.and_then(|g| g.value().try_into().ok().map(u64::from_le_bytes))
        .unwrap_or(1)
}

impl Queue {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path)
            .with_context(|| format!("cannot open queue {}", path.display()))?;
        let tx = db.begin_write()?;
        let epoch = {
            let mut meta = tx.open_table(META)?;
            let existing = meta
                .get("epoch")?
                .map(|g| String::from_utf8_lossy(g.value()).into_owned());
            match existing {
                Some(e) => e,
                None => {
                    let e = uuid::Uuid::new_v4().to_string();
                    meta.insert("epoch", e.as_bytes())?;
                    meta.insert("next_seq", 1u64.to_le_bytes().as_slice())?;
                    e
                }
            }
        };
        tx.open_table(POINTS)?;
        tx.commit()?;
        Ok(Self {
            db: Arc::new(db),
            epoch,
        })
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// Appends points in one durable transaction (group commit) — returns
    /// the seq of the last appended point.
    pub fn append(&self, points: &[StoredPoint]) -> Result<u64> {
        let tx = self.db.begin_write()?;
        let last = {
            let mut meta = tx.open_table(META)?;
            let mut next = meta_u64(meta.get("next_seq")?);
            let mut table = tx.open_table(POINTS)?;
            for p in points {
                let bytes = serde_json::to_vec(p)?;
                table.insert(next, bytes.as_slice())?;
                next += 1;
            }
            meta.insert("next_seq", next.to_le_bytes().as_slice())?;
            next - 1
        };
        tx.commit()?;
        Ok(last)
    }

    /// Up to `limit` points with `seq > after`, in order, bounded by
    /// `max_bytes` of serialized payload (at least one point).
    pub fn read_after(
        &self,
        after: u64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<(u64, StoredPoint)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(POINTS)?;
        let mut out = Vec::new();
        let mut bytes = 0usize;
        for row in table.range((after + 1)..)? {
            let (k, v) = row?;
            let raw = v.value();
            if !out.is_empty() && (out.len() >= limit || bytes + raw.len() > max_bytes) {
                break;
            }
            bytes += raw.len();
            match serde_json::from_slice::<StoredPoint>(raw) {
                Ok(p) => out.push((k.value(), p)),
                Err(e) => tracing::warn!(seq = k.value(), "unreadable queued point skipped: {e}"),
            }
        }
        Ok(out)
    }

    /// Removes every point with `seq <= up_to` (server acknowledgement).
    pub fn ack(&self, up_to: u64) -> Result<u64> {
        let tx = self.db.begin_write()?;
        let removed = {
            let mut table = tx.open_table(POINTS)?;
            let keys: Vec<u64> = table
                .range(..=up_to)?
                .map(|r| r.map(|(k, _)| k.value()))
                .collect::<Result<_, _>>()?;
            for k in &keys {
                table.remove(*k)?;
            }
            keys.len() as u64
        };
        tx.commit()?;
        Ok(removed)
    }

    pub fn len(&self) -> Result<u64> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(POINTS)?.len()?)
    }

    /// Enforces the bounds: drops the oldest points beyond `max_points` and
    /// those captured before `min_ts_ms`. Returns the number dropped.
    pub fn trim(&self, max_points: u64, min_ts_ms: i64) -> Result<u64> {
        let tx = self.db.begin_write()?;
        let dropped = {
            let mut table = tx.open_table(POINTS)?;
            let total = table.len()?;
            let excess = total.saturating_sub(max_points);
            let mut victims = Vec::new();
            for (i, row) in table.iter()?.enumerate() {
                let (k, v) = row?;
                if (i as u64) < excess {
                    victims.push(k.value());
                    continue;
                }
                let old = serde_json::from_slice::<StoredPoint>(v.value())
                    .map(|p| p.ts_ms < min_ts_ms)
                    .unwrap_or(true);
                if !old {
                    break;
                }
                victims.push(k.value());
            }
            for k in &victims {
                table.remove(*k)?;
            }
            victims.len() as u64
        };
        tx.commit()?;
        Ok(dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(key: &str, ts_ms: i64) -> StoredPoint {
        StoredPoint {
            key: key.into(),
            value: serde_json::json!(1),
            ts_ms,
            unit: None,
            record: false,
        }
    }

    #[test]
    fn append_read_ack_keep_order_and_never_reuse_seq() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.redb");
        let q = Queue::open(&path).unwrap();
        let epoch = q.epoch().to_string();
        assert_eq!(q.append(&[p("a", 1), p("b", 2)]).unwrap(), 2);
        assert_eq!(q.append(&[p("c", 3)]).unwrap(), 3);
        let got = q.read_after(0, 10, usize::MAX).unwrap();
        assert_eq!(
            got.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(q.read_after(1, 1, usize::MAX).unwrap()[0].0, 2);
        assert_eq!(q.ack(2).unwrap(), 2);
        assert_eq!(q.len().unwrap(), 1);
        drop(q);

        // Reopen: same epoch, seq continues (a replay is never confused
        // with a fresh point server-side).
        let q = Queue::open(&path).unwrap();
        assert_eq!(q.epoch(), epoch);
        assert_eq!(q.append(&[p("d", 4)]).unwrap(), 4);
        assert_eq!(
            q.read_after(0, 10, usize::MAX)
                .unwrap()
                .iter()
                .map(|(s, _)| *s)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    #[test]
    fn read_respects_byte_budget_but_always_returns_one() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(&dir.path().join("q.redb")).unwrap();
        q.append(&[p("a", 1), p("b", 2), p("c", 3)]).unwrap();
        assert_eq!(q.read_after(0, 10, 1).unwrap().len(), 1);
    }

    #[test]
    fn trim_drops_oldest_beyond_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(&dir.path().join("q.redb")).unwrap();
        q.append(&[p("a", 10), p("b", 20), p("c", 30), p("d", 40)])
            .unwrap();
        assert_eq!(q.trim(3, 0).unwrap(), 1);
        assert_eq!(q.trim(10, 25).unwrap(), 1);
        let left: Vec<String> = q
            .read_after(0, 10, usize::MAX)
            .unwrap()
            .into_iter()
            .map(|(_, p)| p.key)
            .collect();
        assert_eq!(left, vec!["c", "d"]);
    }
}
