//! Device GPS positions (D38): telemetry tap, half-position pairing, upserts.

use super::*;

// ─────────────────────────── positions GPS (D38) ───────────────────────────

pub const GPS_LATITUDE: &str = "latitude";
pub const GPS_LONGITUDE: &str = "longitude";
pub const GPS_ACCURACY: &str = "gps_accuracy_m";
pub const GPS_ALTITUDE: &str = "gps_altitude_m";
pub const GPS_SPEED: &str = "gps_speed_mps";
pub const GPS_HEADING: &str = "gps_heading_deg";

/// `true` if the metric takes part in the position (D38 convention).
pub fn is_gps_metric(name: &str) -> bool {
    matches!(
        name,
        GPS_LATITUDE | GPS_LONGITUDE | GPS_ACCURACY | GPS_ALTITUDE | GPS_SPEED | GPS_HEADING
    )
}

/// Half position waiting for its twin (the `latitude` and
/// `longitude` frames arrive separately).
#[derive(Default, Clone)]
struct GpsHalf {
    latitude: Option<f64>,
    longitude: Option<f64>,
    accuracy: Option<f64>,
    altitude: Option<f64>,
    speed: Option<f64>,
    heading: Option<f64>,
}

impl GpsHalf {
    fn complete(&self) -> Option<(f64, f64)> {
        Some((self.latitude?, self.longitude?))
    }
}

/// TTL of the pending half position (frames a few ms apart;
/// beyond that, the pair is considered lost).
const GPS_HALF_TTL: Duration = Duration::from_secs(300);

static GPS_PENDING: LazyLock<Mutex<HashMap<i64, (GpsHalf, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Bound of the coalescing queue of the GPS writer (distinct devices with
/// a position waiting to be written); beyond, new devices are dropped.
const GPS_WRITES_MAX: usize = 100_000;
/// Size of the half-position cache above which stale halves are pruned.
const GPS_PENDING_PRUNE_AT: usize = 10_000;

/// A complete position waiting for the writer (latest per device wins).
struct PendingPosition {
    org_id: i64,
    device_id: String,
    latitude: f64,
    longitude: f64,
    half: GpsHalf,
    positioned_at: DateTimeWithTimeZone,
}

type PendingWrites = Arc<Mutex<HashMap<i64, PendingPosition>>>;

/// Telemetry sink tap: updates the device's **latest** position
/// (D38) without ever blocking or failing the telemetry stream — the
/// errors are logged and swallowed (O2 batcher « drop, don't block » pattern).
///
/// Writes go through ONE writer task per sink fed by a coalescing map
/// (latest complete position per device): a GPS burst from a fleet costs at
/// most one upsert per device per writer pass, never one task per point.
pub struct GpsTapSink {
    db: DatabaseConnection,
    inner: Arc<dyn TelemetrySink>,
    pending: PendingWrites,
    /// Writer wake-up (capacity 1: a pending wake-up covers every write).
    wake: Option<tokio::sync::mpsc::Sender<()>>,
}

impl GpsTapSink {
    pub fn new(db: DatabaseConnection, inner: Arc<dyn TelemetrySink>) -> Arc<Self> {
        let pending: PendingWrites = Arc::default();
        let wake = tokio::runtime::Handle::try_current().ok().map(|handle| {
            let (tx, rx) = tokio::sync::mpsc::channel::<()>(1);
            handle.spawn(gps_writer(db.clone(), pending.clone(), rx));
            tx
        });
        Arc::new(Self {
            db,
            inner,
            pending,
            wake,
        })
    }

    fn enqueue(&self, device_registry_id: i64, position: PendingPosition) {
        let Some(wake) = &self.wake else {
            // No runtime at construction (never in the server): write inline.
            let db = self.db.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move { write_pending(&db, device_registry_id, position).await });
            }
            return;
        };
        {
            let mut q = self.pending.lock().expect("gps writes");
            if q.len() >= GPS_WRITES_MAX && !q.contains_key(&device_registry_id) {
                tracing::warn!(
                    device = device_registry_id,
                    "gps writer saturated — position dropped"
                );
                return;
            }
            q.insert(device_registry_id, position);
        }
        let _ = wake.try_send(());
    }
}

impl TelemetrySink for GpsTapSink {
    fn send(&self, point: TelemetryPoint) {
        if is_gps_metric(&point.metric_name) {
            // Pairing is synchronous and cheap; the upsert is the writer's.
            if let Some(value) = parse_f64(&point.value) {
                if let Some(((lat, lon), half)) =
                    take_gps_half(point.device_registry_id, &point.metric_name, value)
                {
                    // Every complete pair goes to the history (K26); only the
                    // latest one is upserted below.
                    super::position_log::submit(super::position_log::PositionEntry {
                        org_id: point.org_id,
                        device_id: point.device_id.clone(),
                        t_us: point.timestamp.timestamp_micros(),
                        lon,
                        lat,
                        alt: half.altitude,
                        accuracy_m: half.accuracy,
                        speed_mps: half.speed,
                        heading_deg: half.heading,
                    });
                    self.enqueue(
                        point.device_registry_id,
                        PendingPosition {
                            org_id: point.org_id,
                            device_id: point.device_id.clone(),
                            latitude: lat,
                            longitude: lon,
                            half,
                            positioned_at: point.timestamp.into(),
                        },
                    );
                }
            }
        }
        self.inner.send(point);
    }
}

/// Single writer of a [`GpsTapSink`]: drains the coalesced positions on
/// each wake-up; ends when the sink is dropped.
async fn gps_writer(
    db: DatabaseConnection,
    pending: PendingWrites,
    mut wake: tokio::sync::mpsc::Receiver<()>,
) {
    while wake.recv().await.is_some() {
        loop {
            let batch: Vec<(i64, PendingPosition)> =
                pending.lock().expect("gps writes").drain().collect();
            if batch.is_empty() {
                break;
            }
            for (id, position) in batch {
                write_pending(&db, id, position).await;
            }
        }
    }
}

async fn write_pending(db: &DatabaseConnection, device_registry_id: i64, p: PendingPosition) {
    if let Err(e) = upsert_position(
        db,
        p.org_id,
        device_registry_id,
        &p.device_id,
        p.latitude,
        p.longitude,
        p.half.accuracy,
        p.half.altitude,
        p.half.speed,
        p.half.heading,
        "telemetry",
        p.positioned_at,
    )
    .await
    {
        tracing::warn!(device = %p.device_id, error = %e, "GPS position not saved");
    }
}

/// Telemetry value → f64 (values are already floats at the O2 edge).
fn parse_f64(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

/// Processes a GPS point: assembles the lat/lon pair (half-position cache) and
/// upserts `device_positions`. Silent on an invalid/out-of-range value.
pub async fn observe(db: &DatabaseConnection, point: &TelemetryPoint) -> Result<(), DbErr> {
    let Some(value) = parse_f64(&point.value) else {
        return Ok(());
    };
    // All the synchronous work (half-position cache guard) outside await —
    // the std `Mutex` guard is not Send.
    let Some(((lat, lon), half)) =
        take_gps_half(point.device_registry_id, &point.metric_name, value)
    else {
        return Ok(());
    };
    upsert_position(
        db,
        point.org_id,
        point.device_registry_id,
        &point.device_id,
        lat,
        lon,
        half.accuracy,
        half.altitude,
        half.speed,
        half.heading,
        "telemetry",
        point.timestamp.into(),
    )
    .await
}

/// Merges the GPS point into the device's half position; when the lat/lon
/// pair is complete, consumes it and returns the position + the accumulated
/// options. `None` = invalid value / out of range / incomplete pair.
fn take_gps_half(
    device_registry_id: i64,
    metric: &str,
    value: f64,
) -> Option<((f64, f64), GpsHalf)> {
    let mut pending = GPS_PENDING.lock().expect("GPS_PENDING lock");
    // Bounded memory: halves of devices that never completed their pair
    // are dropped once stale.
    if pending.len() >= GPS_PENDING_PRUNE_AT {
        pending.retain(|_, (_, at)| at.elapsed() <= GPS_HALF_TTL);
    }
    let entry = pending
        .entry(device_registry_id)
        .or_insert_with(|| (GpsHalf::default(), Instant::now()));
    // TTL: half position too old → start again from an empty half.
    if entry.1.elapsed() > GPS_HALF_TTL {
        *entry = (GpsHalf::default(), Instant::now());
    }
    let half = &mut entry.0;
    match metric {
        GPS_LATITUDE if (-90.0..=90.0).contains(&value) => half.latitude = Some(value),
        GPS_LONGITUDE if (-180.0..=180.0).contains(&value) => half.longitude = Some(value),
        GPS_ACCURACY => half.accuracy = Some(value),
        GPS_ALTITUDE => half.altitude = Some(value),
        GPS_SPEED => half.speed = Some(value),
        GPS_HEADING => half.heading = Some(value),
        // Unknown metric (impossible — is_gps_metric) or outside the WGS84 range.
        _ => return None,
    }
    let (lat, lon) = half.complete()?;
    // Complete pair → consume the half position.
    let snapshot = half.clone();
    pending.remove(&device_registry_id);
    Some(((lat, lon), snapshot))
}

/// Shared upsert types (GPS options).
pub struct GpsOptions {
    pub accuracy: Option<f64>,
    pub altitude: Option<f64>,
    pub speed: Option<f64>,
    pub heading: Option<f64>,
}

#[allow(clippy::too_many_arguments)]
async fn upsert_position(
    db: &DatabaseConnection,
    org_id: i64,
    device_registry_id: i64,
    device_id: &str,
    latitude: f64,
    longitude: f64,
    accuracy: Option<f64>,
    altitude: Option<f64>,
    speed: Option<f64>,
    heading: Option<f64>,
    source: &str,
    positioned_at: DateTimeWithTimeZone,
) -> Result<(), DbErr> {
    let dec = |v: f64| Decimal::try_from(v).ok();
    // The business key is device_registry_id (UNIQUE) — not the auto PK.
    let existing = device_positions::Entity::find()
        .filter(device_positions::Column::DeviceRegistryId.eq(device_registry_id))
        .one(db)
        .await?;
    let opts = GpsOptions {
        accuracy,
        altitude,
        speed,
        heading,
    };
    if let Some(row) = existing {
        let mut am: device_positions::ActiveModel = row.into();
        am.org_id = Set(org_id);
        am.device_id = Set(device_id.to_string());
        am.latitude = Set(dec(latitude).ok_or_else(|| DbErr::Custom("invalid latitude".into()))?);
        am.longitude =
            Set(dec(longitude).ok_or_else(|| DbErr::Custom("invalid longitude".into()))?);
        am.accuracy_m = Set(opts.accuracy.and_then(dec));
        am.altitude_m = Set(opts.altitude.and_then(dec));
        am.speed_mps = Set(opts.speed.and_then(dec));
        am.heading_deg = Set(opts.heading.and_then(dec));
        am.source = Set(source.to_string());
        am.positioned_at = Set(positioned_at);
        am.update(db).await?;
    } else {
        let am = device_positions::ActiveModel {
            org_id: Set(org_id),
            device_registry_id: Set(device_registry_id),
            device_id: Set(device_id.to_string()),
            latitude: Set(dec(latitude).ok_or_else(|| DbErr::Custom("invalid latitude".into()))?),
            longitude: Set(dec(longitude).ok_or_else(|| DbErr::Custom("invalid longitude".into()))?),
            accuracy_m: Set(opts.accuracy.and_then(dec)),
            altitude_m: Set(opts.altitude.and_then(dec)),
            speed_mps: Set(opts.speed.and_then(dec)),
            heading_deg: Set(opts.heading.and_then(dec)),
            source: Set(source.to_string()),
            positioned_at: Set(positioned_at),
            ..Default::default()
        };
        am.insert(db).await?;
    }
    Ok(())
}

/// Manual position (POC/tests — admin PUT endpoint).
pub async fn set_manual(
    db: &DatabaseConnection,
    org_id: i64,
    device_id: &str,
    latitude: f64,
    longitude: f64,
    positioned_at: DateTimeWithTimeZone,
) -> Result<(), VizWriteError> {
    validate_lat(latitude)?;
    validate_lon(longitude)?;
    let device = find_device(db, org_id, device_id)
        .await
        .map_err(|_| VizWriteError::Db)?
        .ok_or(VizWriteError::DeviceUnknown)?;
    upsert_position(
        db,
        org_id,
        device.id,
        device_id,
        latitude,
        longitude,
        None,
        None,
        None,
        None,
        "manual",
        positioned_at,
    )
    .await
    .map_err(|_| VizWriteError::Db)
}

/// Latest position of each of the org's devices (live map layer).
pub async fn list_positions(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<device_positions::Model>, DbErr> {
    device_positions::Entity::find()
        .filter(device_positions::Column::OrgId.eq(org_id))
        .order_by_desc(device_positions::Column::PositionedAt)
        .all(db)
        .await
}
