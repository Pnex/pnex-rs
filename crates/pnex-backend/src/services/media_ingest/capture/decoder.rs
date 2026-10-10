//! ffmpeg as a confined decoder (media-ingest.md D160): bytes in on
//! `pipe:0`, 16 kHz mono s16le out on `pipe:1`. No network protocol, no
//! file, the demuxer imposed by what the fetcher saw (never probed), and
//! only audio decoders. The video track (D175) runs a second, equally
//! confined invocation: video decoders only, sampled JPEG frames out on
//! `pipe:1` (`image2pipe`).

use std::path::{Path, PathBuf};

/// Demuxer + allowed decoders for one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFormat {
    /// ffmpeg demuxer name (`-f`, `-format_whitelist`).
    pub demuxer: &'static str,
    /// Comma-separated audio decoders (`-codec_whitelist`); empty = no
    /// audio in this format.
    pub decoders: &'static str,
    /// Comma-separated video decoders of the video invocation; empty = no
    /// video in this format.
    pub video: &'static str,
}

impl InputFormat {
    pub fn has_audio(&self) -> bool {
        !self.decoders.is_empty()
    }

    pub fn has_video(&self) -> bool {
        !self.video.is_empty()
    }
}

/// Video decoders of a stream (D175): native FFmpeg decoders, LGPL.
const VIDEO_DECODERS: &str = "h264,hevc,mjpeg";

const MP3: InputFormat = InputFormat {
    demuxer: "mp3",
    decoders: "mp3float,mp3",
    video: "",
};
const AAC: InputFormat = InputFormat {
    demuxer: "aac",
    decoders: "aac,aac_fixed",
    video: "",
};
const OGG: InputFormat = InputFormat {
    demuxer: "ogg",
    decoders: "vorbis,opus,flac",
    video: "",
};
const FLAC: InputFormat = InputFormat {
    demuxer: "flac",
    decoders: "flac",
    video: "",
};
const WAV: InputFormat = InputFormat {
    demuxer: "wav",
    decoders: "pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,pcm_u8",
    video: "",
};
const MPEGTS: InputFormat = InputFormat {
    demuxer: "mpegts",
    decoders: "aac,aac_fixed,mp3float,mp3,mp2float,mp2",
    video: "h264,hevc",
};
const MP4: InputFormat = InputFormat {
    demuxer: "mov",
    decoders: "aac,aac_fixed,mp3float,mp3,opus,flac",
    video: VIDEO_DECODERS,
};

/// Raw H.264 Annex B (RTSP video track).
pub const H264: InputFormat = InputFormat {
    demuxer: "h264",
    decoders: "",
    video: "h264",
};
/// Raw H.265 Annex B (RTSP video track).
pub const HEVC: InputFormat = InputFormat {
    demuxer: "hevc",
    decoders: "",
    video: "hevc",
};
/// ADTS AAC (RTSP audio track).
pub const ADTS: InputFormat = AAC;

/// Format of an HTTP body from its `Content-Type`, then from the URL path
/// extension. `None` = refused (`format-unsupported`).
pub fn input_format(content_type: Option<&str>, path: &str) -> Option<InputFormat> {
    let ct = content_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let by_type = match ct.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => Some(MP3),
        "audio/aac" | "audio/aacp" | "audio/x-aac" | "audio/aac-adts" => Some(AAC),
        "application/ogg" | "audio/ogg" | "audio/opus" | "audio/vorbis" => Some(OGG),
        "audio/flac" | "audio/x-flac" => Some(FLAC),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some(WAV),
        "video/mp2t" | "video/mpeg" => Some(MPEGTS),
        "audio/mp4" | "audio/x-m4a" | "video/mp4" | "video/iso.segment" => Some(MP4),
        _ => None,
    };
    by_type.or_else(|| {
        let ext = Path::new(path.split('?').next().unwrap_or_default())
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "mp3" => Some(MP3),
            "aac" | "adts" => Some(AAC),
            "ogg" | "oga" | "opus" => Some(OGG),
            "flac" => Some(FLAC),
            "wav" => Some(WAV),
            "ts" => Some(MPEGTS),
            "m4a" | "mp4" | "m4s" | "cmfa" => Some(MP4),
            _ => None,
        }
    })
}

/// Decoder argv (input options before `-i`, D160).
pub fn argv(ffmpeg: &Path, format: InputFormat) -> Vec<String> {
    let mut out = vec![ffmpeg.display().to_string()];
    out.extend(
        [
            "-hide_banner",
            "-nostats",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "pipe",
            "-format_whitelist",
            format.demuxer,
            "-codec_whitelist",
            format.decoders,
            "-f",
            format.demuxer,
            "-i",
            "pipe:0",
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ]
        .map(String::from),
    );
    out
}

/// Video decoder argv (D175): first video stream only, sampled at `fps`,
/// scaled down to at most `max_width` (even height), one JPEG per frame on
/// `pipe:1`. Same input confinement as [`argv`].
pub fn video_argv(ffmpeg: &Path, format: InputFormat, fps: u32, max_width: u32) -> Vec<String> {
    let mut out = vec![ffmpeg.display().to_string()];
    out.extend(
        [
            "-hide_banner",
            "-nostats",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "pipe",
            "-format_whitelist",
            format.demuxer,
            "-codec_whitelist",
            format.video,
            "-f",
            format.demuxer,
            // Few threads: the address space is capped (RLIMIT_AS) and
            // ffmpeg otherwise starts one per core.
            "-threads",
            "2",
            "-filter_threads",
            "1",
            "-i",
            "pipe:0",
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-vf",
        ]
        .map(String::from),
    );
    out.push(format!(
        "fps={fps},scale=w='min({max_width},iw)':h=-2,format=yuvj420p"
    ));
    out.extend(
        [
            "-c:v",
            "mjpeg",
            "-threads",
            "1",
            "-q:v",
            "5",
            "-f",
            "image2pipe",
            "pipe:1",
        ]
        .map(String::from),
    );
    out
}

/// Absolute path of `program` (a path, or a name looked up in `PATH`):
/// the decoder runs with an empty environment.
pub fn resolve(program: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|c| c.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_from_type_then_extension() {
        assert_eq!(input_format(Some("audio/mpeg"), "/live"), Some(MP3));
        assert_eq!(
            input_format(Some("audio/aacp; charset=x"), "/live"),
            Some(AAC)
        );
        assert_eq!(
            input_format(Some("application/octet-stream"), "/seg12.ts?x=1"),
            Some(MPEGTS)
        );
        assert_eq!(input_format(None, "/ep.m4a"), Some(MP4));
        assert_eq!(input_format(Some("text/html"), "/index"), None);
        assert_eq!(input_format(None, "/playlist.m3u8"), None);
    }

    #[test]
    fn input_options_come_before_the_input() {
        let a = argv(Path::new("/usr/bin/ffmpeg"), MP3);
        let i = a.iter().position(|x| x == "-i").unwrap();
        for opt in [
            "-protocol_whitelist",
            "-format_whitelist",
            "-codec_whitelist",
            "-f",
        ] {
            let at = a.iter().position(|x| x == opt).unwrap();
            assert!(at < i, "{opt} must precede -i");
        }
        assert_eq!(a[i + 1], "pipe:0");
        assert_eq!(a.last().unwrap(), "pipe:1");
        assert!(!a.iter().any(|x| x.contains("http")));
    }

    #[test]
    fn video_argv_is_confined_and_bounded() {
        let a = video_argv(Path::new("/usr/bin/ffmpeg"), MPEGTS, 2, 1280);
        let i = a.iter().position(|x| x == "-i").unwrap();
        for opt in ["-protocol_whitelist", "-codec_whitelist", "-f"] {
            assert!(a.iter().position(|x| x == opt).unwrap() < i, "{opt}");
        }
        let wl = a.iter().position(|x| x == "-codec_whitelist").unwrap();
        assert_eq!(a[wl + 1], "h264,hevc");
        assert!(a
            .iter()
            .any(|x| x.starts_with("fps=2,scale=w='min(1280,iw)'")));
        assert!(a.iter().any(|x| x == "image2pipe"));
        assert_eq!(a.last().unwrap(), "pipe:1");
        assert!(!MP3.has_video() && MPEGTS.has_video() && !H264.has_audio());
    }
}
