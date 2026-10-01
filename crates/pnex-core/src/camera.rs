//! Camera & video contract (camera-video.md D73–D80) — pure, wasm-safe.
//!
//! - [`FrameHeader`]: the 16-byte clear header that prefixes every JPEG on
//!   the `/ws/camera` uplink (inside the ChaCha20 ciphertext);
//! - [`FrameSize`]: OV2640 resolutions, wire ids shared by the API, the
//!   `CameraConfig` server message and the firmware;
//! - [`CameraSettings`] / [`CaptureMode`]: per-device capture settings DTO;
//! - [`bus_channel`] / [`frame_key`] / [`BusFrameMeta`]: the Valkey frame bus
//!   between the backend CameraHub (publisher) and the flow runtime
//!   `camera-source` node (subscriber) — messages only carry a reference,
//!   JPEG bytes never cross the flow engine JSON;
//! - [`VideoSegment`]: recorded segment DTO (API + UI).

use serde::{Deserialize, Serialize};

/// Magic of the frame header format, version 1.
pub const FRAME_MAGIC: [u8; 4] = *b"PXC1";
/// Clear header length (magic + seq + uptime + width + height).
pub const FRAME_HEADER_LEN: usize = 16;
/// Default server-side cap on one frame (header + JPEG), override with
/// `PNEX_CAMERA_MAX_FRAME_BYTES`.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 512 * 1024;
/// Valkey TTL of a published frame — long enough for a slow flow branch,
/// short enough to never accumulate (5 fps × 15 s ≈ 75 frames/device).
pub const FRAME_TTL_SECS: u64 = 15;

/// Clear header of one uplink frame (little-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameHeader {
    pub seq: u32,
    pub uptime_ms: u32,
    pub width: u16,
    pub height: u16,
}

/// Why an uplink frame was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    TooShort,
    BadMagic,
    NotJpeg,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooShort => "frame too short",
            Self::BadMagic => "bad frame magic",
            Self::NotJpeg => "payload is not a JPEG",
        })
    }
}

impl FrameHeader {
    pub fn encode(&self) -> [u8; FRAME_HEADER_LEN] {
        let mut out = [0u8; FRAME_HEADER_LEN];
        out[0..4].copy_from_slice(&FRAME_MAGIC);
        out[4..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..12].copy_from_slice(&self.uptime_ms.to_le_bytes());
        out[12..14].copy_from_slice(&self.width.to_le_bytes());
        out[14..16].copy_from_slice(&self.height.to_le_bytes());
        out
    }

    /// Splits a decrypted frame into its header and JPEG body (SOI checked).
    pub fn parse(plain: &[u8]) -> Result<(Self, &[u8]), FrameError> {
        if plain.len() < FRAME_HEADER_LEN + 2 {
            return Err(FrameError::TooShort);
        }
        if plain[0..4] != FRAME_MAGIC {
            return Err(FrameError::BadMagic);
        }
        let u32_at =
            |i: usize| u32::from_le_bytes([plain[i], plain[i + 1], plain[i + 2], plain[i + 3]]);
        let u16_at = |i: usize| u16::from_le_bytes([plain[i], plain[i + 1]]);
        let header = Self {
            seq: u32_at(4),
            uptime_ms: u32_at(8),
            width: u16_at(12),
            height: u16_at(14),
        };
        let jpeg = &plain[FRAME_HEADER_LEN..];
        if jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
            return Err(FrameError::NotJpeg);
        }
        Ok((header, jpeg))
    }
}

/// OV2640 resolutions exposed to users (subset of `framesize_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSize {
    Qvga,
    Cif,
    Vga,
    Svga,
    Xga,
    Hd,
    Sxga,
    Uxga,
}

impl FrameSize {
    pub const ALL: [FrameSize; 8] = [
        Self::Qvga,
        Self::Cif,
        Self::Vga,
        Self::Svga,
        Self::Xga,
        Self::Hd,
        Self::Sxga,
        Self::Uxga,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Qvga => "qvga",
            Self::Cif => "cif",
            Self::Vga => "vga",
            Self::Svga => "svga",
            Self::Xga => "xga",
            Self::Hd => "hd",
            Self::Sxga => "sxga",
            Self::Uxga => "uxga",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.wire() == s)
    }

    pub fn dims(self) -> (u16, u16) {
        match self {
            Self::Qvga => (320, 240),
            Self::Cif => (400, 296),
            Self::Vga => (640, 480),
            Self::Svga => (800, 600),
            Self::Xga => (1024, 768),
            Self::Hd => (1280, 720),
            Self::Sxga => (1280, 1024),
            Self::Uxga => (1600, 1200),
        }
    }
}

/// Who asks the device to capture (D76).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// Streams only while a live viewer is connected (+ grace delay).
    #[default]
    OnDemand,
    /// Streams as long as the device is connected (recording / vision).
    Continuous,
}

impl CaptureMode {
    pub fn wire(self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::Continuous => "continuous",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "on_demand" => Some(Self::OnDemand),
            "continuous" => Some(Self::Continuous),
            _ => None,
        }
    }
}

pub const QUALITY_MIN: u8 = 10;
pub const QUALITY_MAX: u8 = 63;
pub const FPS_MIN: u8 = 1;
pub const FPS_MAX: u8 = 25;

/// Per-device capture settings (API DTO, `device_cameras` row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraSettings {
    pub framesize: FrameSize,
    pub quality: u8,
    pub fps: u8,
    pub capture_mode: CaptureMode,
    #[serde(default)]
    pub vflip: bool,
    #[serde(default)]
    pub hmirror: bool,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            framesize: FrameSize::Vga,
            quality: 12,
            fps: 5,
            capture_mode: CaptureMode::OnDemand,
            vflip: false,
            hmirror: false,
        }
    }
}

impl CameraSettings {
    /// Field-level validation — machine tokens (`range:10..63`), school of
    /// the other DTO checks.
    pub fn check(&self) -> Result<(), Vec<(&'static str, String)>> {
        let mut errs = Vec::new();
        if !(QUALITY_MIN..=QUALITY_MAX).contains(&self.quality) {
            errs.push(("quality", format!("range:{QUALITY_MIN}..{QUALITY_MAX}")));
        }
        if !(FPS_MIN..=FPS_MAX).contains(&self.fps) {
            errs.push(("fps", format!("range:{FPS_MIN}..{FPS_MAX}")));
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// Camera list item (`GET /api/v1/cameras`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraView {
    /// `device_registries.id`.
    pub device: i64,
    /// Human device id (`device_registries.device_id`).
    pub device_id: String,
    pub settings: CameraSettings,
    /// `/ws/camera` uplink currently open.
    pub streaming: bool,
    /// Live viewers currently attached.
    pub viewers: u32,
    /// Unix ms of the last frame received (process lifetime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_frame_ms: Option<i64>,
}

/// Valkey pub/sub channel of one device's frames — keyed by the device
/// slug (`device_registries.device_id`), like the last-value cache and the
/// flow node configs.
pub fn bus_channel(org_id: i64, device_id: &str) -> String {
    format!("pnex:cam:v1:{org_id}:{device_id}")
}

/// Valkey key holding one frame's JPEG bytes (TTL [`FRAME_TTL_SECS`]).
pub fn frame_key(org_id: i64, device_id: &str, seq: u32) -> String {
    format!("pnex:cam:v1:{org_id}:{device_id}:f:{seq}")
}

/// JSON published on [`bus_channel`] for each frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusFrameMeta {
    pub seq: u32,
    /// Server reception time, unix ms (`ts_source = server`).
    pub ts_ms: i64,
    pub width: u16,
    pub height: u16,
    /// JPEG size in bytes.
    pub size: u32,
    /// Valkey key of the JPEG bytes.
    pub key: String,
}

/// Recorded video segment (`video_segments` row, API DTO).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoSegment {
    pub id: String,
    pub device: i64,
    pub device_id: String,
    pub stream: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    pub node_id: String,
    /// RFC 3339.
    pub started_at: String,
    pub ended_at: String,
    pub frame_count: i32,
    pub size_bytes: i64,
    pub width: i32,
    pub height: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Most segments returned by one timeline query (a day of 1-minute
/// segments is 1440).
pub const TIMELINE_MAX_SEGMENTS: u64 = 5000;
/// Widest window of a timeline query (one day plus DST slack).
pub const TIMELINE_MAX_SPAN_SECS: i64 = 26 * 3600;
/// Source bytes cap of a single-file export (the AVI is built in memory).
pub const EXPORT_MAX_BYTES: i64 = 256 * 1024 * 1024;
/// Overlap tolerated between two chained segments (clock jitter between
/// the flush of one segment and the first frame of the next).
pub const CHAIN_TOLERANCE_MS: i64 = 1000;

/// Continuous recording of one camera over a window
/// (`GET /api/v1/cameras/{device}/recordings`): segments in ascending start
/// order, overlapping ones included (the player/export chain them with
/// [`playback_chain`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingTimeline {
    pub segments: Vec<VideoSegment>,
    /// Sum of `size_bytes` of the returned segments.
    pub total_bytes: i64,
    /// More than [`TIMELINE_MAX_SEGMENTS`] segments matched.
    pub truncated: bool,
}

/// OpenObserve logs stream holding the annotation layers of the cameras
/// (D105): each `vision-detect` node with `record_layer` stores the boxes it
/// matched — a few hundred bytes per analysed frame instead of an annotated
/// video. One layer per detection node, unlimited per camera.
pub const DETECTIONS_STREAM: &str = "camera_detections";
/// Most annotations one read returns (a segment at 5 fps is ~300 per layer).
pub const ANNOTATIONS_MAX: i64 = 5000;

/// Detections of one analysed frame, drawn over the recording at playback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameAnnotation {
    /// Layer (detection node) the boxes come from.
    #[serde(default)]
    pub layer_id: String,
    /// Unix ms of the analysed frame (camera timestamp).
    pub ts_ms: i64,
    /// Frame size the boxes refer to.
    pub width: u32,
    pub height: u32,
    pub detections: Vec<crate::vision::Detection>,
}

/// Batch posted by a detection node (`POST /internal/flow/video-annotations`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationBatch {
    pub org_id: i64,
    /// Camera slug.
    pub device_id: String,
    /// Stable layer id: `f{flow_id}-{node_id}`.
    pub layer_id: String,
    /// Display name: the node name, else the model name.
    pub layer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    pub annotations: Vec<FrameAnnotation>,
}

/// Layer id of a detection node.
pub fn layer_id_of(flow_id: i64, node_id: &str) -> String {
    format!("f{flow_id}-{node_id}")
}

/// Annotation layer available for a camera over a window
/// (`GET /api/v1/cameras/{device}/recordings/layers`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationLayer {
    pub id: String,
    pub name: String,
    /// Annotated frames in the window.
    pub count: i64,
}

/// Annotation to draw for a frame shown at `ts_ms`: the closest one within
/// `tolerance_ms` (annotations sorted by `ts_ms`).
pub fn annotation_at(
    anns: &[FrameAnnotation],
    ts_ms: i64,
    tolerance_ms: i64,
) -> Option<&FrameAnnotation> {
    let i = anns.partition_point(|a| a.ts_ms < ts_ms);
    [i.checked_sub(1), Some(i)]
        .into_iter()
        .flatten()
        .filter_map(|j| anns.get(j))
        .filter(|a| (a.ts_ms - ts_ms).abs() <= tolerance_ms)
        .min_by_key(|a| (a.ts_ms - ts_ms).abs())
}

/// Indices of the segments to play back to back, given `(start_ms, end_ms)`
/// spans sorted by start: a segment starting before the end of the last
/// kept one (a second recorder node on the same camera, a replayed flush)
/// is skipped, so playback and export never show the same moment twice.
pub fn playback_chain(spans: &[(i64, i64)]) -> Vec<usize> {
    let mut kept = Vec::new();
    let mut end = i64::MIN;
    for (i, &(start, stop)) in spans.iter().enumerate() {
        if end == i64::MIN || start >= end - CHAIN_TOLERANCE_MS {
            kept.push(i);
            end = stop.max(start);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_at_picks_the_closest() {
        let ann = |ts| FrameAnnotation {
            layer_id: String::new(),
            ts_ms: ts,
            width: 640,
            height: 480,
            detections: Vec::new(),
        };
        let anns = [ann(1000), ann(2000), ann(3000)];
        assert_eq!(annotation_at(&anns, 1900, 300).map(|a| a.ts_ms), Some(2000));
        assert_eq!(annotation_at(&anns, 2600, 300), None);
        assert_eq!(annotation_at(&anns, 2600, 500).map(|a| a.ts_ms), Some(3000));
        assert_eq!(annotation_at(&[], 0, 1000), None);
    }

    #[test]
    fn playback_chain_skips_overlaps() {
        // Back-to-back, then a second recorder overlapping, then a gap.
        let spans = [
            (0, 60_000),
            (60_200, 120_000),
            (60_400, 121_000),
            (119_500, 180_000),
            (300_000, 360_000),
        ];
        assert_eq!(playback_chain(&spans), vec![0, 1, 3, 4]);
        assert!(playback_chain(&[]).is_empty());
    }

    #[test]
    fn header_roundtrip() {
        let h = FrameHeader {
            seq: 0xDEAD_BEEF,
            uptime_ms: 123_456,
            width: 640,
            height: 480,
        };
        let mut plain = h.encode().to_vec();
        plain.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]);
        let (back, jpeg) = FrameHeader::parse(&plain).unwrap();
        assert_eq!(back, h);
        assert_eq!(jpeg.len(), 7);
    }

    #[test]
    fn header_golden_bytes() {
        // Golden vector shared with firmware/lib/pnex/src/pnex_camera.cpp.
        let h = FrameHeader {
            seq: 1,
            uptime_ms: 2,
            width: 640,
            height: 480,
        };
        assert_eq!(
            h.encode(),
            [b'P', b'X', b'C', b'1', 1, 0, 0, 0, 2, 0, 0, 0, 0x80, 0x02, 0xE0, 0x01]
        );
    }

    #[test]
    fn header_rejects() {
        assert_eq!(FrameHeader::parse(&[0; 4]), Err(FrameError::TooShort));
        let mut bad = [0u8; 20];
        assert_eq!(FrameHeader::parse(&bad), Err(FrameError::BadMagic));
        bad[0..4].copy_from_slice(b"PXC1");
        assert_eq!(FrameHeader::parse(&bad), Err(FrameError::NotJpeg));
    }

    #[test]
    fn framesize_wire_roundtrip() {
        for f in FrameSize::ALL {
            assert_eq!(FrameSize::from_wire(f.wire()), Some(f));
            assert_eq!(
                serde_json::to_string(&f).unwrap(),
                format!("\"{}\"", f.wire())
            );
        }
    }

    #[test]
    fn settings_check() {
        assert!(CameraSettings::default().check().is_ok());
        let bad = CameraSettings {
            quality: 5,
            fps: 40,
            ..Default::default()
        };
        let errs = bad.check().unwrap_err();
        assert_eq!(errs.len(), 2);
    }
}
