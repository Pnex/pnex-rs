//! Protocole fil du canal device — WS `/ws/device` (Brick 0, `docs/architecture/brick0.md` §3).
//!
//! **Source de vérité du contrat** : ce fichier. Le firmware C++
//! (`firmware/generic_esp8266`) en est un **miroir** — l'ESP8266 ne compile
//! pas de Rust : le contrat est le schéma fil (tag `"t"`), pas le partage
//! de code.
//!
//! Framing identique à `/ws/sensor/ingest` — auth query base64
//! (`token` + `device_id`), frames texte `base64(nonce(12)‖ChaCha20-nu)`,
//! `PING`/`PONG` au niveau frame, messages métier JSON tagué `t`.
//!
//! Sémantique RPC à la ThingsBoard : toute commande serveur→device est un
//! RPC avec `cmd_id` et réponse `Ack{cmd_id, ok, err}` requise. La pin map
//! est poussée par le serveur (`ProvisionAck`), jamais déclarée dans le
//! firmware.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Modes P0 d'une capability instance — le rôle sensor/actuator se dérive du
/// mode (pas de colonne `role` : modèle sans copies, B0.6).
/// `digital_in`/`adc_in` → sensor ; `digital_out`/`pwm_out` → actuator. ⚠ Le fil
/// sérialise `AdcIn` → `"adc_in"` (serde snake_case) — la chaîne
/// `analog_in` est la convention **base** (colonnes `mode`), jamais le fil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    DigitalIn,
    DigitalOut,
    AdcIn,
    /// PWM output — `Write` carries a duty percentage 0..=100
    /// (firmware scales to the Arduino `analogWrite` 8-bit range;
    /// core default frequency, no server-side frequency config).
    PwmOut,
}

/// État de repos d'une sortie — appliqué à la perte de lien, au boot et à
/// l'admission (safe-states, brick0.md §8). Défaut `Low` — même convention
/// que le défaut serveur de `ModeOpts::safe_state`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeState {
    #[default]
    Low,
    High,
}

/// Options de configuration d'un pin, poussées dans `SetMode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeOpts {
    /// Pull-up interne (refusée sur GPIO16 — pulldown only, `caps::validate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pullup: Option<bool>,
    /// État de repos ; défaut serveur = `Low` si absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_state: Option<SafeState>,
}

/// Pin admis, poussé au device dans `ProvisionAck` (miroir firmware : la
/// carte de pins vient du serveur, jamais du `.bin`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinSpec {
    pub gpio: u16,
    /// Label overlay (« D1 », « A0 ») — dénormalisé pour l'affichage. For a
    /// custom firmware device, the label declared in the sketch.
    pub label: String,
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_state: Option<SafeState>,
    /// Options persistées (pullup) — additif : répare le round-trip du
    /// pullup (avant, un pullup posé en UI était réinitialisé côté device
    /// à chaque re-announce car absent du `ProvisionAck`). Absent sur un
    /// fil historique → défauts, toléré par le firmware.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opts: Option<ModeOpts>,
}

/// Pin **declared by the sketch** (custom firmware) in `Announce::pins` —
/// the sketch is the source of its pins (gpio/mode/label). The server
/// validates them (chip-caps of the board's SoC) then persists them →
/// echoed in the `ProvisionAck`. Generic models never send `pins`: the
/// board overlay stays the authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinDecl {
    pub gpio: u16,
    /// Label libre choisi dans le sketch (« pump », « dht22 »…) —
    /// dénomme la série O2 `{org}/{device}/{label}`.
    pub label: String,
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pullup: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_state: Option<SafeState>,
}

/// Description d'une capacité du manifeste (D47, edge-model.md §2/§7).
/// Ids canon D16 (`[a-z0-9_:]`), familles D44 (`measurement` | `state` |
/// `actuator`). Le manifeste est **additif only** (jamais de renommage,
/// école CONTRACT).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapDesc {
    /// Id de capacité — ex. `temperature`, `digital_state`, `adc_raw`.
    pub id: String,
    /// Famille sémantique D44.
    pub family: String,
    /// Unité (famille `measurement`) — informatif en P0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Version du contrat de capacité ; défaut 1.
    #[serde(default = "default_cap_version")]
    pub version: u32,
}

fn default_cap_version() -> u32 {
    1
}

/// Une régulation castée à une carte mixte (`ControlConfig`) — struct plat
/// unique (miroir ArduinoJson trivial côté C++, pas d'imbrication) : champs
/// PID à 0 pour les TT, `deadband` ignoré pour le PID. Le sens TT est porté
/// par `kind` (`tt_heat` | `tt_cool` | `pid`), comme `RegConfig::kind_str`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlSpec {
    /// Id éditeur du nœud d'origine (`pnex_node_id` projeté) — clé du diag
    /// `RegState` retourné par le device.
    pub node_id: String,
    pub sensor_gpio: u16,
    pub sensor_mode: Mode,
    pub out_gpio: u16,
    pub kind: String,
    pub setpoint: f64,
    pub deadband: f64,
    #[serde(default)]
    pub kp: f64,
    #[serde(default)]
    pub ki: f64,
    #[serde(default)]
    pub kd: f64,
    /// Période du cycle relais time-proportional (PID ; ignoré TT).
    #[serde(default)]
    pub cycle_time_secs: u32,
    #[serde(default)]
    pub min_on_secs: u32,
    #[serde(default)]
    pub min_off_secs: u32,
    #[serde(default)]
    pub sample_ms: u32,
    #[serde(default)]
    pub data_timeout_secs: u32,
    #[serde(default)]
    pub safe_state: SafeState,
}

/// Device → serveur.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DeviceMsg {
    /// Premier message après connexion : admission policy `Validated`
    /// (dérive overlay → `caps::validate` → persiste → `ProvisionAck`).
    Announce {
        /// « esp8266 » | « esp32-c3 » | « esp32 » — informative: admission
        /// validates against the SoC of the device's board.
        chip: String,
        /// Board déclaré par le device (« nodemcu », « d1_mini »…) —
        /// informative only (the real board is the device's board in
        /// database).
        board: String,
        /// Version du firmware générique (politique de re-flash, §10).
        fw: String,
        /// Manifeste de capacités (D47) — `None` pour un fil historique
        /// sans manifeste (additif). Pour un firmware compilé par le
        /// serveur (pin_slave), c'est une **attestation** (l'overlay board
        /// reste l'autorité des pins, bascule douce §7) ; le serveur
        /// journalise qui tourne avec quoi (réservation OTA, §9).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caps: Option<Vec<CapDesc>>,
        /// Pins declared by the sketch (custom firmware — additive).
        /// `None`/absent for generic models: the board overlay stays the
        /// authority. For a custom firmware device, the source of the pin
        /// map (absent = no pin).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pins: Option<Vec<PinDecl>>,
    },
    /// Lecture périodique d'un pin input (ou réponse à un read).
    ///
    /// Étendu D46/D47 (additif) : un fil historique sans ces champs reste
    /// valide. `cap_id` présent → routage sémantique de la série O2
    /// (`{org}/{device}/{capacité}`) ; absent → routage par label overlay
    /// (autorité du pin_slave). `uptime_ms`/`boot_id`/`seq` = clé de dédup
    /// et reconstruction du temps quand le buffering existera (F3, D46).
    ///
    /// D87 (custom firmware): `gpio` is absent for a custom metric — the
    /// report is then routed by `cap_id` alone, which must name a `metric`
    /// cap from the device's announce manifest.
    StateReport {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gpio: Option<u16>,
        value: Value,
        /// Capacité rapportée (D47).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_id: Option<String>,
        /// uptime `millis()` device à la capture (D46).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uptime_ms: Option<u64>,
        /// Id de boot (tiré au boot, D46) — clé de dédup `(boot_id, seq)`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        boot_id: Option<String>,
        /// Ordre par boot (D46) — clé de dédup `(boot_id, seq)`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
    },
    /// Accusé d'un RPC serveur (`cmd_id` requis — sémantique ThingsBoard).
    Ack {
        cmd_id: String,
        ok: bool,
        err: Option<String>,
    },
    /// OTA deployment progress/result (additive — replies to a
    /// `ServerMsg::OtaAvailable`; legacy firmware never emits it).
    /// `phase` = `downloading` | `flashing` | `failed` (wire string, same
    /// school as `ControlSpec.kind`: tolerant to future phases). Any
    /// uplink frame resets the session watchdog — progress keeps the
    /// connection alive.
    OtaState {
        cmd_id: String,
        phase: String,
        /// 0..=100.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
    /// Diagnostic des régulations embarquées (cartes mixtes, control-cards)
    /// — toléré absent : le firmware générique ne l'émet jamais.
    RegState { entries: Vec<RegDiag> },
    /// Edge agent (D95): durable, ordered batch of free-form points drained
    /// from the agent disk queue. `epoch` identifies the queue lifetime (the
    /// agent's `boot_id`, persistent across restarts); `seq` is strictly
    /// increasing within an epoch — the server dedups replays with a
    /// high-water mark and answers `ServerMsg::BatchAck`.
    Batch {
        epoch: String,
        points: Vec<BatchPoint>,
    },
}

/// One free-form edge agent point (D95) — no predeclared schema: any key,
/// any JSON value, optional unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchPoint {
    /// Monotonic sequence number within the batch epoch.
    pub seq: u64,
    /// Raw key as sent by the local producer (normalized server-side).
    pub key: String,
    /// Any JSON value: numbers/booleans become metric series, text/objects
    /// become event records.
    pub value: Value,
    /// Capture time, epoch milliseconds (producer clock or agent receipt).
    pub ts_ms: i64,
    /// Optional free-form unit (`°C`, `kWh`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Force OpenObserve recording for this point, on top of the per-key
    /// server toggle (values always reach the live Valkey cache).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub record: bool,
}

// Manual `Deserialize`: same school as `FlowNodeKind` — internally-tagged
// enums buffer numeric fields through serde's `Content` deserializer and
// `arbitrary_precision` (unified workspace-wide by the starlark dependency)
// turns those buffered numbers into private magic maps, so typed fields
// like `gpio: u16` fail with "invalid type: map". Parsing through a
// `serde_json::Value` keeps numbers as real Numbers. Uplink parse site:
// `ws_device.rs` (runtime device ingest — must survive any feature
// unification).
impl<'de> Deserialize<'de> for DeviceMsg {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        let parsed: Result<Self, String> = (|| {
            let tag = v
                .get("t")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing field `t`".to_string())?;
            match tag {
                "announce" => Ok(Self::Announce {
                    chip: req_str(&v, "chip")?,
                    board: req_str(&v, "board")?,
                    fw: req_str(&v, "fw")?,
                    caps: opt_list(&v, "caps")?,
                    pins: opt_list(&v, "pins")?,
                }),
                "state_report" => Ok(Self::StateReport {
                    gpio: opt_u16(&v, "gpio")?,
                    value: v
                        .get("value")
                        .cloned()
                        .ok_or_else(|| "missing field `value`".to_string())?,
                    cap_id: opt_str(&v, "cap_id"),
                    uptime_ms: opt_u64(&v, "uptime_ms")?,
                    boot_id: opt_str(&v, "boot_id"),
                    seq: opt_u64(&v, "seq")?,
                }),
                "ack" => Ok(Self::Ack {
                    cmd_id: req_str(&v, "cmd_id")?,
                    ok: req_bool(&v, "ok")?,
                    err: opt_str(&v, "err"),
                }),
                "ota_state" => Ok(Self::OtaState {
                    cmd_id: req_str(&v, "cmd_id")?,
                    phase: req_str(&v, "phase")?,
                    progress: opt_u8(&v, "progress")?,
                    err: opt_str(&v, "err"),
                }),
                "reg_state" => Ok(Self::RegState {
                    entries: req_list(&v, "entries")?,
                }),
                "batch" => Ok(Self::Batch {
                    epoch: req_str(&v, "epoch")?,
                    points: req_list(&v, "points")?,
                }),
                other => Err(format!("unknown device message: {other}")),
            }
        })();
        parsed.map_err(serde::de::Error::custom)
    }
}

/// Required string field (arbitrary_precision-safe extraction).
fn req_str(v: &serde_json::Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing or non-string field `{key}`"))
}

/// Optional string field: absent or null maps to `None`.
fn opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .filter(|x| !x.is_null())
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Optional struct list: absent or null maps to `None`.
fn opt_list<T: serde::de::DeserializeOwned>(
    v: &serde_json::Value,
    key: &str,
) -> Result<Option<Vec<T>>, String> {
    match v.get(key).filter(|x| !x.is_null()) {
        Some(x) => serde_json::from_value::<Vec<T>>(x.clone())
            .map(Some)
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

/// Required struct list.
fn req_list<T: serde::de::DeserializeOwned>(
    v: &serde_json::Value,
    key: &str,
) -> Result<Vec<T>, String> {
    match v.get(key).filter(|x| !x.is_null()) {
        Some(x) => serde_json::from_value(x.clone()).map_err(|e| e.to_string()),
        None => Err(format!("missing field `{key}`")),
    }
}

/// Required `u16` field (device GPIO numbers).
fn req_u16(v: &serde_json::Value, key: &str) -> Result<u16, String> {
    v.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| format!("missing or out-of-range field `{key}`"))
}

/// Optional `u16` field: absent or null maps to `None`.
fn opt_u16(v: &serde_json::Value, key: &str) -> Result<Option<u16>, String> {
    match v.get(key).filter(|x| !x.is_null()) {
        Some(x) => x
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("out-of-range field `{key}`")),
        None => Ok(None),
    }
}

/// Required boolean field.
fn req_bool(v: &serde_json::Value, key: &str) -> Result<bool, String> {
    v.get(key)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| format!("missing or non-boolean field `{key}`"))
}

/// Required `u32` field.
fn req_u32(v: &serde_json::Value, key: &str) -> Result<u32, String> {
    v.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("missing or out-of-range field `{key}`"))
}

/// Optional `u64` field: absent or null maps to `None`.
fn opt_u64(v: &serde_json::Value, key: &str) -> Result<Option<u64>, String> {
    match v.get(key).filter(|x| !x.is_null()) {
        Some(x) => x
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("non-integer field `{key}`")),
        None => Ok(None),
    }
}

/// Optional `u8` field: absent or null maps to `None`.
fn opt_u8(v: &serde_json::Value, key: &str) -> Result<Option<u8>, String> {
    match v.get(key).filter(|x| !x.is_null()) {
        Some(x) => x
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("missing or out-of-range field `{key}`")),
        None => Ok(None),
    }
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Réponse à l'`Announce` : pin map complète à appliquer (modes initiaux
    /// + safe-states). Remplace la déclaration statique du firmware.
    ProvisionAck { caps: Vec<PinSpec> },
    /// RPC : changer le mode d'un pin.
    SetMode {
        cmd_id: String,
        gpio: u16,
        mode: Mode,
        #[serde(default)]
        opts: ModeOpts,
    },
    /// RPC : écrire sur une sortie — booléen 0/1 pour `digital_out`,
    /// duty % (nombre 0..=100) pour `pwm_out`.
    Write {
        cmd_id: String,
        gpio: u16,
        value: Value,
    },
    /// RPC : cadencer les lectures d'un pin input (0 = désabonner).
    Subscribe {
        cmd_id: String,
        gpio: u16,
        interval_ms: u32,
    },
    /// RPC : remplacement **complet** du jeu de régulations embarquées du
    /// device (cartes mixtes, control-cards) — `configs = []` remet toutes
    /// les sorties régulées en safe-state. Casté après `ProvisionAck` (à
    /// l'announce) puis à chaque deploy qui change ces configs ; le device
    /// répond `Ack{cmd_id}`. Le firmware générique ignore poliment (pas
    /// d'Ack — comportement attendu, jamais une erreur).
    ControlConfig {
        cmd_id: String,
        configs: Vec<ControlSpec>,
    },
    /// RPC: an OTA payload is available for this device — download `url`
    /// over HTTP(S), verify `sha256`, write the inactive slot, reboot;
    /// progress flows back via `DeviceMsg::OtaState{cmd_id}`. `url` is a
    /// **path**: the device prefixes `https://` + its compiled HOST.
    /// `size` is advisory (8266 staging-space guard).
    OtaAvailable {
        cmd_id: String,
        version: String,
        url: String,
        sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        /// Ed25519 signature (hex, 128 chars) of
        /// [`crate::ota_sig::signed_message`] by the instance OTA key
        /// (SEC-18). The firmware refuses an image without a valid one; the
        /// server never sends an unsigned order.
        sig: String,
    },
    /// RPC: camera capture settings (camera-video.md D76) — pushed after the
    /// announce of a device carrying the `camera` cap, on every settings
    /// change and on every live-demand transition (`on_demand` mode).
    /// `enabled = false` stops the `/ws/camera` uplink; the device answers
    /// `Ack{cmd_id}`. Additive: non-camera firmware answers "unknown
    /// message" and does nothing.
    CameraConfig {
        cmd_id: String,
        enabled: bool,
        /// Wire id of [`crate::camera::FrameSize`] (`vga`, `svga`, …).
        framesize: String,
        /// ESP JPEG quality, 10 (best) ..= 63 (worst).
        quality: u8,
        /// Target frames per second (device-side pacing).
        fps: u8,
        vflip: bool,
        hmirror: bool,
    },
    /// RPC (D88): invoke a custom command registered by the firmware
    /// (`pnex.onCommand(name, …)`, announced as a `command` cap). `args` is
    /// free JSON handed to the sketch callback; the device answers
    /// `Ack{cmd_id}` (`unknown_command` when the name is not registered).
    Command {
        cmd_id: String,
        name: String,
        #[serde(default)]
        args: Value,
    },
    /// Refus d'admission ou erreur fatale — suivi d'une close frame.
    Reject { reason: String },
    /// Edge agent (D95): every point of `epoch` up to `up_to_seq` (inclusive)
    /// has been accepted (or was a duplicate) — the agent may purge them.
    BatchAck { epoch: String, up_to_seq: u64 },
    /// Edge agent (D95): runtime parameters pushed after the announce and on
    /// every settings change. No metric catalogue — ingestion is free-form.
    AgentConfig {
        /// Upper bound of points per `Batch` frame.
        max_batch: u32,
        /// Distinct keys quota of this agent (server-enforced, informative
        /// agent-side).
        max_keys: u32,
    },
}

// Manual `Deserialize`: same school as `DeviceMsg` — the tagged-enum
// Content buffering poisons every nested numeric field (even inside plain
// structs like `ControlSpec`), and pnex-core unit tests DO run under the
// workspace-unified arbitrary_precision serde_json, so the golden-vector
// roundtrips break without this. Parsing through a `serde_json::Value`
// keeps numbers as real Numbers.
impl<'de> Deserialize<'de> for ServerMsg {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        let parsed: Result<Self, String> = (|| {
            let tag = v
                .get("t")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing field `t`".to_string())?;
            match tag {
                "provision_ack" => Ok(Self::ProvisionAck {
                    caps: req_list(&v, "caps")?,
                }),
                "set_mode" => Ok(Self::SetMode {
                    cmd_id: req_str(&v, "cmd_id")?,
                    gpio: req_u16(&v, "gpio")?,
                    mode: serde_json::from_value(
                        v.get("mode")
                            .cloned()
                            .ok_or_else(|| "missing field `mode`".to_string())?,
                    )
                    .map_err(|e| e.to_string())?,
                    opts: match v.get("opts") {
                        Some(o) => serde_json::from_value(o.clone()).map_err(|e| e.to_string())?,
                        None => ModeOpts::default(),
                    },
                }),
                "write" => Ok(Self::Write {
                    cmd_id: req_str(&v, "cmd_id")?,
                    gpio: req_u16(&v, "gpio")?,
                    value: v
                        .get("value")
                        .cloned()
                        .ok_or_else(|| "missing field `value`".to_string())?,
                }),
                "subscribe" => Ok(Self::Subscribe {
                    cmd_id: req_str(&v, "cmd_id")?,
                    gpio: req_u16(&v, "gpio")?,
                    interval_ms: req_u32(&v, "interval_ms")?,
                }),
                "control_config" => Ok(Self::ControlConfig {
                    cmd_id: req_str(&v, "cmd_id")?,
                    configs: req_list(&v, "configs")?,
                }),
                "ota_available" => Ok(Self::OtaAvailable {
                    cmd_id: req_str(&v, "cmd_id")?,
                    version: req_str(&v, "version")?,
                    url: req_str(&v, "url")?,
                    sha256: req_str(&v, "sha256")?,
                    size: opt_u64(&v, "size")?,
                    sig: req_str(&v, "sig")?,
                }),
                "camera_config" => Ok(Self::CameraConfig {
                    cmd_id: req_str(&v, "cmd_id")?,
                    enabled: req_bool(&v, "enabled")?,
                    framesize: req_str(&v, "framesize")?,
                    quality: opt_u8(&v, "quality")?
                        .ok_or_else(|| "missing field `quality`".to_string())?,
                    fps: opt_u8(&v, "fps")?.ok_or_else(|| "missing field `fps`".to_string())?,
                    vflip: v
                        .get("vflip")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    hmirror: v
                        .get("hmirror")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                }),
                "command" => Ok(Self::Command {
                    cmd_id: req_str(&v, "cmd_id")?,
                    name: req_str(&v, "name")?,
                    args: v.get("args").cloned().unwrap_or(Value::Null),
                }),
                "reject" => Ok(Self::Reject {
                    reason: req_str(&v, "reason")?,
                }),
                "batch_ack" => Ok(Self::BatchAck {
                    epoch: req_str(&v, "epoch")?,
                    up_to_seq: opt_u64(&v, "up_to_seq")?
                        .ok_or_else(|| "missing field `up_to_seq`".to_string())?,
                }),
                "agent_config" => Ok(Self::AgentConfig {
                    max_batch: req_u32(&v, "max_batch")?,
                    max_keys: req_u32(&v, "max_keys")?,
                }),
                other => Err(format!("unknown server message: {other}")),
            }
        })();
        parsed.map_err(serde::de::Error::custom)
    }
}

/// « null » fil (ArduinoJson sérialise NaN/Inf en `null`) → `f64::NAN`.
/// Sérialisation : serde_json émet déjà `null` pour NaN — symétrique.
fn f64_nan_or_null<'de, D>(d: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
}

/// Diagnostic d'une régulation embarquée, rapporté par le device
/// (`RegState`, cadence lente ≤ 1 Hz) — alimente la télémétrie/diag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegDiag {
    /// Id du nœud d'origine (`ControlSpec::node_id`).
    pub node_id: String,
    /// Sens de la carte (`tt_heat` | `tt_cool` | `pid`).
    pub kind: String,
    pub setpoint: f64,
    /// Dernière mesure du capteur (NaN si illisible ; `null` fil = NaN).
    #[serde(deserialize_with = "f64_nan_or_null")]
    pub sensor_value: f64,
    /// Sortie courante : 0/100 (TT) ou duty % (PID).
    pub output_pct: f64,
    pub cycle_count: u64,
}

/// Résolution du rôle à partir du mode — sensor/actuator est un rôle dérivé,
/// jamais une colonne (B0.6, modèle sans copies).
pub fn role_of(mode: Mode) -> &'static str {
    match mode {
        Mode::DigitalIn | Mode::AdcIn => "sensor",
        Mode::DigitalOut | Mode::PwmOut => "actuator",
    }
}

/// Firmware version = build id (monotonic decimal string, e.g. `"42"`).
/// Tolerates `1.0.421` shapes (last segment); `None` for non-numeric
/// versions (e.g. the pre-OTA `"1.0.0"` default — equality fallback only).
pub fn fw_version_num(v: &str) -> Option<u64> {
    v.rsplit('.').next()?.parse::<u64>().ok()
}

/// "does the device run at least the target?" — numeric as soon as both
/// parse (a concurrent rebuild overtaking the target still counts as
/// success); strict-equality fallback when either side is non-numeric.
pub fn fw_at_least(reported: &str, target: &str) -> bool {
    match (fw_version_num(reported), fw_version_num(target)) {
        (Some(r), Some(t)) => r >= t,
        _ => reported == target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fw_version_numeric_monotonic() {
        assert_eq!(fw_version_num("42"), Some(42));
        assert_eq!(fw_version_num("1.0.421"), Some(421));
        // Pre-OTA ("1.0.0"): not numerically comparable to a target.
        assert_eq!(fw_version_num("1.0.0"), Some(0));
        assert_eq!(fw_version_num("dev"), None);
        // Numeric as soon as possible; equality fallback otherwise.
        assert!(fw_at_least("43", "42"));
        assert!(!fw_at_least("41", "42"));
        assert!(fw_at_least("1.0.421", "421"));
        assert!(!fw_at_least("1.0.0", "42"));
        assert!(fw_at_least("dev", "dev"));
        assert!(!fw_at_least("dev", "42"));
    }

    #[test]
    fn roundtrip_ota_variants() {
        let s: ServerMsg = serde_json::from_str(
            r#"{"t":"ota_available","cmd_id":"c1","version":"42","url":"/api/v1/ota/firmware/d/42.bin","sha256":"ab12","size":450000,"sig":"00"}"#,
        )
        .unwrap();
        assert!(
            matches!(s, ServerMsg::OtaAvailable { cmd_id, version, size, .. }
            if cmd_id == "c1" && version == "42" && size == Some(450_000))
        );
        let d: DeviceMsg = serde_json::from_str(
            r#"{"t":"ota_state","cmd_id":"c1","phase":"downloading","progress":57}"#,
        )
        .unwrap();
        assert!(matches!(d, DeviceMsg::OtaState { phase, progress, .. }
            if phase == "downloading" && progress == Some(57)));
        // Unknown phase tolerated (ControlSpec.kind school) + optional fields absent.
        let d: DeviceMsg =
            serde_json::from_str(r#"{"t":"ota_state","cmd_id":"c1","phase":"rebooting"}"#).unwrap();
        assert!(matches!(d, DeviceMsg::OtaState { phase, progress, err, .. }
            if phase == "rebooting" && progress.is_none() && err.is_none()));
    }

    #[test]
    fn roundtrip_messages_taguees_t() {
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"announce","chip":"esp8266","board":"nodemcu","fw":"0.1.0"}"#,
        )
        .unwrap();
        assert!(
            matches!(m, DeviceMsg::Announce { chip, board, fw, caps, pins }
            if chip == "esp8266" && board == "nodemcu" && fw == "0.1.0" && caps.is_none() && pins.is_none())
        );
    }

    /// Custom firmware: pins declared by the sketch — `pins` is additive
    /// (a legacy wire without this field stays valid).
    #[test]
    fn announce_avec_pins_declarees_roundtrip() {
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"announce","chip":"esp32-c3","board":"mon_carte_maison","fw":"1.0.0",
                "pins":[{"gpio":2,"label":"pump","mode":"digital_out","safe_state":"low"},
                        {"gpio":3,"label":"door","mode":"digital_in","pullup":true},
                        {"gpio":0,"label":"photo","mode":"adc_in"}]}"#,
        )
        .unwrap();
        let DeviceMsg::Announce {
            pins: Some(pins),
            chip,
            ..
        } = &m
        else {
            panic!("announce attendu, reçu : {m:?}");
        };
        assert_eq!(chip, "esp32-c3");
        assert_eq!(pins.len(), 3);
        assert_eq!(pins[0].gpio, 2);
        assert_eq!(pins[0].label, "pump");
        assert_eq!(pins[0].mode, Mode::DigitalOut);
        assert_eq!(pins[0].pullup, None);
        assert_eq!(pins[0].safe_state, Some(SafeState::Low));
        assert_eq!(pins[1].pullup, Some(true));
        assert_eq!(pins[2].mode, Mode::AdcIn);
        // Roundtrip : pullup=None absent du fil (skip_serializing_if).
        let wire = serde_json::to_string(&m).unwrap();
        let v: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert!(
            v["pins"][0].get("pullup").is_none(),
            "pullup=None ne voyage pas : {wire}"
        );
        let back: DeviceMsg = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, m);
    }

    /// D47 : manifeste annoncé, champs optionnels à défauts (unit absent,
    /// version=1).
    #[test]
    fn announce_avec_manifeste_caps_roundtrip() {
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"announce","chip":"esp8266","board":"nodemcu","fw":"1.0.0",
                "caps":[{"id":"digital_state","family":"state"},
                        {"id":"adc_raw","family":"measurement","unit":"raw"},
                        {"id":"relay_cmd","family":"actuator"}]}"#,
        )
        .unwrap();
        let DeviceMsg::Announce {
            caps: Some(caps), ..
        } = &m
        else {
            panic!("announce attendu, reçu : {m:?}");
        };
        assert_eq!(caps.len(), 3);
        assert_eq!(caps[0].id, "digital_state");
        assert_eq!(caps[0].family, "state");
        assert_eq!(caps[0].version, 1, "défaut version=1");
        assert_eq!(caps[0].unit, None);
        assert_eq!(caps[1].unit.as_deref(), Some("raw"));
        // Roundtrip : unit absent pour None (skip_serializing_if), version
        // toujours émis (pas de skip — forme fil stable).
        let wire = serde_json::to_string(&m).unwrap();
        let v: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert!(
            v["caps"][0].get("unit").is_none(),
            "unit=None ne voyage pas : {wire}"
        );
        assert_eq!(v["caps"][1]["unit"], serde_json::json!("raw"));
        let back: DeviceMsg = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, m);
    }
    #[test]
    fn custom_metric_state_report_without_gpio() {
        // D87: a custom metric carries cap_id and no gpio.
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"state_report","cap_id":"temp","value":21.5,"uptime_ms":10,"boot_id":"b","seq":1}"#,
        )
        .unwrap();
        assert!(
            matches!(&m, DeviceMsg::StateReport { gpio: None, cap_id: Some(c), .. } if c == "temp")
        );
        // No gpio on the wire when None (skip_serializing_if).
        let wire = serde_json::to_string(&m).unwrap();
        assert!(!wire.contains("gpio"), "{wire}");
        let back: DeviceMsg = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, m);
        // Out-of-range gpio is still refused.
        assert!(serde_json::from_str::<DeviceMsg>(
            r#"{"t":"state_report","gpio":70000,"value":1}"#
        )
        .is_err());
    }

    #[test]
    fn custom_command_roundtrip() {
        // D88: ServerMsg::Command wire shape.
        let m = ServerMsg::Command {
            cmd_id: "c1".into(),
            name: "calibrate".into(),
            args: serde_json::json!({"offset": 1.5}),
        };
        let wire = serde_json::to_string(&m).unwrap();
        let v: Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(v["t"], "command");
        assert_eq!(v["name"], "calibrate");
        let back: ServerMsg = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, m);
        // args is optional on the wire (null).
        let back: ServerMsg =
            serde_json::from_str(r#"{"t":"command","cmd_id":"c2","name":"reset"}"#).unwrap();
        assert!(matches!(
            back,
            ServerMsg::Command {
                args: Value::Null,
                ..
            }
        ));
    }

    #[test]
    fn state_report_et_ack_roundtrip() {
        // Fil historique sans champs étendus : reste valide (additif D46/D47).
        let m: DeviceMsg =
            serde_json::from_str(r#"{"t":"state_report","gpio":5,"value":true}"#).unwrap();
        assert!(
            matches!(m, DeviceMsg::StateReport { gpio: Some(5), value, cap_id: None, uptime_ms: None, boot_id: None, seq: None }
            if value == Value::Bool(true))
        );

        // Forme étendue complète (F2) : cap_id + horloge D46.
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"state_report","gpio":5,"value":true,"cap_id":"digital_state",
                "uptime_ms":123456,"boot_id":"a1b2c3d4","seq":42}"#,
        )
        .unwrap();
        assert!(
            matches!(m, DeviceMsg::StateReport { gpio: Some(5), value, cap_id, uptime_ms, boot_id, seq }
            if value == Value::Bool(true)
                && cap_id.as_deref() == Some("digital_state")
                && uptime_ms == Some(123456)
                && boot_id.as_deref() == Some("a1b2c3d4")
                && seq == Some(42))
        );

        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"ack","cmd_id":"abc","ok":false,"err":"pin non configuré"}"#,
        )
        .unwrap();
        assert!(matches!(m, DeviceMsg::Ack { cmd_id, ok: false, .. }
            if cmd_id == "abc"));
    }

    #[test]
    fn server_msgs_roundtrip() {
        let m: ServerMsg =
            serde_json::from_str(r#"{"t":"set_mode","cmd_id":"c1","gpio":5,"mode":"digital_out"}"#)
                .unwrap();
        assert!(
            matches!(m, ServerMsg::SetMode { cmd_id: _, gpio: 5, mode: Mode::DigitalOut, opts }
            if opts == ModeOpts::default())
        );

        let m: ServerMsg =
            serde_json::from_str(r#"{"t":"write","cmd_id":"c2","gpio":5,"value":true}"#).unwrap();
        assert!(matches!(m, ServerMsg::Write { cmd_id, gpio: 5, value }
            if cmd_id == "c2" && value == Value::Bool(true)));
    }
    #[test]
    fn provision_ack_et_reject_roundtrip() {
        let m: ServerMsg = serde_json::from_str(
            r#"{"t":"provision_ack","caps":[{"gpio":5,"label":"D1","mode":"digital_out","safe_state":"low"}]}"#,
        )
        .unwrap();
        assert!(matches!(m, ServerMsg::ProvisionAck { caps }
            if caps.len() == 1
                && caps[0].gpio == 5
                && caps[0].label == "D1"
                && caps[0].mode == Mode::DigitalOut
                && caps[0].safe_state == Some(SafeState::Low)
                && caps[0].opts.is_none()));

        // Round-trip du pullup persisté (fix : opts absents avant — le
        // pullup posé en UI était réinitialisé au device à chaque
        // re-announce).
        let m: ServerMsg = serde_json::from_str(
            r#"{"t":"provision_ack","caps":[{"gpio":3,"label":"door","mode":"digital_in",
                "opts":{"pullup":true}}]}"#,
        )
        .unwrap();
        assert!(matches!(m, ServerMsg::ProvisionAck { ref caps }
            if caps.len() == 1
                && caps[0].opts == Some(ModeOpts { pullup: Some(true), safe_state: None })));
        let wire = serde_json::to_string(&m).unwrap();
        assert!(wire.contains(r#""pullup":true"#), "{wire}");
        // opts=None n'embarque pas d'objet vide (skip_serializing_if).
        let sans = ServerMsg::ProvisionAck {
            caps: vec![PinSpec {
                gpio: 2,
                label: "D0".into(),
                mode: Mode::DigitalIn,
                safe_state: None,
                opts: None,
            }],
        };
        let wire = serde_json::to_string(&sans).unwrap();
        assert!(!wire.contains("opts"), "{wire}");

        let m: ServerMsg =
            serde_json::from_str(r#"{"t":"reject","reason":"device inconnu"}"#).unwrap();
        assert!(matches!(m, ServerMsg::Reject { reason }
            if reason == "device inconnu"));
    }

    #[test]
    fn role_of_derive_le_role_du_mode() {
        assert_eq!(role_of(Mode::DigitalIn), "sensor");
        assert_eq!(role_of(Mode::AdcIn), "sensor");
        assert_eq!(role_of(Mode::DigitalOut), "actuator");
        assert_eq!(role_of(Mode::PwmOut), "actuator");
    }

    /// PWM end-to-end wire contract: mode `pwm_out` roundtrips and the duty
    /// rides the existing `Write.value` / `StateReport.value` as a duty
    /// percentage 0..=100 (hardware abstraction: only the firmware knows the
    /// 8-bit range; no frequency config on the wire).
    #[test]
    fn pwm_out_roundtrip() {
        let m: ServerMsg =
            serde_json::from_str(r#"{"t":"set_mode","cmd_id":"c3","gpio":4,"mode":"pwm_out"}"#)
                .unwrap();
        assert!(
            matches!(m, ServerMsg::SetMode { gpio: 4, mode: Mode::PwmOut, opts, .. }
            if opts == ModeOpts::default())
        );

        let m: ServerMsg =
            serde_json::from_str(r#"{"t":"write","cmd_id":"c4","gpio":4,"value":50}"#).unwrap();
        assert!(matches!(m, ServerMsg::Write { gpio: 4, value, .. }
            if value == Value::Number(50.into())));

        // Device reports duty % back on StateReport (same Value wire).
        let m: DeviceMsg =
            serde_json::from_str(r#"{"t":"state_report","gpio":4,"value":50}"#).unwrap();
        assert!(
            matches!(m, DeviceMsg::StateReport { gpio: Some(4), value, .. }
            if value == Value::Number(50.into()))
        );
    }

    #[test]
    fn control_config_roundtrip() {
        // Remplacement complet : 1 régulation TT + 1 PID sur la même carte.
        let json = r#"{"t":"control_config","cmd_id":"c7","configs":[
            {"node_id":"r1","sensor_gpio":17,"sensor_mode":"adc_in","out_gpio":5,"kind":"tt_heat",
             "setpoint":19.0,"deadband":0.5,"min_on_secs":5,"min_off_secs":5,
             "sample_ms":5000,"data_timeout_secs":30,"safe_state":"low"},
            {"node_id":"r2","sensor_gpio":17,"sensor_mode":"adc_in","out_gpio":4,"kind":"pid",
             "setpoint":21.0,"deadband":0.0,"kp":2.0,"ki":0.1,"kd":0.5,
             "cycle_time_secs":10,"sample_ms":2000,"data_timeout_secs":30,"safe_state":"high"}
        ]}"#;
        let m: ServerMsg = serde_json::from_str(json).unwrap();
        let ServerMsg::ControlConfig { cmd_id, configs } = &m else {
            panic!("variante inattendue : {m:?}");
        };
        assert_eq!(cmd_id, "c7");
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].kind, "tt_heat");
        assert_eq!(configs[0].sensor_gpio, 17);
        assert_eq!(configs[0].sensor_mode, Mode::AdcIn);
        assert_eq!(configs[0].out_gpio, 5);
        assert_eq!(configs[0].min_on_secs, 5);
        assert_eq!(configs[0].safe_state, SafeState::Low);
        assert_eq!(configs[1].kind, "pid");
        assert!((configs[1].kp - 2.0).abs() < 1e-9);
        assert_eq!(configs[1].cycle_time_secs, 10);
        // Roundtrip : le fil re-sérialisé est réutilisable tel quel.
        let back: ServerMsg = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn control_config_vide_remplace_tout() {
        // `[]` = clear : remet toutes les sorties régulées en safe.
        let m: ServerMsg =
            serde_json::from_str(r#"{"t":"control_config","cmd_id":"c8","configs":[]}"#).unwrap();
        assert!(matches!(m, ServerMsg::ControlConfig { cmd_id, configs }
            if cmd_id == "c8" && configs.is_empty()));
    }

    #[test]
    fn reg_state_roundtrip() {
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"reg_state","entries":[
                {"node_id":"r1","kind":"tt_heat","setpoint":19.0,"sensor_value":18.4,
                 "output_pct":100.0,"cycle_count":3}
            ]}"#,
        )
        .unwrap();
        assert!(matches!(m, DeviceMsg::RegState { entries }
            if entries.len() == 1
                && entries[0].node_id == "r1"
                && entries[0].kind == "tt_heat"
                && (entries[0].sensor_value - 18.4).abs() < 1e-9
                && entries[0].output_pct == 100.0
                && entries[0].cycle_count == 3));
        // Le générique n'émet jamais ce message — mais un fil historique
        // sans `reg_state` reste valide : c'est une variante optionnelle.
    }

    /// Convention fil : ArduinoJson sérialise NaN/Inf en `null` — « null »
    /// désérialise en NaN (avant ce fix, un RegState « capteur illisible »
    /// était droppé au parse).
    #[test]
    fn reg_state_sensor_null_devient_nan() {
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"reg_state","entries":[
                {"node_id":"r1","kind":"pid","setpoint":21.0,"sensor_value":null,
                 "output_pct":0.0,"cycle_count":0}
            ]}"#,
        )
        .unwrap();
        let DeviceMsg::RegState { entries } = &m else {
            panic!("reg_state attendu : {m:?}");
        };
        assert!(entries[0].sensor_value.is_nan());
        // Symétrique : NaN ressort en « null » fil.
        let wire = serde_json::to_string(&m).unwrap();
        assert!(wire.contains(r#""sensor_value":null"#), "{wire}");
    }

    #[test]
    fn agent_batch_roundtrip_keeps_free_form_values() {
        let msg = DeviceMsg::Batch {
            epoch: "e1".into(),
            points: vec![
                BatchPoint {
                    seq: 1,
                    key: "temp".into(),
                    value: serde_json::json!(21.5),
                    ts_ms: 1_700_000_000_123,
                    unit: Some("°C".into()),
                    record: false,
                },
                BatchPoint {
                    seq: u64::MAX - 1,
                    key: "status".into(),
                    value: serde_json::json!({"ok": true, "n": 3}),
                    ts_ms: -5,
                    unit: None,
                    record: true,
                },
            ],
        };
        let wire = serde_json::to_string(&msg).unwrap();
        assert!(!wire.contains("\"unit\":null"));
        let back: DeviceMsg = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn agent_server_messages_roundtrip() {
        for msg in [
            ServerMsg::BatchAck {
                epoch: "e1".into(),
                up_to_seq: 42,
            },
            ServerMsg::AgentConfig {
                max_batch: 500,
                max_keys: 1000,
            },
        ] {
            let wire = serde_json::to_string(&msg).unwrap();
            let back: ServerMsg = serde_json::from_str(&wire).unwrap();
            assert_eq!(back, msg);
        }
    }
}
