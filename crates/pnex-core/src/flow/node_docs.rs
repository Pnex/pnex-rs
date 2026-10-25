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
        "inject (trigger) → device_read → calc → metric. A flow runs continuously: a periodic inject (repeat_secs) is the default idiom. Event sources (camera_source, control_source, action_trigger, media_source) need no inject.",
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
        "Only a deployed flow acts on devices (device_write, regulation cards). Dashboards and annotations write controls; a control_source node turns them into messages. Ontology actions invoked from an object page reach the flow through an action_trigger node.",
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
        notes: "One of repeat_secs, cron or once_delay_secs is required (no_trigger otherwise). To drive a device_write output, set payload to 1/0 or true/false, or declare the value on the device_write pin (values): the default timestamp is refused by digital outputs.",
    },
    NodeDoc {
        kind: "device_read",
        summary: "Reads the latest values of the pins of ONE device (live cache, then OpenObserve).",
        config: &[
            ("device_id", "device slug — one device per node"),
            ("pins", "[pin label, e.g. \"A0\", or a metric a custom firmware publishes, e.g. \"temperature\"] — one output port per entry, in this order"),
            ("window_secs", "freshness window 1..=3600 (default 60); no data = payload {} (never an invented zero)"),
        ],
        notes: "Ports: one per pin (payload = the raw pin value, sent only when fresh), then a last \"name\" port whose payload is the device slug (for templates and labels). No telemetry at all usually means no pin is subscribed on the device (Pins page).",
    },
    NodeDoc {
        kind: "device_write",
        summary: "Writes the output pins of ONE device (digital 1/0, PWM duty 0..100) and sends the commands its custom firmware announces. Passthrough output.",
        config: &[
            ("device_id", "device slug — one device per node"),
            ("pins", "[output pin label (digital_out / pwm_out)] — one input row per pin"),
            ("commands", "[command announced by the device's custom firmware (pnex.onCommand)] — one input row per command, after the pins; pins may then be empty"),
            ("values", "optional {pin: value} declared per selected pin — true/false on a digital output, duty 0..100 on PWM; any message reaching that pin writes this value"),
        ],
        notes: "With values, the message is only a trigger: an inject with no payload on the pin's row writes the declared value (a message on no particular row writes every declared value) — the simplest way to switch an output on or off. Incoming payload = map {pin_or_command: value}: only configured names present in the map are written. A scalar payload is routed by the input row it arrives on, or accepted when a single pin/command is configured. A command receives args = {\"value\": payload} (an RGB colour control sends 0xRRGGBB as a number); a name the firmware does not announce is refused by the server (nothing sent). Every rejected write (empty payload, value the pin refuses such as a timestamp on a digital output, unknown name) shows an amber \"Nothing written\" badge on the node with the reason. An output pin has one writer: a pin already written by another deployed flow is refused at deploy (pin_already_assigned); commands have no such rule.",
    },
    NodeDoc {
        kind: "calc",
        summary: "Arithmetic expression on the payload keys (operators, ternary, ^, min/max/abs/round…). Check it with validate_calc_expression.",
        config: &[("expression", "string; variables = payload keys, exact case (pin \"A0\" gives …_A0, never …_a0)")],
        notes: "Input = a JSON object: its number and boolean (1/0) keys are the variables, other keys (arrays, text, nested objects) are ignored — so vision_detect → calc \"count > 0\" works directly. A scalar payload, or an expression naming a missing key, rejects the message: the node shows an amber \"Message rejected\" badge with the reason. Output payload = the number.",
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
            ("attach_field", "optional path of the media id of a PDF made by page_pdf (e.g. \"asset_id\", or a template variable holding the page_pdf output); only SMTP channels attach it"),
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
        summary: "Event source (no inject needed): one message per frame of a camera device or of the video track of a media stream (IP camera). payload = {device_id, seq, ts_ms, width, height, size, frame_key} for a device, {stream, seq, ts_ms, width, height, size, frame_key} for a stream (topic = the device or stream slug) — a reference to the JPEG, never the bytes.",
        config: &[
            ("source", "\"device\" (default) | \"stream\""),
            ("device_id", "camera device slug (required when source = device)"),
            ("stream", "media stream slug (required when source = stream): a stream of the organization whose tracks include video"),
            ("max_fps", "sampling, 0 = every frame (default), max 25"),
        ],
        notes: "Live view needs no flow. A stream source delivers what the stream samples (its fps setting, 1 to 5 frames per second, frames at most 1280 px wide), and only while the stream is enabled; a slug that is not a stream of the organization with a video track refuses the deploy. Waking a device camera does not apply to streams: an enabled stream is captured continuously.",
    },
    NodeDoc {
        kind: "media_source",
        summary: "Event source (no inject needed): one message per transcribed segment of the listed media streams (Media › Audio streams). payload = {stream, segment_id, started_at, ended_at, text, lang, asr_model, words_ref, speakers, speech_ms?}, topic = stream slug. words_ref names the OpenObserve logs stream (tx_<slug>) holding the word timings. speakers = [{label, start_ms, end_ms}] when the stream's profile has diarization, else []; speech_ms = speech found by the VAD when the profile has one.",
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
            ("stream", "recording label shown as \"Label\" in the editor (default: camera or stream slug)"),
        ],
        notes: "Follows the source of the incoming camera_source messages: device frames are recorded under the camera, media stream frames under the stream (listed in the Recordings tab of the stream page). One recorder per camera or stream across the organization's deployed flows.",
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
        summary: "Object detection with a registry model (e.g. YOLOX COCO: person, car, dog…) on camera_source frames. payload = {device_id or stream, ts_ms, count, labels, detections: [{label, score, bbox}], frame_key}.",
        config: &[
            ("model_id", "ml_models id (UUID, required) — Models page"),
            ("labels", "keep only these labels (empty = all)"),
            ("min_score", "0..1, overrides the model threshold when > 0"),
            ("emit", "\"on_detection\" (default) | \"always\""),
            ("max_fps", "inference rate per camera, default 1, 0 = every frame"),
            ("review_project", "Vision Lab project id receiving the doubtful frames (empty = off)"),
            ("review_min / review_max", "doubt band, default 0.25..0.6: a frame with a candidate score inside it is sent"),
            ("review_every_secs", "at most one doubtful frame per camera this often, 5..3600, default 60"),
        ],
        notes: "Works on device and stream frames alike; the annotation layer (record_layer) is stored for device cameras only.",
    },
    NodeDoc {
        kind: "vision_track",
        summary: "Follows the objects found by vision_detect from frame to frame and gives each one a stable track_id (per camera or stream). Port 0 = current tracks {device_id or stream, ts_ms, width, height, count, labels, tracks: [{track_id, label, score, bbox, first_ts_ms, last_ts_ms}]}; port 1 = events {event: \"appear\" | \"disappear\", track_id, label, t0_ms, t1_ms, duration_s, conf_avg}.",
        config: &[
            ("high_score", "0..1, default 0.5: detections at or above it open tracks, lower ones only extend existing tracks"),
            ("min_iou", "]0..1], default 0.2: box overlap needed to match a detection to a track"),
            ("min_hits", "1..100, default 2: matched frames before \"appear\""),
            ("lost_after_secs", "0.1..3600, default 3: unseen this long → \"disappear\""),
        ],
        notes: "Wire it after vision_detect; set the detect min_score below high_score so weak detections can keep a track alive. Raise lost_after_secs at low fps (1 fps → 3 s or more). Port 0 also fires when tracks expire without a new frame, so vision_zone sees the exits. Wire port 1 to event_log to search appearances on the Events page.",
    },
    NodeDoc {
        kind: "text_decide",
        summary: "Asks typed questions about the text of each message (default payload.text: what media_fuse and media_source emit) and adds probabilities, without generating text: the Laya decision model runs on the PneX server. Adds payload.decisions = {question id: {best, probability, probabilities: [[option, p]…], confidence, expected (score)}}, payload.decide_model; the rest of the payload is kept. One output, emitted when the model answered (no text or a failed call = dropped, reason in the node status).",
        config: &[
            ("questions", "1..8 × {id: [a-z0-9_]{1,32} unique, kind: \"choice\" (one option among 2..20) | \"score\" (levels of an ordered rubric, lowest first), instructions: the question (≤ 300 chars), options: labels (≤ 120 chars, unique), descriptions: optional text after each choice label}"),
            ("text_field", "dotted path of the text in msg.payload, default \"text\"; a non-text value is sent as JSON"),
        ],
        notes: "Route on the result with a Function (pnex_function) that reads payload.decisions.<id>.best and payload.decisions.<id>.probability and emits on a named output only when the condition holds (e.g. incident with probability > 0.8 → notify, or a boolean wired to the notify trigger input), or wire it before media_fuse so the organization's LLM only describes the windows that matter. Out of the box the model is close to chance on business questions and over-confident: treat probabilities as a ranking until the organization has labelled real examples and calibrated the model. Needs the decision model installed by the platform administrator (node error \"decide-unavailable\" otherwise). Never wire it to anything that acts on a device by itself (D128).",
    },
    NodeDoc {
        kind: "vision_caption",
        summary: "Describes a camera frame in words with the organization's own model (its default LLM provider, which must accept images: e.g. Claude, GPT-4o, or a local VLM behind an OpenAI-compatible endpoint). Input: any message carrying payload.frame_key (camera_source, vision_detect, vision_track port 0). Output: payload = {caption, caption_id, model, ts_ms, frame_key, device_id or stream, label (when cropped), ai_generated: true}.",
        config: &[
            ("prompt", "question asked about the image (≤ 500 chars; empty = a factual description of people, objects, actions and readable text)"),
            ("min_interval_secs", "1..86400, default 10: at most one caption per camera in this interval (each caption is a paid model call)"),
            ("crop_best", "bool: describe the best detection or track of the message (cropped) instead of the whole frame"),
        ],
        notes: "The image leaves PneX for the organization's provider: wiring this node is that choice. Needs an LLM provider in the organization settings (none = node error status). Model calls of caption and media_fuse nodes share a daily cap per organization (2000 by default). The caption is indicative, never a fact: feed it to media_fuse, an event_log, or a notify for a human, never to anything that acts. Frames older than 15 s are gone from the camera bus: keep it right after the source or detection.",
    },
    NodeDoc {
        kind: "page_pdf",
        summary: "Renders a page version as an A4 PDF (header, page N / M, AI notice) stored in the media library. Port 0 = PDF ready: payload = {asset_id, filename, page_id, version, title}. Port 1 = an AI version waits for a person's approval in Pages: payload = {page_id, version, title}.",
        config: &[
            ("source", "\"message\" (default: payload.page_id + payload.version, what page_write emits) | \"latest_sendable\" (the latest approved or non-AI version of page_id not yet exported by this node)"),
            ("page_id", "the page (source latest_sendable)"),
            ("require_approval", "default true: an AI version goes to port 1 until a person approves it; false = PDF with a \"not reviewed\" notice"),
            ("language", "language of the PDF words: \"fr\" or English (default)"),
        ],
        notes: "Typical report: inject (cron) -> data -> llm_text -> page_write -> page_pdf; port 1 -> pnex_notify (\"report ready to review\"). To send once approved, a second flow: inject (every 15 min) -> page_pdf (latest_sendable) -> pnex_notify with the PDF attached; a version is never sent twice by the same node.",
    },
    NodeDoc {
        kind: "page_write",
        summary: "Writes the Markdown of the message into a page (Pages) of the organization and freezes a named version: a report page per run, or a log page appended to. payload = {page_id, title, version, ai_generated}.",
        config: &[
            ("mode", "\"new_page\" (default: a page per message) | \"append\" (at the end of page_id)"),
            ("title", "title of each new page; {date}, {time}, {week} are replaced (UTC), e.g. \"Weekly report {week}\""),
            ("page_id", "the page appended to (mode append)"),
            ("text_field", "dotted path of the Markdown inside msg.payload, default \"text\" (what llm_text emits)"),
            ("version_label", "label of the named version created after each write, default \"{date}\""),
        ],
        notes: "Markdown kept: # headings, paragraphs, - and 1. lists, > quotes, ---, code fences, **bold**, *italic*, `code`, [links](https://…); a table becomes one paragraph per row. A version holding AI text (payload.ai_generated from llm_text or media_fuse) is marked as such and waits for a person to approve it in Pages before an AI report leaves the organization. Open editors see an append live.",
    },
    NodeDoc {
        kind: "llm_text",
        summary: "Writes a text with the organization's LLM from an instruction and the incoming data: a weekly report, a summary of events, a classification. payload = {text (Markdown), model, ai_generated: true, data_truncated}.",
        config: &[
            ("prompt", "the instruction, 1..4000 chars (e.g. \"Write a weekly report of these events: what happened, when, anything unusual\")"),
            ("data_field", "dotted path of the data inside msg.payload (empty = the whole payload)"),
            ("language", "answer language code (fr, en…), empty = the language of the data"),
            ("max_chars", "longest answer, 100..20000, default 4000"),
        ],
        notes: "The data is passed to the model as data only: instructions found inside it are ignored. Calls count in the organization's daily AI quota (2000 per day with vision_caption and media_fuse). Typical chain: inject (cron) -> data (memory_read, http_fetch, media_fuse output…) -> llm_text -> page_write -> page_pdf -> pnex_notify. Requires an AI provider configured for the organization. An empty answer (a reasoning model that spent its budget thinking) is an error (ai-bad-response) on the failure port, never an empty text.",
    },
    NodeDoc {
        kind: "media_fuse",
        summary: "Every window (default 30 s), turns what the flow received (transcripts from media_source, detection, tracking or zone events, captions from vision_caption) into a short time-stamped description written by the organization's LLM. Output once per non-empty window: payload = {t0_ms, t1_ms, sentences: [{text, sources: [item ids]}], items, rejected, model, ai_generated: true}.",
        config: &[
            ("window_secs", "5..3600, default 30"),
            ("language", "language of the description (\"fr\", \"en\"…; empty = the language of the transcripts)"),
        ],
        notes: "Trust order given to the model: detections and zone events are facts, transcripts are reliable for speech, captions are indicative; it may not invent objects absent from its inputs. Every sentence must cite the ids of its inputs: a sentence without a valid source is dropped (counted in rejected). Inputs are untrusted text passed as data, never as instructions; the model has no tool. Wire several sources into the same node (one per scene). Output is AI-generated: store it with event_log, show it on a dashboard, never act on it.",
    },
    NodeDoc {
        kind: "vision_zone",
        summary: "Zones drawn over the camera image (polygons, normalized 0..1 coordinates). Fed by vision_track port 0, emits one message per zone event: {event: \"enter\" | \"exit\", zone, track_id, label, ts_ms, t0_ms, duration_s (exit), count (objects now in the zone), device_id or stream}.",
        config: &[
            ("zones", "[{name (unique, 1..64 chars), points: [[x, y]…] 3..32 vertices in 0..1}] 1..16 — drawn in the node inspector"),
            ("labels", "keep only these labels (empty = all)"),
            ("anchor", "\"bottom\" (default, bottom centre of the box: feet, wheels) | \"center\""),
            ("min_dwell_secs", "0..3600: time in the zone before \"enter\" fires (\"present for more than N s\")"),
        ],
        notes: "The zones apply to every camera reaching the node: use one vision_zone per camera viewpoint. An object counted in a zone leaves it by \"exit\" (including when its track disappears). Wire it to event_log for searchable events, and, for alerts, to notify behind a Function (pnex_function) that filters on payload.event (one named output per event kind, or a boolean wired to the notify trigger input). It never drives a device by itself (D128).",
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
        kind: "action_trigger",
        summary: "Event source fed by the invocations of one ontology action (e.g. \"start_maintenance\" on a pump), made by a user from the object page. One output port.",
        config: &[
            ("object_type", "object type key, e.g. \"pump\" — must exist with this action at deploy"),
            ("action", "action key declared on the type"),
        ],
        notes: "Message: payload = the action parameters (object), topic = action key, msg.object = {id, type, title}, msg.action = {invocation_id, key, by, ts_ms}. Invoking an action with no deployed flow holding its trigger is refused. Several flows may listen to the same action; each gets every invocation.",
    },
    NodeDoc {
        kind: "ontology_query",
        summary: "Reads the live objects of one ontology type matching property filters (e.g. procedures whose next due date has passed). Input: any message (often a daily inject). One output port.",
        config: &[
            ("object_type", "object type key, must exist at deploy"),
            ("filters", "[{key: property, op: eq|ne|lt|lte|gt|gte|contains|exists|missing|in, value}] — value may be \"$today\" (YYYY-MM-DD), \"$now\" (RFC 3339) or \"$msg.<path>\" (read from the incoming message); `in` takes a list or \"$setting.<pack>.<key>\" (the comma-separated codes of a pack setting, read by the server; an empty setting drops the filter)"),
            ("order_by", "property key to sort on (title by default); descending: bool"),
            ("limit", "1..=500, default 100"),
            ("output", "each (default: one message per object) | array (one message, payload = list)"),
            ("traverse", "[{link_type, direction: out|in}] up to 4 steps: the rows are the objects reached (certification code in [...] then habilite_pour in = its partners)"),
            ("with_links", "up to 8 link types joined to each row, both directions, open links only (200 per type)"),
        ],
        notes: "Each output message: payload = the object's properties, msg.object = {id, type, title, version}, msg.links = {link_type: [{direction: out|in, attributes, valid_from, other: {id, type, title, properties}}]} for with_links, msg.today (YYYY-MM-DD) and msg.now (RFC 3339) of the run; with output array, payload = [{id, title, version, properties, links}]. Other fields of the incoming message are kept: an array query then an each query joins two sets in a function. Dates compare as text (ISO dates sort right). Feed object_upsert with msg.object_id = msg.object.id to update what was read.",
    },
    NodeDoc {
        kind: "object_upsert",
        summary: "Creates or updates one ontology object from msg.payload (its properties), matched on the type's unique property (natural key) or on its id. One output port.",
        config: &[
            ("object_type", "object type key (not a system type such as device or flow), must exist at deploy"),
            ("key", "a unique property of the type (e.g. \"siren\"), or \"id\" to update msg.object_id"),
            ("skip_existing", "bool: an object already there is left as is (rules creating findings must not reset one a human reviewed)"),
        ],
        notes: "msg.payload = {property: value}; the properties given replace those values, the others stay. msg.title sets the title (default: the key value). msg.expected_version, when set, refuses a write on a newer version (409). Output: msg.object = the object written, msg.created = bool. A refused write (validation, role, 600 writes a minute per organization) is logged in the node status and the message dropped. Provenance: flow:<flow id>@<version>.",
    },
    NodeDoc {
        kind: "link_upsert",
        summary: "Opens a typed ontology link between two objects (or updates its attributes when it is already open), or closes it. One output port.",
        config: &[
            ("link_type", "link type key, must exist at deploy"),
            ("close", "bool: close the open link instead of opening it; the message's attributes are merged first (a reason, who decided)"),
            ("skip_closed", "bool: when this pair has a closed link and none open, answer that closed link (valid_to set) instead of opening a new one — a candidate a human rejected is never proposed again"),
            ("propose", "bool: a new link becomes a proposal on the To validate page instead of a live link (msg.payload.confidence 0..=100 and msg.payload.evidence, an object shown to the human, kept with it); an open link still gets its attributes. Output msg.link = {proposed: true, proposal_id} (null when the same values were already proposed or rejected; the same values sent again with evidence while still to validate refresh its evidence and confidence). With msg.payload.accept = a rule text (e.g. \"<pack>.<setting> >= 80 (score 92)\"), the proposal is accepted at once by that rule, shown as Automatic with the rule; a value rejected before stays rejected, and a refused write leaves it to validate"),
        ],
        notes: "msg.payload = {source_id, target_id, attributes?, valid_from?} (object ids). Closing keeps the link in the history. Output: msg.link = the link (valid_to set when skip_closed answered a closed one). Cardinality rules of the link type apply (a second open link where only one is allowed is refused).",
    },
    NodeDoc {
        kind: "site_recipe",
        summary: "Plays a saved website navigation rule (Web sites page) on the server browser at each message: one site, one rule, its steps journaled like a run started from the studio (trigger flow). Ports: 0 passed, 1 failed again, 2 broke (failed right after a passing run: wire it to a notify node to hear once when a site changes).",
        config: &[
            ("recipe_id", "uuid of a saved rule of the organisation (required to deploy); the latest saved version is played"),
            ("params", "values of the rule's parameters, e.g. {code: RS0101}; msg.params (an object of strings) overrides them"),
        ],
        notes: "Trigger it from an inject node with a cron (the studio lists those next runs). msg.payload = {extract name: text}; msg.manifest = {site, recipe, version, run, status, failed_step?, outputs: [{name, kind, sha256, chars, changed, from?, media?, media_version?}]} where changed compares with the previous run that brought the same extract back. On a passing run each extract is also a document of the media library (media = its id; one document per rule, parameters and extract, a new version only when the content changed; a PDF extract is the PDF file). A run takes up to 10 minutes and the node handles one message at a time. A failure concludes nothing: never close or delete records on port 1 or 2. A missing rule or parameter answers on port 1 with msg.error = {code, args}.",
    },
    NodeDoc {
        kind: "web_search",
        summary: "Searches the web through a search provider of the organisation (Organisation page, Web search) and returns candidates; writes nothing. Ports: 0 results (possibly an empty list), 1 failed.",
        config: &[
            ("provider", "name of an org web search provider; empty = the org default"),
            ("limit", "1..=20 results kept after exclusions, default 10"),
            ("country, lang", "two-letter codes (fr, fr) passed to the provider; empty = its choice"),
            ("exclude_domains", "results on these domains or their subdomains are dropped; defaults to directories and social networks (linkedin.com, societe.com, pappers.fr…)"),
        ],
        notes: "The query is msg.payload when it is a string, else msg.payload.query (1 to 400 characters). msg.payload becomes {query, provider, results: [{url, title, snippet, rank, domain}]} (rank starts at 1 after exclusions). Each message is one paid call to the provider and counts against the org's daily web search cap (web-search-quota): feed it a bounded list (ontology_query with a limit), never a loop. On port 1 msg.error = {code, args}: web-search-no-provider, web-search-unavailable, web-search-quota, web-search-query-invalid. A search result is a candidate, not a fact: check it (visit, identifiers) before writing anything.",
    },
    NodeDoc {
        kind: "site_crawl",
        summary: "Walks one website on the server (sitemap then internal links, robots.txt obeyed, 2 s between two requests, never off the site) and sends its relevant pages, then a summary. Input: msg.payload = {url, match: [{kind: word|phrase|regex, value}], link_to?}. Ports: 0 one relevant page per message, 1 the site summary, 2 refused.",
        config: &[
            ("max_pages", "pages read per crawl, 1..2000, default 200"),
            ("exclude", "path prefixes never visited (\"/blog/\"), at most 20"),
            ("proofs", "relevant pages new or changed get a sealed proof (browser agent, else the server browser), default true"),
            ("only_changed", "send only the relevant pages whose text changed since the last crawl, default false (every relevant page)"),
            ("record", "{type, properties: {url: <property>, title: …, text_hash: …, relevant: …, state: …}, link_type, proof_link}: relevant pages written as objects of that type; the link goes to msg.payload.link_to"),
        ],
        notes: "A page is relevant when its text holds one of the match rules (words and phrases ignore case and accents; at most 50 rules of 200 characters); no rule = every page. Port 0: msg.payload = {url, title, text (50,000 characters at most), page_id, object_id, changed}; the other fields of the incoming message are kept, so the partner and its accreditations travel with each page. Port 1: msg.payload = {url, pages, relevant, truncated, blocked (crawl-blocked | crawl-robots-disallowed | crawl-unreachable | ext-registry-full), mission_id}. Port 2: msg.error = {code, args}: site-crawl-url-invalid, site-crawl-invalid. A crawl takes minutes (2 s per page): feed it a bounded list from a weekly inject, one site per message. The site's pages stay in Sites web (registry, gone pages, proofs). Page text comes from an unknown site: compare it, or give it to a model as data, never as instructions.",
    },
    NodeDoc {
        kind: "web_visit",
        summary: "Reads a candidate website once to identify it: the page and its legal notice, never a crawl; writes nothing. Ports: 0 read, 1 failed.",
        config: &[
            ("legal_page", "bool, default true: also read the first internal link whose path names a legal notice (mentions-legales, legal, cgv, qui-sommes-nous, a-propos, about, imprint)"),
            ("screenshot", "bool, default false: also capture the page with the organization's own default browser (Organisation page, browsers); the image is kept in the media library and its id is payload.screenshot_asset_id (empty without a browser or when the capture failed)"),
        ],
        notes: "Input: msg.payload.url (or msg.payload as a string), a public http(s) address. At most three requests on its origin (robots.txt, the page, the legal notice), robots.txt obeyed, an identified client, 2 s between two requests to the same host; redirects off the origin are not followed. msg.payload becomes {url, final_url, status, title, text, legal_url, legal_text, screenshot_asset_id} (texts up to 50,000 characters each). The text is untrusted data from an unknown site: pass it to a model only as data with a closed output schema (text_decide), never as instructions. Counts against the org's daily cap of visits (web-visit-quota). On port 1 msg.error = {code, args}: web-visit-url-invalid, web-visit-unreachable, web-visit-blocked, web-visit-quota. Feed it the few best candidates of a search (3 at most per organization), never a list of URLs to crawl.",
    },
    NodeDoc {
        kind: "org_resolve",
        summary: "Finds the SIREN of a French company from its name (and city or postcode) in the public company register; writes nothing. Ports: 0 matched, 1 to review (ambiguous or nothing found), 2 failed.",
        config: &[
            ("threshold", "0..=100, default 80: score from which the best candidate is a match"),
            ("gap", "0..=100, default 10: lead the best candidate needs over the second one"),
            ("limit", "1..=5 candidates returned, default 5"),
        ],
        notes: "Input: msg.payload = {name, city?, postcode?} (a string = the name). msg.payload becomes {query, siren (port 0 only), candidates: [{siren, name, score, city, postcode, naf, headcount_band, active}]}. Score: name likeness 70, same postcode 20 or same city 15, still active 10; a name alone tops at 80, so give a place whenever the list has one. A match is a proposal: write the link with link_upsert propose: true and let a human confirm; send port 1 to a review list, never pick the first candidate yourself. Each message is one call to the register and counts against the org's daily cap (org-resolve-quota). On port 2 msg.error = {code, args}: org-resolve-unavailable, org-resolve-quota, org-resolve-query-invalid.",
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
