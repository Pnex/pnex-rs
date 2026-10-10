//! `pnex-media-source` — event source of transcribed media segments
//! (media-ingest.md D163). Subscribes to the segment channel of each
//! listed stream (`pnex:media:v1:{org}:{slug}:tx`, published by the
//! backend transcription worker) and emits one message per segment, or per
//! sentence with `emit = sentence`. Only text reaches the flow, never audio.
//!
//! Isolation (R3): channels are built from the stamped `pnex_org_id` and
//! the validated slugs with `transcript_channel`, exact SUBSCRIBE only.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::media_ingest::{
    transcript_channel, transcript_stream, MediaSourceConfig, MediaSourceEmit,
};

const NODE: &str = "pnex-media-source";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(default)]
    streams: Vec<String>,
    #[serde(default)]
    emit: MediaSourceEmit,
    #[serde(default)]
    min_confidence: Option<f64>,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-media-source", red_name = "pnex-media-source")]
struct MediaSourceNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    /// Channel → stream slug.
    channels: HashMap<String, String>,
    client: redis::Client,
}

/// Splits on `.`, `!`, `?` or `…` followed by whitespace; empty sentences
/// are dropped.
pub(crate) fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        let end =
            matches!(c, '.' | '!' | '?' | '…') && chars.peek().is_some_and(|n| n.is_whitespace());
        if end {
            let s = cur.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
            cur.clear();
        }
    }
    let s = cur.trim();
    if !s.is_empty() {
        out.push(s.to_string());
    }
    out
}

/// Payloads emitted for one raw segment event of stream `slug` (pure: no
/// Valkey). Unreadable events and segments under `min_confidence` yield
/// nothing; a segment without confidence passes.
pub(crate) fn payloads_of(
    slug: &str,
    raw: &str,
    emit: MediaSourceEmit,
    min_confidence: Option<f64>,
) -> Vec<serde_json::Value> {
    let Ok(serde_json::Value::Object(mut seg)) = serde_json::from_str::<serde_json::Value>(raw)
    else {
        return Vec::new();
    };
    if let (Some(min), Some(c)) = (
        min_confidence,
        seg.get("confidence").and_then(serde_json::Value::as_f64),
    ) {
        if c < min {
            return Vec::new();
        }
    }
    // The channel names the stream: never trust a slug carried in the body.
    seg.insert("stream".into(), slug.into());
    seg.insert("words_ref".into(), transcript_stream(slug).into());
    seg.entry("speakers")
        .or_insert_with(|| serde_json::json!([]));
    // Word timings stay in O2 (`words_ref`); the event carries them only to
    // give each sentence its speaker.
    let words = seg.remove("words");
    match emit {
        MediaSourceEmit::Segment => vec![serde_json::Value::Object(seg)],
        MediaSourceEmit::Sentence => {
            let text = seg
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let sentences = split_sentences(&text);
            let speakers = sentence_speakers(&sentences, words.as_ref());
            sentences
                .into_iter()
                .zip(speakers)
                .map(|(s, speaker)| {
                    let mut one = seg.clone();
                    one.insert("text".into(), s.into());
                    if let Some(l) = speaker {
                        one.insert("speaker".into(), l.into());
                    }
                    serde_json::Value::Object(one)
                })
                .collect()
        }
    }
}

/// Stream-local speaker label of each sentence: the label holding most of
/// its words' duration. Sentences take the words in order, one per
/// whitespace-separated token (the transcript is the words joined by
/// spaces); `None` without labelled words.
fn sentence_speakers(
    sentences: &[String],
    words: Option<&serde_json::Value>,
) -> Vec<Option<String>> {
    let words = words.and_then(serde_json::Value::as_array);
    let mut at = 0usize;
    sentences
        .iter()
        .map(|s| {
            let n = s.split_whitespace().count();
            let slice = words
                .map(|w| &w[at.min(w.len())..(at + n).min(w.len())])
                .unwrap_or_default();
            at += n;
            let mut by: BTreeMap<&str, u64> = BTreeMap::new();
            for w in slice {
                if let Some(spk) = w.get("spk").and_then(serde_json::Value::as_str) {
                    let d = w["e"]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_sub(w["s"].as_u64().unwrap_or(0));
                    *by.entry(spk).or_default() += d.max(1);
                }
            }
            by.into_iter()
                .max_by_key(|(_, d)| *d)
                .map(|(l, _)| l.to_string())
        })
        .collect()
}

impl MediaSourceNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let check = MediaSourceConfig {
            streams: cfg.streams.clone(),
            emit: cfg.emit,
            min_confidence: cfg.min_confidence,
        };
        if let Some((code, message)) = check.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let channels = cfg
            .streams
            .iter()
            .map(|slug| (transcript_channel(cfg.pnex_org_id, slug), slug.clone()))
            .collect();
        let client = crate::valkey_client(NODE)?;
        Ok(Box::new(MediaSourceNode {
            base: base_node,
            config: cfg,
            channels,
            client,
        }))
    }

    /// One subscription session: returns when the connection drops (the
    /// caller reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let mut pubsub = self
            .client
            .get_async_pubsub()
            .await
            .map_err(|e| format!("valkey connect failed: {e}"))?;
        for channel in self.channels.keys() {
            pubsub
                .subscribe(channel)
                .await
                .map_err(|e| format!("valkey subscribe failed: {e}"))?;
        }
        log::info!(
            "{NODE} [{}] : subscribed to {} stream(s)",
            self.name(),
            self.channels.len()
        );
        let mut stream = pubsub.on_message();
        loop {
            let msg = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                m = stream.next() => match m {
                    Some(m) => m,
                    None => return Err("valkey subscription closed".into()),
                },
            };
            let Some(slug) = self.channels.get(msg.get_channel_name()) else {
                continue;
            };
            let Ok(raw) = msg.get_payload::<String>() else {
                continue;
            };
            for payload in payloads_of(slug, &raw, self.config.emit, self.config.min_confidence) {
                let mut body = BTreeMap::new();
                body.insert("payload".to_string(), crate::variant_of(payload));
                body.insert("topic".to_string(), Variant::from(slug.clone()));
                let envelope = Envelope {
                    port: 0,
                    msg: MsgHandle::with_properties(body),
                };
                if let Err(e) = self.fan_out_one(envelope, stop.child_token()).await {
                    log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name());
                }
            }
        }
    }
}

#[async_trait]
impl FlowNodeBehavior for MediaSourceNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        let mut backoff = 1u64;
        while !stop_token.is_cancelled() {
            match self.session(&stop_token).await {
                Ok(()) => break,
                Err(e) => {
                    log::warn!("{NODE} [{}] : {e} — retry in {backoff} s", self.name());
                    tokio::select! {
                        _ = stop_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX_SECS);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = r#"{"stream":"other","segment_id":"s1","started_at":"2026-10-10T08:00:00Z",
        "ended_at":"2026-10-10T08:00:30Z","text":"Bonjour à tous. Quelle heure est-il ? Il est 8 h… Voilà","lang":"fr","asr_model":"whisper"}"#;

    #[test]
    fn sentences_split_on_terminators_followed_by_space() {
        assert_eq!(
            split_sentences("Un. Deux ! Trois? Quatre… Cinq 3.5 fin..."),
            vec!["Un.", "Deux !", "Trois?", "Quatre…", "Cinq 3.5 fin..."]
        );
        assert!(split_sentences("  ").is_empty());
        assert_eq!(split_sentences("Wait... what"), vec!["Wait...", "what"]);
    }

    #[test]
    fn segment_message_shape() {
        let out = payloads_of("inter", RAW, MediaSourceEmit::Segment, None);
        assert_eq!(out.len(), 1);
        let p = &out[0];
        assert_eq!(p["stream"], "inter");
        assert_eq!(p["words_ref"], "tx_inter");
        assert_eq!(p["speakers"], serde_json::json!([]));
        assert_eq!(p["segment_id"], "s1");
        assert_eq!(p["lang"], "fr");
        assert!(p.get("confidence").is_none());
        assert!(payloads_of("inter", "not json", MediaSourceEmit::Segment, None).is_empty());
    }

    #[test]
    fn sentences_get_the_speaker_of_their_words() {
        let raw = r#"{"text":"Bonjour à tous. Merci !","speakers":[{"label":"S1","start_ms":0,"end_ms":900},{"label":"S2","start_ms":900,"end_ms":1500}],
            "words":[{"w":"Bonjour","s":0,"e":300,"spk":"S1"},{"w":"à","s":300,"e":400,"spk":"S1"},{"w":"tous.","s":400,"e":900,"spk":"S1"},
                     {"w":"Merci","s":900,"e":1300,"spk":"S2"},{"w":"!","s":1300,"e":1400,"spk":"S2"}]}"#;
        let out = payloads_of("inter", raw, MediaSourceEmit::Sentence, None);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["speaker"], "S1");
        assert_eq!(out[1]["speaker"], "S2");
        assert!(out[0].get("words").is_none(), "words stay in O2");
        assert_eq!(out[0]["speakers"][1]["label"], "S2");
        let seg = payloads_of("inter", raw, MediaSourceEmit::Segment, None);
        assert!(seg[0].get("words").is_none() && seg[0].get("speaker").is_none());
        // Without labelled words: no speaker field.
        let plain = payloads_of("inter", RAW, MediaSourceEmit::Sentence, None);
        assert!(plain.iter().all(|p| p.get("speaker").is_none()));
    }

    #[test]
    fn sentence_mode_and_confidence_gate() {
        let out = payloads_of("inter", RAW, MediaSourceEmit::Sentence, Some(0.9));
        let texts: Vec<_> = out.iter().map(|p| p["text"].as_str().unwrap()).collect();
        assert_eq!(
            texts,
            vec![
                "Bonjour à tous.",
                "Quelle heure est-il ?",
                "Il est 8 h…",
                "Voilà"
            ]
        );
        assert!(out.iter().all(|p| p["segment_id"] == "s1"));
        let low = r#"{"text":"x","confidence":0.2}"#;
        assert!(payloads_of("inter", low, MediaSourceEmit::Segment, Some(0.5)).is_empty());
        assert_eq!(
            payloads_of("inter", low, MediaSourceEmit::Segment, Some(0.1)).len(),
            1
        );
    }
}
