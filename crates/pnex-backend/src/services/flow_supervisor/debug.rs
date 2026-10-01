//! Debug feed ring buffer fed by runtime stdout `debug` events (per-flow caps, LRU, TTL).

use super::*;

// ───────────────────────── Feed debug (panneau) ─────────────────────────

/// Anneau du panneau de debug : dernières sorties `debug` du runtime,
/// attribuées par flow (le runtime émet `flow`/`node_red` — une entrée sans
/// attribution est **jetée** : les orgs partagent un seul `flows.json`,
/// jamais de bucket « inconnu »).
static DEBUG_FEED: OnceLock<std::sync::Mutex<DebugFeed>> = OnceLock::new();

/// Caps bornés : un flow bavard ne peut ni exploser la mémoire ni évincer
/// indéfiniment les autres flows.
pub(super) const DEBUG_CAP_PER_FLOW: usize = 200;
const DEBUG_MAX_FLOWS: usize = 64;
const DEBUG_TTL_SECS: i64 = 300;

#[derive(Default)]
struct DebugFeed {
    per_flow: std::collections::HashMap<
        i64,
        std::collections::VecDeque<(std::time::Instant, pnex_core::FlowDebugEntry)>,
    >,
    next_seq: u64,
    /// Dernier accès par flow (évcition LRU au-delà de `DEBUG_MAX_FLOWS`).
    last_touch: std::collections::HashMap<i64, std::time::Instant>,
}

/// `DebugFeed` n'est lu que sous son mutex — horloge mono-thread efficace.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Timestamp RFC 3339 sans dépendance chrono (le formatter de `time` n'est
/// pas une dep du backend) : secondes depuis l'epoch → ISO-8601 UTC.
pub(super) fn rfc3339_now() -> String {
    let secs = now_secs();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Algorithme de conversion date civile (Howard Hinnant) — époque 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (rem / 3600, rem % 3600 / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn feed() -> &'static std::sync::Mutex<DebugFeed> {
    DEBUG_FEED.get_or_init(std::sync::Mutex::default)
}

/// Latest camera/vision node status per `(flow, canvas node)` (D103) —
/// kept apart from the debug feed: statuses are operational health, shown
/// in run mode too, never evicted by a chatty debug node.
static NODE_STATUS: OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<(i64, String), (std::time::Instant, pnex_core::FlowDebugEntry)>,
    >,
> = OnceLock::new();
/// A status older than this is dropped (the nodes beat every 10 s).
const NODE_STATUS_TTL_SECS: u64 = 60;

fn node_status_map() -> &'static std::sync::Mutex<
    std::collections::HashMap<(i64, String), (std::time::Instant, pnex_core::FlowDebugEntry)>,
> {
    NODE_STATUS.get_or_init(std::sync::Mutex::default)
}

/// Last message of every debug/display node per `(flow, canvas node)` —
/// shown under the node in every mode (a flow designer must see what goes
/// in and out), unlike the full feed of the side panel which stays behind
/// `debug_tools`. One bounded entry per node, dropped at the next deploy.
static NODE_LAST: OnceLock<
    std::sync::Mutex<std::collections::HashMap<(i64, String), pnex_core::FlowDebugEntry>>,
> = OnceLock::new();
/// Largest message kept as a last value (serialized bytes); bigger ones
/// are replaced by a truncated preview.
const NODE_LAST_MAX_BYTES: usize = 8 * 1024;

fn node_last_map(
) -> &'static std::sync::Mutex<std::collections::HashMap<(i64, String), pnex_core::FlowDebugEntry>>
{
    NODE_LAST.get_or_init(std::sync::Mutex::default)
}

/// Array lengths tried, in order, when downsampling an oversized message
/// (e.g. a forecast with one point per step over a long horizon).
const DOWNSAMPLE_STEPS: [usize; 4] = [100, 50, 25, 10];

/// Evenly samples `items` down to `max` entries, always keeping the first
/// and the last one.
fn downsample(items: &[serde_json::Value], max: usize) -> Vec<serde_json::Value> {
    if items.len() <= max || max < 2 {
        return items.to_vec();
    }
    let last = items.len() - 1;
    (0..max)
        .map(|i| items[(i * last + (max - 1) / 2) / (max - 1)].clone())
        .collect()
}

/// Recursively downsamples every array longer than `max`; returns whether
/// anything was shortened.
fn downsample_arrays(v: &mut serde_json::Value, max: usize) -> bool {
    match v {
        serde_json::Value::Array(items) => {
            let mut changed = false;
            if items.len() > max {
                *items = downsample(items, max);
                changed = true;
            }
            for item in items.iter_mut() {
                changed |= downsample_arrays(item, max);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for item in map.values_mut() {
                changed |= downsample_arrays(item, max);
            }
            changed
        }
        _ => false,
    }
}

/// Caps a message for the last-value map: over [`NODE_LAST_MAX_BYTES`],
/// long arrays are first evenly downsampled (first/last kept, the object
/// is flagged `"downsampled": true`); when that is not enough it becomes
/// `{"truncated": true, "preview": "<first bytes>"}`.
pub(super) fn cap_message(msg: serde_json::Value) -> serde_json::Value {
    let raw = match &msg {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if raw.len() <= NODE_LAST_MAX_BYTES {
        return msg;
    }
    for max in DOWNSAMPLE_STEPS {
        let mut small = msg.clone();
        if !downsample_arrays(&mut small, max) {
            break;
        }
        if let serde_json::Value::Object(map) = &mut small {
            map.insert("downsampled".into(), serde_json::Value::Bool(true));
        }
        if small.to_string().len() <= NODE_LAST_MAX_BYTES {
            return small;
        }
    }
    let mut end = NODE_LAST_MAX_BYTES;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    serde_json::json!({ "truncated": true, "preview": &raw[..end] })
}

/// Latest message of every debug/display node of a flow.
pub fn node_last_values(flow_id: i64) -> Vec<pnex_core::FlowDebugEntry> {
    let map = node_last_map().lock().expect("lock node last");
    let mut out: Vec<_> = map
        .iter()
        .filter(|((fid, _), _)| *fid == flow_id)
        .map(|(_, e)| e.clone())
        .collect();
    out.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    out
}

/// Latest fresh status of every node of a flow.
pub fn node_statuses(flow_id: i64) -> Vec<pnex_core::FlowDebugEntry> {
    let mut map = node_status_map().lock().expect("lock node status");
    let now = std::time::Instant::now();
    map.retain(|_, (t, _)| now.duration_since(*t).as_secs() <= NODE_STATUS_TTL_SECS);
    let mut out: Vec<_> = map
        .iter()
        .filter(|((fid, _), _)| *fid == flow_id)
        .map(|(_, (_, e))| e.clone())
        .collect();
    out.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    out
}

/// Ingère une ligne stdout `{"event":"debug",...}` du runtime : exige
/// l'attribution `flow` (sinon jetée) et estampille à la réception.
pub fn push_debug(raw: &serde_json::Value) {
    if raw.get("event").and_then(|e| e.as_str()) != Some("debug") {
        return;
    }
    let Some(flow_id) = raw.get("flow").and_then(|f| f.as_i64()) else {
        return;
    };
    let entry = pnex_core::FlowDebugEntry {
        seq: 0, // assigné sous lock
        ts: rfc3339_now(),
        flow_id,
        node_id: raw
            .get("node_red")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: raw.get("name").and_then(|v| v.as_str()).map(str::to_string),
        msg: raw.get("msg").cloned().unwrap_or(serde_json::Value::Null),
        source: raw
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("debug")
            .to_string(),
        topic: raw
            .get("topic")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        msgid: raw
            .get("msgid")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    };
    if entry.source == "pnex-status" && !entry.node_id.is_empty() {
        node_status_map().lock().expect("lock node status").insert(
            (flow_id, entry.node_id.clone()),
            (std::time::Instant::now(), entry.clone()),
        );
    }
    if matches!(entry.source.as_str(), "debug" | "pnex-display") && !entry.node_id.is_empty() {
        let mut last = entry.clone();
        last.msg = cap_message(last.msg);
        node_last_map()
            .lock()
            .expect("lock node last")
            .insert((flow_id, entry.node_id.clone()), last);
    }
    let mut feed = feed().lock().expect("lock debug feed");
    let now = std::time::Instant::now();
    feed.next_seq += 1;
    let mut entry = entry;
    entry.seq = feed.next_seq;
    let q = feed.per_flow.entry(flow_id).or_default();
    q.push_back((now, entry));
    while q.len() > DEBUG_CAP_PER_FLOW {
        q.pop_front();
    }
    feed.last_touch.insert(flow_id, now);
    // Éviction LRU des flows (au-delà du cap, le flow le plus ancien sort).
    while feed.per_flow.len() > DEBUG_MAX_FLOWS {
        let Some(victim) = feed
            .last_touch
            .iter()
            .min_by_key(|(_, t)| **t)
            .map(|(fid, _)| *fid)
        else {
            break;
        };
        feed.per_flow.remove(&victim);
        feed.last_touch.remove(&victim);
    }
}

/// Snapshot du feed d'un flow (les plus anciennes d'abord), entrées TTL
/// dépassées purgées.
pub fn debug_entries(flow_id: i64, limit: usize) -> Vec<pnex_core::FlowDebugEntry> {
    let mut feed = feed().lock().expect("lock debug feed");
    let now = std::time::Instant::now();
    if let Some(q) = feed.per_flow.get_mut(&flow_id) {
        q.retain(|(t, _)| now.duration_since(*t).as_secs() as i64 <= DEBUG_TTL_SECS);
        q.iter()
            .map(|(_, e)| e.clone())
            .skip(q.len().saturating_sub(limit))
            .collect()
    } else {
        Vec::new()
    }
}

/// Purge du feed d'un flow (au succès d'un deploy — les entrées d'une
/// version antérieure ne doivent pas survivre à l'artefact frais).
pub fn clear_debug_feed(flow_id: i64) {
    let mut feed = feed().lock().expect("lock feed");
    feed.per_flow.remove(&flow_id);
    node_status_map()
        .lock()
        .expect("lock node status")
        .retain(|(fid, _), _| *fid != flow_id);
    node_last_map()
        .lock()
        .expect("lock node last")
        .retain(|(fid, _), _| *fid != flow_id);
}

/// ligne a été consommée par le feed/santé/acks (reste rejouée en tracing).
pub(crate) fn handle_runtime_line(line: &str, pending: &PendingAcks) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    match v.get("event").and_then(|e| e.as_str()) {
        Some("debug") => {
            push_debug(&v);
            true
        }
        Some("flow_started" | "flow_error" | "flow_stopped") => {
            record_flow_health(&v);
            resolve_deploy_ack(&v, pending);
            true
        }
        _ => false,
    }
}
