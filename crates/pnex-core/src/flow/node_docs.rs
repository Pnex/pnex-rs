//! Assistant documentation of every flow node kind (D142, layer 1).
//!
//! Single source of the knowledge the in-app assistant has about flow
//! nodes: the backend tool `describe_node_types` serializes this table, it
//! never keeps a hand-written list of its own. Adding a [`super::FlowNodeKind`]
//! variant without its entry here fails the `every_kind_is_documented`
//! guard (9th wiring point of the "new flow node" recipe).
//!
//! Text is English (canonical language; the assistant answers in the
//! user's language). It describes what a user-facing flow author needs:
//! purpose, config fields, ports, and the pitfalls users actually hit.

/// Documentation of one node kind, as the assistant sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeDoc {
    /// Serialized `kind` tag of [`super::FlowNodeKind`] (`"device_read"`…).
    pub kind: &'static str,
    /// What the node does, in one or two sentences.
    pub summary: &'static str,
    /// `(field, meaning)` pairs of the node `config` object.
    pub config: &'static [(&'static str, &'static str)],
    /// Ports, wiring and pitfalls; empty when there is nothing to add.
    pub notes: &'static str,
}

/// Graph-level authoring rules, served before the per-kind docs.
pub const FLOW_AUTHORING_RULES: &[(&str, &str)] = &[
    (
        "graph",
        "A flow graph is {\"nodes\": [...]}. Each node: {\"id\": unique free string, \"kind\": node kind, \"config\": {...}, \"outputs\": [{\"port\": n, \"targets\": [ids of the next nodes]}], \"inputs\": [{\"pin\": row name, \"from\": source node id, \"from_port\": n}] (only for nodes with named input rows)}.",
    ),
    (
        "canonical_pipeline",
        "inject (trigger) → device_read → calc → metric. A flow runs continuously: a periodic inject (repeat_secs) is the default idiom. Event sources (camera_source, control_source, media_source) need no inject.",
    ),
    (
        "payload_key_rule",
        "device_read payload keys are sanitize(device_slug) + \"_\" + sanitize(pin_label), CASE PRESERVED (device \"proud-puffin\" + pin \"A0\" → \"proud_puffin_A0\"; \"proud_puffin_a0\" is rejected as calc_case_mismatch). calc variables are these keys.",
    ),
    (
        "metric_rule",
        "metric writes the series etl_<sanitized name> with device_id=\"flow_<flow id>\"; an object payload writes one series etl_<name>_<field> per numeric field.",
    ),
    (
        "named_input_rows",
        "Wiring a named input row (device_write pin or command, pnex_notify trigger/variable, pnex_function input, json_merge input) takes BOTH sides: the source node lists the target in outputs[port].targets AND the target declares the row in its own inputs.",
    ),
    (
        "saved_is_not_deployed",
        "Saving a flow never deploys it. Deploying, stopping and deleting are human actions in the flow editor; a running flow keeps executing its deployed version until the user redeploys.",
    ),
    (
        "physical_boundary",
        "Only a deployed flow acts on devices (device_write, regulation cards). Dashboards and annotations write controls; a control_source node turns them into messages.",
    ),
];

/// Example graph shown with the docs; kept valid by a test.
pub const FLOW_EXAMPLE: &str = r#"{"nodes": [
  {"id": "n1", "kind": "inject", "config": {"repeat_secs": 2}, "outputs": [{"port": 0, "targets": ["n2"]}]},
  {"id": "n2", "kind": "device_read", "config": {"device_id": "probe-1", "pins": ["A0"]}, "outputs": [{"port": 0, "targets": ["n3"]}]},
  {"id": "n3", "kind": "calc", "config": {"expression": "probe_1_A0 * 0.01"}, "outputs": [{"port": 0, "targets": ["n4"]}]},
  {"id": "n4", "kind": "metric", "config": {"metric_name": "soil_volt"}}
]}"#;

/// Every node kind, in palette order.
pub const NODE_DOCS: &[NodeDoc] = &[
    NodeDoc {
        kind: "inject",
        summary: "Trigger. A flow without an event source needs one.",
        config: &[
            ("repeat_secs", "seconds between injections (e.g. 30) — the default idiom, the flow runs continuously"),
            ("cron", "5-6 field cron expression (alternative to repeat_secs)"),
            ("once_delay_secs", "single injection after this delay (one-off, avoid)"),
            ("topic", "optional string"),
            ("payload", "injected JSON value (null = timestamp)"),
        ],
        notes: "One of repeat_secs, cron or once_delay_secs is required (no_trigger otherwise).",
    },
    NodeDoc {
        kind: "device_read",
        summary: "Reads the latest values of the pins of ONE device (live cache, then OpenObserve).",
        config: &[
            ("device_id", "device slug — one device per node"),
            ("pins", "[pin label, e.g. \"A0\", or a metric a custom firmware publishes, e.g. \"temperature\"] — one output port per entry, in this order"),
            ("window_secs", "freshness window 1..=3600 (default 60); no data = payload {} (never an invented zero)"),
        ],
        notes: "Ports: one per pin (payload {key: value}), then a last \"all\" port with every pin in one object (to combine pins of the same device in a calc). No telemetry at all usually means no pin is subscribed on the device (Pins page).",
    },
    NodeDoc {
        kind: "device_write",
        summary: "Writes the output pins of ONE device (digital 1/0, PWM duty 0..100) and sends the commands its custom firmware announces. Passthrough output.",
        config: &[
            ("device_id", "device slug — one device per node"),
            ("pins", "[output pin label (digital_out / pwm_out)] — one input row per pin"),
            ("commands", "[command announced by the device's custom firmware (pnex.onCommand)] — one input row per command, after the pins; pins may then be empty"),
        ],
        notes: "Incoming payload = map {pin_or_command: value}: only configured names present in the map are written. A scalar payload is routed by the input row it arrives on, or accepted when a single pin/command is configured. A command receives args = {\"value\": payload} (an RGB colour control sends 0xRRGGBB as a number); a name the firmware does not announce is refused by the server (logged, nothing sent). An output pin has one writer: a pin already written by another deployed flow is refused at deploy (pin_already_assigned); commands have no such rule.",
    },
    NodeDoc {
        kind: "calc",
        summary: "Arithmetic expression on the payload keys (operators, ternary, ^, min/max/abs/round…). Check it with validate_calc_expression.",
        config: &[("expression", "string; variables = payload keys, exact case (pin \"A0\" gives …_A0, never …_a0)")],
        notes: "",
    },
    NodeDoc {
        kind: "value",
        summary: "Replaces the payload with a fixed value (static) or a uniform random number in [min, max] (random). Transformer: still needs an upstream trigger.",
        config: &[
            ("mode", "\"static\" (default) | \"random\""),
            ("value", "any JSON value (static; null = missing → value_static_missing)"),
            ("min", "inclusive lower bound (random, default 0)"),
            ("max", "inclusive upper bound (random, default 10)"),
        ],
        notes: "",
    },
    NodeDoc {
        kind: "metric",
        summary: "Writes the payload to OpenObserve as a metric (series etl_<name>, virtual device flow_<id>), visible on the visualization pages.",
        config: &[
            ("metric_name", "string — the etl_ prefix and sanitization are automatic"),
            ("labels", "optional {label_name: source}, at most 5: source = \"msg.topic\", \"payload.<field>\" or \"msg.payload.<a>.<b>\" (one or two levels), anything else is a literal ([A-Za-z0-9_.:-], 1..=64 chars). Names match ^[a-z_][a-z0-9_]{0,31}$; device_id, pred_dev, source_type, ts_source and __* are reserved"),
        ],
        notes: "Labels let a dashboard read the series by label set instead of by device (e.g. {\"stream\": \"msg.topic\", \"taxonomy_version\": \"v1\"}). Resolved values are sanitized (other chars become _, cut to 64), a missing path gives \"unknown\". Pitfall: one node writes at most 200 distinct label-value combinations; past that, new combinations are silently dropped (known ones keep being written). Use stable values (a stream slug, an entity id, a taxonomy version), never free text.",
    },
    NodeDoc {
        kind: "cool_prop",
        summary: "Thermophysical properties (CoolProp, in-process) of a fluid or mixture from two input properties read in the payload.",
        config: &[
            ("fluid_spec", "PropsSI fluid spec: \"Water\", a predefined mixture, or inline \"Propane[0.5]&Ethane[0.5]\""),
            ("input1", "CoolProp input property name, e.g. \"T\""),
            ("input2", "CoolProp input property name, e.g. \"P\""),
            ("v1_key", "payload key holding input 1"),
            ("v2_key", "payload key holding input 2"),
            ("outputs", "[CoolProp output names, e.g. \"Hmass\", \"Dmass\"] — at least one"),
            ("include_phase", "bool: add the textual phase to the payload"),
            ("unit1", "unit of input 1 (empty = SI)"),
            ("unit2", "unit of input 2 (empty = SI)"),
            ("output_units", "{output: unit}; missing = SI"),
        ],
        notes: "Ports: 0 = every output in one object, then one port per output, then the phase port when enabled. Mixtures of the org are managed on the Thermo page.",
    },
    NodeDoc {
        kind: "pnex_notify",
        summary: "Sends a message template to notification channels (in-app, ntfy, webhook, Telegram, Slack, Discord, SMTP). Get the UUIDs with list_notifications.",
        config: &[
            ("channel_ids", "[channel UUID] — at least one"),
            ("template_id", "template UUID"),
            ("template_vars", "[variable name] — exactly the template's variables, one input row each"),
            ("strict", "bool (default false: a failed send is logged and the message passes through)"),
            ("anti_spam", "optional {max_msgs, window_secs} rate limit, e.g. {\"max_msgs\": 3, \"window_secs\": 600}"),
        ],
        notes: "Named input rows: a MANDATORY boolean `trigger` row (sent while true, e.g. a calc `x_A0 > 500`) and one row per template variable. An unwired trigger is refused at save. inputs: [{\"pin\": \"trigger\", \"from\": id, \"from_port\": n}, {\"pin\": var, …}].",
    },
    NodeDoc {
        kind: "http_fetch",
        summary: "Configurable HTTP client request: the response replaces msg.payload (JSON parsed when the content type is JSON, text otherwise) and sets msg.statusCode.",
        config: &[
            ("url", "required http(s)://… (query included, no templating)"),
            ("method", "\"get\" (default) | \"post\""),
            ("headers", "[{name, value}]"),
            ("auth", "{\"mode\": \"none\"} | {\"mode\": \"basic\", username, password} | {\"mode\": \"bearer\", token} | {\"mode\": \"header\", name, value}"),
            ("proxy", "{\"mode\": \"none\"} | {\"mode\": \"custom\", url, username?, password?}"),
            ("timeout_secs", "1..=300 (default 30)"),
            ("body", "literal POST body; absent = the incoming payload"),
            ("on_error", "\"reject\" (default) | \"passthrough\" (payload null + statusCode + http_error)"),
        ],
        notes: "Secret fields (basic and proxy passwords, bearer token, key header value) are moved to the org vault when the flow is saved; the graph then holds a reference, never the value. The server refuses targets pointing at itself or its internal services (loopback, link-local, cloud metadata, single-word host names): the request fails with egress_address_refused / egress_host_refused; LAN addresses (192.168.x.x…) stay reachable unless the platform runs in public mode.",
    },
    NodeDoc {
        kind: "debug",
        summary: "Captures messages into the Debug panel of the flow editor.",
        config: &[
            ("active", "bool (default true)"),
            ("complete", "\"payload\" (default) or \"true\" = whole message"),
            ("console", "bool"),
        ],
        notes: "",
    },
    NodeDoc {
        kind: "display",
        summary: "Live probe: shows the passing value as a badge under the node in the editor. Passthrough.",
        config: &[],
        notes: "No config field.",
    },
    NodeDoc {
        kind: "reg_tt_heat",
        summary: "On/off HEATING regulation card run ON the device itself: the device reads its own sensor and drives its own output; the server only sends the config.",
        config: &[
            ("device_id", "device slug (sensor and actuator on the same device)"),
            ("sensor_pin", "sensor pin label"),
            ("actuator_pin", "digital output pin label"),
            ("setpoint", "setpoint, in the sensor unit"),
            ("deadband", "hysteresis half-band > 0: ON below setpoint − deadband, OFF back at setpoint"),
            ("min_on_secs", "minimum ON time (default 5)"),
            ("min_off_secs", "minimum OFF time (default 5)"),
            ("sample_ms", "sensor sampling period 200..=60000 (default 5000)"),
            ("data_timeout_secs", "silent sensor beyond this → safe state (default 30)"),
            ("safe_state", "\"low\" (default) | \"high\": output at rest"),
        ],
        notes: "Standalone node: no inject, no wiring. Active only while the flow is deployed.",
    },
    NodeDoc {
        kind: "reg_tt_cool",
        summary: "On/off COOLING regulation card (reverse action of reg_tt_heat), run on the device itself.",
        config: &[
            ("device_id", "device slug"),
            ("sensor_pin", "sensor pin label"),
            ("actuator_pin", "digital output pin label"),
            ("setpoint", "setpoint"),
            ("deadband", "hysteresis half-band > 0: ON above setpoint + deadband"),
            ("min_on_secs", "minimum ON time (default 5)"),
            ("min_off_secs", "minimum OFF time (default 5)"),
            ("sample_ms", "sampling period (default 5000)"),
            ("data_timeout_secs", "silent sensor → safe state (default 30)"),
            ("safe_state", "\"low\" (default) | \"high\""),
        ],
        notes: "Standalone node: no inject, no wiring.",
    },
    NodeDoc {
        kind: "reg_pid",
        summary: "PID regulation card run on the device itself, time-proportional relay output (the duty % is spread over cycle_time_secs).",
        config: &[
            ("device_id", "device slug"),
            ("sensor_pin", "sensor pin label"),
            ("actuator_pin", "digital output pin label"),
            ("setpoint", "setpoint"),
            ("kp", "proportional gain ≥ 0"),
            ("ki", "integral gain ≥ 0"),
            ("kd", "derivative gain ≥ 0 (on the measurement)"),
            ("cycle_time_secs", "relay cycle 1..=60 (default 10)"),
            ("sample_ms", "sampling period (default 5000)"),
            ("data_timeout_secs", "silent sensor → safe state (default 30)"),
            ("safe_state", "\"low\" (default) | \"high\""),
        ],
        notes: "Standalone node: no inject, no wiring.",
    },
    NodeDoc {
        kind: "pnex_function",
        summary: "Runs a versioned user function (JavaScript or Starlark) from the Functions page.",
        config: &[
            ("function_id", "functions id (0 = not selected → fn_not_selected)"),
            ("function_name", "display name"),
            ("version_number", "pinned version"),
            ("language", "\"js\" | \"starlark\""),
            ("inputs", "snapshot of the declared inputs (input rows)"),
            ("outputs", "snapshot of the declared outputs (output ports)"),
        ],
        notes: "Find the function with list_functions / get_function (its inputs and outputs give the node rows and ports). A new function version does not change deployed flows: the node pins version_number, the user selects the new version and redeploys.",
    },
    NodeDoc {
        kind: "json_split",
        summary: "Splits a JSON payload: with keys, one named output port per key (payload = value, topic = key); without keys, one message per key or array element.",
        config: &[
            ("keys", "[key] — output ports in this order (empty = single port, one message per key)"),
            ("auto", "bool (default true): the editor regenerates keys from the upstream value/merge"),
        ],
        notes: "A key missing from the payload leaves its port silent.",
    },
    NodeDoc {
        kind: "json_merge",
        summary: "Merges messages into one object: each payload is stored under msg.topic (or default_key) and the accumulated object is emitted on every message.",
        config: &[
            ("default_key", "key for a scalar payload without topic (default \"value\")"),
            ("inputs", "[input name] — named input rows; a wire landing on a row is tagged topic = name"),
        ],
        notes: "State lives while the flow runs and resets on redeploy.",
    },
    NodeDoc {
        kind: "camera_source",
        summary: "Event source (no inject needed): one message per frame of a camera device. payload = {device_id, seq, ts_ms, width, height, size, frame_key} — a reference to the JPEG, never the bytes.",
        config: &[
            ("device_id", "camera device slug (required)"),
            ("max_fps", "sampling, 0 = every frame (default), max 25"),
        ],
        notes: "Live view needs no flow.",
    },
    NodeDoc {
        kind: "media_source",
        summary: "Event source (no inject needed): one message per transcribed segment of the listed media streams (Media page). payload = {stream, segment_id, started_at, ended_at, text, lang, asr_model, words_ref, speakers, speech_ms?}, topic = stream slug. words_ref names the OpenObserve logs stream (tx_<slug>) holding the word timings. speakers = [{label, start_ms, end_ms}] when the stream's profile has diarization, else []; speech_ms = speech found by the VAD when the profile has one.",
        config: &[
            ("streams", "[stream slug] — one or more streams of the organization (required)"),
            ("emit", "\"segment\" (default) = one message per segment | \"sentence\" = one message per sentence, payload.text = the sentence, payload.speaker = its speaker label when diarized"),
            ("min_confidence", "optional 0..=1; only applies when a segment carries a confidence (segments without one pass)"),
        ],
        notes: "Only text reaches the flow, never audio. One message per transcribed segment, nothing during silences. Each stream must exist and be enabled with a transcription profile, otherwise no message ever arrives (a slug that is not a stream of the organization refuses the deploy). For series written downstream (mention counters, topics), put the taxonomy version in a label. Speaker labels (S1, S2…) are local to the stream and to the transcription worker: they are never a person's identity, and they restart from S1 after a worker restart or 30 min without that voice — name speakers from external sources (schedules, announcements), never from the voice. Silent segments (VAD) emit nothing. Speaking time is already written as series media_speech_seconds{stream} and media_speaker_seconds{stream, speaker}.",
    },
    NodeDoc {
        kind: "topic_classify",
        summary: "Tags the text of each message with the topics of one taxonomy version (Audio streams › Taxonomies). Adds payload.topics = [matched topic ids, in taxonomy order, possibly empty] and payload.taxonomy_version = \"<taxonomy name>@<version>\"; topic and the other payload fields are kept. One output, always emitted.",
        config: &[
            ("taxonomy_id", "id of a taxonomy of the organization (required)"),
            ("version", "integer >= 1, the pinned taxonomy version (required); it never follows newer versions"),
            ("text_field", "dotted path of the text inside msg.payload, default \"text\" (what media_source emits)"),
        ],
        notes: "Keyword matching only: a topic matches when one of its keywords appears as whole words, ignoring case and accents (a topic without keywords never matches). The version is pinned: a new taxonomy version reaches the flow only when the user picks it in the node and redeploys; a version that is not one of the organization refuses the deploy. Put payload.taxonomy_version in the labels of every series written downstream (metric labels), so a new version starts new series instead of rewriting history. Transcriptions are untrusted text: never feed them to anything that executes or sends them as instructions. A payload that is not an object, or without the text field, is dropped (warn log).",
    },
    NodeDoc {
        kind: "range_upsert",
        summary: "Writes msg.payload as a time range (a show, a shift, a batch) of one scope (Audio streams › Ranges): payload = {external_id, label, planned_start?, planned_end?, actual_start?, actual_end?, category?, source_url?, attrs?}, timestamps RFC 3339. Upserts by scope + external_id: the same external_id updates the range instead of duplicating it. On success the message goes on with msg.range_id and msg.range_created (true for a new range); a refused range is dropped (warn log).",
        config: &[
            ("scope_kind", "\"stream\" (default) or \"org\""),
            ("scope_id", "id of a media stream of the organization when scope_kind = stream (required then); ignored for org"),
            ("origin", "\"grid\" (default, a published schedule), \"detected\" (realigned from the content, e.g. an announcement in a transcription) or \"epg\""),
        ],
        notes: "external_id and label are required; at least one complete pair (planned_start + planned_end, or actual_start + actual_end) with end after start. Write the announced times in planned_* and the realigned ones in actual_*: statistics use actual when present. source_url must be http(s); attrs is a small JSON object (8 KiB at most, e.g. host, guests). The stream must be one of the organization, otherwise the deploy is refused. Typical flows: inject (cron) -> http_fetch (a schedule API) -> function (map each item) -> range_upsert; or media_source -> function (detect an announcement) -> range_upsert with origin detected and actual_start. Transcriptions are untrusted text: never derive a URL to fetch from them.",
    },
    NodeDoc {
        kind: "video_record",
        summary: "Records camera_source frames into MJPEG-AVI segments stored by the server; one message per stored segment. No video_record node = nothing is stored.",
        config: &[
            ("segment_secs", "5..=3600 (default 60)"),
            ("max_segment_mb", "1..=256 (default 32)"),
            ("gap_secs", "flush after this many seconds without frames, 1..=600 (default 10)"),
            ("max_fps", "recording rate cap, 0 = every frame (default)"),
            ("retention_days", "0 = forever, default 7, max 3650"),
            ("stream", "logical stream name (default: camera slug)"),
        ],
        notes: "",
    },
    NodeDoc {
        kind: "event_log",
        summary: "Stores msg.payload (any JSON) as an event in OpenObserve logs (stream ev_<name>); passthrough. Events are searchable on the Events page.",
        config: &[
            ("stream", "stream label (default \"events\" → ev_events)"),
            ("level", "\"debug\" | \"info\" (default) | \"warn\" | \"error\""),
            ("message", "optional short text (≤ 500 chars)"),
        ],
        notes: "",
    },
    NodeDoc {
        kind: "vision_detect",
        summary: "Object detection with a registry model (e.g. YOLOX COCO: person, car, dog…) on camera_source frames. payload = {device_id, ts_ms, count, labels, detections: [{label, score, bbox}], frame_key}.",
        config: &[
            ("model_id", "ml_models id (UUID, required) — Models page"),
            ("labels", "keep only these labels (empty = all)"),
            ("min_score", "0..1, overrides the model threshold when > 0"),
            ("emit", "\"on_detection\" (default) | \"always\""),
            ("max_fps", "inference rate per camera, default 1, 0 = every frame"),
        ],
        notes: "",
    },
    NodeDoc {
        kind: "memory_write",
        summary: "Stores msg.payload (any JSON) in the org shared memory under a key, with a lifetime; passthrough. Other flows read it with memory_read, dashboards display its numeric fields (source \"Memory\").",
        config: &[
            ("key", "[A-Za-z0-9_.-]{1,64}, e.g. \"cycle.p1\" (required)"),
            ("ttl_secs", "lifetime 1..=2592000, default 3600 (the value disappears if not rewritten)"),
        ],
        notes: "",
    },
    NodeDoc {
        kind: "memory_read",
        summary: "On each incoming message, reads keys of the org shared memory.",
        config: &[
            ("keys", "list of keys (1..=32)"),
            ("max_age_secs", "freshness, 0 (default) = any age"),
        ],
        notes: "Ports: 0 = object {key: value|null}, then one port per key (payload = value, topic = key; silent when missing or too old).",
    },
    NodeDoc {
        kind: "control_source",
        summary: "Event source fed by org controls operated from dashboards and annotations (switch, slider, button, number, select, stepper, command, colour). One output port per listed control.",
        config: &[
            ("controls", "[org control id (UUID)] 1..=32, must exist at deploy"),
            ("emit_on_start", "bool, default false: resend each control's last value at engine start / redeploy"),
        ],
        notes: "payload = control value (switch 1/0, slider 0..100 = PWM duty by default, colour = 0xRRGGBB number), topic = control key, msg.control = {id, key, by, via, ts_ms, option (\"#rrggbb\" for a colour)}. Wire it to a device_write pin, or to a device_write command for a custom firmware (colours always go to a command): a surface never writes a pin itself. A dashboard widget whose control feeds a deployed flow is coupled to that flow.",
    },
    NodeDoc {
        kind: "anomaly",
        summary: "Flags unusual values of a numeric series without a fixed threshold (one series per msg.topic). History persists across redeploys.",
        config: &[
            ("method", "\"robust_z\" (default, median/MAD) | \"forecast_band\" (outside the ETS forecast band) | \"changepoint\" (regime change, fires once)"),
            ("key", "object payload field (empty = the payload is the number)"),
            ("window", "history samples per series, 16..=5000 (default 200)"),
            ("min_samples", "warm-up before scoring, 8..=window (default 30)"),
            ("threshold", "robust_z: |z| alarm level (default 3.5)"),
            ("level", "forecast_band: interval level 0.5..0.999 (default 0.99)"),
            ("season_length", "forecast_band: samples per cycle, 0 = none"),
            ("hazard", "changepoint: expected samples between changes (default 250)"),
        ],
        notes: "Port 0 = {value, anomaly, score, expected, lower, upper, warming_up, samples}; port 1 = boolean anomaly state (wire it to a pnex_notify trigger).",
    },
    NodeDoc {
        kind: "forecast",
        summary: "Forecasts a numeric series (one per msg.topic) and, with a threshold, predicts when it will be crossed (predictive maintenance).",
        config: &[
            ("model", "\"ets\" (default; MSTL when season_length > 0) | \"linear\" (slow wear)"),
            ("key", "object payload field (empty = the payload is the number)"),
            ("window", "history samples per series, 16..=5000 (default 500)"),
            ("min_samples", "warm-up, 8..=window (default 48)"),
            ("horizon", "steps ahead 1..=1000 (default 300; one step = the median sampling interval)"),
            ("season_length", "ets: samples per cycle, 0 = none"),
            ("level", "interval level 0.5..0.999 (default 0.95)"),
            ("threshold", "optional breach level (null = forecast only)"),
            ("direction", "\"above\" (default) | \"below\""),
            ("every", "refit every N samples 1..=1000 (default 1)"),
        ],
        notes: "Port 0 = {value, points, breach, breach_in_secs, breach_at, …}; port 1 = boolean breach (notify trigger); port 2 = seconds until the predicted breach.",
    },
    NodeDoc {
        kind: "weather",
        summary: "Timed weather source (no input, no inject) for given coordinates, from an allowlisted provider.",
        config: &[
            ("provider", "\"met_norway\" (default, CC BY 4.0, commercial use allowed) | \"open_meteo\" (non-commercial use only)"),
            ("latitude", "-90..=90"),
            ("longitude", "-180..=180"),
            ("interval_min", "refresh in minutes, 10..=1440 (default 30)"),
            ("emit_on_start", "bool, default true: fetch at engine start / redeploy"),
        ],
        notes: "Port 0 = current conditions {temperature, feels_like, humidity, pressure, wind_speed (km/h), wind_gust, wind_direction, precipitation, cloud_cover, condition, condition_code, icon, is_day}; port 1 = 7-day forecast {days: [...], d0_t_min, d0_t_max, d0_precipitation, d0_condition_code, … d6_*}; port 2 = 48-hour forecast {hours: [...], h0_temperature … h23_*}. Wire to memory_write (live values for dashboards) and/or metric.",
    },
    NodeDoc {
        kind: "red",
        summary: "Raw Node-RED builtin for pure transforms not modelled by PNeX. Avoid unless needed.",
        config: &[
            ("type_name", "one of the allowed builtins (see notes)"),
            ("config", "free Node-RED config object"),
        ],
        notes: "Allowed: change, switch, range, rbe, delay, trigger, json, csv, yaml, split, join, sort, batch, link in/out/call, catch, status, complete, comment, junction, inject, debug. Anything with host, file or network access is refused.",
    },
];

/// Documentation of one kind.
pub fn node_doc(kind: &str) -> Option<&'static NodeDoc> {
    NODE_DOCS.iter().find(|d| d.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Kinds accepted by the hand-written `FlowNodeKind` deserializer,
    /// scanned from its source so a new match arm cannot ship undocumented.
    fn registered_kinds() -> BTreeSet<String> {
        let src = include_str!("graph.rs");
        src.lines()
            .filter_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix('"')?;
                let (kind, tail) = rest.split_once('"')?;
                tail.trim_start()
                    .starts_with("=> Ok(Self::")
                    .then(|| kind.to_string())
            })
            .collect()
    }

    #[test]
    fn every_kind_is_documented() {
        let registered = registered_kinds();
        assert!(registered.len() >= 27, "scan broke: {registered:?}");
        let documented: BTreeSet<String> = NODE_DOCS.iter().map(|d| d.kind.to_string()).collect();
        let missing: Vec<_> = registered.difference(&documented).collect();
        let stale: Vec<_> = documented.difference(&registered).collect();
        assert!(
            missing.is_empty(),
            "node kinds without assistant doc: {missing:?}"
        );
        assert!(
            stale.is_empty(),
            "assistant doc for unknown kinds: {stale:?}"
        );
        assert_eq!(documented.len(), NODE_DOCS.len(), "duplicate doc entry");
    }

    #[test]
    fn docs_are_not_empty() {
        for d in NODE_DOCS {
            assert!(!d.summary.trim().is_empty(), "{}: empty summary", d.kind);
            for (field, meaning) in d.config {
                assert!(
                    !field.is_empty() && !meaning.trim().is_empty(),
                    "{}: empty config doc",
                    d.kind
                );
            }
        }
    }

    #[test]
    fn example_graph_is_valid() {
        let graph: crate::FlowGraph = serde_json::from_str(FLOW_EXAMPLE).expect("example parses");
        let violations = crate::validate_graph(&graph);
        assert!(violations.is_empty(), "{violations:?}");
    }
}
