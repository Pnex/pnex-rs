//! ffmpeg as a confined decoder (media-ingest.md D160): bytes in on
//! `pipe:0`, 16 kHz mono s16le out on `pipe:1`. No network protocol, no
//! file, the demuxer imposed by what the fetcher saw (never probed), and
//! only audio decoders.

use std::path::{Path, PathBuf};

/// Demuxer + allowed decoders for one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFormat {
    /// ffmpeg demuxer name (`-f`, `-format_whitelist`).
    pub demuxer: &'static str,
    /// Comma-separated decoders (`-codec_whitelist`).
    pub decoders: &'static str,
}

const MP3: InputFormat = InputFormat {
    demuxer: "mp3",
    decoders: "mp3float,mp3",
};
const AAC: InputFormat = InputFormat {
    demuxer: "aac",
    decoders: "aac,aac_fixed",
};
const OGG: InputFormat = InputFormat {
    demuxer: "ogg",
    decoders: "vorbis,opus,flac",
};
const FLAC: InputFormat = InputFormat {
    demuxer: "flac",
    decoders: "flac",
};
const WAV: InputFormat = InputFormat {
    demuxer: "wav",
    decoders: "pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,pcm_u8",
};
const MPEGTS: InputFormat = InputFormat {
    demuxer: "mpegts",
    decoders: "aac,aac_fixed,mp3float,mp3,mp2float,mp2",
};
const MP4: InputFormat = InputFormat {
    demuxer: "mov",
    decoders: "aac,aac_fixed,mp3float,mp3,opus,flac",
};

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
}
